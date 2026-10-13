// Port of tsc/internal/tsoptions/tsconfigparsing.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes (M4 scope — see PORTING-NOTES.md for the full list):
//   - Go parses the tsconfig as an AST SourceFile (ParseConfigFileTextToJson
//     → convertObjectLiteralExpressionToJson) so diagnostics can carry
//     node positions. The port parses the JSON text directly into the
//     `CompilerOptionsValue` tree (a small recursive-descent parser below)
//     — same values, same key order. The M6 config-node wave lands the
//     AST half for the `GetParsedCommandLineOfConfigFilePath` route: the
//     text parses as a JSON SourceFile too (`TsConfigSourceFile`, the
//     node-position surface for option diagnostics + include-reason
//     related info — see the config-syntax section below). The AST→value
//     conversion (`convertConfigFileToObject`) and its span-carrying parse
//     diagnostics stay deferred to the full AST-config wave; on that route
//     the values/errors still come from the text parser.
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
use std::sync::{Arc, Mutex};

use tsc_collections::OrderedMap;
use tsc_core::options_generated::CompilerOptions;
use tsc_diagnostics::Message;
use tsc_json::Value;
use tsc_ast::{NodeId, NodeStore};
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
    /// Go: `validatedFilesSpecBeforeSubstitution []tspath.FileSpec` — the
    /// `files` entries as written (pre-${configDir}); the matched-file-spec
    /// reasons report these (M6 config-node wave).
    pub validated_files_specs_before_substitution: Vec<tsc_tspath::FileSpec>,
    /// Go: `validatedIncludeSpecsBeforeSubstitution []tspath.PathPattern` —
    /// the include entries as written (pre-${configDir}).
    pub validated_include_specs_before_substitution: Vec<tsc_tspath::PathPattern>,
    /// Go: `fileSpecByPath map[tspath.PathKey]tspath.FileSpec` — each
    /// files-list entry's path → the spec as written (first occurrence
    /// wins, exactly Go's `if !exists` guard).
    pub file_spec_by_path: rustc_hash::FxHashMap<PathKey, tsc_tspath::FileSpec>,
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
// TsConfigSourceFile + the AST config-syntax walks (Go:
// tsconfigparsing.go's TsConfigSourceFile/NewTsconfigSourceFileFromFilePath/
// tsconfigToSourceFile/GetTsConfigPropArrayElementValue/ForEachTsConfigPropArray/
// GetCallbackForFindingPropertyAssignmentByValue/GetOptionsSyntaxByArrayElementValue/
// getTsConfigObjectLiteralExpression/ForEachPropertyAssignment — the M6
// config-node diagnostic surface: option diagnostics and include-reason
// related info carry positions read out of the config file parsed as a
// JSON SourceFile)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type TsConfigSourceFile struct`.
pub struct TsConfigSourceFile {
    /// Go: `ExtendedSourceFiles []tspath.RootedFilePath` — populated by the
    /// extends-chain walk (getExtendedConfig). PORT: the port's extends path
    /// does not track the chain's source files yet (the extended-config
    /// AST-config wave — PORTING-NOTES); the field keeps the Go shape.
    pub extended_source_files: Vec<RootedFilePath>,
    /// Go: `SourceFile *ast.SourceFile` — the config text parsed as a JSON
    /// SourceFile (the node-position surface).
    ///
    /// PORT (Send/Sync): the workspace forbids `unsafe` and
    /// `tsc_ast::SourceFile` (parser-era `Cell` fields) is neither `Sync`
    /// nor `Arc`-shareable, while `ParsedCommandLine` must stay `Send +
    /// Sync` (the loader's worker groups share it) — the config AST rides
    /// behind a `Mutex`, read through a `MutexGuard`. Go shares the frozen
    /// pointer freely; the config file is immutable after parse, so the
    /// lock is pure ceremony around read-only walks. Deadlock rule (both
    /// lock sites documented): never call the include processor's
    /// `getCompilerOptionsObjectLiteralSyntax` (which locks) while holding
    /// a guard — `to_related_info` computes the memo BEFORE locking.
    source_file: Mutex<tsc_ast::SourceFile>,
}

/// PORT: `tsc_ast::SourceFile` carries no `Debug` (the arena is huge); the
/// derived-equal manual impl prints the config file's name instead.
impl std::fmt::Debug for TsConfigSourceFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TsConfigSourceFile")
            .field("extended_source_files", &self.extended_source_files)
            .field("source_file", &self.lock_source_file().file_name().as_string())
            .finish()
    }
}

impl TsConfigSourceFile {
    /// Go: the `SourceFile` field — the parsed config AST, read through the
    /// guard (the file is frozen at parse; see the struct PORT note).
    pub fn lock_source_file(&self) -> std::sync::MutexGuard<'_, tsc_ast::SourceFile> {
        self.source_file
            .lock()
            .expect("config source file poisoned")
    }
}

/// Go: `func tsconfigToSourceFile(tsconfigSourceFile *TsConfigSourceFile) *ast.SourceFile` —
/// the locked-read form (see TsConfigSourceFile's Send/Sync PORT note).
pub fn tsconfig_to_source_file(
    tsconfig_source_file: Option<&TsConfigSourceFile>,
) -> Option<std::sync::MutexGuard<'_, tsc_ast::SourceFile>> {
    tsconfig_source_file.map(|config| config.lock_source_file())
}

/// Go: `func NewTsconfigSourceFileFromFilePath(configFileName, configPath, configSourceText) *TsConfigSourceFile`.
///
/// PORT: Go's `configPath` parameter is the caller-computed `PathKey`; the
/// port derives it from the file name through `case_sensitivity` (the one
/// Rust caller has the FS at hand — `get_parsed_command_line_of_config_file_path`).
pub fn new_tsconfig_source_file_from_file_path(
    config_file_name: &RootedFilePath,
    case_sensitivity: CaseSensitivity,
    config_source_text: &str,
) -> TsConfigSourceFile {
    let path_key = case_sensitivity.path_key(&config_file_name.as_path());
    let source_file = tsc_parser::parse_source_file(
        tsc_ast::SourceFileParseOptions {
            file_name: config_file_name.clone(),
            path_key,
            ..Default::default()
        },
        config_source_text,
        tsc_core::scriptkind::ScriptKind::JSON,
    );
    TsConfigSourceFile {
        extended_source_files: Vec::new(),
        source_file: Mutex::new(source_file),
    }
}

/// Go: `func getTsConfigObjectLiteralExpression(tsConfigSourceFile *ast.SourceFile) *ast.ObjectLiteralExpression`.
pub fn get_ts_config_object_literal_expression(
    ts_config_source_file: &tsc_ast::SourceFile,
) -> Option<NodeId> {
    let statements = ts_config_source_file.payload().statements.as_ref()?;
    let first = *statements.nodes.first()?;
    let expression = crate::ast_shims::expression_of(ts_config_source_file, first)?;
    if tsc_ast::is_object_literal_expression(ts_config_source_file.node(expression)) {
        return Some(expression);
    }
    None
}

/// Go: `func ForEachPropertyAssignment[T any](objectLiteral, key string, callback, key2 ...) *T`.
///
/// PORT: the callback receives the property's node handle (Go receives the
/// typed `*ast.PropertyAssignment`; the caller resolves name/initializer
/// through the same `store`). Go's `return callback(...)` semantics are
/// exact: the FIRST name-matching property decides the result, including a
/// `None` callback result (no continued search).
pub fn for_each_property_assignment<T>(
    store: &dyn tsc_ast::NodeStore,
    object_literal: Option<NodeId>,
    key: &str,
    key2: &str,
    callback: impl FnOnce(&dyn tsc_ast::NodeStore, NodeId) -> Option<T>,
) -> Option<T> {
    let object_literal = object_literal?;
    let data = store
        .node(object_literal)
        .as_object_literal_expression()
        .expect("IsObjectLiteralExpression guaranteed by the walk producers")
        .clone();
    let properties = data.properties?;
    for property in properties.nodes.iter() {
        let node = store.node(*property);
        if !tsc_ast::is_property_assignment(node) {
            continue;
        }
        let name = node.name().expect("PropertyAssignment name");
        if let Some(prop_name) = crate::ast_shims::try_get_text_of_property_name(store, name) {
            if prop_name == key || (!key2.is_empty() && prop_name == key2) {
                return callback(store, *property);
            }
        }
    }
    None
}

/// Go: `func ForEachTsConfigPropArray[T any](tsConfigSourceFile, propKey, callback) *T`.
pub fn for_each_ts_config_prop_array<T>(
    ts_config_source_file: Option<&tsc_ast::SourceFile>,
    prop_key: &str,
    callback: impl FnOnce(&dyn tsc_ast::NodeStore, NodeId) -> Option<T>,
) -> Option<T> {
    if let Some(source_file) = ts_config_source_file {
        let object_literal = get_ts_config_object_literal_expression(source_file);
        return for_each_property_assignment(
            source_file,
            object_literal,
            prop_key,
            "",
            callback,
        );
    }
    None
}

