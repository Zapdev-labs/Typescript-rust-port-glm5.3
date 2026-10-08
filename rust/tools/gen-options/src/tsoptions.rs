// gen-options (tsoptions extension) — emits the tsc-tsoptions generated tables
// (SPEC §5.8, §6) from rust/tools/gen-options/data/tsoptions-model.json.
//
// That data file is a mechanical extraction of the FULL
// tools/scripts/tsc/options.ts `OptionsModel` literal (commit
// ec47d33c23e464a17cdf2475632cba629bee8763) — the same single source the
// upstream tools/scripts/tsc/generate-options.ts consumes to emit
// tsc/internal/tsoptions/{options,declarations}_generated.go. The extraction
// is done by data/extract-tsoptions-model.mjs (node type-stripping over
// options.ts); per SPEC §6 the Go output is NEVER a generation input, only the
// parity baseline for tests.
//
// Outputs (all idempotent, rustfmt-stable, checked by --check):
//   - rust/crates/tsoptions/src/options_generated.rs
//     (parseCompilerOptions / defaults / ParseTypeAcquisition / ParseBuildOptions /
//      CompilerOptionsAffect* / ForEachCompilerOptionAffectingBuildInfo /
//      mergeCompilerOptionFields / handleOptionConfigDirTemplateSubstitution /
//      serializeCompilerOptions + serializeCompilerOptionEnum)
//   - rust/crates/tsoptions/src/declarations_generated.rs
//     (the CommandLineOption tables, element/enum/deprecated side maps,
//      LibMap + option enum maps, targetToLibMap)
//   - rust/crates/contentmapper/src/compiler_option_fields_generated.rs
//     (the reflection-driven `compilerOptionFields` json-tag marshaler from
//     tsc/internal/contentmapper/contentmapper.go as an exhaustive match)

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use serde::Deserialize;

use super::rust_type;

// ────────────────────────────────────────────────────────────────────────────
// Full options model (mirrors tools/scripts/tsc/options-model.ts declarations)
// ────────────────────────────────────────────────────────────────────────────

/// GoValue: string | number | boolean | DiagnosticMessage | {go: "core.X"}.
///
/// `DiagnosticMessage` and the `{go: "core.X"}` shape are both `{go: string}`
/// objects — indistinguishable to serde's untagged enum — so they share one
/// variant and are split by the `diagnostics.` / `core.` prefix at use sites.
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum GoValue {
    GoRef { go: String },
    Str(String),
    Bool(bool),
    Number(f64),
}

/// `DiagnosticMessage` (options-model.ts) — the `diagnostic()` factory output.
#[derive(Deserialize, Clone)]
pub struct DiagnosticRef {
    pub go: String,
    #[allow(dead_code)]
    pub text: String,
}

#[derive(Deserialize, Clone)]
pub struct DeclarationMetadata {
    group: String,
    #[serde(rename = "extraShortNames", default)]
    extra_short_names: Vec<String>,
    #[serde(rename = "shortName", default)]
    short_name: Option<String>,
    #[serde(rename = "isFilePath", default)]
    is_file_path: Option<bool>,
    /// Mirrored for provenance only (compiler options carry pathKind at the
    /// top level, not on the declaration).
    #[allow(dead_code)]
    #[serde(rename = "pathKind", default)]
    path_kind: Option<String>,
    #[serde(rename = "isTSConfigOnly", default)]
    is_tsconfig_only: Option<bool>,
    #[serde(rename = "isCommandLineOnly", default)]
    is_command_line_only: Option<bool>,
    description: Option<DiagnosticRef>,
    #[serde(rename = "defaultValueDescription", default)]
    default_value_description: Option<GoValue>,
    #[serde(rename = "showInSimplifiedHelpView", default)]
    show_in_simplified_help_view: Option<bool>,
    category: Option<DiagnosticRef>,
    #[serde(rename = "extraValidation", default)]
    extra_validation: Option<String>,
    #[serde(rename = "minValue", default)]
    min_value: Option<i64>,
    #[serde(rename = "allowConfigDirTemplateSubstitution", default)]
    allow_config_dir_template_substitution: Option<bool>,
    #[serde(rename = "affectsDeclarationPath", default)]
    affects_declaration_path: Option<bool>,
    #[serde(rename = "affectsSemanticDiagnostics", default)]
    affects_semantic_diagnostics: Option<bool>,
    #[serde(rename = "affectsBuildInfo", default)]
    affects_build_info: Option<bool>,
    #[serde(rename = "affectsEmit", default)]
    affects_emit: Option<bool>,
    #[serde(rename = "strictFlag", default)]
    strict_flag: Option<bool>,
    #[serde(rename = "listPreserveFalsyValues", default)]
    list_preserve_falsy_values: Option<bool>,
}

/// CompilerOption (options-model.ts).
#[derive(Deserialize, Clone)]
pub struct FullOption {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(rename = "goName", default)]
    pub go_name: Option<String>,
    /// Mirrored for provenance only (used by the core/API generator outputs).
    #[serde(default)]
    #[allow(dead_code)]
    pub internal: bool,
    #[serde(default)]
    #[allow(dead_code)]
    pub deprecated: bool,
    #[serde(rename = "pathKind", default)]
    pub path_kind: Option<String>,
    #[serde(rename = "parseAliases", default)]
    pub parse_aliases: Vec<String>,
    #[serde(default)]
    pub declaration: Option<DeclarationMetadata>,
    #[serde(rename = "jsconfigDefault", default)]
    pub jsconfig_default: Option<GoValue>,
    /// `showConfig: false` explicitly removes the option from --showConfig.
    #[serde(rename = "showConfig", default)]
    pub show_config: Option<bool>,
}

/// Declaration (options-model.ts) — as it appears in `elements`, `rootOptions`,
/// `typeAcquisition`, `buildOptions` (already kind-resolved there).
#[derive(Deserialize, Clone)]
pub struct RawDeclaration {
    pub name: String,
    pub kind: String,
    #[serde(rename = "shortName", default)]
    pub short_name: Option<String>,
    #[serde(rename = "isFilePath", default)]
    pub is_file_path: Option<bool>,
    #[serde(rename = "pathKind", default)]
    pub path_kind: Option<String>,
    #[serde(rename = "isTSConfigOnly", default)]
    pub is_tsconfig_only: Option<bool>,
    #[serde(rename = "isCommandLineOnly", default)]
    pub is_command_line_only: Option<bool>,
    pub description: Option<DiagnosticRef>,
    #[serde(rename = "defaultValueDescription", default)]
    pub default_value_description: Option<GoValue>,
    #[serde(rename = "showInSimplifiedHelpView", default)]
    pub show_in_simplified_help_view: Option<bool>,
    pub category: Option<DiagnosticRef>,
    #[serde(rename = "extraValidation", default)]
    pub extra_validation: Option<String>,
    #[serde(rename = "minValue", default)]
    pub min_value: Option<i64>,
    #[serde(rename = "listPreserveFalsyValues", default)]
    pub list_preserve_falsy_values: Option<bool>,
    // extension fields on the specialized declaration kinds
    #[serde(default)]
    pub field: Option<FieldRef>,
    #[serde(default)]
    pub variable: Option<String>,
    #[serde(rename = "elementOptions", default)]
    pub element_options: Option<String>,
    #[serde(rename = "jsconfigDefault", default)]
    pub jsconfig_default: Option<GoValue>,
}

#[derive(Deserialize, Clone)]
pub struct FieldRef {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Deserialize, Clone)]
pub struct EnumMapDef {
    #[serde(rename = "goName")]
    pub go_name: String,
    pub values: Vec<EnumMapEntry>,
    #[serde(rename = "deprecatedKeys", default)]
    pub deprecated_keys: Vec<String>,
}

#[derive(Deserialize, Clone)]
pub struct EnumMapEntry {
    pub name: String,
    pub value: GoValue,
}

