// Port of tsc/internal/tsoptions/parsinghelpers.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Also carries the ${configDir} substitution trio (startsWithConfigDirTemplate
// / getSubstitutedPathWithConfigDirTemplate / getSubstitutedStringArray
// WithConfigDirTemplate) and `defaultIncludeSpec`, which Go keeps in
// tsconfigparsing.go — the generated options_generated.rs imports them from
// this module.
//
// PORT notes:
//   - Go's nil-vs-empty slice distinction collapses (empty Vec), matching the
//     core options_generated.rs PORT note; `parseStringMap` returning nil for
//     a non-map reads as `None`.
//   - Go's `parseNumber` accepts `int` or `float64`; JSON numbers surface as
//     f64 through the json facade and as Int when the value was produced by
//     the CLI parser — both arms are handled.
//   - `parseProjectReference`'s result struct keeps Go's has*/valid* flags.
//   - `ProjectReference` is not in the Rust core crate yet (M6 territory);
//     it is defined in tsconfigparsing.rs and re-exported here.

use tsc_collections::{OrderedMap, Set};
use tsc_core::options_generated::{BuildOptions, CompilerOptions};
use tsc_core::tristate::Tristate;
use tsc_tspath::{
    get_normalized_absolute_path, rooted_directory_path_from_normalized,
    rooted_file_path_from_normalized, rooted_path_from_normalized, RootedDirectoryPath,
    RootedFilePath, RootedPath, SourceMapLocation,
};

use crate::commandlineoption::{CommandLineOptionKind, CompilerOptionsValue};
use crate::options_generated::parse_compiler_options;
use crate::tsconfigparsing::ProjectReference;

/// Go: `const configDirTemplate = "${configDir}"` (tsconfigparsing.go).
pub const CONFIG_DIR_TEMPLATE: &str = "${configDir}";

/// Go: `func startsWithConfigDirTemplate(value string) bool` (case-insensitive).
pub fn starts_with_config_dir_template(value: &str) -> bool {
    value.to_ascii_lowercase().starts_with(CONFIG_DIR_TEMPLATE.to_ascii_lowercase().as_str())
}

/// Go: `func getSubstitutedPathWithConfigDirTemplate(value string, basePath tspath.RootedDirectoryPath) string`
/// Go: `func getSubstitutedPathWithConfigDirTemplate(value string, basePath) string`
/// (tsconfigparsing.go): `strings.Replace(value, configDirTemplate, "./", 1)`
/// then normalize against the base path. Note Go's asymmetry: the *check*
/// (`startsWithConfigDirTemplate`) is case-insensitive but the *substitution*
/// matches the exact `${configDir}` casing — callers guard with the check.
pub fn get_substituted_path_with_config_dir_template(
    value: &str,
    base_path: &RootedDirectoryPath,
) -> String {
    get_normalized_absolute_path(&value.replacen(CONFIG_DIR_TEMPLATE, "./", 1), base_path)
        .into_owned()
}

/// Go: `func getSubstitutedStringArrayWithConfigDirTemplate[T ~string](list []T, basePath) []T` —
/// `None` where Go returns nil (no element carries the template).
pub fn get_substituted_string_array_with_config_dir_template(
    list: &[String],
    base_path: &RootedDirectoryPath,
) -> Option<Vec<String>> {
    if !list.iter().any(|item| starts_with_config_dir_template(item)) {
        return None;
    }
    Some(
        list.iter()
            .map(|item| get_substituted_path_with_config_dir_template(item, base_path))
            .collect(),
    )
}

/// Go: `func ParseTristate(value any) core.Tristate`.
pub fn parse_tristate(value: &CompilerOptionsValue) -> Tristate {
    match value {
        CompilerOptionsValue::Null => Tristate::TSUnknown,
        CompilerOptionsValue::Tristate(v) => *v,
        CompilerOptionsValue::Bool(true) => Tristate::TSTrue,
        _ => Tristate::TSFalse,
    }
}

