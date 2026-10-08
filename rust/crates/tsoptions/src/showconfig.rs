// Port of tsc/internal/tsoptions/showconfig.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The `--showConfig` surface: `ConvertToTSConfig` + the implied-options
// table. `serializeCompilerOptions` itself is generated
// (options_generated.rs) — the helpers it calls live here.
//
// PORT notes:
//   - Go's `TSConfig` struct marshals via encoding/json with `omitzero`;
//     the port keeps the same shape and serializes through tsc_json in
//     `TSConfig::marshal` (ordered maps preserve key order, so the output is
//     deterministic — the exit criterion of the M4 wave).
//   - Go's `computeFn[T]` type-erases getters into `any`; the port stores
//     the same getters as `fn(&CompilerOptions) -> CompilerOptionsValue`.

use tsc_collections::OrderedMap;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::tristate::Tristate;
use tsc_tspath::{CaseSensitivity, RootedFilePath, RootedPath};

use crate::commandlineoption::{CommandLineOption, CompilerOptionsValue};
use crate::namemap::command_line_compiler_options_map;
use crate::options_generated::{serialize_compiler_option_enum, serialize_compiler_options};
use crate::parsedcommandline::ParsedCommandLine;
use crate::tsconfigparsing::DEFAULT_INCLUDE_SPEC;

/// Go: `type TSConfig struct` — the --showConfig output shape.
#[derive(Default)]
pub struct TSConfig {
    pub compiler_options: OrderedMap<String, CompilerOptionsValue>,
    pub references: Vec<CompilerOptionsValue>,
    pub files: Vec<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub compile_on_save: Option<bool>,
}

impl TSConfig {
    /// The deterministic JSON form — Go: `json.MarshalIndentWrite(w, tsConfig,
    /// "", "    ")` (tsc/internal/execute/tsc.go:425, i.e. `tsc --showConfig`).
    ///
    /// PORT (found by the showconfig parity twin): the previous port used a
    /// 2-space indent and skipped `compilerOptions` when empty — but Go's
    /// TSConfig marshals with a 4-space indent and the non-omitzero
    /// `CompilerOptions` map is ALWAYS emitted (an empty map renders as
    /// `"compilerOptions": {}`). Both now mirror Go byte-for-byte.
    pub fn marshal(&self) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        let mut first = true;
        let push_key = |out: &mut String, first: &mut bool, key: &str| {
            if !*first {
                out.push_str(",\n");
            }
            *first = false;
            out.push_str("    ");
            out.push_str(&serde_json::json!(key).to_string());
            out.push_str(": ");
        };
        // Go: `CompilerOptions *OrderedMap json:"compilerOptions"` — no
        // omitzero; ConvertToTSConfig always sets it, so the key is always
        // emitted ({} for an empty map, exactly like encoding/json/v2).
        push_key(&mut out, &mut first, "compilerOptions");
        out.push_str(&marshal_value_map(&self.compiler_options, 1));
        if !self.references.is_empty() {
            push_key(&mut out, &mut first, "references");
            out.push_str(&marshal_value_list(&self.references, 1));
        }
        if !self.files.is_empty() {
            push_key(&mut out, &mut first, "files");
            out.push_str(&marshal_string_list(&self.files, 1));
        }
        if !self.include.is_empty() {
            push_key(&mut out, &mut first, "include");
            out.push_str(&marshal_string_list(&self.include, 1));
        }
        if !self.exclude.is_empty() {
            push_key(&mut out, &mut first, "exclude");
            out.push_str(&marshal_string_list(&self.exclude, 1));
        }
        if let Some(compile_on_save) = self.compile_on_save {
            push_key(&mut out, &mut first, "compileOnSave");
            out.push_str(if compile_on_save { "true" } else { "false" });
        }
        if !first {
            out.push('\n');
        }
        out.push('}');
        out
    }
}

fn indent(depth: usize) -> String {
    "    ".repeat(depth)
}

fn marshal_string_list(values: &[String], depth: usize) -> String {
    let inner = values
        .iter()
        .map(|v| format!("{}{}", indent(depth + 1), serde_json::json!(v)))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("[\n{inner}\n{}]", indent(depth))
}

fn marshal_value_list(values: &[CompilerOptionsValue], depth: usize) -> String {
    let inner = values
        .iter()
        .map(|v| format!("{}{}", indent(depth + 1), marshal_value(v, depth + 1)))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("[\n{inner}\n{}]", indent(depth))
}