#[derive(Deserialize, Clone)]
pub struct LibEnumMember {
    pub name: String,
    #[serde(default)]
    pub lib: Option<String>,
    #[serde(rename = "libComment", default)]
    pub lib_comment: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct LibEnumDef {
    pub name: String,
    pub members: Vec<LibEnumMember>,
}

#[derive(Deserialize)]
pub struct TsoptionsModel {
    #[serde(rename = "compilerOptions")]
    pub compiler_options: Vec<FullOption>,
    #[serde(rename = "declarationOrder")]
    pub declaration_order: HashMap<String, Vec<String>>,
    #[serde(rename = "typeAcquisition")]
    pub type_acquisition: Vec<RawDeclaration>,
    #[serde(rename = "buildOptions")]
    pub build_options: Vec<RawDeclaration>,
    /// Mirrored for provenance only (the core struct emission orders by it).
    #[allow(dead_code)]
    #[serde(rename = "buildOptionFieldOrder")]
    pub build_option_field_order: Vec<String>,
    // rootOptions/elements/enumMaps are kept as raw JSON so emission can walk
    // them in document order (serde_json objects deserialize key-sorted into
    // maps; these three are semantically unordered maps in the Go output, so
    // either order is faithful — we keep the JSON document order).
    // rootOptions is an array in options.ts (declaration order is semantic for
    // the tsconfigRootOptionsMap element list); elements/enumMaps are objects
    // (semantically unordered maps in the Go output) — emitted in JSON
    // document order.
    #[serde(rename = "rootOptions")]
    pub root_options: Vec<RawDeclaration>,
    #[serde(rename = "elements")]
    pub elements_raw: serde_json::Value,
    #[serde(rename = "enumMaps")]
    pub enum_maps_raw: serde_json::Value,
    pub enums: Vec<LibEnumDef>,
}

// ────────────────────────────────────────────────────────────────────────────
// Name-mapping helpers
// ────────────────────────────────────────────────────────────────────────────

/// `diagnostics.Foo_bar` (or the bare `Foo_bar`) → `FOO_BAR` (the Rust
/// messages_generated static name).
fn diag_static_name(go_name: &str) -> String {
    let stripped = go_name.strip_prefix("diagnostics.").unwrap_or(go_name);
    let mut out = String::with_capacity(stripped.len());
    let mut prev_sep = true;
    for ch in stripped.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    out
}

/// A `&'static tsc_diagnostics::X` reference expression for a diagnostic ref.
fn diag_expr(diag: &DiagnosticRef) -> String {
    format!("&tsc_diagnostics::{}", diag_static_name(&diag.go))
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Go field name: goName, or name with capitalized head (options.ts fieldName()).
fn go_field_name(name: &str, go_name: Option<&String>) -> String {
    match go_name {
        Some(n) => n.clone(),
        None => {
            let mut chars = name.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

fn snake_case_field(name: &str, go_name: Option<&String>) -> String {
    super::snake_case(&go_field_name(name, go_name))
}

fn enum_name_of_type(ty: &str) -> &str {
    ty.strip_prefix("core.").unwrap_or(ty)
}

fn enum_fn_snake(enum_name: &str) -> String {
    let mut out = String::with_capacity(enum_name.len() + 6);
    let mut prev_lower = false;
    for ch in enum_name.chars() {
        if ch.is_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_lower = false;
        } else {
            prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
            out.push(ch);
        }
    }
    out
}

/// optionKind(options-model.ts): Tristate→Boolean, string→String, *int→Number,
/// []string/[]PluginImport→List, OrderedMap→Object, enums→Enum.
fn option_kind(ty: &str) -> &'static str {
    match ty {
        "Tristate" => "Boolean",
        "string" => "String",
        "*int" => "Number",
        "[]string" | "[]PluginImport" => "List",
        "*collections.OrderedMap[string, []string]" => "Object",
        _ => "Enum",
    }
}

/// The Go field type the core struct uses for `opt` — the model carries
/// `type: "string"` for path-typed fields and separates them by `pathKind`
/// (Go's core struct has typed path fields).
fn effective_go_type(opt: &FullOption) -> String {
    match (opt.ty.as_str(), opt.path_kind.as_deref()) {
        ("string", Some("file")) => "tspath.RootedFilePath".to_string(),
        ("string", Some("directory")) => "tspath.RootedDirectoryPath".to_string(),
        ("string", Some("fileOrDirectory")) => "tspath.RootedPath".to_string(),
        ("string", Some("sourceMapLocation")) => "tspath.SourceMapLocation".to_string(),
        ("[]string", Some("directory")) => "[]tspath.RootedDirectoryPath".to_string(),
        _ => opt.ty.clone(),
    }
}

/// The zero member of a core enum (Go's numeric literal `0` in generated code).
fn enum_zero_member(ty: &str) -> &'static str {
    let name = enum_name_of_type(ty);
    match name {
        "ModuleDetectionKind" | "ModuleKind" | "NewLineKind" | "ScriptTarget" | "JsxEmit" => "None",
        "ModuleResolutionKind" => "Unknown",
        other => panic!("gen-options: no zero member for enum {other}"),
    }
}

/// `.clone()` only when the Rust field type is not `Copy`
/// (clippy::clone_on_copy would fire on Tristate/enums/ints).
fn clone_suffix(ty: &str) -> &'static str {
    match ty {
        "Tristate" | "*int" => "",
        "string" | "[]string" | "[]PluginImport" | "[]tspath.RootedDirectoryPath"
        | "*collections.OrderedMap[string, []string]" => ".clone()",
        _ => {
            if ty.starts_with("tspath.") {
                ".clone()"
            } else {
                "" // core enums are Copy
            }
        }
    }
}

/// The zero-value expression for a CompilerOptions field (generate-options.ts
/// zeroValue(), applied to the Go field type).
fn zero_expr(ty: &str) -> String {
    match ty {
        "Tristate" => "Tristate::TSUnknown".to_string(),
        "string" => "String::new()".to_string(),
        "*int" => "None".to_string(),
        // PORT: Go's explicit-null branch writes nil; the Option<Vec> field
        // keeps that exact (a later non-nil source can still override).
        "[]string" | "[]PluginImport" | "[]tspath.RootedDirectoryPath" => "None".to_string(),
        "*collections.OrderedMap[string, []string]" => "None".to_string(),
        "tspath.RootedFilePath" | "tspath.RootedDirectoryPath" | "tspath.RootedPath"
        | "tspath.SourceMapLocation" => format!("{}::default()", rust_type(ty)),
        other => format!("{}::{}", enum_name_of_type(other), enum_zero_member(other)),
    }
}

/// The `!= zero` comparison for a CompilerOptions field.
fn not_zero(field: &str, ty: &str) -> String {
    match ty {
        "Tristate" => format!("{field} != Tristate::TSUnknown"),
        "string" => format!("!{field}.is_empty()"),
        "*int" => format!("{field}.is_some()"),
        "[]string" | "[]PluginImport" | "[]tspath.RootedDirectoryPath" => {
            format!("{field}.is_some()")
        }
        "*collections.OrderedMap[string, []string]" => format!("{field}.is_some()"),
        "tspath.RootedFilePath" | "tspath.RootedDirectoryPath" | "tspath.RootedPath"
        | "tspath.SourceMapLocation" => format!("!{field}.is_empty()"),
        other => format!(
            "{field} != {}::{}",
            enum_name_of_type(other),
            enum_zero_member(other)
        ),
    }
}

/// The CompilerOptionsValue expression carrying a field of Go type `ty`.
fn value_variant(ty: &str, field: &str) -> String {
    match ty {
        "Tristate" => format!("CompilerOptionsValue::Tristate({field})"),
        "string" => format!("CompilerOptionsValue::Str({field}.clone())"),
        "*int" => format!("CompilerOptionsValue::Int({field}.unwrap())"),
        "[]string" => format!("CompilerOptionsValue::StrArray({field}.clone().unwrap())"),
        "[]PluginImport" => format!("CompilerOptionsValue::PluginImports({field}.clone().unwrap())"),
        "*collections.OrderedMap[string, []string]" => {
            format!("CompilerOptionsValue::PathsMap({field}.clone().unwrap())")
        }
        "tspath.RootedFilePath" | "tspath.RootedDirectoryPath" | "tspath.RootedPath"
        | "tspath.SourceMapLocation" => {
            // PORT: Go passes the typed path as `any`; the port's value model
            // carries its JSON form (a string) — identical once marshaled.
            format!("CompilerOptionsValue::Str({field}.as_string().to_owned())")
        }
        other => format!(
            "CompilerOptionsValue::{}({field})",
            enum_name_of_type(other)
        ),
    }
}

/// The parseCompilerOptions assignment for one compiler option
/// (generate-options.ts parserAssignment()).
fn parser_assignment(opt: &FullOption, target: &str) -> String {
    match opt.path_kind.as_deref() {
        Some("file") => return format!("{target} = parse_file_name(value);"),
        Some("directory") => {
            return if opt.ty == "[]string" {
                // PORT: Go parseDirectoryNames returns a non-nil slice; the
                // Option<Vec> field wraps it in Some (Go nil only when unset).
                format!("{target} = Some(parse_directory_names(value));")
            } else {
                format!("{target} = parse_directory_name(value);")
            };
        }
        Some("fileOrDirectory") => {
            return format!("{target} = parse_file_or_directory_name(value);");
        }
        Some("sourceMapLocation") => {
            return format!("{target} = to_source_map_location(parse_string(value));");
        }
        _ => {}
    }
    match opt.name.as_str() {
        "lib" => {
            return format!(
                "if let CompilerOptionsValue::StrArray(libs) = value {{ {target} = Some(libs.clone()); }} else {{ {target} = Some(parse_string_array(value)); }}"
            );
        }
        "plugins" => {
            return format!(
                "if let CompilerOptionsValue::Array(plugins) = value {{ {target} = Some(plugins.iter().map(|plugin| match plugin {{ CompilerOptionsValue::Object(plugin_map) => PluginImport {{ name: parse_string(&plugin_map.get_or_zero(\"name\")) }}, _ => PluginImport {{ name: String::new() }} }}).collect()); }}"
            );
        }
        _ => {}
    }
    let parser = match opt.ty.as_str() {
        "Tristate" => "parse_tristate(value)".to_string(),
        "string" => "parse_string(value)".to_string(),
        "*int" => "parse_number(value)".to_string(),
        // PORT: Go's parser assigns the (non-nil) slice the JSON decode
        // produced; `Some(...)` marks the field as explicitly set — including
        // an explicit empty `[]`, which must not collapse into unset.
        "[]string" => "Some(parse_string_array(value))".to_string(),
        "*collections.OrderedMap[string, []string]" => "parse_string_map(value)".to_string(),
        "[]PluginImport" => panic!("gen-options: Plugin options require the plugin parser"),
        other => format!(
            "value.as_{}_flag()",
            enum_fn_snake(enum_name_of_type(other))
        ),
    };
    format!("{target} = {parser};")
}

// ────────────────────────────────────────────────────────────────────────────
// Loading
// ────────────────────────────────────────────────────────────────────────────

static MODEL: OnceLock<TsoptionsModel> = OnceLock::new();

pub fn load() -> &'static TsoptionsModel {
    MODEL.get_or_init(|| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = manifest_dir.join("data/tsoptions-model.json");
        let json_str = std::fs::read_to_string(&input)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", input.display()));
        let raw: serde_json::Value = serde_json::from_str(&json_str)
            .unwrap_or_else(|e| panic!("cannot parse {}: {e}", input.display()));
        serde_json::from_value(raw)
            .unwrap_or_else(|e| panic!("cannot parse {}: {e}", input.display()))
    })
}

