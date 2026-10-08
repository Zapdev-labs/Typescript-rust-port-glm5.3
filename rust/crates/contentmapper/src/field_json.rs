// The per-type marshal helpers behind the generated
// `compiler_option_fields_generated.rs` — Go performs these marshals through
// `json.Marshal(field.Interface())` on the reflect-extracted field, whose
// semantics each helper mirrors for one core field type (see the generated
// file's PORT notes).

use tsc_core::options_generated::{
    JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind, NewLineKind, PluginImport,
    ScriptTarget,
};
use tsc_core::tristate::Tristate;
use tsc_json::Value;
use tsc_tspath::{RootedDirectoryPath, RootedFilePath, RootedPath, SourceMapLocation};

/// A `json.RawMessage`-style owned JSON value (see tsc_json::Value).
fn raw(json: String) -> Option<Value> {
    Some(
        serde_json::value::RawValue::from_string(json)
            .expect("contentmapper: marshal produced invalid JSON"),
    )
}

/// Go: `json.Marshal(core.Tristate)` — `true`/`false`, and `null` for the
/// unknown value (IsZero skip happens per-field in the generated match).
pub fn tristate(value: Tristate) -> Option<Value> {
    if value == Tristate::TSUnknown {
        return None;
    }
    raw(if value == Tristate::TSTrue { "true" } else { "false" }.to_string())
}

/// Go: `json.Marshal(string)` — `None` when empty (Go's IsZero).
pub fn string_value(value: &str) -> Option<Value> {
    if value.is_empty() {
        return None;
    }
    raw(serde_json::json!(value).to_string())
}

/// Go: `json.Marshal(*int)` — `None` when nil.
pub fn int_value(value: Option<i64>) -> Option<Value> {
    let n = value?;
    raw(n.to_string())
}

/// Go: `json.Marshal([]string)` — `None` (Go nil slice) skips via IsZero;
/// PORT: a non-nil empty slice is NOT zero in Go's reflect, so `Some(empty)`
/// marshals as `[]` (nil-vs-empty preserved via the core `Option<Vec<_>>`).
pub fn string_array(values: Option<&[String]>) -> Option<Value> {
    let values = values?;
    raw(serde_json::json!(values).to_string())
}

/// Go: `json.Marshal([]core.PluginImport)` (array of `{"name": ...}`) —
/// `None` (Go nil) skips; `Some(empty)` marshals `[]` (see string_array).
pub fn plugin_imports(imports: Option<&[PluginImport]>) -> Option<Value> {
    let imports = imports?;
    let items: Vec<serde_json::Value> = imports
        .iter()
        .map(|import| serde_json::json!({ "name": import.name }))
        .collect();
    raw(serde_json::json!(items).to_string())
}

/// Go: `json.Marshal(map[string][]string)` — `None` (Go nil pointer) skips
/// via IsZero; PORT: Go's IsZero tests the *pointer*, so a non-nil empty map
/// is NOT zero and marshals as `{}` — the empty-map skip is dropped.
pub fn paths_map(value: Option<&tsc_collections::OrderedMap<String, Vec<String>>>) -> Option<Value> {
    let map = value?;
    let mut out = String::with_capacity(32);
    out.push('{');
    let mut first = true;
    for (key, paths) in map.entries() {
        if !first {
            out.push(',');
        }
        first = false;
        use std::fmt::Write;
        let _ = write!(out, "{}:{}", serde_json::json!(key), serde_json::json!(paths));
    }
    out.push('}');
    raw(out)
}

/// Go: `json.Marshal(tspath.Rooted*)` / `SourceMapLocation` — the wrapped
/// string; `None` when empty (Go's IsZero). The path newtypes all
/// `Deref<Target = str>`, which is exactly the marshal Go's json tag gets.
pub fn rooted_string<T: std::ops::Deref<Target = str>>(value: &T) -> Option<Value> {
    string_value(value)
}

/// `[]tspath.RootedDirectoryPath` (rootDirs/typeRoots) — `None` (Go nil)
/// skips; `Some(empty)` marshals `[]` (see string_array).
pub fn rooted_directory_array(values: Option<&[RootedDirectoryPath]>) -> Option<Value> {
    let values = values?;
    raw(serde_json::json!(values
        .iter()
        .map(|v| v.as_string().to_owned())
        .collect::<Vec<_>>())
    .to_string())
}

/// Go: `json.Marshal(core.<Enum>)` — the raw int32 value.
macro_rules! enum_value {
    ($fn_name:ident, $ty:ty) => {
        pub fn $fn_name(value: $ty) -> Option<Value> {
            if value as i32 == 0 {
                return None;
            }
            raw((value as i32).to_string())
        }
    };
}

enum_value!(jsx_emit, JsxEmit);
enum_value!(module_kind, ModuleKind);
enum_value!(module_resolution_kind, ModuleResolutionKind);
enum_value!(module_detection_kind, ModuleDetectionKind);
enum_value!(new_line_kind, NewLineKind);
enum_value!(script_target, ScriptTarget);

/// The remaining path-typed fields share `rooted_string`'s string marshal;
/// listed for the generated match's referenced-but-unreachable arm names.
#[allow(dead_code)]
fn _path_marshal_kinds(_f: &RootedFilePath, _d: &RootedDirectoryPath, _p: &RootedPath, _s: &SourceMapLocation) {}