/// Go: `func ParseStringArray(value any) []string` — non-string elements are
/// dropped; a non-array value reads as empty.
pub fn parse_string_array(value: &CompilerOptionsValue) -> Vec<String> {
    match value {
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

/// Go: `func parseStringMap(value any) *collections.OrderedMap[string, []string]`.
pub fn parse_string_map(value: &CompilerOptionsValue) -> Option<OrderedMap<String, Vec<String>>> {
    match value {
        CompilerOptionsValue::Object(m) => {
            let mut result = OrderedMap::<String, Vec<String>>::with_size_hint(m.size());
            for (k, v) in m.entries() {
                result.set(k.clone(), parse_string_array(v));
            }
            Some(result)
        }
        CompilerOptionsValue::PathsMap(m) => Some(m.clone()),
        _ => None,
    }
}

/// Go: `func ParseString(value any) string`.
pub fn parse_string(value: &CompilerOptionsValue) -> String {
    match value {
        CompilerOptionsValue::Str(s) => s.clone(),
        _ => String::new(),
    }
}

/// Go: `func parseFileName(value any) tspath.RootedFilePath`.
pub fn parse_file_name(value: &CompilerOptionsValue) -> RootedFilePath {
    let path = parse_string(value);
    if path.is_empty() {
        return RootedFilePath::default();
    }
    rooted_file_path_from_normalized(&path)
}

/// Go: `func parseDirectoryName(value any) tspath.RootedDirectoryPath`.
pub fn parse_directory_name(value: &CompilerOptionsValue) -> RootedDirectoryPath {
    let path = parse_string(value);
    if path.is_empty() {
        return RootedDirectoryPath::default();
    }
    rooted_directory_path_from_normalized(&path)
}

/// Go: `func parseFileOrDirectoryName(value any) tspath.RootedPath`.
pub fn parse_file_or_directory_name(value: &CompilerOptionsValue) -> RootedPath {
    let path = parse_string(value);
    if path.is_empty() {
        return RootedPath::default();
    }
    rooted_path_from_normalized(&path)
}

/// Go: `func parseDirectoryNames(value any) []tspath.RootedDirectoryPath`.
pub fn parse_directory_names(value: &CompilerOptionsValue) -> Vec<RootedDirectoryPath> {
    parse_string_array(value)
        .into_iter()
        .map(|path| {
            if path.is_empty() {
                RootedDirectoryPath::default()
            } else {
                rooted_directory_path_from_normalized(&path)
            }
        })
        .collect()
}

/// Go: `func toSourceMapLocation` — tspath.ToSourceMapLocation(ParseString(value)).
pub fn to_source_map_location(path: String) -> SourceMapLocation {
    tsc_tspath::to_source_map_location(&path)
}
/// Go: `func parseNumber(value any) *int`.
pub fn parse_number(value: &CompilerOptionsValue) -> Option<i64> {
    match value {
        CompilerOptionsValue::Int(n) => Some(*n),
        CompilerOptionsValue::Float(n) => Some(*n as i64),
        _ => None,
    }
}

/// Go: `type projectReferenceParseResult struct`.
#[derive(Default)]
pub struct ProjectReferenceParseResult {
    pub reference: ProjectReference,
    pub path: String,
    pub has_path: bool,
    pub path_valid: bool,
    pub has_circular: bool,
    pub circular_valid: bool,
}

/// Go: `func parseProjectReference(json any) *projectReferenceParseResult`.
pub fn parse_project_reference(json: &CompilerOptionsValue) -> Option<ProjectReferenceParseResult> {
    match json {
        CompilerOptionsValue::Object(v) => {
            let mut result = ProjectReferenceParseResult::default();
            if let Some(value) = v.get("path") {
                result.has_path = true;
                if let CompilerOptionsValue::Str(path) = value {
                    result.path = path.clone();
                    result.path_valid = true;
                }
            }
            if let Some(value) = v.get("circular") {
                result.has_circular = true;
                if let CompilerOptionsValue::Bool(circular) = value {
                    result.reference.circular = *circular;
                    result.circular_valid = true;
                }
            }
            Some(result)
        }
        _ => None,
    }
}

/// Go: `func parseContentMapper(value any) (*contentmapper.Mapper, []*ast.Diagnostic)`.
pub fn parse_content_mapper(
    value: &CompilerOptionsValue,
) -> (Option<tsc_contentmapper::Mapper>, Vec<tsc_ast::Diagnostic>) {
    let CompilerOptionsValue::Object(v) = value else {
        return (None, Vec::new());
    };
    let mut errors = Vec::new();
    let mut mapper = tsc_contentmapper::Mapper::default();
    match v.get("package") {
        Some(CompilerOptionsValue::Str(s)) if !s.is_empty() => mapper.definition.package = s.clone(),
        _ => errors.push(crate::errors::new_compiler_diagnostic(
            &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
            &["contentMapper.package".to_string(), "string".to_string()],
        )),
    }
    match v.get("extensions") {
        Some(extensions) => match parse_string_array_strict(extensions) {
            Some(strs) => mapper.definition.extensions = strs,
            None => errors.push(crate::errors::new_compiler_diagnostic(
                &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
                &["contentMapper.extensions".to_string(), "string[]".to_string()],
            )),
        },
        None => errors.push(crate::errors::new_compiler_diagnostic(
            &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
            &["contentMapper.extensions".to_string(), "string[]".to_string()],
        )),
    }
    match v.get("options") {
        Some(CompilerOptionsValue::Object(options)) => {
            mapper.definition.options = crate::tsconfigparsing::marshal_compiler_options_value(options);
        }
        Some(_) => errors.push(crate::errors::new_compiler_diagnostic(
            &tsc_diagnostics::COMPILER_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
            &["contentMapper.options".to_string(), "object".to_string()],
        )),
        None => {}
    }
    if !errors.is_empty() {
        return (None, errors);
    }
    (Some(mapper), errors)
}

/// Go: `func parseStringArrayStrict(value any) ([]string, bool)`.
pub fn parse_string_array_strict(value: &CompilerOptionsValue) -> Option<Vec<String>> {
    match value {
        CompilerOptionsValue::StrArray(arr) => Some(arr.clone()),
        CompilerOptionsValue::Array(arr) => {
            let mut result = Vec::with_capacity(arr.len());
            for v in arr {
                match v {
                    CompilerOptionsValue::Str(s) => result.push(s.clone()),
                    _ => return None,
                }
            }
            Some(result)
        }
        _ => None,
    }
}

/// Go: `func parseJsonToStringKey(json any) *collections.OrderedMap[string, any]`.
pub fn parse_json_to_string_key(
    json: &CompilerOptionsValue,
) -> OrderedMap<String, CompilerOptionsValue> {
    let mut result = OrderedMap::<String, CompilerOptionsValue>::with_size_hint(6);
    if let CompilerOptionsValue::Object(m) = json {
        // Go: the quirky double-set for "extends" (array then original) is
        // preserved: the second Set overwrites for non-string values.
        for key in [
            "include",
            "exclude",
            "files",
            "references",
            "contentMappers",
            "extends",
            "compilerOptions",
            "excludes",
            "typeAcquisition",
        ] {
            if let Some(v) = m.get(key) {
                if key == "extends" {
                    if let CompilerOptionsValue::Str(s) = v {
                        result.set(
                            "extends".to_string(),
                            CompilerOptionsValue::Array(vec![CompilerOptionsValue::Str(
                                s.clone(),
                            )]),
                        );
                    }
                    result.set("extends".to_string(), v.clone());
                } else {
                    result.set(key.to_string(), v.clone());
                }
            }
        }
    }
    result
}

/// Go: `type optionParser interface` — `ParseOption` mutates; the diagnostic
/// accessors are static (Go's are methods but constant per parser type).
pub trait OptionParser {
    fn parse_option(&mut self, key: &str, value: &CompilerOptionsValue);
    fn unknown_option_diagnostic() -> Option<&'static tsc_diagnostics::Message>;
    fn unknown_did_you_mean_diagnostic() -> Option<&'static tsc_diagnostics::Message>;
}

/// Go: `type compilerOptionsParser struct`.
pub struct CompilerOptionsParser<'a> {
    pub compiler_options: &'a mut CompilerOptions,
    /// Go: `unresolvedPaths unresolvedCompilerOptionPaths` (nil disables
    /// template capture).
    pub unresolved_paths: Option<&'a mut OrderedMap<String, CompilerOptionsValue>>,
}

impl OptionParser for CompilerOptionsParser<'_> {
    fn parse_option(&mut self, key: &str, value: &CompilerOptionsValue) {
        if let Some(unresolved) = &mut self.unresolved_paths {
            if compiler_option_contains_config_dir_template(key, value) {
                unresolved.set(key.to_string(), value.clone());
                return;
            }
        }
        parse_compiler_options(key, value, self.compiler_options);
    }

    fn unknown_option_diagnostic() -> Option<&'static tsc_diagnostics::Message> {
        Some(&tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0)
    }

    fn unknown_did_you_mean_diagnostic() -> Option<&'static tsc_diagnostics::Message> {
        Some(&tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0_DID_YOU_MEAN_1)
    }
}