/// The JSON form of a `CompilerOptionsValue` (Go marshals the `any` value).
pub fn marshal_value(value: &CompilerOptionsValue, depth: usize) -> String {
    match value {
        CompilerOptionsValue::Null => "null".to_string(),
        CompilerOptionsValue::Bool(b) => b.to_string(),
        CompilerOptionsValue::Int(n) => n.to_string(),
        CompilerOptionsValue::Float(f) => {
            // Go: integral float64s print without a decimal point.
            if f.fract() == 0.0 && f.is_finite() {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        CompilerOptionsValue::Str(s) => serde_json::json!(s).to_string(),
        CompilerOptionsValue::StrArray(strings) => marshal_string_list(strings, depth),
        CompilerOptionsValue::Array(values) => marshal_value_list(values, depth),
        CompilerOptionsValue::Object(map) => marshal_value_map(map, depth),
        CompilerOptionsValue::PathsMap(map) => {
            let as_object: OrderedMap<String, CompilerOptionsValue> = map
                .entries()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        CompilerOptionsValue::StrArray(v.clone()),
                    )
                })
                .collect();
            let mut out = OrderedMap::<String, CompilerOptionsValue>::new();
            for (k, v) in as_object.entries() {
                out.set(k.clone(), v.clone());
            }
            marshal_value_map(&out, depth)
        }
        CompilerOptionsValue::PluginImports(imports) => {
            let values: Vec<CompilerOptionsValue> = imports
                .iter()
                .map(|import| {
                    let mut map = OrderedMap::new();
                    map.set(
                        "name".to_string(),
                        CompilerOptionsValue::Str(import.name.clone()),
                    );
                    CompilerOptionsValue::Object(map)
                })
                .collect();
            marshal_value_list(&values, depth)
        }
        CompilerOptionsValue::Tristate(t) => match *t {
            Tristate::TSTrue => "true".to_string(),
            Tristate::TSFalse => "false".to_string(),
            Tristate::TSUnknown => "null".to_string(),
        },
        CompilerOptionsValue::JsxEmit(v) => (*v as i32).to_string(),
        CompilerOptionsValue::ModuleKind(v) => (*v as i32).to_string(),
        CompilerOptionsValue::ModuleResolutionKind(v) => (*v as i32).to_string(),
        CompilerOptionsValue::ModuleDetectionKind(v) => (*v as i32).to_string(),
        CompilerOptionsValue::NewLineKind(v) => (*v as i32).to_string(),
        CompilerOptionsValue::ScriptTarget(v) => (*v as i32).to_string(),
    }
}

fn marshal_value_map(map: &OrderedMap<String, CompilerOptionsValue>, depth: usize) -> String {
    if map.is_empty() {
        return "{}".to_string();
    }
    let inner = map
        .entries()
        .map(|(k, v)| {
            format!(
                "{}{}: {}",
                indent(depth + 1),
                serde_json::json!(k),
                marshal_value(v, depth + 1)
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n{inner}\n{}}}", indent(depth))
}

/// Go: `type impliedOption struct` + `var impliedOptions` — the
/// computedOptions table. `dependencies` are Go struct field names.
struct ImpliedOption {
    /// The Go struct field name of the option (e.g. "Module").
    name: &'static str,
    /// The Go struct field names this option depends on.
    dependencies: &'static [&'static str],
    compute: fn(&CompilerOptions) -> CompilerOptionsValue,
}

fn compute_module(options: &CompilerOptions) -> CompilerOptionsValue {
    CompilerOptionsValue::ModuleKind(options.get_emit_module_kind())
}

fn compute_module_resolution(options: &CompilerOptions) -> CompilerOptionsValue {
    CompilerOptionsValue::ModuleResolutionKind(options.get_module_resolution_kind())
}

fn compute_module_detection(options: &CompilerOptions) -> CompilerOptionsValue {
    CompilerOptionsValue::ModuleDetectionKind(options.get_emit_module_detection_kind())
}

fn compute_bool(value: bool) -> CompilerOptionsValue {
    CompilerOptionsValue::Bool(value)
}

fn compute_isolated_modules(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_isolated_modules())
}

fn compute_preserve_const_enums(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.should_preserve_const_enums())
}

fn compute_declaration(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_emit_declarations())
}

fn compute_declaration_map(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_are_declaration_maps_enabled())
}

fn compute_incremental(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.is_incremental())
}

fn compute_use_define_for_class_fields(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_use_define_for_class_fields())
}

fn compute_resolve_package_json_exports(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_resolve_package_json_exports())
}

fn compute_resolve_package_json_imports(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_resolve_package_json_imports())
}

fn compute_resolve_json_module(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_resolve_json_module())
}

fn compute_allow_js(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_allow_js())
}

fn compute_allow_importing_ts_extensions(options: &CompilerOptions) -> CompilerOptionsValue {
    compute_bool(options.get_allow_importing_ts_extensions())
}