/// Go: `func GetCallbackForFindingPropertyAssignmentByValue(value string) func(property *ast.PropertyAssignment) *ast.Node` —
/// the element-search helper as a callable (Go's closure factory is public;
/// the port exposes the equivalent per-property step for the `target`-value
/// related-info arm in file_include.rs).
pub fn get_callback_for_finding_property_assignment_by_value<'a>(
    store: &'a dyn tsc_ast::NodeStore,
    value: &'a str,
) -> impl Fn(NodeId) -> Option<NodeId> + 'a {
    move |property: NodeId| find_property_assignment_by_value(store, property, value)
}

/// The shared body of Go's callback factory (see above).
fn find_property_assignment_by_value(
    store: &dyn tsc_ast::NodeStore,
    property: NodeId,
    value: &str,
) -> Option<NodeId> {
    let initializer = store
        .node(property)
        .as_property_assignment()
        .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
        .initializer;
    if initializer.is_none() {
        // Go: nil Initializer → IsArrayLiteralExpression nil-receiver false.
        return None;
    }
    let initializer_node = store.node(initializer);
    if !tsc_ast::is_array_literal_expression(initializer_node) {
        return None;
    }
    let elements = initializer_node
        .as_array_literal_expression()
        .expect("IsArrayLiteralExpression checked")
        .elements
        .clone();
    let elements = elements?;
    elements.nodes.iter().find_map(|element| {
        let node = store.node(*element);
        (tsc_ast::is_string_literal(node)
            && node.as_string_literal().expect("StringLiteral data").text.as_ref() == value)
            .then_some(*element)
    })
}

/// Go: `func GetTsConfigPropArrayElementValue(tsConfigSourceFile, propKey, elementValue) *ast.StringLiteral` —
/// the `files`/`include` array element whose text equals `element_value`
/// (the include-reason related-info anchor).
pub fn get_ts_config_prop_array_element_value(
    ts_config_source_file: Option<&tsc_ast::SourceFile>,
    prop_key: &str,
    element_value: &str,
) -> Option<NodeId> {
    for_each_ts_config_prop_array(ts_config_source_file, prop_key, |store, property| {
        find_property_assignment_by_value(store, property, element_value)
    })
}

/// Go: `func GetOptionsSyntaxByArrayElementValue(objectLiteral, propKey, elementValue) *ast.Node` —
/// the same element search rooted at a compilerOptions object literal (the
/// `types`/`lib` related-info arms).
pub fn get_options_syntax_by_array_element_value(
    store: &dyn tsc_ast::NodeStore,
    object_literal: Option<NodeId>,
    prop_key: &str,
    element_value: &str,
) -> Option<NodeId> {
    for_each_property_assignment(store, object_literal, prop_key, "", |store, property| {
        find_property_assignment_by_value(store, property, element_value)
    })
}

// ────────────────────────────────────────────────────────────────────────────
// AST → value conversion (Go: tsconfigparsing.go's convertConfigFileToObject/
// convertToJson/convertPropertyValueToJson/convertObjectLiteralExpressionToJson/
// convertArrayLiteralExpressionToJson/isDoubleQuotedString — the M6
// config-file wave: the config-file route's VALUES come from the config AST
// now, and the conversion's span-carrying diagnostics (the `?` token, the
// non-property-assignment elements) ride `ParsedCommandLine.errors` exactly
// like Go's)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isDoubleQuotedString(node *ast.Node) bool` = `ast.IsStringLiteral(node)`
/// — trivially true at its only call site (the conversion's StringLiteral arm
/// already matched the kind); kept verbatim so the arm reads like Go's. The
/// unquoted-string diagnostics come from the JSON PARSE (ScriptKindJSON),
/// riding the config SourceFile — never from the conversion.
fn is_double_quoted_string(node: &tsc_ast::Node) -> bool {
    tsc_ast::is_string_literal(node)
}

/// Go: `func IsComputedNonLiteralName(name) bool` (ast/utilities.go) — the
/// crate-local shim (the ast crate's PORT gap; see ast_shims.rs).
fn is_computed_non_literal_name(store: &dyn tsc_ast::NodeStore, name: tsc_ast::NodeId) -> bool {
    let node = store.node(name);
    if node.kind != tsc_ast::Kind::ComputedPropertyName {
        return false;
    }
    let expression = node
        .as_computed_property_name()
        .expect("ComputedPropertyName data")
        .expression;
    let expression_node = store.node(expression);
    !(tsc_ast::is_string_literal_like(expression_node)
        || tsc_ast::is_numeric_literal(expression_node))
}

/// Go: `ast.NewDiagnostic(file, loc, message, args...)` — the RAW-loc form
/// (vs. `CreateDiagnosticForNodeInSourceFile`'s skip-trivia node range); the
/// conversion's diagnostics use the node's `Loc` exactly as Go does.
fn new_diagnostic_at_loc(
    source_file: &tsc_ast::SourceFile,
    loc: tsc_core::text::TextRange,
    message: &'static tsc_diagnostics::Message,
    args: &[String],
) -> tsc_ast::Diagnostic {
    let text = tsc_diagnostics::format(message.text(), args);
    tsc_ast::Diagnostic::new(
        Some(source_file.as_node_id()),
        loc,
        message.code(),
        message.category(),
        "",
        Some(message),
        text,
        message.key(),
        args.to_vec(),
    )
}

/// Go: `float64(jsnum.FromString(text))` — the value carries as `Int` when
/// integral else `Float`, matching the text parser's shape (the parsed twin
/// proves both forms; Go's `any` is float64 either way and every downstream
/// consumer accepts both — PORTING-NOTES).
fn numeric_literal_value(text: &str, negate: bool) -> CompilerOptionsValue {
    let number: f64 = tsc_jsnum::from_string(text).0;
    let number = if negate { -number } else { number };
    if number.fract() == 0.0 && number.is_finite() && number.abs() < 9.0e15 {
        CompilerOptionsValue::Int(number as i64)
    } else {
        CompilerOptionsValue::Float(number)
    }
}

/// Go: `func convertConfigFileToObject(sourceFile, jsonConversionNotifier) (any, []*ast.Diagnostic)`.
///
/// PORT (notifier residual): Go's `jsonConversionNotifier` (the per-key
/// option-typed conversion + `parseOwnConfigOfJsonSourceFile`'s
/// compilerOptions/typeAcquisition/root arms) is NOT ported this wave — both
/// routes call with the nil notifier, so this is the pure value walk; the
/// option-typed normalization keeps happening in the shared worker's
/// convertOptionsFromJson arm. The notifier's positioned unknown-option and
/// root-compiler-option diagnostics stay deferred to the full AST-config
/// wave (PORTING-NOTES).
pub fn convert_config_file_to_object(
    source_file: &tsc_ast::SourceFile,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let root_expression = source_file
        .payload()
        .statements
        .as_ref()
        .and_then(|statements| statements.nodes.first())
        .and_then(|first| crate::ast_shims::expression_of(source_file, *first));
    if let Some(root_expression) = root_expression {
        if !tsc_ast::is_object_literal_expression(source_file.node(root_expression)) {
            let base_file_name = if source_file.file_name().base_name() == "jsconfig.json" {
                "jsconfig.json"
            } else {
                "tsconfig.json"
            };
            let errors = vec![crate::errors::create_diagnostic_for_node_in_source_file(
                source_file,
                root_expression,
                &tsc_diagnostics::THE_ROOT_VALUE_OF_A_0_FILE_MUST_BE_AN_OBJECT,
                &[base_file_name.to_string()],
            )];
            // Last-ditch error recovery (Go comment): the JSON parser
            // synthesizes a top-level array literal for some broken inputs;
            // the first object-literal element converts instead — Go DROPS
            // the root diagnostic on this arm (exact mirror).
            if tsc_ast::is_array_literal_expression(source_file.node(root_expression)) {
                let elements = source_file
                    .node(root_expression)
                    .as_array_literal_expression()
                    .expect("IsArrayLiteralExpression checked")
                    .elements
                    .clone();
                if let Some(elements) = elements
                    && let Some(first_object) = elements.nodes.iter().find(|element| {
                        tsc_ast::is_object_literal_expression(source_file.node(**element))
                    })
                {
                    return convert_to_json(source_file, Some(*first_object));
                }
            }
            return (CompilerOptionsValue::Object(OrderedMap::new()), errors);
        }
    }
    convert_to_json(source_file, root_expression)
}

/// Go: `func convertToJson(sourceFile, rootExpression, returnValue, jsonConversionNotifier)`
/// — the `returnValue = true` shape (Go's only reachable form).
///
/// PORT: Go's nil-root marker is `struct{}{}` — a non-map value every caller
/// normalizes to the empty map; the port carries
/// [`CompilerOptionsValue::Null`] (the same non-map fallback,
/// PORTING-NOTES).
fn convert_to_json(
    source_file: &tsc_ast::SourceFile,
    root_expression: Option<tsc_ast::NodeId>,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    match root_expression {
        Some(root) => convert_property_value_to_json(source_file, Some(root), None),
        None => (CompilerOptionsValue::Null, Vec::new()),
    }
}