/// Document-ordered entries of a JSON object, deserialized per value.
fn ordered_entries<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
) -> Vec<(String, T)> {
    let map = value
        .as_object()
        .unwrap_or_else(|| panic!("gen-options: expected JSON object"));
    map.iter()
        .map(|(key, val)| {
            (
                key.clone(),
                serde_json::from_value(val.clone())
                    .unwrap_or_else(|e| panic!("gen-options: bad entry {key}: {e}")),
            )
        })
        .collect()
}

// ────────────────────────────────────────────────────────────────────────────
// CommandLineOption literal emission
// ────────────────────────────────────────────────────────────────────────────

/// Generate-options.ts `declarationLiteral()` — one CommandLineOption literal,
/// fields in Go struct-declaration order.
#[allow(clippy::too_many_arguments)]
fn declaration_literal(
    name: &str,
    kind: &str,
    short_name: Option<&str>,
    is_file_path: Option<bool>,
    path_kind: Option<&str>,
    is_tsconfig_only: Option<bool>,
    is_command_line_only: Option<bool>,
    description: Option<&DiagnosticRef>,
    default_value_description: Option<&GoValue>,
    show_in_simplified_help_view: Option<bool>,
    category: Option<&DiagnosticRef>,
    extra_validation: Option<&str>,
    min_value: Option<i64>,
    list_preserve_falsy_values: Option<bool>,
    element_options: Option<&str>,
) -> String {
    let mut fields = vec![
        format!("name: {}", quote(name)),
        format!("kind: CommandLineOptionKind::{kind}"),
        format!(
            "short_name: {}",
            short_name.map_or("\"\"".to_string(), quote)
        ),
    ];
    // Rust struct literals in `static`s cannot omit fields (no FRU in const
    // position), so — unlike Go's sparse literals — every field is emitted,
    // with the zero value where options.ts leaves it unset.
    let path_variant = match path_kind {
        Some(pk) => match pk {
            "file" => "File",
            "directory" => "Directory",
            "fileOrDirectory" => "FileOrDirectory",
            "sourceMapLocation" => "SourceMapLocation",
            "fileSpec" => "FileSpec",
            "pathPattern" => "PathPattern",
            "resolvedPathPattern" => "ResolvedPathPattern",
            "configLocator" => "ConfigLocator",
            other => panic!("gen-options: unknown pathKind {other}"),
        },
        None => "None",
    };
    fields.push(format!("path_kind: CommandLineOptionPathKind::{path_variant}"));
    fields.push(format!("is_file_path: {}", is_file_path.unwrap_or(false)));
    fields.push(format!(
        "is_tsconfig_only: {}",
        is_tsconfig_only.unwrap_or(false)
    ));
    fields.push(format!(
        "is_command_line_only: {}",
        is_command_line_only.unwrap_or(false)
    ));
    fields.push(match description {
        Some(desc) => format!("description: Some({})", diag_expr(desc)),
        None => "description: None".to_string(),
    });
    fields.push(match default_value_description {
        Some(dvd) => format!(
            "default_value_description: Some({})",
            default_value_description_expr(dvd)
        ),
        None => "default_value_description: None".to_string(),
    });
    fields.push(format!(
        "show_in_simplified_help_view: {}",
        show_in_simplified_help_view.unwrap_or(false)
    ));
    fields.push(match category {
        Some(cat) => format!("category: Some({})", diag_expr(cat)),
        None => "category: None".to_string(),
    });
    fields.push(match extra_validation {
        Some(ev) => {
            assert_eq!(ev, "locale", "gen-options: unknown extraValidation {ev}");
            "extra_validation: ExtraValidation::Locale".to_string()
        }
        None => "extra_validation: ExtraValidation::None".to_string(),
    });
    fields.push(format!("min_value: {}", min_value.unwrap_or(0)));
    fields.push(format!(
        "list_preserve_falsy_values: {}",
        list_preserve_falsy_values.unwrap_or(false)
    ));
    let element_variant = match element_options {
        Some(eo) => match eo {
            "compilerOptions" => "CompilerOptions",
            "typeAcquisition" => "TypeAcquisition",
            "extends" => "Extends",
            "tsconfigRoot" => "TsconfigRoot",
            other => panic!("gen-options: unknown elementOptions {other}"),
        },
        None => "None",
    };
    fields.push(format!("element_options: ElementOptionsKind::{element_variant}"));
    let mut out = String::new();
    let _ = writeln!(out, "CommandLineOption {{");
    for field in fields {
        let _ = writeln!(out, "    {field},");
    }
    let _ = writeln!(out, "}}");
    out
}

fn default_value_description_expr(value: &GoValue) -> String {
    match value {
        GoValue::Str(s) => format!("DefaultValueDescription::Str({})", quote(s)),
        GoValue::Bool(b) => format!("DefaultValueDescription::Bool({b})"),
        GoValue::Number(n) => format!("DefaultValueDescription::Int({n})"),
        GoValue::GoRef { go } => {
            if let Some(name) = go.strip_prefix("diagnostics.") {
                format!(
                    "DefaultValueDescription::Message(&tsc_diagnostics::{})",
                    diag_static_name(name)
                )
            } else if go == "core.TSUnknown" {
                "DefaultValueDescription::Tristate(Tristate::TSUnknown)".to_string()
            } else {
                let (enum_name, member) = split_core_ref(go);
                format!(
                    "DefaultValueDescription::{enum_name}({enum_name}::{member})"
                )
            }
        }
    }
}

/// A RawDeclaration literal (elements/rootOptions/typeAcquisition/buildOptions
/// rows are already kind-resolved in the model).
fn raw_declaration_literal(decl: &RawDeclaration, element_options: Option<&str>) -> String {
    declaration_literal(
        &decl.name,
        &decl.kind,
        decl.short_name.as_deref(),
        decl.is_file_path,
        decl.path_kind.as_deref(),
        decl.is_tsconfig_only,
        decl.is_command_line_only,
        decl.description.as_ref(),
        decl.default_value_description.as_ref(),
        decl.show_in_simplified_help_view,
        decl.category.as_ref(),
        decl.extra_validation.as_deref(),
        decl.min_value,
        decl.list_preserve_falsy_values,
        element_options,
    )
}

/// A `&CommandLineOption {...},` slice element, properly indented
/// (`declaration_literal` emits at base indentation).
///
/// The trailing comma is load-bearing: without it the elements parse as a
/// `expr & expr` bitwise-and chain instead of a list.
fn slice_element(lit: &str) -> String {
    let mut out = String::new();
    for line in lit.trim_end().lines() {
        if line == "}" {
            out.push_str("    },\n");
        } else if let Some(rest) = line.strip_prefix("CommandLineOption {") {
            out.push_str("    &CommandLineOption {");
            out.push_str(rest);
            out.push('\n');
        } else {
            out.push_str("        ");
            out.push_str(line.trim_start());
            out.push('\n');
        }
    }
    out
}

/// A `pub static NAME: CommandLineOption = CommandLineOption {...};` body
/// (fields at 4 spaces, closing brace at column 0).
fn static_body(lit: &str) -> String {
    let mut out = String::new();
    for line in lit.trim_end().lines() {
        if line == "}" {
            out.push_str("}\n");
        } else if let Some(rest) = line.strip_prefix("CommandLineOption {") {
            out.push_str("CommandLineOption {");
            out.push_str(rest);
            out.push('\n');
        } else {
            out.push_str("    ");
            out.push_str(line.trim_start());
            out.push('\n');
        }
    }
    out
}