/// Go: `type buildOptionsParser struct` (parsinghelpers.go).
pub struct BuildOptionsParser<'a> {
    pub build_options: &'a mut BuildOptions,
}

impl OptionParser for BuildOptionsParser<'_> {
    fn parse_option(&mut self, key: &str, value: &CompilerOptionsValue) {
        crate::options_generated::parse_build_options(key, value, self.build_options);
    }

    fn unknown_option_diagnostic() -> Option<&'static tsc_diagnostics::Message> {
        Some(&tsc_diagnostics::UNKNOWN_BUILD_OPTION_0)
    }

    fn unknown_did_you_mean_diagnostic() -> Option<&'static tsc_diagnostics::Message> {
        Some(&tsc_diagnostics::UNKNOWN_BUILD_OPTION_0_DID_YOU_MEAN_1)
    }
}

/// Go: `func convertMapToOptions[O optionParser](compilerOptions, result) O` —
/// values have already been validated and converted.
pub fn convert_map_to_options<O: OptionParser>(
    compiler_options: &OrderedMap<String, CompilerOptionsValue>,
    result: &mut O,
) {
    for (key, value) in compiler_options.entries() {
        result.parse_option(key, value);
    }
}

/// Go: `func compilerOptionContainsConfigDirTemplate(key string, value any) bool`.
pub fn compiler_option_contains_config_dir_template(
    key: &str,
    value: &CompilerOptionsValue,
) -> bool {
    let Some(option) = crate::namemap::command_line_compiler_options_map().get(key) else {
        return false;
    };
    let mut path_kind = option.path_kind;
    if option.kind == CommandLineOptionKind::List {
        let Some(element) = option.elements() else {
            return false;
        };
        path_kind = element.path_kind;
    }
    if !path_kind.is_rooted() {
        return false;
    }
    if option.kind == CommandLineOptionKind::List {
        return parse_string_array(value)
            .iter()
            .any(|item| starts_with_config_dir_template(item));
    }
    matches!(value, CompilerOptionsValue::Str(path) if starts_with_config_dir_template(path))
}

