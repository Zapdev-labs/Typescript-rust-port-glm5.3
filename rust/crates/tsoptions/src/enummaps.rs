// Port of tsc/internal/tsoptions/enummaps.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The lib tables live in declarations_generated.rs (LibMap / targetToLibMap);
// this module carries the accessors over them.

use std::sync::OnceLock;

use tsc_collections::{new_set_from_items, OrderedMap, Set};
use tsc_core::options_generated::CompilerOptions;

use crate::commandlineoption::CompilerOptionsValue;

/// Go: `var Libs = slices.Collect(LibMap.Keys())`.
pub fn libs() -> Vec<String> {
    crate::declarations_generated::lib_map().keys().cloned().collect()
}

/// Go: `var LibFilesSet = collections.NewSetFromItems(...)` (the map's values).
pub fn lib_files_set() -> &'static Set<String> {
    static SET: OnceLock<Set<String>> = OnceLock::new();
    SET.get_or_init(|| {
        new_set_from_items(
            crate::declarations_generated::lib_map()
                .values()
                .map(|v| match v {
                    CompilerOptionsValue::Str(s) => s.clone(),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>(),
        )
    })
}

/// Go: `func GetLibFileName(libName string) (string, bool)` — checks whether
/// libName is a valid lib name or file name, converting to the filename.
pub fn get_lib_file_name(lib_name: &str) -> Option<String> {
    let lib_name = tsc_tspath::to_file_name_lower_case(lib_name).into_owned();
    if lib_files_set().has(&lib_name) {
        return Some(lib_name);
    }
    crate::declarations_generated::lib_map()
        .get(&lib_name)
        .and_then(|v| match v {
            CompilerOptionsValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

/// Go: `func TargetToLibMap() map[core.ScriptTarget]string` — the accessor
/// view over the generated table (entries identical to the Go map).
pub fn target_to_lib_map(
) -> impl Iterator<Item = (tsc_core::options_generated::ScriptTarget, &'static str)> {
    use tsc_core::options_generated::ScriptTarget;
    // The Go map's key set (non-alias members; the generated match covers
    // exactly these targets).
    [
        ScriptTarget::ES2015,
        ScriptTarget::ES2016,
        ScriptTarget::ES2017,
        ScriptTarget::ES2018,
        ScriptTarget::ES2019,
        ScriptTarget::ES2020,
        ScriptTarget::ES2021,
        ScriptTarget::ES2022,
        ScriptTarget::ES2023,
        ScriptTarget::ES2024,
        ScriptTarget::ES2025,
        ScriptTarget::ES2026,
        ScriptTarget::ESNext,
    ]
    .into_iter()
    .filter_map(|target| {
        crate::declarations_generated::target_to_lib(target)
            .map(|name| (target, name))
    })
}

/// Go: `func GetDefaultLibFileName(options *core.CompilerOptions) string`.
pub fn get_default_lib_file_name(options: &CompilerOptions) -> String {
    crate::declarations_generated::target_to_lib(options.get_emit_script_target())
        .unwrap_or("lib.d.ts")
        .to_string()
}

/// Unused import placeholder so the ordered-map import stays documented.
#[allow(dead_code)]
fn _ordered_map_type(_: &OrderedMap<String, CompilerOptionsValue>) {}
