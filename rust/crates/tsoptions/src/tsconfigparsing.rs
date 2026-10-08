// Port of tsc/internal/tsoptions/tsconfigparsing.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes (M4 scope — see PORTING-NOTES.md for the full list):
//   - Go parses the tsconfig as an AST SourceFile (ParseConfigFileTextToJson
//     → convertObjectLiteralExpressionToJson) so diagnostics can carry
//     node positions. The port parses the JSON text directly into the
//     `CompilerOptionsValue` tree (a small recursive-descent parser below)
//     — same values, same key order; diagnostics degrade to
//     compiler-diagnostics without spans until the AST-config path lands.
//   - Go's `extendedConfigCache`/`resolutionStack` cycle machinery is
//     carried; the `getExtendedConfig` node-module-resolution arm (module
//     specifiers like "extends": "@tsconfig/...") is seam'd to the module
//     crate via `ResolveConfigHost` (the module crate is landing
//     concurrently); when the seam returns None the arm reports the same
//     "cannot read" diagnostic Go produces for an unreadable file.
//   - `getFileNamesFromConfigSpecs` (glob-driven file enumeration) is not
//     part of this wave; the parsed result carries the validated specs and
//     an empty `file_names` (the --showConfig runner only prints them).
//   - `ProjectReference` is a core type in Go but is not in the Rust core
//     crate yet (M6); it is defined here with a PORT note.

use std::collections::HashSet;
use std::sync::Arc;

use tsc_collections::OrderedMap;
use tsc_core::options_generated::CompilerOptions;
use tsc_diagnostics::Message;
use tsc_json::Value;
use tsc_tspath::{
    get_normalized_absolute_path, to_path_pattern, CaseSensitivity, PathKey, RootedDirectoryPath,
    RootedFilePath,
};

use crate::commandlineoption::{CommandLineOption, CompilerOptionsValue};
use crate::errors::new_compiler_diagnostic;
use crate::namemap::command_line_compiler_options_map;
use crate::options_generated::{get_default_compiler_options, parse_compiler_options};
use crate::parsinghelpers::{
    get_substituted_string_array_with_config_dir_template, merge_parsed_compiler_options,
    parse_project_reference, starts_with_config_dir_template, OptionParser,
};
use crate::parsedcommandline::ParsedCommandLine;

/// Go: `const defaultIncludeSpec = "**/*"` (tsconfigparsing.go).
pub const DEFAULT_INCLUDE_SPEC: &str = "**/*";

/// Go: `type core.ProjectReference struct` — a project reference as resolved
/// from a tsconfig. PORT: defined here (not in core) until the M6 wave adds
/// it to tsc-core; see PORTING-NOTES.
#[derive(Clone, Debug, Default)]
pub struct ProjectReference {
    /// The resolved path to the tsconfig (Go: `Path tspath.RootedFilePath`).
    pub path: RootedFilePath,
    /// The original `path` value from the tsconfig (before resolution).
    pub original_path: String,
    pub circular: bool,
}

/// Go: `type parsedCompilerOptions struct` (compilerOptions +
/// unresolvedCompilerOptionPaths).
#[derive(Clone, Debug, Default)]
pub struct ParsedCompilerOptions {
    /// Go: the embedded `*core.CompilerOptions`.
    pub options: CompilerOptions,
    /// Go: `unresolvedPaths unresolvedCompilerOptionPaths` — option values
    /// that carry the ${configDir} template, keyed by option name.
    pub unresolved_paths: OrderedMap<String, CompilerOptionsValue>,
}

impl ParsedCompilerOptions {
    /// The `&parsedCompilerOptions{CompilerOptions: ..., unresolvedPaths: ...}`
    /// constructor shape (Go: `make(unresolvedCompilerOptionPaths)`).
    pub fn new(options: CompilerOptions) -> Self {
        ParsedCompilerOptions {
            options,
            unresolved_paths: OrderedMap::new(),
        }
    }
}

/// Go: `func (o *parsedCompilerOptions) ParseOption(key string, value any)` —
/// the unresolved-path capture + generated parse, exactly the
/// compilerOptionsParser logic (parsinghelpers.rs owns the shared trait).
impl OptionParser for ParsedCompilerOptions {
    fn parse_option(&mut self, key: &str, value: &CompilerOptionsValue) {
        if crate::parsinghelpers::compiler_option_contains_config_dir_template(key, value) {
            self.unresolved_paths.set(key.to_string(), value.clone());
            return;
        }
        // Go: ParseCompilerOptions returns early on nil values.
        if value.is_null() {
            return;
        }
        parse_compiler_options(key, value, &mut self.options);
    }

    fn unknown_option_diagnostic() -> Option<&'static Message> {
        Some(&tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0)
    }

    fn unknown_did_you_mean_diagnostic() -> Option<&'static Message> {
        Some(&tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0_DID_YOU_MEAN_1)
    }
}

/// Marshal a parsed options object (contentMapper.options) to raw JSON.
pub fn marshal_compiler_options_value(
    options: &OrderedMap<String, CompilerOptionsValue>,
) -> Option<Value> {
    serde_json::value::RawValue::from_string(crate::showconfig::marshal_value(
        &CompilerOptionsValue::Object(options.clone()),
        0,
    ))
    .ok()
}

/// Go: `type configFileSpecs struct` — the validated include/exclude/files
/// specs of a parsed config.
#[derive(Clone, Debug, Default)]
pub struct ConfigFileSpecs {
    pub validated_include_specs: Vec<tsc_tspath::PathPattern>,
    pub validated_exclude_specs: Vec<tsc_tspath::PathPattern>,
    /// Go: `validatedFilesSpec []tspath.FileSpec`.
    pub validated_files_specs: Vec<tsc_tspath::FileSpec>,
    /// Go: `validatedFileNames []tspath.RootedFilePath` — the files-list
    /// entries resolved against the config directory.
    pub validated_file_names: Vec<RootedFilePath>,
    /// Go: whether include defaulted to ["**/*"].
    pub is_default_include_spec: bool,
}

/// Go: `type ExtendedConfigCacheEntry` (the resolution-stack entry shape).
pub struct ExtendedConfigCacheEntry {
    pub extended_file_names: Vec<RootedFilePath>,
}

/// Go: `type ExtendedConfigCache interface` — simplified to a callable map.
pub type ExtendedConfigCache = Arc<dyn Fn(&str) -> Option<CompilerOptionsValue> + Send + Sync>;

/// The module-resolution seam. Go resolves `extends` values that are module
/// specifiers via the module resolver (module crate); the Rust module crate
/// is landing concurrently, so the seam is a trait this crate consumes.
/// PORT: swap to `tsc_module` types on integration (PORTING-NOTES).
pub trait ResolveConfigHost: Send + Sync {
    /// Resolve a module-specifier `extends` value; `None` = not resolvable
    /// (Go falls through to the "cannot read file" diagnostic).
    fn resolve(&self, specifier: &str, containing_file: &RootedFilePath)
        -> Option<RootedFilePath>;
}

// ────────────────────────────────────────────────────────────────────────────
// JSON text → CompilerOptionsValue (ordered)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func ParseConfigFileTextToJson(fileName, path, jsonText) (any, []*ast.Diagnostic)` —
/// the values side; diagnostics degrade per the module PORT notes (Go's come
/// from parsing the file as a JSON SourceFile; the port reports the text
/// parser's error as a raw diagnostic).
pub fn parse_config_file_text_to_json(
    json_text: &str,
) -> (Option<CompilerOptionsValue>, Vec<tsc_ast::Diagnostic>) {
    match parse_json_text(json_text) {
        Ok(value) => (Some(value), Vec::new()),
        Err(error) => (
            None,
            vec![json_parse_error_diagnostic(&error)],
        ),
    }
}