/// compilerDeclarations(): flatten compilerOptions with declarations into
/// (group, literals) pairs; alias entries (extraShortNames) drop description +
/// showInSimplifiedHelpView, exactly like the Go generator.
fn compiler_declarations(model: &TsoptionsModel) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for group in ["commonOptionsWithBuild", "optionsForCompiler"] {
        let order = &model.declaration_order[group];
        let mut literals: Vec<(String, String)> = Vec::new();
        for opt in &model.compiler_options {
            let Some(decl) = &opt.declaration else { continue };
            if decl.group != group {
                continue;
            }
            let kind = option_kind(&opt.ty);
            // Go: `pathKind: option.type === "[]string" ? undefined : option.pathKind`
            let path_kind = if opt.ty == "[]string" {
                None
            } else {
                opt.path_kind.as_deref()
            };
            literals.push((
                opt.name.clone(),
                declaration_literal(
                    &opt.name,
                    kind,
                    decl.short_name.as_deref(),
                    decl.is_file_path,
                    path_kind,
                    decl.is_tsconfig_only,
                    decl.is_command_line_only,
                    decl.description.as_ref(),
                    decl.default_value_description.as_ref(),
                    decl.show_in_simplified_help_view,
                    decl.category.as_ref(),
                    decl.extra_validation.as_deref(),
                    decl.min_value,
                    decl.list_preserve_falsy_values,
                    None,
                ),
            ));
            for alias in &decl.extra_short_names {
                literals.push((
                    opt.name.clone(),
                    declaration_literal(
                        &opt.name,
                        kind,
                        Some(alias),
                        decl.is_file_path,
                        path_kind,
                        decl.is_tsconfig_only,
                        decl.is_command_line_only,
                        None, // description dropped for aliases
                        decl.default_value_description.as_ref(),
                        None, // showInSimplifiedHelpView dropped for aliases
                        decl.category.as_ref(),
                        decl.extra_validation.as_deref(),
                        decl.min_value,
                        decl.list_preserve_falsy_values,
                        None,
                    ),
                ));
            }
        }
        // orderByName: order by declarationOrder[group], all options covered.
        let mut names: Vec<String> = literals.iter().map(|(n, _)| n.clone()).collect();
        names.sort();
        names.dedup();
        for name in &names {
            assert!(
                order.contains(name),
                "gen-options: option {name} missing from {group} order"
            );
        }
        literals.sort_by_key(|(name, _)| {
            order
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("gen-options: option {name} out of order"))
        });
        out.push((
            group.to_string(),
            literals.into_iter().map(|(_, lit)| lit).collect(),
        ));
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// options_generated.rs emission
// ────────────────────────────────────────────────────────────────────────────

fn emit_parse_compiler_options(out: &mut String, model: &TsoptionsModel) {
    let _ = writeln!(
        out,
        r#"/// Go: `func parseCompilerOptions(key string, value any, allOptions *core.CompilerOptions) (foundKey bool)`.
pub fn parse_compiler_options(
    key: &str,
    value: &CompilerOptionsValue,
    all_options: &mut CompilerOptions,
) -> bool {{
    let key = command_line_compiler_options_map()
        .get(key)
        .map(|option| option.name)
        .unwrap_or(key);
    match key {{"#
    );
    for opt in &model.compiler_options {
        let field = snake_case_field(&opt.name, opt.go_name.as_ref());
        let mut keys = vec![quote(&opt.name)];
        keys.extend(opt.parse_aliases.iter().map(|a| quote(a)));
        let _ = writeln!(out, "        {} => {{", keys.join(" | "));
        let assignment = parser_assignment(opt, &format!("all_options.{field}"));
        for line in assignment.lines() {
            let _ = writeln!(out, "            {line}");
        }
        let _ = writeln!(out, "        }}");
    }
    let _ = writeln!(
        out,
        r#"        _ => return false,
    }}
    true
}}"#
    );
}

fn emit_defaults(out: &mut String, model: &TsoptionsModel) {
    let jsconfig: Vec<&FullOption> = model
        .compiler_options
        .iter()
        .filter(|o| o.jsconfig_default.is_some())
        .collect();
    let _ = writeln!(
        out,
        r#"/// Go: `func getDefaultCompilerOptions(configFileName tspath.RootedFilePath) *parsedCompilerOptions`.
pub fn get_default_compiler_options(config_file_name: &RootedFilePath) -> ParsedCompilerOptions {{
    let mut options = CompilerOptions::default();
    if !config_file_name.is_empty() && config_file_name.base_name() == "jsconfig.json" {{
        options = CompilerOptions {{"#
    );
    for opt in &jsconfig {
        let field = snake_case_field(&opt.name, opt.go_name.as_ref());
        let expr = match opt.jsconfig_default.as_ref().unwrap() {
            GoValue::Bool(true) => "Tristate::TSTrue".to_string(),
            GoValue::Bool(false) => "Tristate::TSFalse".to_string(),
            GoValue::Number(n) => format!("Some({n})"),
            GoValue::Str(s) => quote(s),
            other => panic!("gen-options: bad jsconfig default: {other:?}"),
        };
        let _ = writeln!(out, "            {field}: {expr},");
    }
    let _ = writeln!(
        out,
        r#"            ..CompilerOptions::default()
        }};
    }}
    ParsedCompilerOptions::new(options)
}}

/// Go: `func getDefaultTypeAcquisition(configFileName tspath.RootedFilePath) *core.TypeAcquisition`.
pub fn get_default_type_acquisition(config_file_name: &RootedFilePath) -> TypeAcquisition {{
    if !config_file_name.is_empty() && config_file_name.base_name() == "jsconfig.json" {{
        return TypeAcquisition {{"#
    );
    for opt in &model.type_acquisition {
        if matches!(opt.jsconfig_default, Some(GoValue::Bool(true))) {
            let field = super::snake_case(&opt.field.as_ref().unwrap().name);
            let _ = writeln!(out, "            {field}: Tristate::TSTrue,");
        }
    }
    let _ = writeln!(
        out,
        r#"            ..TypeAcquisition::default()
        }};
    }}
    TypeAcquisition::default()
}}"#
    );
}

