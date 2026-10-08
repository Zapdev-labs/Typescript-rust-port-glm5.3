// gen-options — core compiler-options code generator (SPEC §5.8, §6).
//
// Reads rust/tools/gen-options/data/options-model.json — a hand-mirrored
// extraction of the subset of tools/scripts/tsc/options.ts that
// tools/scripts/tsc/generate-options.ts emits into
// tsc/internal/core/options_generated.go — and emits
// rust/crates/core/src/options_generated.rs: the PluginImport/CompilerOptions/
// TypeAcquisition/BuildOptions structs (with Clone + the generated Equals),
// the 6 numeric enums with exact values and Go-parity Display stringers, the
// ModuleKindToModuleResolutionKind lookup, and generated parity tests that
// assert against the Go output.
//
// Per SPEC §6, Go output (options_generated.go) is NEVER a generation input;
// it is used only as the parity baseline for the tests. The JSON data file is
// the single input.
//
// Deterministic and idempotent; `--check` verifies the checked-in output is
// current (exit 1 with a summary if stale). Output goes through rustfmt (the
// Go generator runs `format.Source`; same role), so the emitted file is
// `cargo fmt`-stable and byte-identical on regeneration.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

// ────────────────────────────────────────────────────────────────────────────
// JSON model (mirrors the core-relevant subset of tools/scripts/tsc/
// options-model.ts; see data/README.md for provenance and re-verification)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(untagged)]
enum EnumValue {
    Number(i64),
    Alias(String),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Stringer {
    /// Go: `stringer: "name"` — display text is the member name.
    ByName,
    /// Pre-resolved from an options.ts `enumMaps` entry: member name → display.
    Map(HashMap<String, String>),
}

#[derive(Deserialize)]
struct EnumMember {
    name: String,
    value: EnumValue,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    trailing_comment: Option<String>,
    /// Only on ModuleKind members: the ModuleResolutionKind they imply
    /// (Go: `ModuleKindToModuleResolutionKind` map entries).
    #[serde(default)]
    module_resolution: Option<String>,
}

#[derive(Deserialize)]
struct EnumDef {
    name: String,
    #[serde(default)]
    stringer: Option<Stringer>,
    members: Vec<EnumMember>,
}

/// A stored struct field. `name` is the tsconfig/CLI name; `goName` overrides
/// options.ts `fieldName()` capitalization when present.
#[derive(Deserialize)]
struct Field {
    name: String,
    #[serde(default)]
    go_name: Option<String>,
    /// Go field type string, after generate-options.ts `goType()` mapping.
    #[serde(rename = "type")]
    ty: Option<String>,
    #[serde(default)]
    section: Option<String>,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    internal: bool,
    #[serde(default)]
    comment: Option<String>,
}

#[derive(Deserialize)]
struct Model {
    enums: Vec<EnumDef>,
    #[serde(rename = "pluginImportFields")]
    plugin_import_fields: Vec<Field>,
    #[serde(rename = "compilerOptions")]
    compiler_options: Vec<Field>,
    #[serde(rename = "typeAcquisition")]
    type_acquisition: Vec<Field>,
    #[serde(rename = "buildOptions")]
    build_options: Vec<Field>,
    #[serde(rename = "buildOptionFieldOrder")]
    build_option_field_order: Vec<String>,
}

// ────────────────────────────────────────────────────────────────────────────
// Validation (mirrors generate-options.ts `validateOptions` for this subset)
// ────────────────────────────────────────────────────────────────────────────

const KNOWN_GO_TYPES: &[&str] = &[
    "Tristate",
    "string",
    "*int",
    "[]string",
    "[]PluginImport",
    "*collections.OrderedMap[string, []string]",
    "tspath.RootedFilePath",
    "tspath.RootedDirectoryPath",
    "[]tspath.RootedDirectoryPath",
    "tspath.RootedPath",
    "tspath.SourceMapLocation",
];

fn fail(msg: String) -> ! {
    panic!("gen-options: invalid model: {msg}");
}

/// Go generate-options.ts `fieldName`: goName, or name with capitalized head.
fn field_name(field: &Field) -> String {
    match &field.go_name {
        Some(name) => name.clone(),
        None => {
            let mut chars = field.name.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

fn snake_case(go_name: &str) -> String {
    let chars: Vec<char> = go_name.chars().collect();
    let mut out = String::with_capacity(go_name.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_lowercase();
            let boundary_run = i > 0
                && chars[i - 1].is_uppercase()
                && chars.get(i + 1).is_some_and(|next| next.is_lowercase());
            if i > 0 && (prev_lower || boundary_run) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn validate(model: &Model) {
    let enum_names: HashSet<&str> = model.enums.iter().map(|e| e.name.as_str()).collect();
    if enum_names.len() != model.enums.len() {
        fail("duplicate enum names".into());
    }

    for enum_def in &model.enums {
        let mut names = HashSet::new();
        let mut values = HashSet::new();
        let mut last: Option<i64> = None;
        for member in &enum_def.members {
            if !names.insert(member.name.as_str()) {
                fail(format!("duplicate {} member {}", enum_def.name, member.name));
            }
            match &member.value {
                EnumValue::Number(v) => {
                    if !values.insert(*v) {
                        fail(format!(
                            "duplicate {} value {v} ({}); Rust enums need unique discriminants — value aliases must use string values",
                            enum_def.name, member.name
                        ));
                    }
                    // Derived PartialOrd/Ord must match the numeric order.
                    if last.is_some_and(|prev| *v < prev) {
                        fail(format!(
                            "{} member {} value {v} breaks monotonic declaration order (required: derived Ord == numeric order)",
                            enum_def.name, member.name
                        ));
                    }
                    last = Some(*v);
                }
                EnumValue::Alias(target) => {
                    if !enum_def.members.iter().any(|m| {
                        m.name == *target && matches!(m.value, EnumValue::Number(_))
                    }) {
                        fail(format!(
                            "{} alias {} references non-declared member {target:?}",
                            enum_def.name, member.name
                        ));
                    }
                }
            }
        }
        // Stringers must cover every non-zero member (Go's enumStrings asserts
        // display text for each).
        if let Some(stringer) = &enum_def.stringer {
            match stringer {
                Stringer::ByName => {}
                Stringer::Map(map) => {
                    for member in &enum_def.members {
                        if matches!(member.value, EnumValue::Number(n) if n != 0)
                            && !map.contains_key(&member.name)
                        {
                            fail(format!(
                                "missing stringer display text for {}::{}",
                                enum_def.name, member.name
                            ));
                        }
                    }
                }
            }
        }
        if enum_def.name == "ModuleKind" {
            let module_resolution_kind = model
                .enums
                .iter()
                .find(|e| e.name == "ModuleResolutionKind")
                .unwrap_or_else(|| fail("ModuleKind requires ModuleResolutionKind".into()));
            for member in &enum_def.members {
                if let Some(target) = &member.module_resolution {
                    if !module_resolution_kind
                        .members
                        .iter()
                        .any(|m| m.name == *target)
                    {
                        fail(format!(
                            "ModuleKind::{} moduleResolution {target:?} is not a ModuleResolutionKind member",
                            member.name
                        ));
                    }
                }
            }
        }
    }

    let mut field_names = HashSet::new();
    for field in model
        .plugin_import_fields
        .iter()
        .chain(&model.compiler_options)
        .chain(&model.type_acquisition)
    {
        if !field_names.insert(field_name(field)) {
            fail(format!("duplicate field name {}", field_name(field)));
        }
        let Some(ty) = &field.ty else {
            fail(format!("field {} has no type", field_name(field)));
        };
        if !KNOWN_GO_TYPES.contains(&ty.as_str()) && !enum_names.contains(ty.as_str()) {
            fail(format!("unknown field type {ty:?} for {}", field_name(field)));
        }
    }

    // BuildOptions: only rows with stored fields; order must be total.
    let stored: Vec<&Field> = model
        .build_options
        .iter()
        .filter(|f| f.go_name.is_some())
        .collect();
    let order: HashSet<&str> = model
        .build_option_field_order
        .iter()
        .map(|s| s.as_str())
        .collect();
    if order.len() != model.build_option_field_order.len() {
        fail("buildOptionFieldOrder has duplicates".into());
    }
    for field in &stored {
        if !order.contains(field.name.as_str()) {
            fail(format!(
                "stored BuildOptions field {} missing from buildOptionFieldOrder",
                field.name
            ));
        }
    }
    for name in &model.build_option_field_order {
        if !stored.iter().any(|f| f.name == *name) {
            fail(format!("buildOptionFieldOrder entry {name:?} has no stored field"));
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Rust emission
// ────────────────────────────────────────────────────────────────────────────

/// Go field type → Rust field type. Enum names pass through unchanged
/// (validated to be model enums).
fn rust_type(go_type: &str) -> String {
    match go_type {
        "Tristate" => "Tristate".into(),
        "string" => "String".into(),
        "*int" => "Option<i64>".into(),
        "[]string" => "Vec<String>".into(),
        "[]PluginImport" => "Vec<PluginImport>".into(),
        "*collections.OrderedMap[string, []string]" => {
            "Option<OrderedMap<String, Vec<String>>>".into()
        }
        "tspath.RootedFilePath" => "RootedFilePath".into(),
        "tspath.RootedDirectoryPath" => "RootedDirectoryPath".into(),
        "[]tspath.RootedDirectoryPath" => "Vec<RootedDirectoryPath>".into(),
        "tspath.RootedPath" => "RootedPath".into(),
        "tspath.SourceMapLocation" => "SourceMapLocation".into(),
        other => other.into(),
    }
}

const GO_COMMIT: &str = "ec47d33c23e464a17cdf2475632cba629bee8763";

fn emit_doc_comments(out: &mut String, field: &Field) {
    if let Some(section) = &field.section {
        // Go emits the section as a standalone comment + blank line.
        let _ = writeln!(out, "\n    // {section}\n");
    }
    if let Some(comment) = &field.comment {
        let _ = writeln!(out, "    /// {comment}");
    }
    if field.deprecated {
        let _ = writeln!(
            out,
            "    /// Deprecated: Do not use outside of options parsing and validation."
        );
    }
}

fn emit_struct(out: &mut String, model: &Model, name: &str, doc: &str, fields: &[Field]) {
    let _ = writeln!(out, "{doc}");
    let _ = writeln!(
        out,
        "#[derive(Clone, Debug, Default)]\npub struct {name} {{"
    );
    for field in fields {
        let Some(ty) = &field.ty else {
            fail(format!("field {} has no type", field_name(field)));
        };
        emit_doc_comments(out, field);
        let _ = writeln!(
            out,
            "    pub {}: {},",
            snake_case(&field_name(field)),
            rust_type(ty)
        );
    }
    let _ = writeln!(out, "}}\n");
    let _ = model; // used only by callers that need enum lookup
}

fn emit_equals(out: &mut String, fields: &[Field]) {
    let _ = writeln!(
        out,
        r#"impl CompilerOptions {{
    /// Equals reports whether all stored option values are equal, including nil versus empty collections.
    /// Paths are compared by ordered entries, ignoring backing-storage allocation.
    ///
    /// PORT: Go's generated `Equals` is mirrored field-for-field in declaration
    /// order. The Go slice nil-vs-empty distinction collapses (fields are
    /// plain `Vec`s, so `(a == nil) != (b == nil) || !slices.Equal(a, b)`
    /// reduces to `a != b`); the `*int` and `Paths` nil handling is exact via
    /// `Option`. Go's `options == other` pointer shortcut has no counterpart.
    pub fn equals(&self, other: &CompilerOptions) -> bool {{"#
    );
    for field in fields {
        let field_name_str = field_name(field);
        let ident = snake_case(&field_name_str);
        let Some(ty) = &field.ty else {
            fail(format!("field {field_name_str} has no type"));
        };
        let cmp = if ty == "*collections.OrderedMap[string, []string]" {
            format!(
                "if !OrderedMap::equal_func(self.{ident}.as_ref(), other.{ident}.as_ref(), |a, b| a == b) {{\n            return false;\n        }}"
            )
        } else {
            format!("if self.{ident} != other.{ident} {{\n            return false;\n        }}")
        };
        let _ = writeln!(out, "        {cmp}");
    }
    let _ = writeln!(out, "        true\n    }}\n}}\n");
}

fn emit_enums(out: &mut String, model: &Model) {
    for enum_def in &model.enums {
        let numeric: Vec<&EnumMember> = enum_def
            .members
            .iter()
            .filter(|m| matches!(m.value, EnumValue::Number(_)))
            .collect();
        let aliases: Vec<&EnumMember> = enum_def
            .members
            .iter()
            .filter(|m| matches!(m.value, EnumValue::Alias(_)))
            .collect();

        let _ = writeln!(
            out,
            "/// Go: `type {} int32` (tsc/internal/core/options_generated.go).",
            enum_def.name
        );
        let _ = writeln!(
            out,
            "#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]"
        );
        let _ = writeln!(out, "#[repr(i32)]\npub enum {} {{", enum_def.name);
        for member in &numeric {
            let value = match member.value {
                EnumValue::Number(v) => v,
                EnumValue::Alias(_) => unreachable!(),
            };
            let mut prefix = String::new();
            if value == 0 {
                prefix.push_str("    #[default]\n");
            }
            if let Some(comment) = &member.comment {
                for line in comment.split('\n') {
                    let _ = writeln!(out, "    /// {line}");
                }
            }
            let trailing = member
                .trailing_comment
                .as_deref()
                .map(|c| format!(" // {c}"))
                .unwrap_or_default();
            let _ = writeln!(out, "{prefix}    {} = {value}{trailing},", member.name);
        }
        let _ = writeln!(out, "}}\n");

        // from_i32 — mirrors how Go's int32 constants round-trip.
        let _ = writeln!(out, "impl {} {{", enum_def.name);
        let _ = writeln!(
            out,
            "    pub fn from_i32(value: i32) -> Option<{}> {{",
            enum_def.name
        );
        let _ = writeln!(out, "        match value {{");
        for member in &numeric {
            let value = match member.value {
                EnumValue::Number(v) => v,
                EnumValue::Alias(_) => unreachable!(),
            };
            let _ = writeln!(
                out,
                "            {value} => Some({}::{}),",
                enum_def.name, member.name
            );
        }
        let _ = writeln!(out, "            _ => None,\n        }\n    }}");

        // Value-alias members (e.g. ScriptTargetLatest = ScriptTargetESNext)
        // become associated consts; Rust enums cannot share discriminants.
        for alias in aliases {
            let target = match &alias.value {
                EnumValue::Alias(target) => target,
                _ => unreachable!(),
            };
            let _ = writeln!(
                out,
                "\n    /// Go: `{}{} {} = {}{}` (value alias).\n    #[allow(non_upper_case_globals)] // Go-parity name",
                enum_def.name, alias.name, enum_def.name, enum_def.name, target
            );
            let _ = writeln!(
                out,
                "    pub const {}: {} = {}::{};",
                alias.name, enum_def.name, enum_def.name, target
            );
        }
        let _ = writeln!(out, "}}\n");
    }
}

fn emit_module_kind_map(out: &mut String, model: &Model) {
    let module_kind = model
        .enums
        .iter()
        .find(|e| e.name == "ModuleKind")
        .expect("ModuleKind");
    let entries: Vec<&EnumMember> = module_kind
        .members
        .iter()
        .filter(|m| m.module_resolution.is_some())
        .collect();
    let _ = writeln!(
        out,
        "/// Go: `var ModuleKindToModuleResolutionKind = map[ModuleKind]ModuleResolutionKind{{...}}`.\n/// A missing key in Go yields the zero value; the port returns `None` for\n/// kinds with no entry."
    );
    let _ = writeln!(
        out,
        "pub fn module_kind_to_module_resolution_kind(\n    module_kind: ModuleKind,\n) -> Option<ModuleResolutionKind> {{\n    match module_kind {{"
    );
    for member in entries {
        let target = member.module_resolution.as_deref().unwrap();
        let _ = writeln!(
            out,
            "        ModuleKind::{} => Some(ModuleResolutionKind::{}),",
            member.name, target
        );
    }
    let _ = writeln!(out, "        _ => None,\n    }}\n}}\n");
}

fn emit_stringers(out: &mut String, model: &Model) {
    for enum_def in &model.enums {
        let Some(stringer) = &enum_def.stringer else {
            continue;
        };
        let _ = writeln!(
            out,
            "// Go: `func (value {}) String() string` — panics on the zero value\n// (and, in Go, on out-of-range ints; impossible on the closed Rust enum).",
            enum_def.name
        );
        let _ = writeln!(
            out,
            "impl fmt::Display for {} {{\n    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{\n        match self {{",
            enum_def.name
        );
        for member in &enum_def.members {
            let display = match &member.value {
                EnumValue::Number(0) => {
                    let _ = writeln!(
                        out,
                        "            {}::{} => panic!(\"should not use zero value of {}\"),",
                        enum_def.name, member.name, enum_def.name
                    );
                    continue;
                }
                EnumValue::Number(_) | EnumValue::Alias(_) => match stringer {
                    Stringer::ByName => member.name.clone(),
                    Stringer::Map(map) => map
                        .get(&member.name)
                        .unwrap_or_else(|| {
                            fail(format!(
                                "no stringer text for {}::{}",
                                enum_def.name, member.name
                            ))
                        })
                        .clone(),
                },
            };
            let _ = writeln!(
                out,
                "            {}::{} => f.write_str(\"{}\"),",
                enum_def.name, member.name, display
            );
        }
        let _ = writeln!(out, "        }}\n    }}\n}}\n");
    }
}

fn emit_tests(out: &mut String, model: &Model) {
    let _ = write!(
        out,
        r#"#[cfg(test)]
mod options_generated_test {{
    // Parity spot-checks against tsc/internal/core/options_generated.go
    // @ {GO_COMMIT} — baseline reference only. Per SPEC §6 the Go output is
    // never a generation input; these assertions pin the Rust mirror to it
    // (same role as GO-BASELINE.txt for the gen-ast ordinal table).

    use super::*;

"#
    );

    for enum_def in &model.enums {
        let numeric: Vec<&EnumMember> = enum_def
            .members
            .iter()
            .filter(|m| matches!(m.value, EnumValue::Number(_)))
            .collect();
        let aliases: Vec<&EnumMember> = enum_def
            .members
            .iter()
            .filter(|m| matches!(m.value, EnumValue::Alias(_)))
            .collect();
        let baseline = numeric
            .iter()
            .map(|m| {
                let v = match m.value {
                    EnumValue::Number(v) => v,
                    _ => unreachable!(),
                };
                format!("{}={v}", m.name)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let probe = numeric
            .iter()
            .map(|m| match m.value {
                EnumValue::Number(v) => v,
                _ => unreachable!(),
            })
            .max()
            .unwrap_or(0)
            + 1;
        let fn_name = snake_case(&enum_def.name);
        let _ = writeln!(
            out,
            "    #[test]\n    fn {fn_name}_matches_go_baseline() {{\n        // Baseline (options_generated.go): {baseline}.\n        let values = ["
        );
        for member in &numeric {
            let v = match member.value {
                EnumValue::Number(v) => v,
                _ => unreachable!(),
            };
            let _ = writeln!(
                out,
                "            ({}::{}, {v}i32),",
                enum_def.name, member.name
            );
        }
        let _ = writeln!(
            out,
            "        ];\n        for (member, value) in values {{\n            assert_eq!(member as i32, value);\n            assert_eq!({}::from_i32(value), Some(member));\n        }}\n        assert_eq!(values.len(), {});\n        assert_eq!({}::from_i32({probe}), None);",
            enum_def.name,
            numeric.len(),
            enum_def.name
        );
        for alias in &aliases {
            let target = match &alias.value {
                EnumValue::Alias(target) => target,
                _ => unreachable!(),
            };
            let _ = writeln!(
                out,
                "        assert_eq!({}::{}, {}::{});",
                enum_def.name, alias.name, enum_def.name, target
            );
        }
        let _ = writeln!(out, "    }}\n");
    }

    // Stringer parity + zero-value panics.
    let _ = writeln!(
        out,
        r#"    #[test]
    fn stringer_outputs_match_go() {{
        // Baseline: ModuleResolutionKind.String() and JsxEmit.String() in
        // options_generated.go.
        assert_eq!(ModuleResolutionKind::Node10.to_string(), "Node10");
        assert_eq!(ModuleResolutionKind::Node16.to_string(), "Node16");
        assert_eq!(ModuleResolutionKind::NodeNext.to_string(), "NodeNext");
        assert_eq!(ModuleResolutionKind::Bundler.to_string(), "Bundler");
        assert_eq!(ModuleResolutionKind::Classic.to_string(), "Classic");
        assert_eq!(JsxEmit::Preserve.to_string(), "preserve");
        assert_eq!(JsxEmit::React.to_string(), "react");
        assert_eq!(JsxEmit::ReactNative.to_string(), "react-native");
        assert_eq!(JsxEmit::ReactJSX.to_string(), "react-jsx");
        assert_eq!(JsxEmit::ReactJSXDev.to_string(), "react-jsxdev");
    }}

    #[test]
    #[should_panic(expected = "should not use zero value of ModuleResolutionKind")]
    fn module_resolution_kind_zero_display_panics_like_go() {{
        let _ = ModuleResolutionKind::Unknown.to_string();
    }}

    #[test]
    #[should_panic(expected = "should not use zero value of JsxEmit")]
    fn jsx_emit_zero_display_panics_like_go() {{
        let _ = JsxEmit::None.to_string();
    }}
"#
    );

    // ModuleKindToModuleResolutionKind parity.
    let _ = writeln!(
        out,
        r#"    #[test]
    fn module_kind_to_module_resolution_kind_matches_go() {{
        // Baseline: the Go map has exactly two entries.
        assert_eq!(
            module_kind_to_module_resolution_kind(ModuleKind::Node16),
            Some(ModuleResolutionKind::Node16)
        );
        assert_eq!(
            module_kind_to_module_resolution_kind(ModuleKind::NodeNext),
            Some(ModuleResolutionKind::NodeNext)
        );
        assert_eq!(module_kind_to_module_resolution_kind(ModuleKind::ESNext), None);
    }}
"#
    );

    // Clone/Equals behavior, exercised on one field of each type class.
    let find = |ty: &str| -> &Field {
        model
            .compiler_options
            .iter()
            .find(|f| f.ty.as_deref() == Some(ty))
            .unwrap_or_else(|| fail(format!("no compiler option of type {ty:?} to test")))
    };
    let tristate = find("Tristate");
    let target = find("ScriptTarget");
    let strings = find("[]string");
    let opt_int = find("*int");
    let paths = find("*collections.OrderedMap[string, []string]");
    let t = snake_case(&field_name(tristate));
    let tgt = snake_case(&field_name(target));
    let vec = snake_case(&field_name(strings));
    let num = snake_case(&field_name(opt_int));
    let paths_field = snake_case(&field_name(paths));
    let _ = write!(
        out,
        r#"    #[test]
    fn clone_and_equals_match_go() {{
        // Baseline: the generated Clone/Equals in options_generated.go —
        // clones are value-equal; any differing field breaks Equals.
        let mut options = CompilerOptions::default();
        options.{t} = Tristate::TSTrue;
        options.{tgt} = ScriptTarget::ES2020;
        options.{vec} = vec!["lib.es2015.d.ts".to_string()];
        options.{num} = Some(2);

        let clone = options.clone();
        assert!(options.equals(&clone));
        assert!(clone.equals(&options));

        let mut other = clone.clone();
        other.strict = Tristate::TSTrue;
        assert!(!options.equals(&other));

        let mut other = clone.clone();
        other.{num} = None;
        assert!(!options.equals(&other));

        let mut other = clone.clone();
        other.{vec} = Vec::new();
        assert!(!options.equals(&other));

        let mut other = clone.clone();
        let mut paths = OrderedMap::new();
        paths.set("*".to_string(), vec!["out/*".to_string()]);
        other.{paths_field} = Some(paths);
        assert!(!options.equals(&other));

        let mut same = clone.clone();
        let mut paths = OrderedMap::new();
        paths.set("*".to_string(), vec!["out/*".to_string()]);
        same.{paths_field} = other.{paths_field}.clone();
        assert!(other.equals(&same));

        assert!(CompilerOptions::default().equals(&CompilerOptions::default()));
    }}
"#
    );
    let _ = writeln!(out, "}}");
}

fn generate(model: &Model) -> String {
    validate(model);

    let mut out = String::with_capacity(64 * 1024);
    let _ = write!(
        out,
        r#"// Code generated by rust/tools/gen-options from rust/tools/gen-options/data/options-model.json. DO NOT EDIT.
// Rust mirror of tsc/internal/core/options_generated.go @ {GO_COMMIT}
// (upstream: tools/scripts/tsc/generate-options.ts emits that file from
// tools/scripts/tsc/options.ts — the same rows/options data mirrored here;
// per SPEC §6 the Go output is never a generation input, only the parity
// baseline for the tests at the bottom of this file).
//
// PORT notes (vs options_generated.go):
//   - Go's `noCopy` embed and json tags have no Rust counterpart; dropped.
//   - Go `*int` -> `Option<i64>` (Go int is 64-bit here).
//   - Go's slice nil-vs-empty distinction collapses: nil -> empty Vec, and
//     the generated Equals compares Vec values (Go's
//     `(a == nil) != (b == nil) || !slices.Equal(a, b)` reduces to `a != b`).
//   - `Clone` (Go's generated method) -> `#[derive(Clone)]` (same shallow,
//     field-by-field copy semantics).
//   - Value-alias enum members (`ScriptTargetLatest` = `ScriptTargetESNext`)
//     become associated consts; Rust enums cannot share discriminants.
//   - Go's panicking `String()` methods become `Display` impls with identical
//     outputs; zero values panic, exactly as in Go.
//   - Derived `PartialOrd`/`Ord` match the numeric order (validated: member
//     values are monotonic in declaration order).

use std::fmt;

use tsc_collections::OrderedMap;
use tsc_tspath::{RootedDirectoryPath, RootedFilePath, RootedPath, SourceMapLocation};

use crate::tristate::Tristate;

"#
    );

    // Order mirrors the Go file: PluginImport, CompilerOptions (+Clone/Equals),
    // numeric enums (+ alias consts), ModuleKindToModuleResolutionKind,
    // stringers, TypeAcquisition, BuildOptions.

    emit_struct(
        &mut out,
        model,
        "PluginImport",
        "/// Go: `type PluginImport struct { Name string }`.",
        &model.plugin_import_fields,
    );

    emit_struct(
        &mut out,
        model,
        "CompilerOptions",
        "/// CompilerOptions contains the compiler options exposed by the API.",
        &model.compiler_options,
    );

    let _ = writeln!(
        &mut out,
        "// Clone creates a shallow copy of the CompilerOptions.\n// PORT: Go's generated `Clone` field-by-field copy -> `#[derive(Clone)]`."
    );
    emit_equals(&mut out, &model.compiler_options);

    emit_enums(&mut out, model);
    emit_module_kind_map(&mut out, model);
    emit_stringers(&mut out, model);

    emit_struct(
        &mut out,
        model,
        "TypeAcquisition",
        "/// Go: `type TypeAcquisition struct`.",
        &model.type_acquisition,
    );

    // BuildOptions: stored fields only, ordered by buildOptionFieldOrder
    // (Go: `orderByName(buildOptions, options.buildOptionFieldOrder, ...)`).
    let mut build_fields: Vec<&Field> = model
        .build_options
        .iter()
        .filter(|f| f.go_name.is_some())
        .collect();
    build_fields.sort_by_key(|f| {
        model
            .build_option_field_order
            .iter()
            .position(|name| name == &f.name)
            .unwrap_or_else(|| fail(format!("BuildOptions field {} out of order", f.name)))
    });
    emit_struct(
        &mut out,
        model,
        "BuildOptions",
        "/// Go: `type BuildOptions struct`.",
        &build_fields,
    );

    emit_tests(&mut out, model);
    out
}

// ────────────────────────────────────────────────────────────────────────────
// main
// ────────────────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let input = manifest_dir.join("data/options-model.json");
    let output = manifest_dir
        .join("../..")
        .join("crates/core/src/options_generated.rs");

    let json_str = std::fs::read_to_string(&input)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", input.display()));
    let model: Model = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", input.display()));
    let generated = generate(&model);

    // Go runs `format.Source` over the buffer; run rustfmt over the output so
    // the emitted file is `cargo fmt`-stable (and byte-identical on
    // regeneration). The temp file lives next to the target so rustfmt
    // resolves the same config `cargo fmt` would.
    let temp = output.with_extension("rs.tmp");
    std::fs::write(&temp, &generated)
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", temp.display()));
    let formatted = match std::process::Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .arg(&temp)
        .output()
    {
        Ok(result) if result.status.success() => std::fs::read_to_string(&temp)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", temp.display())),
        Ok(result) => {
            eprintln!(
                "gen-options: rustfmt failed, keeping unformatted output:\n{}",
                String::from_utf8_lossy(&result.stderr)
            );
            generated
        }
        Err(err) => {
            eprintln!("gen-options: rustfmt unavailable ({err}), keeping unformatted output");
            generated
        }
    };
    let _ = std::fs::remove_file(&temp);

    if check {
        let current = std::fs::read_to_string(&output).ok();
        match current {
            Some(cur) if cur == formatted => {
                println!(
                    "options_generated.rs: OK (current, {} lines)",
                    formatted.lines().count()
                );
                ExitCode::SUCCESS
            }
            Some(cur) => {
                let diff_line = cur
                    .lines()
                    .zip(formatted.lines())
                    .position(|(a, b)| a != b)
                    .map(|i| i + 1)
                    .unwrap_or_else(|| cur.lines().count().max(formatted.lines().count()) + 1);
                println!(
                    "options_generated.rs: STALE — first difference at line {diff_line} (checked-in {} lines, expected {} lines)",
                    cur.lines().count(),
                    formatted.lines().count()
                );
                println!("re-run `cargo run -p gen-options` and commit");
                ExitCode::FAILURE
            }
            None => {
                println!(
                    "options_generated.rs: MISSING (expected at {})",
                    output.display()
                );
                ExitCode::FAILURE
            }
        }
    } else {
        let existing = std::fs::read_to_string(&output).ok();
        if existing.as_deref() != Some(formatted.as_str()) {
            std::fs::write(&output, &formatted)
                .unwrap_or_else(|e| panic!("cannot write {}: {e}", output.display()));
        }
        println!(
            "gen-options: {} enum(s), {} compiler options, {} type acquisition fields, {} build options -> {} ({} lines{})",
            model.enums.len(),
            model.compiler_options.len(),
            model.type_acquisition.len(),
            model.build_options.iter().filter(|f| f.go_name.is_some()).count(),
            output.display(),
            formatted.lines().count(),
            if existing.as_deref() == Some(formatted.as_str()) { ", unchanged" } else { "" }
        );
        ExitCode::SUCCESS
    }
}
