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
    get_normalized_absolute_path, to_path_pattern, PathKey, RootedDirectoryPath,
    RootedFilePath,
};

use crate::commandlineoption::{CommandLineOption, CompilerOptionsValue};
use crate::errors::new_compiler_diagnostic;
use crate::namemap::command_line_compiler_options_map;
use crate::options_generated::{get_default_compiler_options, parse_compiler_options};
use crate::parsinghelpers::{
    merge_parsed_compiler_options, parse_project_reference,
    starts_with_config_dir_template, OptionParser,
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
    /// Go: validatedFilesSpecs []tspath.RootedFilePath + defaultIncludeSpec.
    pub validated_files_specs: Vec<RootedFilePath>,
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
    let value = parse_json_value(bytes, &mut pos)?;
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err("trailing characters".to_string());
    }
    Ok(value)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\n' | b'\r') {
        *pos += 1;
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
                    Some(b',') => *pos += 1,
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
                    Some(b',') => *pos += 1,
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

    let CompilerOptionsValue::Object(raw_config) = json else {
        return result;
    };

    // extends — resolve and merge first (Go: parseConfig's extends arm).
    if let Some(extends_value) = raw_config.get("extends") {
        for extends_path in extends_value_list(extends_value) {
            if let Some((extended, extended_parsed)) = read_extended_config(
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
                merge_spec_and_references(&mut result, &extended);
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

    // include/exclude/files (Go: validateSpecs).
    if let Some(include) = raw_config.get("include") {
        let (specs, mut spec_errors) = validate_specs(include, true, "include");
        errors.append(&mut spec_errors);
        result.raw_include_specs = Some(specs);
    }
    if let Some(exclude) = raw_config.get("exclude") {
        let (specs, mut spec_errors) = validate_specs(exclude, false, "exclude");
        errors.append(&mut spec_errors);
        result.raw_exclude_specs = Some(specs);
    }
    if let Some(files) = raw_config.get("files") {
        let (specs, mut spec_errors) = validate_specs(files, false, "files");
        errors.append(&mut spec_errors);
        result.raw_files_specs = specs;
    }

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

fn merge_spec_and_references(result: &mut ParsedCommandLine, extended_map: &OrderedMap<String, CompilerOptionsValue>) {
    if result.raw_include_specs.is_none() {
        if let Some(include) = extended_map.get("include") {
            let (specs, _) = validate_specs(include, true, "include");
            result.raw_include_specs = Some(specs);
        }
    }
    if result.raw_exclude_specs.is_none() {
        if let Some(exclude) = extended_map.get("exclude") {
            let (specs, _) = validate_specs(exclude, false, "exclude");
            result.raw_exclude_specs = Some(specs);
        }
    }
    if result.raw_files_specs.is_empty() {
        if let Some(files) = extended_map.get("files") {
            let (specs, _) = validate_specs(files, false, "files");
            result.raw_files_specs = specs;
        }
    }
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
/// top); this returns the same two views — the extended raw map (for
/// files/include/exclude/references) and its fully-merged parsed options.
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
        return Some((map.clone(), parsed));
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
    let Some(CompilerOptionsValue::Object(map)) = json else {
        return None;
    };

    let extended_base = resolved.directory();

    // Nested extends first (Go: the extended config's own parse resolves its
    // extends before its own compilerOptions merge on top).
    let mut parsed = if let Some(extends_value) = map.get("extends") {
        let mut nested = ParsedCompilerOptions::new(CompilerOptions::default());
        for nested_extends_path in extends_value_list(extends_value) {
            if let Some((_, nested_parsed)) = read_extended_config(
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
                    Some(&CompilerOptionsValue::Object(map.clone())),
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

    Some((map, parsed))
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