fn emit_stored_parsers(out: &mut String, model: &TsoptionsModel) {
    // ParseTypeAcquisition
    let _ = writeln!(
        out,
        r#"/// Go: `func ParseTypeAcquisition(key string, value any, allOptions *core.TypeAcquisition)`.
pub fn parse_type_acquisition(
    key: &str,
    value: &CompilerOptionsValue,
    all_options: &mut TypeAcquisition,
) {{
    if value.is_null() {{
        return;
    }}
    // PORT: Go checks `allOptions == nil` (unreachable for &mut in Rust).
    match key {{"#
    );
    for decl in &model.type_acquisition {
        let field = super::snake_case(&decl.field.as_ref().unwrap().name);
        let _ = writeln!(out, "        {} => {{", quote(&decl.name));
        let stored = FullOption {
            name: decl.name.clone(),
            ty: decl.field.as_ref().unwrap().ty.clone(),
            go_name: Some(decl.field.as_ref().unwrap().name.clone()),
            internal: false,
            deprecated: false,
            path_kind: decl.path_kind.clone(),
            parse_aliases: Vec::new(),
            declaration: None,
            jsconfig_default: None,
            show_config: None,
        };
        let assignment = parser_assignment(&stored, &format!("all_options.{field}"));
        for line in assignment.lines() {
            let _ = writeln!(out, "            {line}");
        }
        let _ = writeln!(out, "        }}");
    }
    let _ = writeln!(out, "        _ => {{}}\n    }}\n}}");

    // ParseBuildOptions
    let _ = writeln!(
        out,
        r#"/// Go: `func ParseBuildOptions(key string, value any, allOptions *core.BuildOptions)`.
pub fn parse_build_options(
    key: &str,
    value: &CompilerOptionsValue,
    all_options: &mut BuildOptions,
) {{
    if value.is_null() {{
        return;
    }}
    // PORT: Go checks `allOptions == nil` (unreachable for &mut in Rust).
    let key = build_name_map().get(key).map(|option| option.name).unwrap_or(key);
    match key {{"#
    );
    for decl in &model.build_options {
        let Some(field_ref) = &decl.field else { continue };
        let field = super::snake_case(&field_ref.name);
        let _ = writeln!(out, "        {} => {{", quote(&decl.name));
        let stored = FullOption {
            name: decl.name.clone(),
            ty: field_ref.ty.clone(),
            go_name: Some(field_ref.name.clone()),
            internal: false,
            deprecated: false,
            path_kind: decl.path_kind.clone(),
            parse_aliases: Vec::new(),
            declaration: None,
            jsconfig_default: None,
            show_config: None,
        };
        let assignment = parser_assignment(&stored, &format!("all_options.{field}"));
        for line in assignment.lines() {
            let _ = writeln!(out, "            {line}");
        }
        let _ = writeln!(out, "        }}");
    }
    let _ = writeln!(out, "        _ => {{}}\n    }}\n}}");
}

fn emit_option_comparisons(out: &mut String, model: &TsoptionsModel) {
    for (name, flag) in [
        ("SemanticDiagnostics", "affects_semantic_diagnostics"),
        ("DeclarationPath", "affects_declaration_path"),
        ("Emit", "affects_emit"),
    ] {
        let mut expressions = Vec::new();
        for opt in &model.compiler_options {
            let Some(decl) = &opt.declaration else { continue };
            let flagged = match flag {
                "affects_semantic_diagnostics" => decl.affects_semantic_diagnostics,
                "affects_declaration_path" => decl.affects_declaration_path,
                _ => decl.affects_emit,
            };
            if !flagged.unwrap_or(false) {
                continue;
            }
            let field = snake_case_field(&opt.name, opt.go_name.as_ref());
            let expr = if decl.strict_flag.unwrap_or(false) {
                format!("old.get_strict_option_value(old.{field}) != new.get_strict_option_value(new.{field})")
            } else {
                format!("old.{field} != new.{field}")
            };
            expressions.push(expr);
        }
        let _ = writeln!(
            out,
            r#"/// Go: `func CompilerOptionsAffect{name}(oldOptions, newOptions *core.CompilerOptions) bool`.
pub fn compiler_options_affect_{snakename}(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
) -> bool {{
    // Go: `if oldOptions == newOptions {{ return false }}` — pointer identity;
    // two Nones are the same "nil" here.
    let same = match (old_options, new_options) {{
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::eq(a, b),
        _ => false,
    }};
    if same {{
        return false;
    }}
    let (Some(old), Some(new)) = (old_options, new_options) else {{
        return true;
    }};
    {exprs}
}}

"#,
            snakename = super::snake_case(name),
            exprs = if expressions.is_empty() {
                "false".to_string()
            } else {
                expressions.join(" ||\n        ")
            }
        );
    }
}

fn emit_build_info_options(out: &mut String, model: &TsoptionsModel) {
    let _ = writeln!(
        out,
        r#"/// Go: `func ForEachCompilerOptionAffectingBuildInfo(options *core.CompilerOptions, fn func(option *CommandLineOption, value any))` —
/// visits nonzero options in CompilerOptions field order.
pub fn for_each_compiler_option_affecting_build_info(
    options: &CompilerOptions,
    f: &mut dyn FnMut(&'static CommandLineOption, CompilerOptionsValue),
) {{"#
    );
    for opt in &model.compiler_options {
        let Some(decl) = &opt.declaration else { continue };
        if !decl.affects_build_info.unwrap_or(false) {
            continue;
        }
        let field = snake_case_field(&opt.name, opt.go_name.as_ref());
        let ty = effective_go_type(opt);
        let cond = not_zero(&format!("options.{field}"), &ty);
        let value = value_variant(&ty, &format!("options.{field}"));
        let _ = writeln!(
            out,
            r#"    if {cond} {{
        f(
            command_line_compiler_options_map().get({name}).unwrap(),
            {value},
        );
    }}"#,
            name = quote(&opt.name)
        );
    }
    let _ = writeln!(out, "}}\n");
}

fn emit_merge_compiler_option_fields(out: &mut String, model: &TsoptionsModel) {
    let _ = writeln!(
        out,
        r#"/// Go: `func mergeCompilerOptionFields(targetOptions, sourceOptions *core.CompilerOptions, explicitNullFields collections.Set[string])`.
///
/// PORT: Go's slice nil-vs-empty distinction is exact via the core fields'
/// `Option<Vec<_>>` (None = Go nil); explicit-null writes the zero value
/// exactly as Go writes nil/""/0.
pub fn merge_compiler_option_fields(
    target_options: &mut CompilerOptions,
    source_options: &CompilerOptions,
    explicit_null_fields: &Set<String>,
) {{"#
    );
    for opt in &model.compiler_options {
        let field = snake_case_field(&opt.name, opt.go_name.as_ref());
        let ty = effective_go_type(opt);
        let zero = zero_expr(&ty);
        let nonzero = not_zero(&format!("source_options.{field}"), &ty);
        let _ = writeln!(
            out,
            r#"    if explicit_null_fields.has({name}) {{
        target_options.{field} = {zero};
    }} else if {nonzero} {{
        target_options.{field} = source_options.{field}{clone};
    }}"#,
            name = quote(&opt.name),
            clone = clone_suffix(&ty)
        );
    }
    let _ = writeln!(out, "}}\n");
}

fn emit_config_dir_substitution(out: &mut String, model: &TsoptionsModel) {
    let substituted: Vec<&FullOption> = model
        .compiler_options
        .iter()
        .filter(|o| {
            o.path_kind.is_none()
                && o.declaration
                    .as_ref()
                    .is_some_and(|d| d.allow_config_dir_template_substitution.unwrap_or(false))
        })
        .collect();
    let _ = writeln!(
        out,
        r#"/// Go: `func handleOptionConfigDirTemplateSubstitution(compilerOptions *parsedCompilerOptions, basePath tspath.RootedDirectoryPath)`.
pub fn handle_option_config_dir_template_substitution(
    compiler_options: &mut ParsedCompilerOptions,
    base_path: &RootedDirectoryPath,
) {{"#
    );
    for opt in &substituted {
        let field = snake_case_field(&opt.name, opt.go_name.as_ref());
        match opt.ty.as_str() {
            "string" => {
                let _ = writeln!(
                    out,
                    r#"    if starts_with_config_dir_template(&compiler_options.options.{field}) {{
        compiler_options.options.{field} =
            get_substituted_path_with_config_dir_template(&compiler_options.options.{field}, base_path);
    }}"#
                );
            }
            "[]string" => {
                let _ = writeln!(
                    out,
                    r#"    if let Some(current) = &compiler_options.options.{field} {{
        if let Some(substitution) =
            get_substituted_string_array_with_config_dir_template(current, base_path)
        {{
            compiler_options.options.{field} = Some(substitution);
        }}
    }}"#
                );
            }
            "*collections.OrderedMap[string, []string]" => {
                let _ = writeln!(
                    out,
                    r#"    if let Some(paths) = &compiler_options.options.{field} {{
        let mut substituted: Option<OrderedMap<String, Vec<String>>> = None;
        for (k, v) in paths.entries() {{
            if let Some(substitution) = get_substituted_string_array_with_config_dir_template(v, base_path) {{
                let map = substituted.get_or_insert_with(|| paths.clone());
                map.set(k.clone(), substitution);
            }}
        }}
        if let Some(substituted) = substituted {{
            compiler_options.options.{field} = Some(substituted);
        }}
    }}"#
                );
            }
            other => panic!("gen-options: unsupported configDir substitution type {}: {other}", opt.name),
        }
    }
    let _ = writeln!(
        out,
        r#"    // PORT: Go ranges over `unresolvedPaths` while ParseCompilerOptions mutates the
    // options (not the map); the port takes the map out first — same values,
    // same order (OrderedMap insertion order), and the final `clear` is the take.
    let unresolved_paths = std::mem::take(&mut compiler_options.unresolved_paths);
    for (key, value) in unresolved_paths.entries().map(|(k, v)| (k.clone(), v.clone())) {{
        let option = command_line_compiler_options_map().get(&key).unwrap();
        let value = if option.kind == CommandLineOptionKind::List {{
            let paths = parse_string_array(&value);
            CompilerOptionsValue::Array(
                paths.into_iter()
                    .map(|path| {{
                        CompilerOptionsValue::Str(get_substituted_path_with_config_dir_template(&path, base_path))
                    }})
                    .collect(),
            )
        }} else {{
            CompilerOptionsValue::Str(get_substituted_path_with_config_dir_template(&parse_string(&value), base_path))
        }};
        parse_compiler_options(&key, &value, &mut compiler_options.options);
    }}
}}"#
    );
}

fn emit_show_config(out: &mut String, model: &TsoptionsModel) {
    // serializedOptions filter (generate-options.ts showConfig()).
    let serialized: Vec<&FullOption> = model
        .compiler_options
        .iter()
        .filter(|o| {
            o.show_config != Some(false)
                && o.declaration.is_some()
                && o.declaration.as_ref().unwrap().category.as_ref().map(|c| c.go.as_str()) != Some("diagnostics.Command_line_Options")
                && o.declaration.as_ref().unwrap().category.as_ref().map(|c| c.go.as_str()) != Some("diagnostics.Output_Formatting")
        })
        .collect();
    let enum_maps: HashMap<String, EnumMapDef> =
        ordered_entries::<EnumMapDef>(&model.enum_maps_raw)
            .into_iter()
            .collect();
    let elements: Vec<(String, RawDeclaration)> =
        ordered_entries::<RawDeclaration>(&model.elements_raw);

    let _ = writeln!(
        out,
        r#"/// Go: `func serializeCompilerOptions(options *core.CompilerOptions, configFilePath tspath.RootedFilePath, caseSensitivity tspath.CaseSensitivity) *collections.OrderedMap[string, any]`.
pub fn serialize_compiler_options(
    options: &CompilerOptions,
    config_file_path: &RootedFilePath,
    case_sensitivity: &CaseSensitivity,
) -> OrderedMap<String, CompilerOptionsValue> {{
    let mut result = OrderedMap::with_size_hint(32);"#
    );
    for opt in &serialized {
        let field = snake_case_field(&opt.name, opt.go_name.as_ref());
        // OrderedMap::set takes owned keys; the JSON name literal becomes a String.
        let name = format!("{}.to_string()", quote(&opt.name));
        let _ = &name;
        let decl = opt.declaration.as_ref().unwrap();
        let kind = option_kind(&opt.ty);
        let element = elements
            .iter()
            .find(|(k, _)| k == &opt.name)
            .map(|(_, v)| v);
        match kind {
            "Enum" => {
                let _ = writeln!(
                    out,
                    r#"    if {} {{
        let value = serialize_compiler_option_enum(&{});
        if !value.is_empty() {{
            result.set({name}, CompilerOptionsValue::Str(value));
        }}
    }}"#,
                    not_zero(&format!("options.{field}"), &effective_go_type(opt)),
                    value_variant(&effective_go_type(opt), &format!("options.{field}"))
                );
            }
            "Boolean" => {
                let _ = writeln!(
                    out,
                    r#"    if options.{field} == Tristate::TSTrue || options.{field} == Tristate::TSFalse {{
        result.set({name}, CompilerOptionsValue::Bool(options.{field} == Tristate::TSTrue));
    }}"#
                );
            }
            "String" => match opt.path_kind.as_deref() {
                Some("file") | Some("directory") => {
                    let _ = writeln!(
                        out,
                        r#"    if !options.{field}.is_empty() {{
        result.set(
            {name},
            CompilerOptionsValue::Str(serialize_compiler_option_path(
                &options.{field}.as_path(),
                config_file_path,
                case_sensitivity,
            )),
        );
    }}"#
                    );
                }
                Some("fileOrDirectory") => {
                    let _ = writeln!(
                        out,
                        r#"    if !options.{field}.is_empty() {{
        result.set(
            {name},
            CompilerOptionsValue::Str(serialize_compiler_option_path(
                &options.{field},
                config_file_path,
                case_sensitivity,
            )),
        );
    }}"#
                    );
                }
                Some("sourceMapLocation") => {
                    // Go: `result.Set("mapRoot", options.MapRoot)` — the raw
                    // location string, not a serialized path.
                    let _ = writeln!(
                        out,
                        r#"    if !options.{field}.is_empty() {{
        result.set({name}, CompilerOptionsValue::Str(options.{field}.as_string().to_owned()));
    }}"#
                    );
                }
                _ => {
                    let _ = writeln!(
                        out,
                        r#"    if !options.{field}.is_empty() {{
        result.set({name}, CompilerOptionsValue::Str(options.{field}.clone()));
    }}"#
                    );
                }
            },
            "List" => {
                if opt.path_kind.as_deref() == Some("directory") {
                    assert_eq!(opt.ty, "[]string", "gen-options: unsupported path list {}", opt.name);
                    let _ = writeln!(
                        out,
                        r#"    if options.{field}.is_some() {{
        result.set(
            {name},
            CompilerOptionsValue::StrArray(
                options.{field}.iter().flatten().map(|value| {{
                    if value.is_empty() {{
                        return String::new();
                    }}
                    serialize_compiler_option_path(&value.as_path(), config_file_path, case_sensitivity)
                }}).collect(),
            ),
        );
    }}"#
                    );
                } else if element.is_some_and(|e| e.kind == "Enum") {
                    assert_eq!(opt.ty, "[]string", "gen-options: unsupported enum list {}", opt.name);
                    let enum_map = enum_maps
                        .get(&opt.name)
                        .unwrap_or_else(|| panic!("gen-options: missing enum map {}", opt.name));
                    let _ = writeln!(
                        out,
                        r#"    if options.{field}.is_some() {{
        result.set(
            {name},
            CompilerOptionsValue::StrArray(serialize_compiler_option_enum_list(
                options.{field}.as_deref().unwrap(),
                crate::declarations_generated::{map_fn}(),
            )),
        );
    }}"#,
                        map_fn = super::snake_case(&enum_map.go_name)
                    );
                } else if opt.ty == "[]PluginImport" {
                    // Go: `result.Set("plugins", options.Plugins)` — the typed
                    // slice; the value model carries it as PluginImports.
                    let _ = writeln!(
                        out,
                        r#"    if options.{field}.is_some() {{
        result.set({name}, CompilerOptionsValue::PluginImports(options.{field}.clone().unwrap()));
    }}"#
                    );
                } else {
                    // Go: `if options.X != nil { result.Set("x", options.X) }` —
                    // None (Go nil) omits the key; Some(empty) emits `[]`.
                    let _ = writeln!(
                        out,
                        r#"    if options.{field}.is_some() {{
        result.set({name}, CompilerOptionsValue::StrArray(options.{field}.clone().unwrap()));
    }}"#
                    );
                }
            }
            "Number" => {
                // *int — the default (non-path) branch: raw value.
                let _ = writeln!(
                    out,
                    r#"    if let Some(value) = options.{field} {{
        result.set({name}, CompilerOptionsValue::Int(value));
    }}"#
                );
            }
            "Object" => {
                // paths — Go: `if options.Paths != nil {{ result.Set("paths", options.Paths) }}`
                let _ = writeln!(
                    out,
                    r#"    if options.{field}.is_some() {{
        result.set({name}, CompilerOptionsValue::PathsMap(options.{field}.clone().unwrap()));
    }}"#
                );
            }
            other => panic!("gen-options: unsupported showConfig kind {other} for {}", opt.name),
        }
        let _ = &decl;
    }
    let _ = writeln!(out, "    result\n}}\n");

    // serializeCompilerOptionEnum
    let enum_options: Vec<&FullOption> = serialized
        .iter()
        .copied()
        .filter(|o| option_kind(&o.ty) == "Enum")
        .collect();
    let _ = writeln!(
        out,
        r#"/// Go: `func serializeCompilerOptionEnum(value any) string`.
pub fn serialize_compiler_option_enum(value: &CompilerOptionsValue) -> String {{
    match value {{"#
    );
    for opt in &enum_options {
        let enum_name = enum_name_of_type(&opt.ty);
        let enum_map = enum_maps
            .get(&opt.name)
            .unwrap_or_else(|| panic!("gen-options: missing enum map {}", opt.name));
        let _ = writeln!(
            out,
            "        CompilerOptionsValue::{enum_name}(value) => {{"
        );
        let mut seen: Vec<String> = Vec::new();
        for entry in &enum_map.values {
            let GoValue::GoRef { go } = &entry.value else {
                panic!("gen-options: enum map {} entry {} not a core ref", opt.name, entry.name);
            };
            let member = go
                .strip_prefix("core.")
                .and_then(|s| s.strip_prefix(enum_name))
                .unwrap_or_else(|| panic!("gen-options: bad core ref {go}"));
            if seen.contains(&member.to_string()) {
                continue;
            }
            seen.push(member.to_string());
            let _ = writeln!(
                out,
                "            if *value == {enum_name}::{member} {{ return {}.to_string(); }}",
                quote(&entry.name)
            );
        }
        let _ = writeln!(out, "        }}");
    }
    let _ = writeln!(out, "        _ => {{}}\n    }}\n    String::new()\n}}");
}

pub fn emit_options_generated(model: &TsoptionsModel) -> String {
    let mut out = String::with_capacity(120 * 1024);
    out.push_str(r#"// Code generated by rust/tools/gen-options from rust/tools/gen-options/data/tsoptions-model.json. DO NOT EDIT.
// Rust mirror of tsc/internal/tsoptions/options_generated.go @ GO_COMMIT
// (upstream: tools/scripts/tsc/generate-options.ts emits that file from
// tools/scripts/tsc/options.ts — the same model data extracted into
// tsoptions-model.json; per SPEC §6 the Go output is never a generation
// input, only the parity baseline for tests).
//
// PORT notes (vs options_generated.go):
//   - Go's `any` option values become `CompilerOptionsValue` (commandlineoption.rs),
//     which models exactly the dynamic types that flow through the option maps.
//   - Go's nil-vs-empty slice distinction is exact via the core fields'
//     `Option<Vec<_>>` (None = Go nil; `Some(parse_string_array(value))`
//     keeps an explicit `[]` distinct from unset, as Go's JSON decode does).
//   - `floatOrInt32ToFlag[T]` becomes the per-enum `as_<T>_flag()` value accessors.
//   - Typed path values passed to ForEachCompilerOptionAffectingBuildInfo are
//     carried as their JSON (string) form — identical once marshaled.
//   - getDefaultCompilerOptions/... return ParsedCompilerOptions (tsconfigparsing.rs),
//     the Go return shape.

use tsc_collections::{OrderedMap, Set};
use tsc_core::options_generated::{
    BuildOptions, CompilerOptions, JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind,
    NewLineKind, PluginImport, ScriptTarget, TypeAcquisition,
};
use tsc_core::tristate::Tristate;
use tsc_tspath::{
    CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath, SourceMapLocation,
};

use crate::commandlineoption::{CommandLineOption, CommandLineOptionKind, CompilerOptionsValue};
use crate::namemap::{build_name_map, command_line_compiler_options_map};
use crate::parsinghelpers::{
    get_substituted_path_with_config_dir_template, get_substituted_string_array_with_config_dir_template,
    parse_directory_name, parse_directory_names, parse_file_name, parse_file_or_directory_name,
    parse_number, parse_string, parse_string_array, parse_string_map, parse_tristate,
    to_source_map_location,
};
use crate::showconfig::{serialize_compiler_option_enum_list, serialize_compiler_option_path};
use crate::tsconfigparsing::ParsedCompilerOptions;

"#);
    let _ = writeln!(out);
    emit_parse_compiler_options(&mut out, model);
    out.push('\n');
    emit_defaults(&mut out, model);
    out.push('\n');
    emit_stored_parsers(&mut out, model);
    emit_option_comparisons(&mut out, model);
    emit_build_info_options(&mut out, model);
    emit_merge_compiler_option_fields(&mut out, model);
    emit_config_dir_substitution(&mut out, model);
    emit_show_config(&mut out, model);
    out
}

// ────────────────────────────────────────────────────────────────────────────
// declarations_generated.rs emission
// ────────────────────────────────────────────────────────────────────────────

fn upper_static_name(go_var: &str) -> String {
    go_var
        .chars()
        .map(|c| if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c })
        .collect()
}

fn element_static_name(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == '.' {
            out.push('_');
            prev_lower = false;
        } else if ch.is_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(ch);
            prev_lower = false;
        } else {
            out.push(ch.to_ascii_uppercase());
            prev_lower = ch.is_lowercase();
        }
    }
    format!("{out}_ELEMENT")
}

pub fn emit_declarations_generated(model: &TsoptionsModel) -> String {
    let mut out = String::with_capacity(160 * 1024);
    out.push_str(r#"// Code generated by rust/tools/gen-options from rust/tools/gen-options/data/tsoptions-model.json. DO NOT EDIT.
// Rust mirror of tsc/internal/tsoptions/declarations_generated.go @ GO_COMMIT
// (upstream: tools/scripts/tsc/generate-options.ts emits that file from
// tools/scripts/tsc/options.ts; per SPEC §6 the Go output is never a
// generation input, only the parity baseline for tests).
//
// PORT notes (vs declarations_generated.go):
//   - Go's `var X = []*CommandLineOption{...}` slices become `static`
//     slices of `&CommandLineOption` (all fields are const-constructible).
//     Rust literals are FULLY explicit (const literals cannot omit fields,
//     and there is no field-update syntax in const position) — fields Go
//     leaves unset carry their zero value here.
//   - Go's map literals (commandLineOptionElements/EnumMap/Deprecated/
//     targetToLibMap) become `match`-based lookup functions over statics —
//     same entries, same membership semantics; Go map iteration order is
//     random so no order parity is lost. The enum option maps
//     (LibMap/moduleOptionMap/...) are OnceLock-built OrderedMaps preserving
//     the Go entry order (which options.ts arrays fix).
//   - `ElementOptions` fields reference lazily-built name maps via
//     `ElementOptionsKind`; the accessor lives in namemap.rs (Go builds the
//     maps at package init).

use std::sync::OnceLock;

use tsc_collections::{MapEntry, OrderedMap};
use tsc_core::options_generated::{
    JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind, NewLineKind, ScriptTarget,
};
use tsc_core::tristate::Tristate;

use crate::commandlineoption::{
    CommandLineOption, CommandLineOptionKind, CommandLineOptionPathKind, CompilerOptionsValue,
    DefaultValueDescription, ElementOptionsKind, ExtraValidation,
};

"#);
    let _ = writeln!(out);

    // declaration groups
    let groups = compiler_declarations(model);
    let group_static: HashMap<&str, &str> = [
        ("commonOptionsWithBuild", "COMMON_OPTIONS_WITH_BUILD"),
        ("optionsForCompiler", "OPTIONS_FOR_COMPILER"),
    ]
    .into_iter()
    .collect();
    for (group, literals) in &groups {
        let static_name = group_static[group.as_str()];
        let _ = writeln!(
            out,
            "/// Go: `var {group} = []*CommandLineOption{{...}}`.\npub static {static_name}: &[&CommandLineOption] = &["
        );
        for lit in literals {
            let _ = write!(out, "{}", slice_element(lit));
        }
        let _ = writeln!(out, "];\n");
    }

    // typeAcquisitionDecls
    let _ = writeln!(
        out,
        "/// Go: `var typeAcquisitionDecls = []*CommandLineOption{{...}}`.\npub static TYPE_ACQUISITION_DECLS: &[&CommandLineOption] = &["
    );
    for decl in &model.type_acquisition {
        let _ = write!(out, "{}", slice_element(&raw_declaration_literal(decl, None)));
    }
    let _ = writeln!(out, "];\n");

    // OptionsDeclarations / BuildOpts (Go slices.Concat)
    let _ = writeln!(
        out,
        r#"/// Go: `var OptionsDeclarations = slices.Concat(commonOptionsWithBuild, optionsForCompiler)`.
pub fn options_declarations() -> Vec<&'static CommandLineOption> {{
    [COMMON_OPTIONS_WITH_BUILD, OPTIONS_FOR_COMPILER].concat()
}}

/// Go: `var BuildOpts = slices.Concat(commonOptionsWithBuild, OptionsForBuild)`.
pub fn build_opts() -> Vec<&'static CommandLineOption> {{
    [COMMON_OPTIONS_WITH_BUILD, OPTIONS_FOR_BUILD].concat()
}}