static IMPLIED_OPTIONS: &[ImpliedOption] = &[
    ImpliedOption {
        name: "Module",
        dependencies: &["Target"],
        compute: compute_module,
    },
    ImpliedOption {
        name: "ModuleResolution",
        dependencies: &["Module", "Target"],
        compute: compute_module_resolution,
    },
    ImpliedOption {
        name: "ModuleDetection",
        dependencies: &["Module", "Target"],
        compute: compute_module_detection,
    },
    ImpliedOption {
        name: "IsolatedModules",
        dependencies: &["VerbatimModuleSyntax"],
        compute: compute_isolated_modules,
    },
    ImpliedOption {
        name: "PreserveConstEnums",
        dependencies: &["IsolatedModules", "VerbatimModuleSyntax"],
        compute: compute_preserve_const_enums,
    },
    ImpliedOption {
        name: "Declaration",
        dependencies: &["Composite"],
        compute: compute_declaration,
    },
    ImpliedOption {
        name: "DeclarationMap",
        dependencies: &["Declaration", "Composite"],
        compute: compute_declaration_map,
    },
    ImpliedOption {
        name: "Incremental",
        dependencies: &["Composite"],
        compute: compute_incremental,
    },
    ImpliedOption {
        name: "UseDefineForClassFields",
        dependencies: &["Target", "Module"],
        compute: compute_use_define_for_class_fields,
    },
    ImpliedOption {
        name: "ResolvePackageJsonExports",
        dependencies: &["ModuleResolution", "Module", "Target"],
        compute: compute_resolve_package_json_exports,
    },
    ImpliedOption {
        name: "ResolvePackageJsonImports",
        dependencies: &[
            "ModuleResolution",
            "ResolvePackageJsonExports",
            "Module",
            "Target",
        ],
        compute: compute_resolve_package_json_imports,
    },
    ImpliedOption {
        name: "ResolveJsonModule",
        dependencies: &["ModuleResolution", "Module", "Target"],
        compute: compute_resolve_json_module,
    },
    ImpliedOption {
        name: "AllowJs",
        dependencies: &["CheckJs"],
        compute: compute_allow_js,
    },
    ImpliedOption {
        name: "AllowImportingTsExtensions",
        dependencies: &["RewriteRelativeImportExtensions"],
        compute: compute_allow_importing_ts_extensions,
    },
];