/// Go: `func ParseCompilerOptions(key string, value any, allOptions *core.CompilerOptions)` —
/// the nil-checking wrapper around the generated `parseCompilerOptions`.
pub fn parse_compiler_options_wrapped(
    key: &str,
    value: Option<&CompilerOptionsValue>,
    all_options: &mut CompilerOptions,
) {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return;
    };
    parse_compiler_options(key, value, all_options);
}

/// Go: `func mergeCompilerOptions(targetOptions, sourceOptions *core.CompilerOptions, rawSource any)`.
pub fn merge_compiler_options(
    target_options: &mut CompilerOptions,
    source_options: Option<&CompilerOptions>,
    raw_source: Option<&CompilerOptionsValue>,
) {
    let Some(source_options) = source_options else {
        return;
    };
    let explicit_null_fields = explicit_null_fields_from_raw(raw_source);
    crate::options_generated::merge_compiler_option_fields(
        target_options,
        source_options,
        &explicit_null_fields,
    );
}

/// The explicitly-null keys under `compilerOptions` in the raw JSON
/// (Go: the inlined loop in `mergeCompilerOptions`).
fn explicit_null_fields_from_raw(raw_source: Option<&CompilerOptionsValue>) -> Set<String> {
    let mut fields = Set::new();
    let Some(CompilerOptionsValue::Object(raw_map)) = raw_source else {
        return fields;
    };
    if let Some(CompilerOptionsValue::Object(compiler_options_map)) = raw_map.get("compilerOptions")
    {
        for (key, value) in compiler_options_map.entries() {
            if value.is_null() {
                fields.add(key.clone());
            }
        }
    }
    fields
}