"#
    );

    // commandLineOptionElements: element statics + lookup fn
    let elements: Vec<(String, RawDeclaration)> =
        ordered_entries::<RawDeclaration>(&model.elements_raw);
    for (name, decl) in &elements {
        let static_name = element_static_name(name);
        let _ = writeln!(
            out,
            "/// Go: `commandLineOptionElements[{key}]`.\nstatic {static_name}: CommandLineOption = {};",
            static_body(&raw_declaration_literal(decl, None)),
            key = quote(name),
        );
    }
    let _ = writeln!(
        out,
        "/// Go: `var commandLineOptionElements = map[string]*CommandLineOption{{...}}` —\n/// the element option of a list/listOrElement option, keyed by option name.\npub fn command_line_option_elements(name: &str) -> Option<&'static CommandLineOption> {{\n    match name {{"
    );
    for (name, _) in &elements {
        let _ = writeln!(
            out,
            "        {} => Some(&{}),",
            quote(name),
            element_static_name(name)
        );
    }
    let _ = writeln!(out, "        _ => None,\n    }}\n}}\n");

    // TscBuildOption + OptionsForBuild
    let build = &model.build_options[0];
    let _ = writeln!(
        out,
        "/// Go: `var TscBuildOption = CommandLineOption{{...}}`.\npub static TSC_BUILD_OPTION: CommandLineOption = {};",
        static_body(&raw_declaration_literal(build, None))
    );
    let _ = writeln!(
        out,
        "/// Go: `var OptionsForBuild = []*CommandLineOption{{&TscBuildOption, ...}}`.\npub static OPTIONS_FOR_BUILD: &[&CommandLineOption] = &["
    );
    let _ = writeln!(out, "    &TSC_BUILD_OPTION,");
    for decl in &model.build_options[1..] {
        let _ = write!(out, "{}", slice_element(&raw_declaration_literal(decl, None)));
    }
    let _ = writeln!(out, "];\n");

    // root declarations (compilerOptionsDeclaration, typeAcquisitionDeclaration,
    // extendsOptionDeclaration, compileOnSaveCommandLineOption)
    for decl in &model.root_options {
        let Some(variable) = &decl.variable else { continue };
        let element_options = if decl.name == "extends" {
            Some("extends")
        } else {
            decl.element_options.as_deref()
        };
        let _ = writeln!(
            out,
            "/// Go: `var {variable} = &CommandLineOption{{...}}`.\npub static {}: CommandLineOption = {};",
            upper_static_name(variable),
            static_body(&raw_declaration_literal(decl, element_options))
        );
    }
    // tsconfigRootOptionsMap
    let _ = writeln!(
        out,
        "/// Go: `var tsconfigRootOptionsMap = &CommandLineOption{{ Name: \"undefined\", ... }}` —\n/// the element options list is built lazily from the root declarations in namemap.rs.\npub static TSCONFIG_ROOT_OPTIONS_MAP: CommandLineOption = CommandLineOption {{\n    name: \"undefined\",\n    kind: CommandLineOptionKind::Object,\n    short_name: \"\",\n    path_kind: CommandLineOptionPathKind::None,\n    is_file_path: false,\n    is_tsconfig_only: false,\n    is_command_line_only: false,\n    description: None,\n    default_value_description: None,\n    show_in_simplified_help_view: false,\n    category: None,\n    extra_validation: ExtraValidation::None,\n    min_value: 0,\n    list_preserve_falsy_values: false,\n    element_options: ElementOptionsKind::TsconfigRoot,\n}};\n"
    );

    // enum option maps (LibMap etc.) — OnceLock-built OrderedMaps
    let enum_maps: Vec<(String, EnumMapDef)> = ordered_entries::<EnumMapDef>(&model.enum_maps_raw);
    for (name, map) in &enum_maps {
        let fn_name = super::snake_case(&map.go_name);
        let _ = writeln!(
            out,
            "/// Go: `var {go} = collections.NewOrderedMapFromList([]collections.MapEntry[string, any]{{...}})`.\npub fn {fn_name}() -> &'static OrderedMap<String, CompilerOptionsValue> {{",
            go = map.go_name
        );
        let _ = writeln!(
            out,
            "    static MAP: OnceLock<OrderedMap<String, CompilerOptionsValue>> = OnceLock::new();"
        );
        let _ = writeln!(
            out,
            "    MAP.get_or_init(|| OrderedMap::from_list(["
        );
        for entry in &map.values {
            let value_expr = match &entry.value {
                GoValue::Str(s) => format!("CompilerOptionsValue::Str({}.to_owned())", quote(s)),
                GoValue::GoRef { go } => {
                    let (enum_name, member) = split_core_ref(go);
                    format!("CompilerOptionsValue::{enum_name}({enum_name}::{member})")
                }
                other => panic!(
                    "gen-options: enum map {name} entry {} has non-core/str value {other:?}",
                    entry.name
                ),
            };
            let _ = writeln!(
                out,
                "        MapEntry {{ key: {}.to_string(), value: {value_expr} }},",
                quote(&entry.name)
            );
        }
        let _ = writeln!(out, "    ]))\n}}\n");
        let _ = name;
    }

    // commandLineOptionEnumMap / commandLineOptionDeprecated / targetToLibMap
    let _ = writeln!(
        out,
        "/// Go: `var commandLineOptionEnumMap = map[string]*collections.OrderedMap[string, any]{{...}}`.\npub fn command_line_option_enum_map(name: &str) -> Option<&'static OrderedMap<String, CompilerOptionsValue>> {{\n    match name {{"
    );
    for (name, map) in &enum_maps {
        let _ = writeln!(
            out,
            "        {} => Some({}()),",
            quote(name),
            super::snake_case(&map.go_name)
        );
    }
    let _ = writeln!(out, "        _ => None,\n    }}\n}}\n");

    let _ = writeln!(
        out,
        "/// Go: `var commandLineOptionDeprecated = map[string]*collections.Set[string]{{...}}`.\n///\n/// PORT: Go returns a *Set; the port returns a static slice with identical\n/// membership semantics (formatEnumTypeKeys filters by `DeprecatedKeys().Has`).\npub fn command_line_option_deprecated(name: &str) -> Option<&'static [&'static str]> {{\n    match name {{"
    );
    for (name, map) in &enum_maps {
        if map.deprecated_keys.is_empty() {
            continue;
        }
        let keys = map
            .deprecated_keys
            .iter()
            .map(|k| quote(k))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "        {} => Some(&[{keys}]),", quote(name));
    }
    let _ = writeln!(out, "        _ => None,\n    }}\n}}\n");

    let _ = writeln!(
        out,
        "/// Go: `var targetToLibMap = map[core.ScriptTarget]string{{...}}`.\npub fn target_to_lib(target: ScriptTarget) -> Option<&'static str> {{\n    match target {{"
    );
    let script_target = model
        .enums
        .iter()
        .find(|e| e.name == "ScriptTarget")
        .expect("ScriptTarget enum");
    for member in script_target.members.iter().rev() {
        let Some(lib) = &member.lib else { continue };
        let comment = member
            .lib_comment
            .as_ref()
            .map(|c| format!(" // {c}"))
            .unwrap_or_default();
        let _ = writeln!(out, "        ScriptTarget::{} => Some({}),{comment}", member.name, quote(lib));
    }
    let _ = writeln!(out, "        _ => None,\n    }}\n}}");
    out
}