/// Go: `func ConvertToTSConfig(configParseResult *ParsedCommandLine, configFileName tspath.RootedFilePath) *TSConfig`.
pub fn convert_to_tsconfig(
    config_parse_result: &ParsedCommandLine,
    config_file_name: &RootedFilePath,
) -> TSConfig {
    let config_file_name_owned;
    let config_file_name = if config_file_name.is_empty() {
        config_file_name_owned = config_parse_result
            .base_directory
            .resolve_file("tsconfig.json");
        &config_file_name_owned
    } else {
        config_file_name
    };
    let case_sensitivity = config_parse_result.case_sensitivity;

    // Build the list of all resolved files as relative paths from the config file.
    let mut files = Vec::new();
    for f in config_parse_result.file_names() {
        if let Some(relative_path) =
            case_sensitivity.relative_path_from_file(config_file_name, f)
        {
            files.push(relative_path.as_module_specifier().as_string().to_owned());
        } else {
            files.push(f.as_string().to_owned());
        }
    }

    // Serialize compiler options.
    let mut option_map = serialize_compiler_options(
        &config_parse_result.compiler_options.options,
        config_file_name,
        &case_sensitivity,
    );

    for name in [
        "showConfig",
        "configFile",
        "configFilePath",
        "help",
        "init",
        "listFilesOnly",
        "listEmittedFiles",
        "project",
        "build",
        "version",
    ] {
        option_map.delete(name);
    }

    add_implied_options(&mut option_map, &config_parse_result.compiler_options.options);

    let mut config = TSConfig {
        compiler_options: option_map,
        ..TSConfig::default()
    };

    // Add references.
    let refs = config_parse_result.project_references();
    if !refs.is_empty() {
        let mut references = Vec::new();
        for r in refs {
            let mut r#ref = OrderedMap::new();
            r#ref.set(
                "path".to_string(),
                CompilerOptionsValue::Str(r.original_path.clone()),
            );
            if r.circular {
                r#ref.set("circular".to_string(), CompilerOptionsValue::Bool(true));
            }
            references.push(CompilerOptionsValue::Object(r#ref));
        }
        config.references = references;
    }

    // Add files.
    if !files.is_empty() {
        config.files = files;
    }

    // Add include/exclude from configFileSpecs.
    if let Some(specs) = config_parse_result.config_file_specs() {
        let include = filter_same_as_default_include(
            &specs
                .validated_include_specs
                .iter()
                .map(|s| s.as_string().to_owned())
                .collect::<Vec<_>>(),
        );
        if !include.is_empty() {
            config.include = include;
        }
        config.exclude = specs
            .validated_exclude_specs
            .iter()
            .map(|s| s.as_string().to_owned())
            .collect();
    }

    // Add compileOnSave.
    if let Some(compile_on_save) = config_parse_result.compile_on_save {
        if compile_on_save {
            config.compile_on_save = Some(true);
        }
    }

    config
}

/// Go: `func filterSameAsDefaultInclude(specs []string) []string`.
fn filter_same_as_default_include(specs: &[String]) -> Vec<String> {
    if specs.is_empty() {
        return Vec::new();
    }
    if specs.len() == 1 && specs[0] == DEFAULT_INCLUDE_SPEC {
        return Vec::new();
    }
    specs.to_vec()
}

/// Go: `func getNameOfCompilerOptionValue(value any, enumMap) string`.
pub fn get_name_of_compiler_option_value(
    value: &str,
    enum_map: &OrderedMap<String, CompilerOptionsValue>,
) -> String {
    for (k, v) in enum_map.entries() {
        if matches!(v, CompilerOptionsValue::Str(s) if s == value) {
            return k.clone();
        }
    }
    String::new()
}

/// Go: `func serializeCompilerOptionPath(value tspath.RootedPath, configFilePath, caseSensitivity) string`.
pub fn serialize_compiler_option_path(
    value: &RootedPath,
    config_file_path: &RootedFilePath,
    case_sensitivity: &CaseSensitivity,
) -> String {
    if let Some(relative_path) =
        case_sensitivity.relative_path_from_file_to_path(config_file_path, value)
    {
        return relative_path.as_module_specifier().as_string().to_owned();
    }
    value.as_string().to_owned()
}

/// Go: `func serializeCompilerOptionEnumList(values []string, enumMap) []string`.
pub fn serialize_compiler_option_enum_list(
    values: &[String],
    enum_map: &OrderedMap<String, CompilerOptionsValue>,
) -> Vec<String> {
    values
        .iter()
        .map(|value| {
            let name = get_name_of_compiler_option_value(value, enum_map);
            if name.is_empty() {
                value.clone()
            } else {
                name
            }
        })
        .collect()
}

/// Go: `func addImpliedOptions(optionMap, options)`.
fn add_implied_options(
    option_map: &mut OrderedMap<String, CompilerOptionsValue>,
    options: &CompilerOptions,
) {
    // Build the set of explicitly provided option JSON names.
    let provided: std::collections::HashSet<String> =
        option_map.keys().cloned().collect();

    let default_opts = CompilerOptions::default();

    for entry in IMPLIED_OPTIONS {
        let Some(option_decl) = command_line_compiler_options_map().get(entry.name) else {
            continue;
        };
        if provided.contains(option_decl.name) {
            continue;
        }
        if !any_dependency_provided(entry.dependencies, &provided) {
            continue;
        }
        let implied = (entry.compute)(options);
        let default_val = (entry.compute)(&default_opts);
        if implied == default_val {
            continue;
        }
        let Some(serialized) = serialize_implied_option_value(option_decl, &implied) else {
            continue;
        };
        option_map.set(option_decl.name.to_string(), serialized);
    }
}

/// Go: `func anyDependencyProvided(dependencies []string, provided map[string]bool) bool`.
fn any_dependency_provided(
    dependencies: &[&'static str],
    provided: &std::collections::HashSet<String>,
) -> bool {
    dependencies.iter().any(|dep| {
        command_line_compiler_options_map()
            .get(dep)
            .is_some_and(|dep_decl| provided.contains(dep_decl.name))
    })
}

/// Go: `func serializeImpliedOptionValue(optionDecl *CommandLineOption, value any) any`.
fn serialize_implied_option_value(
    option_decl: &CommandLineOption,
    value: &CompilerOptionsValue,
) -> Option<CompilerOptionsValue> {
    if value.is_null() {
        return None;
    }
    if let Some(_enum_map) = option_decl.enum_map() {
        let s = serialize_compiler_option_enum(value);
        if !s.is_empty() {
            return Some(CompilerOptionsValue::Str(s));
        }
        return None;
    }
    match value {
        CompilerOptionsValue::Bool(_) => Some(value.clone()),
        CompilerOptionsValue::Tristate(t) => match *t {
            Tristate::TSTrue => Some(CompilerOptionsValue::Bool(true)),
            Tristate::TSFalse => Some(CompilerOptionsValue::Bool(false)),
            Tristate::TSUnknown => None,
        },
        other => Some(other.clone()),
    }
}