/// Go: `func convertPropertyValueToJson(sourceFile, valueExpression, option, returnValue, jsonConversionNotifier)`.
///
/// `value_expression: None` mirrors Go's nil `*ast.Expression` (Kind
/// `Unknown` on the nil receiver → the error tail at the zero range, Go's
/// nil-node behavior). `option` is `None` at every call site this wave (the
/// nil-notifier shape — see [`convert_config_file_to_object`]'s PORT note).
fn convert_property_value_to_json(
    source_file: &tsc_ast::SourceFile,
    value_expression: Option<tsc_ast::NodeId>,
    option: Option<&'static CommandLineOption>,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let Some(value_expression) = value_expression else {
        return property_value_conversion_error(source_file, None, option);
    };
    let node = source_file.node(value_expression);
    match node.kind {
        tsc_ast::Kind::TrueKeyword => (CompilerOptionsValue::Bool(true), Vec::new()),
        tsc_ast::Kind::FalseKeyword => (CompilerOptionsValue::Bool(false), Vec::new()),
        // Go: `case ast.KindNullKeyword: // todo: how to manage null` → nil.
        tsc_ast::Kind::NullKeyword => (CompilerOptionsValue::Null, Vec::new()),
        tsc_ast::Kind::StringLiteral => {
            debug_assert!(is_double_quoted_string(node));
            let text = node
                .as_string_literal()
                .expect("StringLiteral data")
                .text
                .to_string();
            (CompilerOptionsValue::Str(text), Vec::new())
        }
        tsc_ast::Kind::NumericLiteral => {
            let text = node
                .as_numeric_literal()
                .expect("NumericLiteral data")
                .text
                .to_string();
            (numeric_literal_value(&text, false), Vec::new())
        }
        tsc_ast::Kind::PrefixUnaryExpression => {
            let data = node
                .as_prefix_unary_expression()
                .expect("PrefixUnaryExpression data")
                .clone();
            if data.operator != tsc_ast::Kind::MinusToken
                || source_file.node(data.operand).kind != tsc_ast::Kind::NumericLiteral
            {
                // Go: `break` — not valid JSON syntax → the error tail.
                return property_value_conversion_error(
                    source_file,
                    Some(value_expression),
                    option,
                );
            }
            let text = source_file
                .node(data.operand)
                .as_numeric_literal()
                .expect("NumericLiteral data")
                .text
                .to_string();
            (numeric_literal_value(&text, true), Vec::new())
        }
        tsc_ast::Kind::ObjectLiteralExpression => {
            convert_object_literal_expression_to_json(source_file, value_expression, option)
        }
        tsc_ast::Kind::ArrayLiteralExpression => {
            let elements = node
                .as_array_literal_expression()
                .expect("ArrayLiteralExpression data")
                .elements
                .clone();
            convert_array_literal_expression_to_json(source_file, elements.as_ref(), option)
        }
        _ => property_value_conversion_error(source_file, Some(value_expression), option),
    }
}

/// Go: convertPropertyValueToJson's "Not in expected format" tail.
fn property_value_conversion_error(
    source_file: &tsc_ast::SourceFile,
    value_expression: Option<tsc_ast::NodeId>,
    option: Option<&'static CommandLineOption>,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let loc = value_expression.map_or_else(
        || tsc_core::text::TextRange::new(0, 0),
        |node| source_file.node(node).loc,
    );
    let diagnostic = match option {
        Some(option) => new_diagnostic_at_loc(
            source_file,
            loc,
            &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
            &[
                option.name.to_string(),
                crate::errors::get_compiler_option_value_type_string(option),
            ],
        ),
        None => new_diagnostic_at_loc(
            source_file,
            loc,
            &tsc_diagnostics::PROPERTY_VALUE_CAN_ONLY_BE_STRING_LITERAL_NUMERIC_LITERAL_TRUE_FALSE_NULL_OBJECT_LITERAL_OR_ARRAY_LITERAL,
            &[],
        ),
    };
    (CompilerOptionsValue::Null, vec![diagnostic])
}

/// Go: `func convertObjectLiteralExpressionToJson(sourceFile, returnValue, node, objectOption, jsonConversionNotifier)`.
///
/// PORT: the notifier's per-key callback is skipped (the nil-notifier
/// shape); with `objectOption == None` Go's looked-up element option stays
/// nil (the nil ElementOptions guard), so nested values convert against a
/// nil option — the `Compiler_option_0_requires_a_value_of_type_1` arm is
/// unreachable this wave and the parameter is kept for the Go-shape
/// correspondence (underscore-bound until the notifier wave reads it).
#[allow(clippy::used_underscore_binding)]
fn convert_object_literal_expression_to_json(
    source_file: &tsc_ast::SourceFile,
    node_id: tsc_ast::NodeId,
    _object_option: Option<&'static CommandLineOption>,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let data = source_file
        .node(node_id)
        .as_object_literal_expression()
        .expect("IsObjectLiteralExpression checked by the walk producers")
        .clone();
    let mut result = OrderedMap::new();
    let mut errors: Vec<tsc_ast::Diagnostic> = Vec::new();
    let properties = data.properties.clone().unwrap_or_default();
    for element in properties.nodes.iter() {
        let element_node = source_file.node(*element);
        if !tsc_ast::is_property_assignment(element_node) {
            errors.push(new_diagnostic_at_loc(
                source_file,
                element_node.loc,
                &tsc_diagnostics::PROPERTY_ASSIGNMENT_EXPECTED,
                &[],
            ));
            continue;
        }
        let element_data = element_node
            .as_property_assignment()
            .expect("IsPropertyAssignment checked")
            .clone();
        if let Some(token) = element_data.postfix_token
            && source_file.node(token).kind == tsc_ast::Kind::QuestionToken
        {
            // Go: `element.QuestionToken()` — the PostfixToken when it is a
            // `?`; `The_0_modifier...` at the token's Loc.
            errors.push(new_diagnostic_at_loc(
                source_file,
                source_file.node(token).loc,
                &tsc_diagnostics::THE_0_MODIFIER_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                &["?".to_string()],
            ));
        }
        let key_text = if !is_computed_non_literal_name(source_file, element_data.name) {
            crate::ast_shims::try_get_text_of_property_name(
                source_file,
                element_data.name,
            )
        } else {
            None
        };
        let key_text = key_text.unwrap_or_default();
        let (value, err) =
            convert_property_value_to_json(source_file, Some(element_data.initializer), None);
        errors.extend(err);
        if !key_text.is_empty() {
            result.set(key_text, value);
        }
    }
    (CompilerOptionsValue::Object(result), errors)
}

/// Go: `func convertArrayLiteralExpressionToJson(sourceFile, elements, elementOption, returnValue)`.
///
/// PORT: the `returnValue = false` validation-only arm is unreachable (both
/// routes convert); the parameter is dropped.
fn convert_array_literal_expression_to_json(
    source_file: &tsc_ast::SourceFile,
    elements: Option<&tsc_ast::NodeList>,
    element_option: Option<&'static CommandLineOption>,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let Some(elements) = elements else {
        // Go: nil Elements → `len(elements) == 0` → the non-nil empty array.
        return (CompilerOptionsValue::Array(Vec::new()), Vec::new());
    };
    if elements.nodes.is_empty() {
        // Always return an empty array, even if elements is nil (Go comment).
        return (CompilerOptionsValue::Array(Vec::new()), Vec::new());
    }
    let mut errors: Vec<tsc_ast::Diagnostic> = Vec::new();
    let mut value: Vec<CompilerOptionsValue> = Vec::new();
    for element in elements.nodes.iter() {
        let (converted, err) =
            convert_property_value_to_json(source_file, Some(*element), element_option);
        errors.extend(err);
        // Go: `if convertedValue != nil` — nulls filter out of the array.
        if !converted.is_null() {
            value.push(converted);
        }
    }
    (CompilerOptionsValue::Array(value), errors)
}