/// The text-parser failure form (Go surfaces the first SourceFile parse
/// diagnostic; the AST-config path is future work — module PORT notes).
fn json_parse_error_diagnostic(error: &str) -> tsc_ast::Diagnostic {
    tsc_ast::Diagnostic::new(
        None,
        tsc_core::text::TextRange::new(0, 0),
        0,
        tsc_diagnostics::Category::Error,
        "",
        None,
        format!("Syntax error while parsing tsconfig: {error}"),
        "",
        Vec::new(),
    )
}

/// A minimal order-preserving JSON reader. Numbers surface as Int when
/// integral (Go unmarshals into `any` as float64; the port keeps both forms
/// and the helpers accept either), strings unescape the standard escapes.
pub fn parse_json_text(text: &str) -> Result<CompilerOptionsValue, String> {
    let bytes = text.as_bytes();
    let mut pos = 0;
    // Go's scanner strips a leading BOM before parsing (skipBOM).
    if bytes.starts_with("\u{feff}".as_bytes()) {
        pos = 3;
    }
    let value = parse_json_value(bytes, &mut pos)?;
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err("trailing characters".to_string());
    }
    Ok(value)
}

/// Go parses the config text as a TS SourceFile (ScriptKindJSON), i.e.
/// JSONC: comments and trailing commas are legal everywhere. The text
/// parser therefore skips `//` and `/* */` comments wherever whitespace is
/// skipped (PORT: found by the showconfig parity twin — the 12-comments
/// fixture diverged until this mirrored Go's ParseConfigFileTextToJson).
fn skip_ws(bytes: &[u8], pos: &mut usize) {
    loop {
        while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\n' | b'\r') {
            *pos += 1;
        }
        if bytes[*pos..].starts_with(b"//") {
            *pos += 2;
            while *pos < bytes.len() && bytes[*pos] != b'\n' {
                *pos += 1;
            }
            continue;
        }
        if bytes[*pos..].starts_with(b"/*") {
            *pos += 2;
            while *pos < bytes.len() && !bytes[*pos..].starts_with(b"*/") {
                *pos += 1;
            }
            *pos = (*pos + 2).min(bytes.len());
            continue;
        }
        return;
    }
}

fn parse_json_value(bytes: &[u8], pos: &mut usize) -> Result<CompilerOptionsValue, String> {
    skip_ws(bytes, pos);
    match bytes.get(*pos) {
        None => Err("unexpected end of input".to_string()),
        Some(b'{') => {
            *pos += 1;
            let mut map = OrderedMap::new();
            skip_ws(bytes, pos);
            if bytes.get(*pos) == Some(&b'}') {
                *pos += 1;
                return Ok(CompilerOptionsValue::Object(map));
            }
            loop {
                skip_ws(bytes, pos);
                let CompilerOptionsValue::Str(key) = parse_json_value(bytes, pos)? else {
                    return Err("object key must be a string".to_string());
                };
                skip_ws(bytes, pos);
                if bytes.get(*pos) != Some(&b':') {
                    return Err("expected ':'".to_string());
                }
                *pos += 1;
                let value = parse_json_value(bytes, pos)?;
                map.set(key, value);
                skip_ws(bytes, pos);
                match bytes.get(*pos) {
                    Some(b',') => {
                        *pos += 1;
                        skip_ws(bytes, pos);
                        // Trailing comma before the close (JSONC, Go's
                        // ScriptKindJSON parse).
                        if bytes.get(*pos) == Some(&b'}') {
                            *pos += 1;
                            return Ok(CompilerOptionsValue::Object(map));
                        }
                    }
                    Some(b'}') => {
                        *pos += 1;
                        return Ok(CompilerOptionsValue::Object(map));
                    }
                    _ => return Err("expected ',' or '}'".to_string()),
                }
            }
        }
        Some(b'[') => {
            *pos += 1;
            let mut list = Vec::new();
            skip_ws(bytes, pos);
            if bytes.get(*pos) == Some(&b']') {
                *pos += 1;
                return Ok(CompilerOptionsValue::Array(list));
            }
            loop {
                list.push(parse_json_value(bytes, pos)?);
                skip_ws(bytes, pos);
                match bytes.get(*pos) {
                    Some(b',') => {
                        *pos += 1;
                        skip_ws(bytes, pos);
                        // Trailing comma before the close (JSONC).
                        if bytes.get(*pos) == Some(&b']') {
                            *pos += 1;
                            return Ok(CompilerOptionsValue::Array(list));
                        }
                    }
                    Some(b']') => {
                        *pos += 1;
                        return Ok(CompilerOptionsValue::Array(list));
                    }
                    _ => return Err("expected ',' or ']'".to_string()),
                }
            }
        }
        Some(b'"') => {
            let s = parse_json_string(bytes, pos)?;
            Ok(CompilerOptionsValue::Str(s))
        }
        Some(b't') if bytes[*pos..].starts_with(b"true") => {
            *pos += 4;
            Ok(CompilerOptionsValue::Bool(true))
        }
        Some(b'f') if bytes[*pos..].starts_with(b"false") => {
            *pos += 5;
            Ok(CompilerOptionsValue::Bool(false))
        }
        Some(b'n') if bytes[*pos..].starts_with(b"null") => {
            *pos += 4;
            Ok(CompilerOptionsValue::Null)
        }
        Some(_) => {
            let start = *pos;
            while *pos < bytes.len()
                && matches!(bytes[*pos], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
            {
                *pos += 1;
            }
            let raw = std::str::from_utf8(&bytes[start..*pos]).map_err(|_| "bad number")?;
            let number: f64 = raw.parse().map_err(|_| format!("bad number {raw}"))?;
            if number.fract() == 0.0 && number.is_finite() && number.abs() < 9.0e15 {
                Ok(CompilerOptionsValue::Int(number as i64))
            } else {
                Ok(CompilerOptionsValue::Float(number))
            }
        }
    }
}

fn parse_json_string(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    debug_assert_eq!(bytes.get(*pos), Some(&b'"'));
    *pos += 1;
    let mut out = String::new();
    while *pos < bytes.len() {
        match bytes[*pos] {
            b'"' => {
                *pos += 1;
                return Ok(out);
            }
            b'\\' => {
                *pos += 1;
                match bytes.get(*pos) {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'b') => out.push('\u{8}'),
                    Some(b'f') => out.push('\u{c}'),
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'u') => {
                        let hex = std::str::from_utf8(&bytes[*pos + 1..*pos + 5])
                            .map_err(|_| "bad \\u escape")?;
                        let code = u32::from_str_radix(hex, 16).map_err(|_| "bad \\u escape")?;
                        out.push(char::from_u32(code).ok_or("bad \\u escape")?);
                        *pos += 4;
                    }
                    _ => return Err("bad escape".to_string()),
                }
                *pos += 1;
            }
            _ => {
                let start = *pos;
                while *pos < bytes.len() && bytes[*pos] != b'"' && bytes[*pos] != b'\\' {
                    *pos += 1;
                }
                out.push_str(std::str::from_utf8(&bytes[start..*pos]).map_err(|_| "bad utf-8")?);
            }
        }
    }
    Err("unterminated string".to_string())
}