/// Go: `func mergeParsedCompilerOptions(targetOptions, sourceOptions *parsedCompilerOptions, rawSource any)`.
pub fn merge_parsed_compiler_options(
    target_options: &mut crate::tsconfigparsing::ParsedCompilerOptions,
    source_options: Option<&crate::tsconfigparsing::ParsedCompilerOptions>,
    raw_source: Option<&CompilerOptionsValue>,
) {
    let Some(source_options) = source_options else {
        return;
    };
    merge_compiler_options(
        &mut target_options.options,
        Some(&source_options.options),
        raw_source,
    );
    if let Some(CompilerOptionsValue::Object(raw_map)) = raw_source {
        if let Some(CompilerOptionsValue::Object(compiler_options_map)) =
            raw_map.get("compilerOptions")
        {
            for key in compiler_options_map.keys() {
                target_options.unresolved_paths.delete(key);
            }
        }
    }
    for (key, value) in source_options.unresolved_paths.entries() {
        target_options
            .unresolved_paths
            .set(key.clone(), value.clone());
    }
}

/// Go: `func convertToOptionsWithAbsolutePaths(optionsBase *collections.OrderedMap[string, any], optionMap CommandLineOptionNameMap, cwd tspath.RootedDirectoryPath)`.
pub fn convert_to_options_with_absolute_paths(
    options_base: &mut OrderedMap<String, CompilerOptionsValue>,
    option_map: &crate::namemap::NameMap,
    cwd: &RootedDirectoryPath,
) {
    let keys: Vec<String> = options_base.keys().cloned().collect();
    for o in keys {
        if let Some(value) = options_base.get(&o) {
            if let Some((result, true)) =
                convert_option_to_absolute_path(&o, value, option_map, cwd)
            {
                options_base.set(o.clone(), result);
            }
        }
    }
}

/// Go: `func ConvertOptionToAbsolutePath(o string, v any, optionMap CommandLineOptionNameMap, cwd tspath.RootedDirectoryPath) (any, bool)`.
pub fn convert_option_to_absolute_path(
    o: &str,
    v: &CompilerOptionsValue,
    option_map: &crate::namemap::NameMap,
    cwd: &RootedDirectoryPath,
) -> Option<(CompilerOptionsValue, bool)> {
    let option = option_map.get(o)?;
    if option.kind == CommandLineOptionKind::List {
        if option.elements().is_some_and(|e| e.is_file_path) {
            match v {
                CompilerOptionsValue::StrArray(arr) => {
                    let mapped: Vec<String> = arr
                        .iter()
                        .map(|item| get_normalized_absolute_path(item, cwd).into_owned())
                        .collect();
                    return Some((CompilerOptionsValue::StrArray(mapped), true));
                }
                CompilerOptionsValue::Array(arr) => {
                    let mapped: Vec<CompilerOptionsValue> = arr
                        .iter()
                        .map(|item| match item {
                            CompilerOptionsValue::Str(s) => CompilerOptionsValue::Str(
                                get_normalized_absolute_path(s, cwd).into_owned(),
                            ),
                            other => other.clone(),
                        })
                        .collect();
                    return Some((CompilerOptionsValue::Array(mapped), true));
                }
                _ => {}
            }
        }
    } else if option.is_file_path {
        if let CompilerOptionsValue::Str(value) = v {
            return Some((
                CompilerOptionsValue::Str(get_normalized_absolute_path(value, cwd).into_owned()),
                true,
            ));
        }
    }
    None
}