// ────────────────────────────────────────────────────────────────────────────
// JSON text → CompilerOptionsValue (ordered)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func ParseConfigFileTextToJson(fileName, path, jsonText) (any, []*ast.Diagnostic)`.
///
/// The AST route (the M6 config-file wave): the text parses as a JSON
/// SourceFile and converts through [`convert_config_file_to_object`]; the
/// conversion errors are REPLACED by the FIRST parse diagnostic when the
/// parse produced any (Go's exact override — the parse diagnostic carries
/// the position; the conversion of a broken file would only echo it).
pub fn parse_config_file_text_to_json(
    file_name: &RootedFilePath,
    path_key: PathKey,
    json_text: &str,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let json_source_file = tsc_parser::parse_source_file(
        tsc_ast::SourceFileParseOptions {
            file_name: file_name.clone(),
            path_key,
            ..Default::default()
        },
        json_text,
        tsc_core::scriptkind::ScriptKind::JSON,
    );
    let (config, errors) = convert_config_file_to_object(&json_source_file);
    let parse_diagnostics = json_source_file.diagnostics();
    let errors = if !parse_diagnostics.is_empty() {
        vec![parse_diagnostics[0].clone()]
    } else {
        errors
    };
    (config, errors)
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

/// Go: the worker's `errors []*ast.Diagnostic` — every Go diagnostic carries
/// its own `File()` (the config file it was created against — the main
/// config, an extended config's file, or nil for a compiler diagnostic);
/// the port pairs at push time (see `ParsedCommandLine::error_files`).
#[derive(Default)]
pub(crate) struct WorkerErrors {
    diagnostics: Vec<tsc_ast::Diagnostic>,
    files: Vec<Option<RootedFilePath>>,
}

impl WorkerErrors {
    /// Go: `ast.NewCompilerDiagnostic` — the fileless form (nil `File()`).
    fn push_compiler(&mut self, diagnostic: tsc_ast::Diagnostic) {
        self.diagnostics.push(diagnostic);
        self.files.push(None);
    }

    /// A diagnostic created against `file` (Go: the diagnostic's `File()`).
    fn push_for_file(
        &mut self,
        diagnostic: tsc_ast::Diagnostic,
        file: Option<RootedFilePath>,
    ) {
        self.diagnostics.push(diagnostic);
        self.files.push(file);
    }
}

/// Go: `func isStringValue(value any) bool`.
fn is_string_value(value: &CompilerOptionsValue) -> bool {
    matches!(value, CompilerOptionsValue::Str(_))
}

/// Go: `reflect.TypeOf(element) == orderedMapType`.
fn is_object_value(value: &CompilerOptionsValue) -> bool {
    matches!(value, CompilerOptionsValue::Object(_))
}

/// Go: the worker's `getPropFromRaw` closure + `propOfRaw` — the raw-prop
/// read with Go's wrong-value markers. `Some(list)` = Go's non-nil
/// `sliceValue`; the `""` wrong-value is only reachable on the config-file
/// route (Go's `else if sourceFile == nil` guard leaves it empty there —
/// the json route's diagnostics are suppressed exactly as Go's).
fn get_prop_from_raw(
    raw_config: &OrderedMap<String, CompilerOptionsValue>,
    prop: &str,
    source_file_route: bool,
    element_type_name: &str,
    validate_element: impl Fn(&CompilerOptionsValue) -> bool,
    errors: &mut WorkerErrors,
) -> (Option<Vec<CompilerOptionsValue>>, &'static str) {
    let Some(value) = raw_config.get(prop) else {
        return (None, "no-prop");
    };
    if value.is_null() {
        return (None, "no-prop");
    }
    let list = match value {
        CompilerOptionsValue::Array(list) => list.clone(),
        // A JSON array of strings parses as StrArray on the command-line
        // route — the same slice shape (Go's []any).
        CompilerOptionsValue::StrArray(list) => list
            .iter()
            .map(|s| CompilerOptionsValue::Str(s.clone()))
            .collect(),
        _ => {
            if !source_file_route {
                errors.push_compiler(new_compiler_diagnostic(
                    &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
                    &[prop.to_string(), "Array".to_string()],
                ));
                return (None, "not-array");
            }
            // Go: the sourceFile route reports nothing and leaves the
            // wrong-value empty.
            return (None, "");
        }
    };
    // Go: `if _, ok := result.([]any); ok { if sourceFile == nil &&
    // !core.Every(result, validateElement) { ... } }`.
    if !source_file_route && !list.iter().all(validate_element) {
        errors.push_compiler(new_compiler_diagnostic(
            &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
            &[prop.to_string(), element_type_name.to_string()],
        ));
    }
    (Some(list), "")
}

/// The string elements of a raw prop list (Go's `isStringValue` filters).
fn string_values(list: &[CompilerOptionsValue]) -> Vec<String> {
    list.iter()
        .filter_map(|value| match value {
            CompilerOptionsValue::Str(spec) => Some(spec.clone()),
            _ => None,
        })
        .collect()
}

/// Go: `func ParseJsonConfigFileContent(json, fs, basePath, existingOptions, configFileName, resolutionStack, extendedConfigCache) *ParsedCommandLine`.
///
/// `json` is the already-parsed top-level object (Go passes the result of
/// `ParseConfigFileTextToJson`); `existing_options` seeds the compiler
/// options exactly as in Go. The config-file route is
/// [`parse_json_source_file_config_file_content`]; both share the worker.
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
    parse_json_config_file_content_worker(
        Some(json),
        None,
        fs,
        base_path,
        existing_options,
        config_file_name,
        resolution_stack,
        extended_config_cache,
        resolve_config_host,
    )
}

/// Go: `func ParseJsonSourceFileConfigFileContent(sourceFile, fs, basePath, existingOptions, existingOptionsRaw, resolutionStack, extendedConfigCache) *ParsedCommandLine`
/// — the config-file route (the M6 wave): the VALUES come from the config
/// AST ([`convert_config_file_to_object`]), the conversion's diagnostics
/// ride `errors`, and the JSON PARSE diagnostics ride the
/// `TsConfigSourceFile` itself (surfaced through
/// `ParsedCommandLine::get_config_file_parsing_diagnostics` — Go's concat —
/// and NOT through `errors`).
///
/// PORT (notifier residual): Go's route parses the per-key options through
/// `parseOwnConfigOfJsonSourceFile`'s notifier; the port emulates it on the
/// shared worker with `CompilerOptionsDiagSites` (FCS tranche 5) — the
/// value-conversion diagnostics (TS5024/TS6046/...) and the unknown-option
/// diagnostics anchor at the config property assignments exactly like the
/// notifier; the notifier's element-typed value conversion stays deferred
/// (PORTING-NOTES).
#[allow(clippy::too_many_arguments)]
pub fn parse_json_source_file_config_file_content(
    source_file: std::sync::Arc<TsConfigSourceFile>,
    fs: &dyn tsc_vfs::Fs,
    base_path: &RootedDirectoryPath,
    existing_options: Option<&CompilerOptions>,
    config_file_name: &RootedFilePath,
    resolution_stack: &[PathKey],
    extended_config_cache: Option<&ExtendedConfigCache>,
    resolve_config_host: Option<&dyn ResolveConfigHost>,
) -> ParsedCommandLine {
    parse_json_config_file_content_worker(
        None,
        Some(source_file),
        fs,
        base_path,
        existing_options,
        config_file_name,
        resolution_stack,
        extended_config_cache,
        resolve_config_host,
    )
}