// ────────────────────────────────────────────────────────────────────────────
// Config content parsing
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func ParseJsonConfigFileContent(json, fs, basePath, existingOptions, configFileName, resolutionStack, extendedConfigCache) *ParsedCommandLine`.
///
/// `json` is the already-parsed top-level object (Go passes the result of
/// `ParseConfigFileTextToJson`); `existing_options` seeds the compiler
/// options exactly as in Go.
#[allow(clippy::too_many_arguments)]
pub fn parse_json_config_file_content(
    json: &CompilerOptionsValue,
    fs: &dyn tsc_vfs::Fs,
    base_path: &RootedDirectoryPath,
    existing_options: Option<&CompilerOptions>,
    config_file_name: &RootedFilePath,
    resolution_stack: &[PathKey],
    extended_config_cache: Option<&ExtendedConfigCache>,
    resolve_config_host: Option<&dyn ResolveConfigHost>,
) -> ParsedCommandLine {
    let mut errors = Vec::new();
    // Go's No-inputs check compares the CALLER's stack length; the port
    // seeds the stack below, so remember the input length first.
    let resolution_stack_len = resolution_stack.len();
    let mut resolution_stack: Vec<PathKey> = resolution_stack.to_vec();
    // PORT: Go's callers seed the stack with the config file's own path
    // (GetParsedCommandLineOfConfigFilePath); the port seeds it here when
    // empty so extends cycles are detected self-contained.
    if resolution_stack.is_empty() && !config_file_name.is_empty() {
        resolution_stack.push(tsc_tspath::path_key_from_canonical(
            config_file_name.as_string(),
        ));
    }

    let mut result = ParsedCommandLine::new(
        config_file_name.clone(),
        base_path.clone(),
        fs.case_sensitivity(),
    );

    // existingOptions (Go: seeds the options the same way).
    if let Some(existing) = existing_options {
        result.compiler_options.options = existing.clone();
    } else {
        // Go: the worker starts from getDefaultCompilerOptions(configFileName).
        result.compiler_options = get_default_compiler_options(config_file_name);
    }

    // Go: `if configFileName != "" { basePathForFileNames = configFileName.Directory() }`.
    let base_path_for_file_names = if !config_file_name.is_empty() {
        config_file_name.directory()
    } else {
        base_path.clone()
    };
    // Go: the No-inputs diagnostic compares the CALLER's resolutionStack
    // length (the port seeds the stack with the config itself above, so
    // `resolution_stack_len` above holds the caller's length).

    // Go: `normalizeJsonValue` + the worker's empty-map fallback for a
    // non-object root — parsing continues with an empty config.
    let mut raw_config: OrderedMap<String, CompilerOptionsValue> = match json {
        CompilerOptionsValue::Object(map) => map.clone(),
        _ => OrderedMap::new(),
    };

    // extends — resolve and merge first (Go: parseConfig's extends arm).
    if let Some(extends_value) = raw_config.get("extends") {
        for extends_path in extends_value_list(extends_value) {
            if let Some((extended, extended_parsed, extended_path)) = read_extended_config(
                &extends_path,
                fs,
                base_path,
                config_file_name,
                &mut resolution_stack,
                extended_config_cache,
                resolve_config_host,
                &mut errors,
            ) {
                merge_parsed_compiler_options(
                    &mut result.compiler_options,
                    Some(&extended_parsed),
                    Some(&CompilerOptionsValue::Object(extended.clone())),
                );
                // Go: applyExtendedConfig's setPropertyValue — include/
                // exclude/files/compileOnSave inherit from the extended
                // config's (already-merged) raw, rewritten relative to THIS
                // config's basePath, only where the own raw lacks the key.
                merge_extended_raw_into_config(
                    &mut raw_config,
                    &extended,
                    &extended_path.directory(),
                    base_path,
                    fs.case_sensitivity(),
                );
            }
        }
    }

    // compilerOptions
    if let Some(compiler_options_value) = raw_config.get("compilerOptions") {
        let (parsed, conversion_errors) =
            convert_options_from_json_worker(compiler_options_value.clone(), base_path);
        errors.extend(conversion_errors);
        merge_parsed_compiler_options(
            &mut result.compiler_options,
            Some(&parsed),
            Some(&CompilerOptionsValue::Object(raw_config.clone())),
        );
    }

    // typeAcquisition
    if let Some(CompilerOptionsValue::Object(_)) = raw_config.get("typeAcquisition") {
        // Parsed in full by the M4 CLI surface; carried through unchanged.
    }

    // ── files/include/exclude: Go's getPropFromRaw + validateSpecs arm ──

    // Go: `fileSpecs := getPropFromRaw("files", isStringValue, "string")` —
    // Some(list) where Go's propOfRaw.sliceValue is non-nil (the key exists
    // with a non-null array value).
    let files_prop = string_array_prop(&raw_config, "files", &mut errors);
    if let Some(list) = &files_prop {
        if list.is_empty() {
            // Go: the empty-files-list diagnostic (The_files_list_in_config_file_0_is_empty)
            // — only with zero-or-no references and no extends.
            let references_prop = raw_config.get("references");
            let has_zero_or_no_references = match references_prop {
                // Go: "no-prop" (absent or null) or "not-array" (present,
                // non-array) or an empty array.
                None | Some(CompilerOptionsValue::Null) => true,
                Some(CompilerOptionsValue::Array(list)) => list.is_empty(),
                Some(_) => true,
            };
            let has_extends = match raw_config.get("extends") {
                None | Some(CompilerOptionsValue::Null) => false,
                Some(_) => true,
            };
            if has_zero_or_no_references && !has_extends {
                errors.push(new_compiler_diagnostic(
                    &tsc_diagnostics::THE_FILES_LIST_IN_CONFIG_FILE_0_IS_EMPTY,
                    &[config_file_name.as_string().to_owned()],
                ));
            }
        }
    }
    let include_prop = string_array_prop(&raw_config, "include", &mut errors);
    let mut exclude_prop = string_array_prop(&raw_config, "exclude", &mut errors);
    if exclude_prop.is_none() {
        // Go: outDir/declarationDir imply the exclude list when exclude is absent.
        let out_dir = result.compiler_options.options.out_dir.as_string().to_owned();
        let declaration_dir = result
            .compiler_options
            .options
            .declaration_dir
            .as_string()
            .to_owned();
        let mut values: Vec<String> = Vec::new();
        if !out_dir.is_empty() {
            values.push(out_dir);
        }
        if !declaration_dir.is_empty() {
            values.push(declaration_dir);
        }
        if !values.is_empty() {
            exclude_prop = Some(values);
        }
    }
    let (include_prop, is_default_include_spec) = match (files_prop.is_none(), include_prop.is_none()) {
        (true, true) => (Some(vec![DEFAULT_INCLUDE_SPEC.to_string()]), true),
        _ => (include_prop, false),
    };

    // validateSpecs + ${configDir} substitution (include/exclude).
    let mut validated_include_specs: Option<Vec<tsc_tspath::PathPattern>> = None;
    if let Some(include) = &include_prop {
        let (specs, mut spec_errors) = validate_specs(
            &CompilerOptionsValue::StrArray(include.clone()),
            true,
            "include",
        );
        errors.append(&mut spec_errors);
        let substituted = get_substituted_string_array_with_config_dir_template(
            &include.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>(),
            &base_path_for_file_names,
        );
        validated_include_specs = Some(match substituted {
            Some(substituted) => substituted
                .iter()
                .map(|s| tsc_tspath::to_path_pattern(s))
                .collect(),
            None => specs,
        });
    }
    let mut validated_exclude_specs: Option<Vec<tsc_tspath::PathPattern>> = None;
    if let Some(exclude) = &exclude_prop {
        let (specs, mut spec_errors) = validate_specs(
            &CompilerOptionsValue::StrArray(exclude.clone()),
            false,
            "exclude",
        );
        errors.append(&mut spec_errors);
        let substituted = get_substituted_string_array_with_config_dir_template(
            &exclude.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>(),
            &base_path_for_file_names,
        );
        validated_exclude_specs = Some(match substituted {
            Some(substituted) => substituted
                .iter()
                .map(|s| tsc_tspath::to_path_pattern(s))
                .collect(),
            None => specs,
        });
    }

    // files: Go filters non-strings and converts to FileSpec (NO spec
    // validation — wildcard diagnostics do not apply to the files list).
    let mut validated_files_specs: Vec<tsc_tspath::FileSpec> = Vec::new();
    let mut validated_files_specs_before_substitution: Vec<tsc_tspath::FileSpec> = Vec::new();
    if let Some(files) = &files_prop {
        for spec in files {
            validated_files_specs_before_substitution.push(tsc_tspath::to_file_spec(spec));
        }
        let substituted = get_substituted_string_array_with_config_dir_template(
            &files.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>(),
            &base_path_for_file_names,
        );
        validated_files_specs = match substituted {
            Some(substituted) => substituted
                .iter()
                .map(|s| tsc_tspath::to_file_spec(s))
                .collect(),
            None => validated_files_specs_before_substitution.clone(),
        };
    }
    let validated_file_names: Vec<RootedFilePath> = validated_files_specs
        .iter()
        .map(|spec| {
            if spec.is_empty() {
                tsc_tspath::rooted_file_path_from_path(base_path_for_file_names.as_path())
            } else {
                tsc_tspath::to_rooted_file_path(spec.as_string(), &base_path_for_file_names)
            }
        })
        .collect();

    // ── File enumeration (Go: getFileNamesFromConfigSpecs) + the
    // No-inputs diagnostic — ported with the showconfig parity twin; see
    // the module PORT notes. ──
    let file_names = get_file_names_from_config_specs(
        &ConfigFileSpecs {
            validated_include_specs: validated_include_specs.clone().unwrap_or_default(),
            validated_exclude_specs: validated_exclude_specs.clone().unwrap_or_default(),
            validated_files_specs: validated_files_specs.clone(),
            validated_file_names: validated_file_names.clone(),
            is_default_include_spec,
        },
        &base_path_for_file_names,
        &result.compiler_options.options,
        fs,
    );
    let can_json_report_no_input_files =
        !raw_config.has("files") && !raw_config.has("references");
    if file_names.is_empty() && can_json_report_no_input_files && resolution_stack_len == 0 {
        let include_specs_json = stringify_json_string_list(include_prop.as_deref().unwrap_or(&[]));
        let exclude_specs_json = stringify_json_string_list(exclude_prop.as_deref().unwrap_or(&[]));
        errors.push(new_compiler_diagnostic(
            &tsc_diagnostics::NO_INPUTS_WERE_FOUND_IN_CONFIG_FILE_0_SPECIFIED_INCLUDE_PATHS_WERE_1_AND_EXCLUDE_PATHS_WERE_2,
            &[
                config_file_name.as_string().to_owned(),
                include_specs_json,
                exclude_specs_json,
            ],
        ));
    }
    result.file_names = file_names;
    result.validated_include_specs = validated_include_specs;
    result.validated_exclude_specs = validated_exclude_specs;
    result.validated_files_specs = validated_files_specs;
    result.validated_file_names = validated_file_names;
    result.raw_include_spec_values = include_prop.clone().unwrap_or_default();
    result.raw_exclude_spec_values = exclude_prop.clone().unwrap_or_default();
    result.is_default_include_spec = is_default_include_spec;

    // references (Go: parseProjectReference).
    if let Some(CompilerOptionsValue::Array(references)) = raw_config.get("references") {
        for reference in references {
            if let Some(parsed) = parse_project_reference(reference) {
                if parsed.has_path && parsed.path_valid {
                    let resolved = get_normalized_absolute_path(&parsed.path, base_path);
                    result.project_references.push(ProjectReference {
                        path: tsc_tspath::rooted_file_path_from_normalized(&resolved),
                        original_path: parsed.path,
                        circular: parsed.reference.circular,
                    });
                } else if parsed.has_path {
                    errors.push(new_compiler_diagnostic(
                        &tsc_diagnostics::FILE_0_NOT_FOUND,
                        &[String::new()],
                    ));
                }
            }
        }
    }

    // compileOnSave
    if let Some(CompilerOptionsValue::Bool(compile_on_save)) = raw_config.get("compileOnSave") {
        result.compile_on_save = Some(*compile_on_save);
    }

    result.errors = errors;
    result
}

/// Go: `applyExtendedConfig`'s `setPropertyValue` closure — inherits
/// include/exclude/files/compileOnSave from an extended config's raw map
/// into the extending config's raw map, only where the extending map lacks
/// the key, rewriting relative spec strings relative to the extending
/// config's basePath (Go: `ConvertToRelativePath` + `CombinePaths`).
///
/// PORT (found by the showconfig parity twin): the port previously copied
/// the extended specs verbatim; Go rewrites them, so `"include": ["src/**/*"]`
/// in a base at `<dir>/base/` becomes `base/src/**/*` in the child at `<dir>`.
fn merge_extended_raw_into_config(
    raw_config: &mut OrderedMap<String, CompilerOptionsValue>,
    extended_map: &OrderedMap<String, CompilerOptionsValue>,
    extended_directory: &RootedDirectoryPath,
    base_path: &RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
) {
    let mut relative_difference = String::new();
    let rewrite = |path: &str, relative_difference: &mut String| -> String {
        if starts_with_config_dir_template(path) || tsc_tspath::is_rooted_disk_path(path) {
            return path.to_string();
        }
        if relative_difference.is_empty() {
            *relative_difference = tsc_tspath::convert_to_relative_path(
                extended_directory.as_string(),
                base_path,
                case_sensitivity,
            );
        }
        tsc_tspath::combine_paths(relative_difference, &[path])
    };
    for property_name in ["include", "exclude", "files"] {
        if raw_config.has(property_name) {
            continue;
        }
        let Some(value) = extended_map.get(property_name) else {
            continue;
        };
        let list = match value {
            CompilerOptionsValue::StrArray(list) => Some(list.clone()),
            CompilerOptionsValue::Array(list) => {
                // Go maps `[]any` element-wise; non-strings pass through
                // (only real configs carry plain string specs here).
                Some(
                    list.iter()
                        .map(|v| match v {
                            CompilerOptionsValue::Str(s) => s.clone(),
                            other => marshal_value_json(other),
                        })
                        .collect(),
                )
            }
            _ => None,
        };
        if let Some(list) = list {
            let rewritten: Vec<String> = list
                .iter()
                .map(|p| rewrite(p, &mut relative_difference))
                .collect();
            raw_config.set(
                property_name.to_string(),
                CompilerOptionsValue::StrArray(rewritten),
            );
        }
    }
    // Go: `if extendedRawMap.Has("compileOnSave") { if value, ok := ...(bool); ok { result.compileOnSave = value } }`
    // then `if result.compileOnSave && !ownConfig.raw.Has("compileOnSave") { ownConfig.raw.Set(...) }`.
    if let Some(CompilerOptionsValue::Bool(compile_on_save)) = extended_map.get("compileOnSave") {
        if *compile_on_save && !raw_config.has("compileOnSave") {
            raw_config.set("compileOnSave".to_string(), CompilerOptionsValue::Bool(true));
        }
    }
}

/// The JSON text of a `CompilerOptionsValue` for the non-string pass-through
/// in [`merge_extended_raw_into_config`] (values are spec strings in every
/// real config; only their textual JSON form is needed here).
fn marshal_value_json(value: &CompilerOptionsValue) -> String {
    match value {
        CompilerOptionsValue::Str(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// Go: `getPropFromRaw(prop, isStringValue, elementTypeName)` — the
/// string-array extraction. `Some(list)` where Go's `propOfRaw.sliceValue`
/// is non-nil (key present with a non-null ARRAY value); a present
/// non-array value pushes Go's `Compiler option '0' requires a value of
/// type 'Array'` diagnostic and yields `None` (Go's nil sliceValue).
fn string_array_prop(
    raw_config: &OrderedMap<String, CompilerOptionsValue>,
    prop: &str,
    errors: &mut Vec<tsc_ast::Diagnostic>,
) -> Option<Vec<String>> {
    let value = raw_config.get(prop)?;
    if value.is_null() {
        return None;
    }
    match value {
        CompilerOptionsValue::StrArray(list) => Some(list.clone()),
        CompilerOptionsValue::Array(list) => {
            let strings: Vec<String> = list
                .iter()
                .filter_map(|v| match v {
                    CompilerOptionsValue::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if strings.len() != list.len() {
                errors.push(new_compiler_diagnostic(
                    &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
                    &[prop.to_string(), "string".to_string()],
                ));
            }
            Some(strings)
        }
        _ => {
            // Go: not a slice → the "requires a value of type 'Array'"
            // diagnostic + wrongValue "not-array" (nil sliceValue).
            errors.push(new_compiler_diagnostic(
                &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
                &[prop.to_string(), "Array".to_string()],
            ));
            None
        }
    }
}

/// Go: `core.StringifyJson(specs, "", "")` for a `[]string` — the compact
/// JSON array form used as the No-inputs diagnostic's arguments
/// (`["a","b"]`; `[]` for the empty list).
fn stringify_json_string_list(values: &[String]) -> String {
    let items: Vec<String> = values
        .iter()
        .map(|v| serde_json::json!(v).to_string())
        .collect();
    format!("[{}]", items.join(","))
}

// ────────────────────────────────────────────────────────────────────────────
// File enumeration (Go: tsconfigparsing.go's getFileNamesFromConfigSpecs and
// its helpers, plus vfs/vfsmatch's SpecMatcher — ported in tsc-module).
// PORT: this surface was explicitly deferred by the M4 wave's PORT notes;
// the showconfig parity twin requires it (every real config enumerates its
// input files), so it is ported here now, mirroring Go line for line.
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetSupportedExtensions(options, extraExtensions) [][]string`.
pub fn get_supported_extensions(
    options: &CompilerOptions,
    extra_extensions: &[String],
) -> Vec<Vec<String>> {
    let need_js_extensions = options.get_allow_js();
    let builtins: &[&[&str]] = if need_js_extensions {
        tsc_tspath::ALL_SUPPORTED_EXTENSIONS
    } else {
        tsc_tspath::SUPPORTED_TS_EXTENSIONS
    };
    let builtins: Vec<Vec<String>> = builtins
        .iter()
        .map(|group| group.iter().map(|s| s.to_string()).collect())
        .collect();
    if extra_extensions.is_empty() {
        return builtins;
    }
    let flat_builtins: Vec<String> = builtins.iter().flatten().cloned().collect();
    let mut result: Vec<Vec<String>> = Vec::new();
    for ext in extra_extensions {
        if !flat_builtins.contains(ext) {
            result.push(vec![ext.clone()]);
        }
    }
    if result.is_empty() {
        return builtins;
    }
    builtins.into_iter().chain(result).collect()
}

/// Go: `func GetSupportedExtensionsWithJsonIfResolveJsonModule(options, supportedExtensions)`.
pub fn get_supported_extensions_with_json_if_resolve_json_module(
    options: &CompilerOptions,
    supported_extensions: Vec<Vec<String>>,
) -> Vec<Vec<String>> {
    if !options.get_resolve_json_module() {
        return supported_extensions;
    }
    let groups: Vec<Vec<&str>> = supported_extensions
        .iter()
        .map(|group| group.iter().map(String::as_str).collect())
        .collect();
    let same = |want: &[&[&str]]| -> bool {
        groups.len() == want.len()
            && groups
                .iter()
                .zip(want)
                .all(|(group, want)| group.len() == want.len() && group.iter().zip(*want).all(|(a, b)| a == b))
    };
    if same(tsc_tspath::ALL_SUPPORTED_EXTENSIONS) {
        return tsc_tspath::ALL_SUPPORTED_EXTENSIONS_WITH_JSON
            .iter()
            .map(|group| group.iter().map(|s| s.to_string()).collect())
            .collect();
    }
    if same(tsc_tspath::SUPPORTED_TS_EXTENSIONS) {
        return tsc_tspath::SUPPORTED_TS_EXTENSIONS_WITH_JSON
            .iter()
            .map(|group| group.iter().map(|s| s.to_string()).collect())
            .collect();
    }
    let mut result = supported_extensions;
    result.push(vec![tsc_tspath::EXTENSION_JSON.to_string()]);
    result
}

/// Go: `func hasFileWithHigherPriorityExtension(file, extensions, hasFile) bool`.
fn has_file_with_higher_priority_extension(
    file: &RootedFilePath,
    extensions: &[Vec<String>],
    has_file: &dyn Fn(&RootedFilePath) -> bool,
) -> bool {
    let mut extension_group: Vec<String> = Vec::new();
    for group in extensions {
        let group_refs: Vec<&str> = group.iter().map(String::as_str).collect();
        if file.extension_is_one_of(&group_refs) {
            extension_group.extend(group.iter().cloned());
        }
    }
    if extension_group.is_empty() {
        return false;
    }
    for ext in &extension_group {
        // d.ts files match the .ts extension; the case-sensitive sort order
        // is d.ts, .ts, .tsx — so don't match .d.ts against .ts.
        if file.extension_is(ext) && (ext != tsc_tspath::EXTENSION_TS || !file.extension_is(tsc_tspath::EXTENSION_DTS)) {
            return false;
        }
        if has_file(&file.change_extension(ext)) {
            if ext == tsc_tspath::EXTENSION_DTS
                && (file.extension_is(tsc_tspath::EXTENSION_JS) || file.extension_is(tsc_tspath::EXTENSION_JSX))
            {
                // LEGACY BEHAVIOR (Go comment): declaration files load
                // alongside their js(x) counterparts.
                continue;
            }
            return true;
        }
    }
    false
}

/// Go: `func removeWildcardFilesWithLowerPriorityExtension[V any]` —
/// removes already-included wildcard files with a lower extension priority.
fn remove_wildcard_files_with_lower_priority_extension(
    file: &RootedFilePath,
    wildcard_files: &mut OrderedMap<PathKey, RootedFilePath>,
    extensions: &[Vec<String>],
    case_sensitivity: CaseSensitivity,
) {
    let mut extension_group: Vec<String> = Vec::new();
    for group in extensions {
        let group_refs: Vec<&str> = group.iter().map(String::as_str).collect();
        if file.extension_is_one_of(&group_refs) {
            extension_group.extend(group.iter().cloned());
        }
    }
    if extension_group.is_empty() {
        return;
    }
    for ext in extension_group.iter().rev() {
        if file.extension_is(ext) {
            return;
        }
        let lower_priority_path = file.change_extension(ext);
        let key = case_sensitivity.path_key(&lower_priority_path.as_path());
        wildcard_files.delete(&key);
    }
}

/// Go: `func getFileNamesFromConfigSpecs(configFileSpecs, basePath, options, host, extraExtensions)` —
/// the literal-files map + wildcard enumeration + JSON include map, merged
/// in Go's order (literal, wildcard, wildcard-json).
///
/// PORT: extraExtensions (content-mapper extensions) is always empty in the
/// M4 surface; the parameter is kept for signature parity.
pub fn get_file_names_from_config_specs(
    config_file_specs: &ConfigFileSpecs,
    base_path: &RootedDirectoryPath,
    options: &CompilerOptions,
    fs: &dyn tsc_vfs::Fs,
) -> Vec<RootedFilePath> {
    let case_sensitivity = fs.case_sensitivity();
    let key_mapper =
        |file_name: &RootedFilePath| -> PathKey { case_sensitivity.path_key(&file_name.as_path()) };

    let mut literal_file_map: OrderedMap<PathKey, RootedFilePath> = OrderedMap::new();
    let mut wildcard_file_map: OrderedMap<PathKey, RootedFilePath> = OrderedMap::new();
    let mut wildcard_json_file_map: OrderedMap<PathKey, RootedFilePath> = OrderedMap::new();

    let validated_file_names = &config_file_specs.validated_file_names;
    let validated_include_specs = &config_file_specs.validated_include_specs;
    let validated_exclude_specs = &config_file_specs.validated_exclude_specs;

    let supported_extensions = get_supported_extensions(options, &[]);
    let supported_extensions_with_json =
        get_supported_extensions_with_json_if_resolve_json_module(options, supported_extensions.clone());

    // Literal files are always included verbatim — include/exclude cannot
    // remove them.
    for file_name in validated_file_names {
        literal_file_map.set(key_mapper(file_name), file_name.clone());
    }

    let mut json_only_include_matcher: Option<tsc_module::SpecMatcher> = None;
    if !validated_include_specs.is_empty() {
        let extensions: Vec<String> = supported_extensions_with_json.iter().flatten().cloned().collect();
        let exclude_strings: Vec<String> = validated_exclude_specs
            .iter()
            .map(|s| s.as_string().to_owned())
            .collect();
        let include_strings: Vec<String> = validated_include_specs
            .iter()
            .map(|s| s.as_string().to_owned())
            .collect();
        let files = tsc_module::read_directory(
            fs,
            base_path,
            &extensions,
            &exclude_strings,
            &include_strings,
            tsc_module::UNLIMITED_DEPTH,
        );
        for file in files {
            if file.extension_is(tsc_tspath::EXTENSION_JSON) {
                if json_only_include_matcher.is_none() {
                    let includes: Vec<String> = validated_include_specs
                        .iter()
                        .map(|s| s.as_string().to_owned())
                        .filter(|include| include.ends_with(tsc_tspath::EXTENSION_JSON))
                        .collect();
                    json_only_include_matcher = tsc_module::new_spec_matcher(
                        &includes,
                        base_path,
                        tsc_module::Usage::Files,
                        case_sensitivity,
                    );
                }
                let mut include_index = -1;
                if let Some(matcher) = &json_only_include_matcher {
                    include_index = matcher.match_file_name_index(&file);
                }
                if include_index != -1 {
                    let key = key_mapper(&file);
                    if !literal_file_map.has(&key) && !wildcard_json_file_map.has(&key) {
                        wildcard_json_file_map.set(key, file);
                    }
                }
                continue;
            }
            // Skip files already included with a higher priority extension.
            if has_file_with_higher_priority_extension(&file, &supported_extensions, &|file_name| {
                let canonical = key_mapper(file_name);
                literal_file_map.has(&canonical) || wildcard_file_map.has(&canonical)
            }) {
                continue;
            }
            remove_wildcard_files_with_lower_priority_extension(
                &file,
                &mut wildcard_file_map,
                &supported_extensions,
                case_sensitivity,
            );
            let key = key_mapper(&file);
            if !literal_file_map.has(&key) && !wildcard_file_map.has(&key) {
                wildcard_file_map.set(key, file);
            }
        }
    }

    let mut files = Vec::with_capacity(
        literal_file_map.size() + wildcard_file_map.size() + wildcard_json_file_map.size(),
    );
    files.extend(literal_file_map.values().cloned());
    files.extend(wildcard_file_map.values().cloned());
    files.extend(wildcard_json_file_map.values().cloned());
    files
}

/// Go: convertCompilerOptionsFromJsonWorker — parse the raw compilerOptions
/// object through the generated table with ${configDir} capture.
/// Go: `func isCompilerOptionsValue(option *CommandLineOption, value any) bool`.
fn is_compiler_options_value(option: &'static CommandLineOption, value: &CompilerOptionsValue) -> bool {
    if value.is_null() {
        return !option.disallow_null_or_undefined();
    }
    match option.kind {
        crate::commandlineoption::CommandLineOptionKind::List => {
            matches!(value, CompilerOptionsValue::Array(_) | CompilerOptionsValue::StrArray(_))
        }
        crate::commandlineoption::CommandLineOptionKind::ListOrElement => {
            matches!(value, CompilerOptionsValue::Array(_) | CompilerOptionsValue::StrArray(_))
                || is_compiler_options_value(
                    option.elements().expect("listOrElement option without element"),
                    value,
                )
        }
        crate::commandlineoption::CommandLineOptionKind::String => {
            matches!(value, CompilerOptionsValue::Str(_))
        }
        crate::commandlineoption::CommandLineOptionKind::Boolean => {
            matches!(value, CompilerOptionsValue::Bool(_))
        }
        crate::commandlineoption::CommandLineOptionKind::Number => {
            matches!(value, CompilerOptionsValue::Int(_) | CompilerOptionsValue::Float(_))
        }
        crate::commandlineoption::CommandLineOptionKind::Object => {
            matches!(value, CompilerOptionsValue::Object(_) | CompilerOptionsValue::PathsMap(_))
        }
        crate::commandlineoption::CommandLineOptionKind::Enum => {
            matches!(value, CompilerOptionsValue::Str(_))
        }
    }
}

/// Go: `func normalizeNonListOptionValue(option, basePath, value) any`.
fn normalize_non_list_option_value(
    option: &'static CommandLineOption,
    base_path: &RootedDirectoryPath,
    value: CompilerOptionsValue,
) -> CompilerOptionsValue {
    if option.path_kind.is_rooted() {
        if let CompilerOptionsValue::Str(path) = &value {
            let mut path = tsc_tspath::normalize_slashes(path).into_owned();
            if !starts_with_config_dir_template(&path) {
                path = get_normalized_absolute_path(&path, base_path).into_owned();
            }
            if path.is_empty() {
                path = ".".to_string();
            }
            return CompilerOptionsValue::Str(path);
        }
    }
    value
}

/// Go: `func convertJsonOptionOfListType(option, values, basePath, ...) ([]any, []*ast.Diagnostic)`.
fn convert_json_option_of_list_type(
    option: &'static CommandLineOption,
    values: &CompilerOptionsValue,
    base_path: &RootedDirectoryPath,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let mut errors: Vec<tsc_ast::Diagnostic> = Vec::new();
    let element = option.elements().expect("list option without element");
    let list = match values {
        CompilerOptionsValue::Array(list) => list.clone(),
        // A JSON array of strings parses as StrArray — same slice shape.
        CompilerOptionsValue::StrArray(list) => list
            .iter()
            .map(|s| CompilerOptionsValue::Str(s.clone()))
            .collect(),
        _ => return (CompilerOptionsValue::Null, errors),
    };
    let converted = convert_json_option_of_list_values(
        element,
        &list,
        base_path,
        option.list_preserve_falsy_values,
        &mut errors,
    );
    (converted, errors)
}

/// The element-mapping + falsy-filter half of `convertJsonOptionOfListType`
/// (Go's `core.MapIndex` + `core.Filter`); the per-element diagnostics append
/// to `errors` exactly as Go's shared slice does.
fn convert_json_option_of_list_values(
    element: &'static CommandLineOption,
    list: &[CompilerOptionsValue],
    base_path: &RootedDirectoryPath,
    list_preserve_falsy_values: bool,
    errors: &mut Vec<tsc_ast::Diagnostic>,
) -> CompilerOptionsValue {
    let mut mapped_values: Vec<CompilerOptionsValue> = Vec::with_capacity(list.len());
    for value in list {
        let (result, element_errors) = convert_json_option(element, value, base_path);
        mapped_values.push(result);
        errors.extend(element_errors);
    }
    if !list_preserve_falsy_values {
        mapped_values.retain(|value| !is_falsy_value(value));
    }
    CompilerOptionsValue::Array(mapped_values)
}

/// Go: `v != nil && v != false && v != 0 && v != ""`.
fn is_falsy_value(value: &CompilerOptionsValue) -> bool {
    match value {
        CompilerOptionsValue::Null => true,
        CompilerOptionsValue::Bool(b) => !*b,
        CompilerOptionsValue::Int(i) => *i == 0,
        CompilerOptionsValue::Float(f) => *f == 0.0,
        CompilerOptionsValue::Str(s) => s.is_empty(),
        _ => false,
    }
}

/// Go: `func convertJsonOption(opt, value, basePath, propertyAssignment, valueExpression, sourceFile) (any, []*ast.Diagnostic)` —
/// the M4 subset (no AST nodes; see module PORT notes).
fn convert_json_option(
    opt: &'static CommandLineOption,
    value: &CompilerOptionsValue,
    base_path: &RootedDirectoryPath,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    if opt.is_command_line_only {
        return (
            CompilerOptionsValue::Null,
            vec![new_compiler_diagnostic(
                &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_SPECIFIED_ON_COMMAND_LINE,
                &[opt.name.to_string()],
            )],
        );
    }
    if !is_compiler_options_value(opt, value) {
        return (
            CompilerOptionsValue::Null,
            vec![new_compiler_diagnostic(
                &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
                &[
                    opt.name.to_string(),
                    crate::errors::get_compiler_option_value_type_string(opt),
                ],
            )],
        );
    }
    match opt.kind {
        crate::commandlineoption::CommandLineOptionKind::List => {
            convert_json_option_of_list_type(opt, value, base_path)
        }
        crate::commandlineoption::CommandLineOptionKind::ListOrElement => {
            if matches!(value, CompilerOptionsValue::Array(_) | CompilerOptionsValue::StrArray(_)) {
                convert_json_option_of_list_type(opt, value, base_path)
            } else {
                convert_json_option(opt.elements().expect("element"), value, base_path)
            }
        }
        crate::commandlineoption::CommandLineOptionKind::Enum => {
            if value.is_null() {
                return (CompilerOptionsValue::Null, Vec::new());
            }
            let CompilerOptionsValue::Str(s) = value else {
                return (CompilerOptionsValue::Null, Vec::new());
            };
            crate::commandlineparser::convert_json_option_of_enum_type(opt, s)
        }
        _ => {
            let (validated, errors) =
                crate::commandlineparser::validate_json_option_value(opt, value.clone());
            if !errors.is_empty() {
                return (CompilerOptionsValue::Null, errors);
            }
            match validated {
                Some(value) => (normalize_non_list_option_value(opt, base_path, value), Vec::new()),
                None => (CompilerOptionsValue::Null, Vec::new()),
            }
        }
    }
}

/// Go: `func convertOptionsFromJson[O optionParser](optionsNameMap, jsonOptions, basePath, result) (O, []*ast.Diagnostic)` —
/// the parsedCompilerOptions instantiation the parse worker uses.
fn convert_options_from_json_worker(
    json_options: CompilerOptionsValue,
    base_path: &RootedDirectoryPath,
) -> (ParsedCompilerOptions, Vec<tsc_ast::Diagnostic>) {
    let mut parsed = ParsedCompilerOptions::new(CompilerOptions::default());
    let mut errors = Vec::new();
    let CompilerOptionsValue::Object(map) = &json_options else {
        return (parsed, errors);
    };
    let options_name_map = command_line_compiler_options_map();
    for (key, value) in map.entries() {
        let Some(option) = options_name_map.get(key) else {
            errors.push(crate::errors::create_unknown_option_error(
                key,
                ParsedCompilerOptions::unknown_option_diagnostic()
                    .expect("compiler options parser diagnostic"),
                "",
                None,
                None,
                None,
                ParsedCompilerOptions::unknown_did_you_mean_diagnostic(),
                Some(options_name_map),
            ));
            continue;
        };
        if option.name != key.as_str() {
            // Case-insensitive match found but exact case doesn't match —
            // provide the "did you mean" suggestion (Go: skips the option).
            errors.push(new_compiler_diagnostic(
                ParsedCompilerOptions::unknown_did_you_mean_diagnostic()
                    .expect("compiler options did-you-mean diagnostic"),
                &[key.clone(), option.name.to_string()],
            ));
            continue;
        }
        let (converted, conversion_errors) = convert_json_option(option, value, base_path);
        errors.extend(conversion_errors);
        parsed.parse_option(key, &converted);
    }
    // ${configDir} templates substitute against the config's directory.
    crate::options_generated::handle_option_config_dir_template_substitution(&mut parsed, base_path);
    (parsed, errors)
}

/// Go: readJsonConfigFile + getExtendedConfig — resolve one `extends` value.
///
/// PORT: Go's getExtendedConfig fully parses the extended config (recursively
/// resolving its own extends first, then merging its own compilerOptions on
/// top); this returns the same three views — the extended raw map (for
/// files/include/exclude/references, with nested-extends spec inheritance
/// and the relative rewrite already applied, exactly like Go's merged
/// `ownConfig.raw`), its fully-merged parsed options, and the resolved path
/// (the rewrite anchor for the extending config).
#[allow(clippy::too_many_arguments)]
fn read_extended_config(
    extends_path: &str,
    fs: &dyn tsc_vfs::Fs,
    base_path: &RootedDirectoryPath,
    config_file_name: &RootedFilePath,
    resolution_stack: &mut Vec<PathKey>,
    extended_config_cache: Option<&ExtendedConfigCache>,
    resolve_config_host: Option<&dyn ResolveConfigHost>,
    errors: &mut Vec<tsc_ast::Diagnostic>,
) -> Option<(
    OrderedMap<String, CompilerOptionsValue>,
    ParsedCompilerOptions,
    RootedFilePath,
)> {
    // Module specifier arm (seam'd to the module crate; see ResolveConfigHost).
    let resolved = if !extends_path.is_empty()
        && !extends_path.starts_with('/')
        && !extends_path.starts_with("./")
        && !extends_path.starts_with("../")
    {
        resolve_config_host.and_then(|host| host.resolve(extends_path, config_file_name))?
    } else {
        let normalized = if starts_with_config_dir_template(extends_path) {
            let trimmed = extends_path
                .strip_prefix(crate::parsinghelpers::CONFIG_DIR_TEMPLATE)
                .unwrap_or(extends_path);
            get_normalized_absolute_path(trimmed, base_path)
        } else {
            get_normalized_absolute_path(extends_path, base_path)
        };
        // Go appends /tsconfig.json when the extends target is a directory
        // or a basename without extension (tsconfig.json/tsconfig.*/...).
        resolve_extends_target(&normalized, fs)
    };

    let path_key = tsc_tspath::path_key_from_canonical(resolved.as_string());
    if resolution_stack.contains(&path_key) {
        // Go: the cycle diagnostic (path_in_an_extends_option) — the AST-based
        // variant text; a raw diagnostic keeps the cycle message.
        errors.push(json_parse_error_diagnostic(&format!(
            "circularity detected while resolving configurations: '{extends_path}'"
        )));
        return None;
    }
    if let Some(cache) = extended_config_cache
        && let Some(CompilerOptionsValue::Object(map)) = cache(extends_path)
    {
        // PORT: the cache seam stores the raw map (Go caches the parsed
        // command line); the parsed form is rebuilt from it.
        let (parsed, cache_errors) =
            convert_options_from_json_worker(map.get("compilerOptions").cloned().unwrap_or(CompilerOptionsValue::Null), &resolved.directory());
        errors.extend(cache_errors);
        return Some((map.clone(), parsed, resolved));
    }
    resolution_stack.push(path_key);

    let text = fs.read_file(&resolved).or_else(|| {
        errors.push(new_compiler_diagnostic(
            &tsc_diagnostics::FILE_0_NOT_FOUND,
            &[resolved.as_string().to_owned()],
        ));
        None
    })?;
    let (json, mut parse_errors) = parse_config_file_text_to_json(&text);
    errors.append(&mut parse_errors);
    let Some(CompilerOptionsValue::Object(mut map)) = json else {
        return None;
    };

    let extended_base = resolved.directory();
    let case_sensitivity = fs.case_sensitivity();

    // Nested extends first (Go: the extended config's own parse resolves its
    // extends before its own compilerOptions merge on top; the nested specs
    // inherit into this level's raw with the relative rewrite, exactly like
    // Go's applyExtendedConfig setPropertyValue).
    let mut parsed = if let Some(extends_value) = map.get("extends") {
        let mut nested = ParsedCompilerOptions::new(CompilerOptions::default());
        for nested_extends_path in extends_value_list(extends_value) {
            if let Some((nested_map, nested_parsed, nested_path)) = read_extended_config(
                &nested_extends_path,
                fs,
                &extended_base,
                &resolved,
                resolution_stack,
                extended_config_cache,
                resolve_config_host,
                errors,
            ) {
                merge_parsed_compiler_options(
                    &mut nested,
                    Some(&nested_parsed),
                    Some(&CompilerOptionsValue::Object(nested_map.clone())),
                );
                merge_extended_raw_into_config(
                    &mut map,
                    &nested_map,
                    &nested_path.directory(),
                    &extended_base,
                    case_sensitivity,
                );
            }
        }
        nested
    } else {
        ParsedCompilerOptions::new(CompilerOptions::default())
    };

    // Then the extended config's own compilerOptions on top.
    if let Some(compiler_options_value) = map.get("compilerOptions") {
        let (own, conversion_errors) =
            convert_options_from_json_worker(compiler_options_value.clone(), &extended_base);
        errors.extend(conversion_errors);
        merge_parsed_compiler_options(
            &mut parsed,
            Some(&own),
            Some(&CompilerOptionsValue::Object(map.clone())),
        );
    }

    Some((map, parsed, resolved))
}

/// Go: `getExtendsConfigPathOrArray`'s value-shape flattening.
fn extends_value_list(extends_value: &CompilerOptionsValue) -> Vec<String> {
    match extends_value {
        CompilerOptionsValue::Str(s) => vec![s.clone()],
        CompilerOptionsValue::StrArray(arr) => arr.clone(),
        CompilerOptionsValue::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                CompilerOptionsValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Go: getExtendsConfigPath's extension-adding logic (tsconfig.json /
/// tsconfig.*.json basenames and directories).
fn resolve_extends_target(
    normalized: &str,
    fs: &dyn tsc_vfs::Fs,
) -> RootedFilePath {
    let as_file = tsc_tspath::rooted_file_path_from_normalized(normalized);
    if fs.file_exists(&as_file) {
        return as_file;
    }
    // `./tsconfig-base` → `./tsconfig-base.json`; `./base-dir` → its
    // `tsconfig.json`.
    let with_json = format!("{normalized}.json");
    let as_file = tsc_tspath::rooted_file_path_from_normalized(&with_json);
    if fs.file_exists(&as_file) {
        return as_file;
    }
    let directory = tsc_tspath::rooted_directory_path_from_normalized(normalized);
    let with_tsconfig = get_normalized_absolute_path("tsconfig.json", &directory);
    tsc_tspath::rooted_file_path_from_normalized(&with_tsconfig)
}

/// Go: `func validateSpecs(specs any, disallowTrailingRecursion bool, specKey string) ([]tspath.PathPattern, []*ast.Diagnostic)`.
pub fn validate_specs(
    specs: &CompilerOptionsValue,
    disallow_trailing_recursion: bool,
    spec_key: &str,
) -> (Vec<tsc_tspath::PathPattern>, Vec<tsc_ast::Diagnostic>) {
    let mut errors = Vec::new();
    let mut result = Vec::new();
    let list = match specs {
        CompilerOptionsValue::StrArray(arr) => arr.clone(),
        CompilerOptionsValue::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                CompilerOptionsValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        CompilerOptionsValue::Str(s) => vec![s.clone()],
        _ => Vec::new(),
    };
    for spec in &list {
        if let Some(message) = spec_to_diagnostic(spec, disallow_trailing_recursion) {
            errors.push(new_compiler_diagnostic(message, std::slice::from_ref(spec)));
            continue;
        }
        result.push(to_path_pattern(spec));
    }
    let _ = spec_key;
    (result, errors)
}

/// Go: `func specToDiagnostic(spec string, disallowTrailingRecursion bool) *diagnostics.Message`.
fn spec_to_diagnostic(
    spec: &str,
    disallow_trailing_recursion: bool,
) -> Option<&'static Message> {
    if disallow_trailing_recursion && invalid_trailing_recursion(spec) {
        return Some(
            &tsc_diagnostics::FILE_SPECIFICATION_CANNOT_END_IN_A_RECURSIVE_DIRECTORY_WILDCARD_ASTERISK_ASTERISK_COLON_0,
        );
    }
    if invalid_dot_dot_after_recursive_wildcard(spec) {
        return Some(
            &tsc_diagnostics::FILE_SPECIFICATION_CANNOT_CONTAIN_A_PARENT_DIRECTORY_THAT_APPEARS_AFTER_A_RECURSIVE_DIRECTORY_WILDCARD_ASTERISK_ASTERISK_COLON_0,
        );
    }
    None
}

/// Go: `func invalidTrailingRecursion(spec string) bool` — matches `**`,
/// `/**`, `**/` and `/**/`, but not `a**b`.
fn invalid_trailing_recursion(spec: &str) -> bool {
    let s = spec.strip_suffix('/').unwrap_or(spec);
    s == "**" || s.ends_with("/**")
}

/// Go: `func invalidDotDotAfterRecursiveWildcard(s string) bool` — true when
/// some `..` segment comes after some `**/` segment (the regex-free rework).
fn invalid_dot_dot_after_recursive_wildcard(s: &str) -> bool {
    let wildcard_index = if s.starts_with("**/") {
        Some(0)
    } else {
        s.find("/**/")
    };
    let Some(wildcard_index) = wildcard_index else {
        return false;
    };
    let last_dot_index = if s.ends_with("/..") {
        Some(s.len())
    } else {
        s.rfind("/../")
    };
    last_dot_index.is_some_and(|last_dot| last_dot > wildcard_index)
}

/// Go: `func GetParsedCommandLineOfConfigFilePath(...)` — the --showConfig
/// runner's entry: read + parse + resolve one config file.
pub fn get_parsed_command_line_of_config_file_path(
    config_file_name: &RootedFilePath,
    fs: &dyn tsc_vfs::Fs,
    resolve_config_host: Option<&dyn ResolveConfigHost>,
) -> ParsedCommandLine {
    let base_path = config_file_name.directory();
    let text = match fs.read_file(config_file_name) {
        Some(text) => text,
        None => {
            let mut result = ParsedCommandLine::new(
                config_file_name.clone(),
                base_path,
                fs.case_sensitivity(),
            );
            result.errors.push(new_compiler_diagnostic(
                &tsc_diagnostics::FILE_0_NOT_FOUND,
                &[config_file_name.as_string().to_owned()],
            ));
            return result;
        }
    };
    let (json, parse_errors) = parse_config_file_text_to_json(&text);
    let mut result = parse_json_config_file_content(
        &json.unwrap_or(CompilerOptionsValue::Null),
        fs,
        &base_path,
        None,
        config_file_name,
        &[],
        None,
        resolve_config_host,
    );
    result.errors.extend(parse_errors);
    result
}

/// Kept for the future file-enumeration path (getFileNamesFromConfigSpecs);
/// listed so the seam surface is stable.
#[allow(dead_code)]
pub fn supported_extensions_hint(_options: &CompilerOptions) -> Vec<Vec<&'static str>> {
    vec![vec![".ts", ".tsx", ".d.ts"]]
}

/// Marker used by tests to assert the resolution-stack type matches Go.
#[allow(dead_code)]
fn _resolution_stack_type(_stack: &HashSet<PathKey>) {}