fn split_core_ref(go: &str) -> (String, String) {
    let stripped = go
        .strip_prefix("core.")
        .unwrap_or_else(|| panic!("gen-options: expected core.* ref, got {go}"));
    for enum_name in [
        "ModuleDetectionKind",
        "ModuleResolutionKind",
        "ModuleKind",
        "NewLineKind",
        "ScriptTarget",
        "JsxEmit",
    ] {
        if let Some(member) = stripped.strip_prefix(enum_name) {
            return (enum_name.to_string(), member.to_string());
        }
    }
    panic!("gen-options: unknown core enum ref {go}");
}

// ────────────────────────────────────────────────────────────────────────────
// contentmapper compiler_option_fields emission
// ────────────────────────────────────────────────────────────────────────────

pub fn emit_contentmapper_fields(model: &TsoptionsModel) -> String {
    let mut out = String::with_capacity(48 * 1024);
    let _ = write!(
        out,
        r#"// Code generated by rust/tools/gen-options from rust/tools/gen-options/data/tsoptions-model.json. DO NOT EDIT.
// Rust mirror of tsc/internal/contentmapper/contentmapper.go's reflection-driven
// `compilerOptionFields` map (json tag -> struct field index) + the
// `MarshalDeclaredOptions` field marshal, as an exhaustive match over the
// CompilerOptions json tags (same tags tools/scripts/tsc/generate-options.ts
// writes into core/options_generated.go).
//
// PORT notes (vs contentmapper.go):
//   - Go skips a field when `reflect.Value.IsZero()` is true; the port mirrors
//     per type (Tristate unknown / "" / nil / 0). Slice fields are
//     `Option<Vec<_>>`, matching Go's IsZero on slices (nil ⇒ skip, non-nil
//     empty ⇒ marshal `[]`, exactly as Go's reflect does).
//   - Go marshals typed paths (RootedDirectoryPath etc.) via their json
//     MarshalJSON (a string); the port marshals `as_string()`.
//   - Go marshals enum fields as their raw int32 values; same here.

use tsc_core::options_generated::CompilerOptions;
use tsc_json::Value;

use crate::field_json;

"#
    );
    let _ = writeln!(
        out,
        "/// Go: `compilerOptionFields` + `MarshalDeclaredOptions`'s per-field marshal —\n/// returns the json form of the named option when set, None when unset/zero.\npub fn marshal_compiler_option_field(options: &CompilerOptions, name: &str) -> Option<Value> {{\n    match name {{"
    );
    for opt in &model.compiler_options {
        let field = snake_case_field(&opt.name, opt.go_name.as_ref());
        let call = if opt.path_kind.is_some() && opt.ty.starts_with("[]") {
            // `[]string` + pathKind (rootDirs, typeRoots) maps to
            // Vec<RootedDirectoryPath>.
            format!("field_json::rooted_directory_array(options.{field}.as_deref())")
        } else if opt.path_kind.is_some() {
            // Path-typed options (model `type: "string"` + `pathKind`) map to
            // Rooted*/SourceMapLocation fields, all `Deref<Target = str>`;
            // Go marshals the underlying string, treating empty as unset.
            format!("field_json::rooted_string(&options.{field})")
        } else {
            match opt.ty.as_str() {
                "Tristate" => format!("field_json::tristate(options.{field})"),
                "string" => format!("field_json::string_value(&options.{field})"),
                "*int" => format!("field_json::int_value(options.{field})"),
                "[]string" => format!("field_json::string_array(options.{field}.as_deref())"),
                "[]PluginImport" => {
                    format!("field_json::plugin_imports(options.{field}.as_deref())")
                }
                "*collections.OrderedMap[string, []string]" => {
                    format!("field_json::paths_map(options.{field}.as_ref())")
                }
                "JsxEmit" | "ModuleDetectionKind" | "ModuleKind" | "ModuleResolutionKind"
                | "NewLineKind" | "ScriptTarget" => {
                    let enum_name = enum_name_of_type(&opt.ty);
                    format!(
                        "field_json::{}(options.{field})",
                        super::snake_case(enum_name)
                    )
                }
                other => panic!("gen-options: unknown field type {other}"),
            }
        };
        let _ = writeln!(out, "        {} => {call},", quote(&opt.name));
    }
    let _ = writeln!(out, "        _ => None,\n    }}\n}}");
    out
}