/// Go: `func parseJsonConfigFileContentWorker(json, sourceFile, fs, basePath, existingOptions, existingOptionsRaw, configFileName, resolutionStack, extendedConfigCache) *ParsedCommandLine`.
#[allow(clippy::too_many_arguments)]
fn parse_json_config_file_content_worker(
    json: Option<&CompilerOptionsValue>,
    source_file: Option<std::sync::Arc<TsConfigSourceFile>>,
    fs: &dyn tsc_vfs::Fs,
    base_path: &RootedDirectoryPath,
    existing_options: Option<&CompilerOptions>,
    config_file_name: &RootedFilePath,
    resolution_stack: &[PathKey],
    extended_config_cache: Option<&ExtendedConfigCache>,
    resolve_config_host: Option<&dyn ResolveConfigHost>,
) -> ParsedCommandLine {
    debug_assert!(
        json.is_some() != source_file.is_some(),
        "Go: debug.Assert((json == nil && sourceFile != nil) || (json != nil && sourceFile == nil))"
    );
    let source_file_route = source_file.is_some();
    let mut errors = WorkerErrors::default();
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

    // The raw top-level object — the json route's passed value (Go:
    // normalizeJsonValue + the empty-map fallback), the config-file route's
    // AST conversion (the parse diagnostics ride the SourceFile, NOT
    // errors; the conversion's own diagnostics DO ride errors — Go's
    // parseConfig carries them out).
    let raw_config_source = match (json, &source_file) {
        (Some(json), _) => json.clone(),
        (None, Some(source_file)) => {
            let file = source_file.lock_source_file();
            let (value, conversion_errors) = convert_config_file_to_object(&file);
            drop(file);
            for diagnostic in conversion_errors {
                errors.push_for_file(diagnostic, Some(config_file_name.clone()));
            }
            value
        }
        (None, None) => unreachable!("the route assert guarantees one side"),
    };
    let mut raw_config: OrderedMap<String, CompilerOptionsValue> = match raw_config_source {
        CompilerOptionsValue::Object(map) => map,
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
        // PORT(FCS tranche 5): the config-file route threads the config AST
        // (Go: parseOwnConfigOfJsonSourceFile's notifier) — the option
        // conversion diagnostics (TS5024/TS6046/...) carry the property
        // assignment's INITIALIZER loc natively; the fileless worker route
        // stays for the json route. The value tree comes from
        // convert_config_file_to_object's AST walk, so per-key sites pair
        // with the value entries 1:1.
        let (parsed, conversion_errors) = if let Some(config) = &source_file {
            let guard = config.lock_source_file();
            let sites = CompilerOptionsDiagSites::new(&guard);
            convert_options_from_json_worker(compiler_options_value.clone(), base_path, Some(&sites))
        } else {
            convert_options_from_json_worker(compiler_options_value.clone(), base_path, None)
        };
        // PORT (file pairing): Go's route-positioned option diagnostics
        // (the notifier residual — see parse_json_source_file_config_file_
        // content's PORT note) pair with the config file here so the
        // conformance render keeps the file identity; the positions are
        // REAL now (see OptionDiagSite).
        for diagnostic in conversion_errors {
            errors.push_for_file(diagnostic, Some(config_file_name.clone()));
        }
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

    // Go: `if configFileName != "" && parsedConfig.options != nil {
    // parsedConfig.options.ConfigFilePath = configFileName }` — feeds
    // verifyCompilerOptions' tsBuildInfoFile arm and the rootDir-inferred
    // check.
    if !config_file_name.is_empty() {
        result.compiler_options.options.config_file_path = config_file_name.clone();
    }

    // ── files/include/exclude/references: Go's getPropFromRaw + the
    // empty-files / validateSpecs / references arms ──

    // Go: references first (`referencesOfRaw := getPropFromRaw("references", ...)`).
    let (references_value, references_wrong) = get_prop_from_raw(
        &raw_config,
        "references",
        source_file_route,
        "object",
        is_object_value,
        &mut errors,
    );
    // Go: `fileSpecs := getPropFromRaw("files", isStringValue, "string")`.
    let (files_value, files_wrong) = get_prop_from_raw(
        &raw_config,
        "files",
        source_file_route,
        "string",
        is_string_value,
        &mut errors,
    );
    // Go: `if fileSpecs.sliceValue != nil || fileSpecs.wrongValue == ""`
    // — the empty-files-list arm.
    if files_value.is_some() || files_wrong.is_empty() {
        // Go: "no-prop" (absent or null), "not-array" (present, non-array),
        // or an empty array ⇒ zero-or-no references.
        let has_zero_or_no_references = references_wrong == "no-prop"
            || references_wrong == "not-array"
            || references_value
                .as_ref()
                .is_none_or(|references| references.is_empty());
        let has_extends = match raw_config.get("extends") {
            None | Some(CompilerOptionsValue::Null) => false,
            Some(_) => true,
        };
        if files_value.as_ref().is_some_and(|files| files.is_empty())
            && has_zero_or_no_references
            && !has_extends
        {
            if let Some(source_file) = &source_file {
                // Go: positioned at the `files` property's INITIALIZER,
                // with the file name ("tsconfig.json" without a config
                // file name).
                let file_name_arg = if config_file_name.is_empty() {
                    "tsconfig.json".to_string()
                } else {
                    config_file_name.as_string().to_owned()
                };
                let guard = source_file.lock_source_file();
                let node_value = for_each_ts_config_prop_array(
                    Some(&*guard),
                    "files",
                    |_store, property| {
                        let initializer = guard
                            .node(property)
                            .as_property_assignment()
                            .expect("IsPropertyAssignment guaranteed by the walk")
                            .initializer;
                        // Go: a nil Initializer yields the nil node (the
                        // compiler-diagnostic fallback).
                        initializer.is_none().then_some(initializer)
                    },
                );
                errors.push_for_file(
                    crate::errors::create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                        Some(&*guard),
                        node_value,
                        &tsc_diagnostics::THE_FILES_LIST_IN_CONFIG_FILE_0_IS_EMPTY,
                        &[file_name_arg],
                    ),
                    Some(config_file_name.clone()),
                );
            } else {
                errors.push_compiler(new_compiler_diagnostic(
                    &tsc_diagnostics::THE_FILES_LIST_IN_CONFIG_FILE_0_IS_EMPTY,
                    &[config_file_name.as_string().to_owned()],
                ));
            }
        }
    }
    let (include_value, _include_wrong) = get_prop_from_raw(
        &raw_config,
        "include",
        source_file_route,
        "string",
        is_string_value,
        &mut errors,
    );
    let (mut exclude_value, exclude_wrong) = get_prop_from_raw(
        &raw_config,
        "exclude",
        source_file_route,
        "string",
        is_string_value,
        &mut errors,
    );
    if exclude_wrong == "no-prop" {
        // Go: outDir/declarationDir imply the exclude list when exclude is
        // absent ("no-prop" only — a wrong-typed exclude keeps Go's
        // "not-array" marker and does NOT inherit).
        let out_dir = result.compiler_options.options.out_dir.as_string().to_owned();
        let declaration_dir = result
            .compiler_options
            .options
            .declaration_dir
            .as_string()
            .to_owned();
        let mut values: Vec<CompilerOptionsValue> = Vec::new();
        if !out_dir.is_empty() {
            values.push(CompilerOptionsValue::Str(out_dir));
        }
        if !declaration_dir.is_empty() {
            values.push(CompilerOptionsValue::Str(declaration_dir));
        }
        if !values.is_empty() {
            // Go resets the wrong-value marker to "" (sliceValue now
            // non-nil); the marker has no later reader this wave.
            exclude_value = Some(values);
        }
    }
    let (include_value, is_default_include_spec) =
        match (files_value.is_none(), include_value.is_none()) {
            (true, true) => (
                Some(vec![CompilerOptionsValue::Str(
                    DEFAULT_INCLUDE_SPEC.to_string(),
                )]),
                true,
            ),
            _ => (include_value, false),
        };
    let include_prop: Option<Vec<String>> = include_value.as_ref().map(|list| string_values(list));
    let exclude_prop: Option<Vec<String>> = exclude_value.as_ref().map(|list| string_values(list));

    // validateSpecs + ${configDir} substitution (include/exclude).
    let mut validated_include_specs: Option<Vec<tsc_tspath::PathPattern>> = None;
    let mut validated_include_specs_before_substitution: Vec<tsc_tspath::PathPattern> = Vec::new();
    if let Some(include) = &include_value {
        let specs = validate_specs_route(
            include,
            true,
            &source_file,
            "include",
            config_file_name,
            &mut errors,
        );
        validated_include_specs_before_substitution = specs.clone();
        let substituted = get_substituted_string_array_with_config_dir_template(
            &specs.iter().map(|s| s.as_string().to_owned()).collect::<Vec<_>>(),
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
    if let Some(exclude) = &exclude_value {
        let specs = validate_specs_route(
            exclude,
            false,
            &source_file,
            "exclude",
            config_file_name,
            &mut errors,
        );
        let substituted = get_substituted_string_array_with_config_dir_template(
            &specs.iter().map(|s| s.as_string().to_owned()).collect::<Vec<_>>(),
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
    let files_strings: Vec<String> = files_value
        .as_ref()
        .map(|list| string_values(list))
        .unwrap_or_default();
    let mut validated_files_specs: Vec<tsc_tspath::FileSpec> = Vec::new();
    let mut validated_files_specs_before_substitution: Vec<tsc_tspath::FileSpec> = Vec::new();
    for spec in &files_strings {
        validated_files_specs_before_substitution.push(tsc_tspath::to_file_spec(spec));
    }
    if !files_strings.is_empty() {
        let substituted = get_substituted_string_array_with_config_dir_template(
            &files_strings,
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
    // Go: `fileSpecByPath` — the files-list path → the AS-WRITTEN spec
    // (first occurrence wins). The matched-file-spec include reasons read
    // it (M6 config-node wave).
    let case_sensitivity = fs.case_sensitivity();
    let mut file_spec_by_path: rustc_hash::FxHashMap<PathKey, tsc_tspath::FileSpec> =
        rustc_hash::FxHashMap::default();
    for (index, _spec) in validated_files_specs.iter().enumerate() {
        let path = case_sensitivity.path_key(&validated_file_names[index].as_path());
        file_spec_by_path.entry(path).or_insert_with(|| {
            validated_files_specs_before_substitution
                .get(index)
                .cloned()
                .unwrap_or_default()
        });
    }

    // ── File enumeration (Go: getFileNamesFromConfigSpecs) + the
    // No-inputs diagnostic — ported with the showconfig parity twin; see
    // the module PORT notes. ──
    let file_names = get_file_names_from_config_specs(
        &ConfigFileSpecs {
            validated_include_specs: validated_include_specs.clone().unwrap_or_default(),
            validated_exclude_specs: validated_exclude_specs.clone().unwrap_or_default(),
            validated_files_specs: validated_files_specs.clone(),
            validated_file_names: validated_file_names.clone(),
            validated_files_specs_before_substitution: validated_files_specs_before_substitution
                .clone(),
            validated_include_specs_before_substitution: validated_include_specs_before_substitution
                .clone(),
            file_spec_by_path: file_spec_by_path.clone(),
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
        // Go: the fileless compiler diagnostic on BOTH routes.
        errors.push_compiler(new_compiler_diagnostic(
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
    result.validated_files_specs_before_substitution = validated_files_specs_before_substitution;
    result.validated_include_specs_before_substitution =
        validated_include_specs_before_substitution;
    result.file_spec_by_path = file_spec_by_path;
    result.raw_include_spec_values = include_prop.clone().unwrap_or_default();
    result.raw_exclude_spec_values = exclude_prop.clone().unwrap_or_default();
    result.is_default_include_spec = is_default_include_spec;

    // references (Go: the worker's getProjectReferences closure —
    // parseProjectReference + the three positioned invalid-reference
    // diagnostics).
    if let Some(references) = &references_value {
        for (index, reference) in references.iter().enumerate() {
            let Some(parsed) = parse_project_reference(reference) else {
                continue;
            };
            if !parsed.has_path || !parsed.path_valid {
                push_reference_diagnostic(
                    &mut errors,
                    create_diagnostic_at_project_reference_property(
                        source_file.as_deref(),
                        index,
                        "path",
                        &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
                        &["reference.path".to_string(), "string".to_string()],
                    ),
                    config_file_name,
                );
                continue;
            }
            if parsed.path.is_empty() {
                push_reference_diagnostic(
                    &mut errors,
                    create_diagnostic_at_project_reference_property(
                        source_file.as_deref(),
                        index,
                        "path",
                        &tsc_diagnostics::COMPILER_OPTION_0_CANNOT_BE_GIVEN_AN_EMPTY_STRING,
                        &["reference.path".to_string()],
                    ),
                    config_file_name,
                );
                continue;
            }
            if parsed.has_circular && !parsed.circular_valid {
                push_reference_diagnostic(
                    &mut errors,
                    create_diagnostic_at_project_reference_property(
                        source_file.as_deref(),
                        index,
                        "circular",
                        &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
                        &["reference.circular".to_string(), "boolean".to_string()],
                    ),
                    config_file_name,
                );
            }
            let resolved = get_normalized_absolute_path(&parsed.path, &base_path_for_file_names);
            result.project_references.push(ProjectReference {
                path: tsc_tspath::rooted_file_path_from_normalized(&resolved),
                original_path: parsed.path,
                circular: parsed.reference.circular,
            });
        }
    }

    // compileOnSave
    if let Some(CompilerOptionsValue::Bool(compile_on_save)) = raw_config.get("compileOnSave") {
        result.compile_on_save = Some(*compile_on_save);
    }

    result.errors = errors.diagnostics;
    result.error_files = errors.files;
    result.config_file = source_file;
    result
}

/// The reference diagnostics' pairing — Go's positioned form carries the
/// config file, the compiler-diagnostic fallback is fileless (the
/// diagnostic's own `file` marker decides, mirroring Go's `File()`).
fn push_reference_diagnostic(
    errors: &mut WorkerErrors,
    diagnostic: tsc_ast::Diagnostic,
    config_file_name: &RootedFilePath,
) {
    let file = if diagnostic.file().is_some() {
        Some(config_file_name.clone())
    } else {
        None
    };
    errors.push_for_file(diagnostic, file);
}

/// Go: the worker's `validateSpecs` call shape — the positioned spec
/// diagnostics (the config-file route) pair with the config file; the json
/// route's degrade to the fileless compiler diagnostic exactly on Go's nil
/// `jsonSourceFile`.
fn validate_specs_route(
    specs: &[CompilerOptionsValue],
    disallow_trailing_recursion: bool,
    source_file: &Option<std::sync::Arc<TsConfigSourceFile>>,
    spec_key: &str,
    config_file_name: &RootedFilePath,
    errors: &mut WorkerErrors,
) -> Vec<tsc_tspath::PathPattern> {
    let (specs, spec_errors) = match source_file {
        Some(source_file) => {
            let guard = source_file.lock_source_file();
            validate_specs(
                specs,
                disallow_trailing_recursion,
                Some(&*guard),
                spec_key,
            )
            // `guard` drops here; the diagnostics are already built.
        }
        None => validate_specs(specs, disallow_trailing_recursion, None, spec_key),
    };
    for diagnostic in spec_errors {
        errors.push_for_file(diagnostic, Some(config_file_name.clone()));
    }
    specs
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

/// Go: the `propertyAssignment *ast.PropertyAssignment, valueExpression
/// *ast.Expression, sourceFile *ast.SourceFile` convertJsonOption param
/// triple — `None` everywhere = Go's nil (the command-line and cached
/// extends routes).
///
/// PORT(FCS tranche 5): REAL now (the M4 form dropped the AST params) —
/// the config-file route threads the parsed config AST so the option
/// conversion diagnostics (TS5024 `Compiler_option_0_requires_a_value_of_
/// type_1`, TS6046 `Argument_for_0_option_must_be_colon_1`, ...) carry the
/// failing property assignment's INITIALIZER loc natively, exactly like
/// Go's `onPropertySet` → `convertJsonOption(option, value, basePath,
/// propertyAssignment, propertyAssignment.Initializer, sourceFile)`.
#[derive(Clone, Copy)]
pub(crate) struct OptionDiagSite<'a> {
    source_file: Option<&'a tsc_ast::SourceFile>,
    property_assignment: Option<tsc_ast::NodeId>,
    value_expression: Option<tsc_ast::NodeId>,
}

impl<'a> OptionDiagSite<'a> {
    /// Go's nil triple.
    pub(crate) const NONE: OptionDiagSite<'static> = OptionDiagSite {
        source_file: None,
        property_assignment: None,
        value_expression: None,
    };

    /// Go: `propertyAssignment.Initializer` (the config AST guarantees
    /// PropertyAssignment payloads on the walk).
    fn initializer_of(store: &dyn tsc_ast::NodeStore, property_assignment: tsc_ast::NodeId) -> Option<tsc_ast::NodeId> {
        store
            .node(property_assignment)
            .as_property_assignment()
            .map(|d| d.initializer)
    }

    /// Go: `valueExpression.Elements()[index]` (convertJsonOptionOfListType
    /// re-anchors each list element at the array literal's matching element).
    fn element_expression(
        &self,
        store: &dyn tsc_ast::NodeStore,
        index: usize,
    ) -> Option<tsc_ast::NodeId> {
        let expression = self.value_expression?;
        match &store.node(expression).data {
            tsc_ast::NodeData::ArrayLiteralExpression(array) => array
                .elements
                .as_ref()
                .and_then(|elements| elements.nodes.get(index).copied()),
            _ => None,
        }
    }
}

/// The per-key `OptionDiagSite`s for one config file's `compilerOptions`
/// object: each option key maps to its PropertyAssignment node in the
/// config AST (last occurrence wins, matching the value tree's OrderedMap
/// insert — Go's notifier walks each duplicate; the corpus has unique keys).
pub(crate) struct CompilerOptionsDiagSites<'a> {
    source_file: &'a tsc_ast::SourceFile,
    assignments: rustc_hash::FxHashMap<String, tsc_ast::NodeId>,
}

impl<'a> CompilerOptionsDiagSites<'a> {
    pub(crate) fn new(source_file: &'a tsc_ast::SourceFile) -> Self {
        let mut assignments = rustc_hash::FxHashMap::default();
        // Go: tsconfigToSourceFile's root object literal → the
        // `compilerOptions` PropertyAssignment → its initializer object
        // literal's PropertyAssignments (Go's onPropertySet walk).
        let root_expression = source_file
            .payload()
            .statements
            .as_ref()
            .and_then(|statements| statements.nodes.first())
            .and_then(|first| crate::ast_shims::expression_of(source_file, *first));
        let compiler_options_initializer = root_expression
            .filter(|&root| tsc_ast::is_object_literal_expression(source_file.node(root)))
            .and_then(|root| {
                let properties = source_file
                    .node(root)
                    .as_object_literal_expression()
                    .expect("IsObjectLiteralExpression checked")
                    .properties
                    .clone()
                    .unwrap_or_default();
                properties
                    .nodes
                    .iter()
                    .find(|property| {
                        let property_node = source_file.node(**property);
                        tsc_ast::is_property_assignment(property_node)
                            && property_node.name().is_some_and(|name| {
                                crate::ast_shims::try_get_text_of_property_name(
                                    source_file,
                                    name,
                                ) == Some("compilerOptions".to_string())
                            })
                    })
                    .and_then(|compiler_options| {
                        OptionDiagSite::initializer_of(source_file, *compiler_options)
                    })
            });
        if let Some(initializer) = compiler_options_initializer
            && tsc_ast::is_object_literal_expression(source_file.node(initializer))
        {
            let properties = source_file
                .node(initializer)
                .as_object_literal_expression()
                .expect("IsObjectLiteralExpression checked")
                .properties
                .clone()
                .unwrap_or_default();
            for property in properties.nodes.iter() {
                let property_node = source_file.node(*property);
                if !tsc_ast::is_property_assignment(property_node) {
                    continue;
                }
                if let Some(key) = property_node.name().and_then(|name| {
                    crate::ast_shims::try_get_text_of_property_name(source_file, name)
                }) {
                    // Last occurrence wins (the value tree's OrderedMap
                    // insert shape; Go's notifier walks each duplicate).
                    assignments.insert(key, *property);
                }
            }
        }
        CompilerOptionsDiagSites {
            source_file,
            assignments,
        }
    }

    /// Go: the per-key site — `convertJsonOption(option, value, basePath,
    /// propertyAssignment, propertyAssignment.Initializer, sourceFile)`.
    pub(crate) fn site(&self, key: &str) -> OptionDiagSite<'a> {
        let property_assignment = self.assignments.get(key).copied();
        let value_expression = property_assignment
            .and_then(|property| OptionDiagSite::initializer_of(self.source_file, property));
        OptionDiagSite {
            source_file: Some(self.source_file),
            property_assignment,
            value_expression,
        }
    }
}

/// Go: `func convertJsonOptionOfListType(option, values, basePath, ...) ([]any, []*ast.Diagnostic)`.
fn convert_json_option_of_list_type(
    option: &'static CommandLineOption,
    values: &CompilerOptionsValue,
    base_path: &RootedDirectoryPath,
    site: OptionDiagSite<'_>,
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
        site,
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
    site: OptionDiagSite<'_>,
    errors: &mut Vec<tsc_ast::Diagnostic>,
) -> CompilerOptionsValue {
    let mut mapped_values: Vec<CompilerOptionsValue> = Vec::with_capacity(list.len());
    for (index, value) in list.iter().enumerate() {
        // Go: `if valueExpression != nil { expression =
        // valueExpression.Elements()[index] }` — each list element's
        // diagnostics anchor at the array literal's matching element.
        let element_site = match site.source_file {
            Some(source_file) => OptionDiagSite {
                source_file: Some(source_file),
                property_assignment: site.property_assignment,
                value_expression: site.element_expression(source_file, index),
            },
            None => OptionDiagSite::NONE,
        };
        let (result, element_errors) = convert_json_option(element, value, base_path, element_site);
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

/// Go: `func convertJsonOption(opt, value, basePath, propertyAssignment, valueExpression, sourceFile) (any, []*ast.Diagnostic)`.
///
/// PORT(FCS tranche 5): the `propertyAssignment`/`valueExpression`/
/// `sourceFile` triple is REAL now (the M4 subset dropped the AST params);
/// each diagnostic anchors exactly where Go's does — the
/// command-line-only option at the property assignment's NAME, the type
/// mismatch (5024) and enum (6046) at the property assignment's
/// INITIALIZER (`valueExpression`).
fn convert_json_option(
    opt: &'static CommandLineOption,
    value: &CompilerOptionsValue,
    base_path: &RootedDirectoryPath,
    site: OptionDiagSite<'_>,
) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    if opt.is_command_line_only {
        // Go: `nodeValue = propertyAssignment.Name()`; the compiler
        // diagnostic only when BOTH sourceFile and nodeValue are nil.
        let node_value = site
            .property_assignment
            .and_then(|property| site.source_file?.node(property).name());
        if site.source_file.is_none() && node_value.is_none() {
            return (
                CompilerOptionsValue::Null,
                vec![new_compiler_diagnostic(
                    &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_SPECIFIED_ON_COMMAND_LINE,
                    &[opt.name.to_string()],
                )],
            );
        }
        return (
            CompilerOptionsValue::Null,
            vec![crate::errors::create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                site.source_file,
                node_value,
                &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_SPECIFIED_ON_COMMAND_LINE,
                &[opt.name.to_string()],
            )],
        );
    }
    if !is_compiler_options_value(opt, value) {
        return (
            CompilerOptionsValue::Null,
            vec![crate::errors::create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                site.source_file,
                site.value_expression,
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
            convert_json_option_of_list_type(opt, value, base_path, site)
        }
        crate::commandlineoption::CommandLineOptionKind::ListOrElement => {
            if matches!(value, CompilerOptionsValue::Array(_) | CompilerOptionsValue::StrArray(_)) {
                convert_json_option_of_list_type(opt, value, base_path, site)
            } else {
                convert_json_option(opt.elements().expect("element"), value, base_path, site)
            }
        }
        crate::commandlineoption::CommandLineOptionKind::Enum => {
            if value.is_null() {
                return (CompilerOptionsValue::Null, Vec::new());
            }
            let CompilerOptionsValue::Str(s) = value else {
                return (CompilerOptionsValue::Null, Vec::new());
            };
            crate::commandlineparser::convert_json_option_of_enum_type(
                opt,
                s,
                site.value_expression,
                site.source_file,
            )
        }
        _ => {
            let (validated, errors) = crate::commandlineparser::validate_json_option_value(
                opt,
                value.clone(),
                site.value_expression,
                site.source_file,
            );
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
///
/// PORT(FCS tranche 5): `sites` carries the config AST's compilerOptions
/// property assignments (the config-file route); `None` = Go's nil
/// sourceFile (the command-line / cached-extends routes — fileless
/// diagnostics).
fn convert_options_from_json_worker(
    json_options: CompilerOptionsValue,
    base_path: &RootedDirectoryPath,
    sites: Option<&CompilerOptionsDiagSites<'_>>,
) -> (ParsedCompilerOptions, Vec<tsc_ast::Diagnostic>) {
    let mut parsed = ParsedCompilerOptions::new(CompilerOptions::default());
    let mut errors = Vec::new();
    let CompilerOptionsValue::Object(map) = &json_options else {
        return (parsed, errors);
    };
    let options_name_map = command_line_compiler_options_map();
    for (key, value) in map.entries() {
        // Go: onPropertySet's convertJsonOption call — the site anchors the
        // option's diagnostics at the config property assignment.
        let site = sites.map(|sites| sites.site(key)).unwrap_or(OptionDiagSite::NONE);
        let looked_up = options_name_map.get(key);
        // Go: the config-file route (convertObjectLiteralExpressionToJson's
        // notifier lookup) NILS case-insensitive matches —
        // `if option != nil && option.Name != keyText { option = nil }` —
        // so they land in the unknown-option arm (the did-you-mean
        // suggestion positioned at the property NAME, exactly the arm
        // below); the fileless worker route keeps Go's separate
        // did-you-mean arm.
        let option = if sites.is_some() {
            looked_up.filter(|option| option.name == key.as_str())
        } else {
            looked_up
        };
        let Some(option) = option else {
            errors.push(crate::errors::create_unknown_option_error(
                key,
                ParsedCompilerOptions::unknown_option_diagnostic()
                    .expect("compiler options parser diagnostic"),
                "",
                site.source_file,
                site.property_assignment.and_then(|property| site
                    .source_file
                    .and_then(|source_file| source_file.node(property).name())),
                None,
                ParsedCompilerOptions::unknown_did_you_mean_diagnostic(),
                Some(options_name_map),
            ));
            continue;
        };
        if option.name != key.as_str() {
            // Case-insensitive match found but exact case doesn't match —
            // provide the "did you mean" suggestion (Go: skips the option).
            errors.push(crate::errors::create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                site.source_file,
                site.property_assignment.and_then(|property| site
                    .source_file
                    .and_then(|source_file| source_file.node(property).name())),
                ParsedCompilerOptions::unknown_did_you_mean_diagnostic()
                    .expect("compiler options did-you-mean diagnostic"),
                &[key.clone(), option.name.to_string()],
            ));
            continue;
        }
        let (converted, conversion_errors) = convert_json_option(option, value, base_path, site);
        errors.extend(conversion_errors);
        parsed.parse_option(key, &converted);
    }
    // ${configDir} templates substitute against the config's directory.
    crate::options_generated::handle_option_config_dir_template_substitution(&mut parsed, base_path);
    (parsed, errors)
}

/// Go: readJsonConfigFile + ParseExtendedConfig + getExtendedConfig —
/// resolve one `extends` value.
///
/// The AST route (the M6 wave): the extended config's text parses as a
/// `TsConfigSourceFile`; Go's `ParseExtendedConfig` semantics carry exactly —
/// read errors report and skip the merge; the extended file's JSON PARSE
/// diagnostics replace the whole conversion (`entry.errors = parseDiagnostics`
/// — every one of them, skipping the conversion); otherwise the conversion's
/// own diagnostics ride errors and the extended values merge.
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
    errors: &mut WorkerErrors,
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
        // Go: parseConfig's circularity check — the fileless compiler
        // diagnostic (`Circularity_detected_while_resolving_configuration_Colon_0`,
        // no arguments); the extended config parses to the empty raw and
        // does not merge.
        errors.push_compiler(new_compiler_diagnostic(
            &tsc_diagnostics::CIRCULARITY_DETECTED_WHILE_RESOLVING_CONFIGURATION_COLON_0,
            &[],
        ));
        return None;
    }
    if let Some(cache) = extended_config_cache
        && let Some(CompilerOptionsValue::Object(map)) = cache(extends_path)
    {
        // PORT: the cache seam stores the raw map (Go caches the parsed
        // command line); the parsed form is rebuilt from it.
        let (parsed, cache_errors) = convert_options_from_json_worker(
            map.get("compilerOptions")
                .cloned()
                .unwrap_or(CompilerOptionsValue::Null),
            &resolved.directory(),
            // PORT(FCS tranche 5): the cache seam stores the raw map only
            // (Go caches the parsed command line with its config AST) —
            // no AST nodes available; the fileless worker route.
            None,
        );
        for diagnostic in cache_errors {
            errors.push_for_file(diagnostic, Some(resolved.clone()));
        }
        return Some((map.clone(), parsed, resolved));
    }
    resolution_stack.push(path_key);

    // Go: `tryReadFile` — the fileless `Cannot_read_file_0` compiler
    // diagnostic on an unreadable file.
    let text = fs.read_file(&resolved).or_else(|| {
        errors.push_compiler(new_compiler_diagnostic(
            &tsc_diagnostics::CANNOT_READ_FILE_0,
            &[resolved.as_string().to_owned()],
        ));
        None
    })?;

    // The AST route: NewTsconfigSourceFileFromFilePath + ParseExtendedConfig.
    let extended_config_file =
        new_tsconfig_source_file_from_file_path(&resolved, fs.case_sensitivity(), &text);
    let parse_diagnostics = extended_config_file
        .lock_source_file()
        .diagnostics()
        .to_vec();
    if !parse_diagnostics.is_empty() {
        // Go: `entry.errors = parseDiagnostics` — ALL the extended file's
        // parse diagnostics (the conversion is skipped entirely); each
        // carries the EXTENDED file's identity.
        for diagnostic in parse_diagnostics {
            errors.push_for_file(diagnostic, Some(resolved.clone()));
        }
        return None;
    }
    let (json, conversion_errors) =
        convert_config_file_to_object(&extended_config_file.lock_source_file());
    for diagnostic in conversion_errors {
        errors.push_for_file(diagnostic, Some(resolved.clone()));
    }
    let CompilerOptionsValue::Object(mut map) = json else {
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
        // PORT(FCS tranche 5): the extended config's own AST threads too
        // (Go: ParseExtendedConfig → parseOwnConfigOfJsonSourceFile with
        // the EXTENDED file's sourceFile — its option diagnostics carry
        // positions in the extended file).
        let guard = extended_config_file.lock_source_file();
        let sites = CompilerOptionsDiagSites::new(&guard);
        let (own, conversion_errors) =
            convert_options_from_json_worker(compiler_options_value.clone(), &extended_base, Some(&sites));
        drop(guard);
        for diagnostic in conversion_errors {
            errors.push_for_file(diagnostic, Some(resolved.clone()));
        }
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

/// Go: `func validateSpecs(specs any, disallowTrailingRecursion bool, jsonSourceFile *ast.SourceFile, specKey string) ([]tspath.PathPattern, []*ast.Diagnostic)`.
///
/// The invalid-spec diagnostics position at the matching array element
/// through `GetTsConfigPropArrayElementValue` when the config source file is
/// present (`None` degrades to the compiler-diagnostic form exactly on
/// Go's nil `jsonSourceFile`). Non-string elements skip (Go's `continue`).
pub fn validate_specs(
    specs: &[CompilerOptionsValue],
    disallow_trailing_recursion: bool,
    json_source_file: Option<&tsc_ast::SourceFile>,
    spec_key: &str,
) -> (Vec<tsc_tspath::PathPattern>, Vec<tsc_ast::Diagnostic>) {
    let mut errors = Vec::new();
    let mut result = Vec::new();
    for value in specs {
        let CompilerOptionsValue::Str(spec) = value else {
            continue;
        };
        if let Some(message) = spec_to_diagnostic(spec, disallow_trailing_recursion) {
            let element =
                get_ts_config_prop_array_element_value(json_source_file, spec_key, spec);
            errors.push(
                crate::errors::create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                    json_source_file,
                    element,
                    message,
                    std::slice::from_ref(spec),
                ),
            );
        } else {
            result.push(to_path_pattern(spec));
        }
    }
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

/// Go: `func CreateDiagnosticAtReferenceSyntax(config *ParsedCommandLine, index int, message, args...) *ast.Diagnostic`
/// — the `references` array's `index`-th element node, `nil` without a
/// config file (Go's nil-ConfigFile arm; the caller falls back to the
/// compiler-diagnostic form).
pub fn create_diagnostic_at_reference_syntax(
    config: &ParsedCommandLine,
    index: usize,
    message: &'static tsc_diagnostics::Message,
    args: &[String],
) -> Option<tsc_ast::Diagnostic> {
    let config_file = config.config_file.as_ref()?;
    let guard = config_file.lock_source_file();
    for_each_ts_config_prop_array(Some(&*guard), "references", |store, property| {
        let initializer = store
            .node(property)
            .as_property_assignment()
            .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
            .initializer;
        if initializer.is_none() {
            return None;
        }
        let data = store
            .node(initializer)
            .as_array_literal_expression()
            .expect("IsArrayLiteralExpression checked below")
            .clone();
        let elements = data.elements?;
        let value = *elements.nodes.get(index)?;
        Some(crate::errors::create_diagnostic_for_node_in_source_file(
            &guard, value, message, args,
        ))
    })
}

/// Go: `func createDiagnosticAtProjectReferenceProperty(sourceFile *TsConfigSourceFile, index int, propertyName string, message, args...) *ast.Diagnostic`
/// — the `index`-th reference's `propertyName` property INITIALIZER (the
/// reference OBJECT itself when the property is missing), through
/// `CreateDiagnosticForNodeInSourceFileOrCompilerDiagnostic` (nil source
/// file → the fileless compiler form).
fn create_diagnostic_at_project_reference_property(
    source_file: Option<&TsConfigSourceFile>,
    index: usize,
    property_name: &str,
    message: &'static tsc_diagnostics::Message,
    args: &[String],
) -> tsc_ast::Diagnostic {
    let guard = source_file.map(|source_file| source_file.lock_source_file());
    let node = guard.as_ref().and_then(|guard| {
        for_each_ts_config_prop_array(Some(guard), "references", |store, property| {
            let initializer = store
                .node(property)
                .as_property_assignment()
                .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
                .initializer;
            if initializer.is_none() {
                return None;
            }
            let data = store
                .node(initializer)
                .as_array_literal_expression()
                .expect("IsArrayLiteralExpression checked below")
                .clone();
            let elements = data.elements?;
            let reference = *elements.nodes.get(index)?;
            if !tsc_ast::is_object_literal_expression(store.node(reference)) {
                return None;
            }
            let property_node = for_each_property_assignment(
                store,
                Some(reference),
                property_name,
                "",
                |store, property| {
                    let initializer = store
                        .node(property)
                        .as_property_assignment()
                        .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
                        .initializer;
                    initializer.is_none().then_some(initializer)
                },
            );
            property_node.or(Some(reference))
        })
    });
    crate::errors::create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
        guard.as_deref(),
        node,
        message,
        args,
    )
}

/// Go: `func GetParsedCommandLineOfConfigFilePath(configFileName, path, options, optionsRaw, fs, extendedConfigCache) (*ParsedCommandLine, []*ast.Diagnostic)`
/// — the config-file route (the M6 wave): read + parse the config text as a
/// `TsConfigSourceFile`, then the source-file route worker. The values come
/// from the config AST; the parse diagnostics ride the config file
/// (`GetConfigFileParsingDiagnostics`), the conversion diagnostics ride
/// `errors` — Go's exact route split.
pub fn get_parsed_command_line_of_config_file_path(
    config_file_name: &RootedFilePath,
    fs: &dyn tsc_vfs::Fs,
    resolve_config_host: Option<&dyn ResolveConfigHost>,
) -> ParsedCommandLine {
    let base_path = config_file_name.directory();
    // Go: tryReadFile — the fileless `Cannot_read_file_0` compiler
    // diagnostic on an unreadable file (Go returns the errors unrecoverable;
    // the port keeps the single-ParsedCommandLine shape the runner/host
    // consume — the diagnostic rides `errors` with the same message/args).
    let text = match fs.read_file(config_file_name) {
        Some(text) => text,
        None => {
            let mut result = ParsedCommandLine::new(
                config_file_name.clone(),
                base_path,
                fs.case_sensitivity(),
            );
            result.errors.push(new_compiler_diagnostic(
                &tsc_diagnostics::CANNOT_READ_FILE_0,
                &[config_file_name.as_string().to_owned()],
            ));
            return result;
        }
    };
    let config_file = new_tsconfig_source_file_from_file_path(
        config_file_name,
        fs.case_sensitivity(),
        &text,
    );
    parse_json_source_file_config_file_content(
        std::sync::Arc::new(config_file),
        fs,
        &base_path,
        None,
        config_file_name,
        &[],
        None,
        resolve_config_host,
    )
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
