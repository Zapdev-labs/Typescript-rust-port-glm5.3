// Ported from tsc/internal/module/util.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::cmp::Ordering;
use std::sync::OnceLock;

use tsc_ast::SourceFile;
use tsc_core::options_generated::{CompilerOptions, JsxEmit};
use tsc_diagnostics::Message;
use tsc_semver::{Version, must_parse, try_parse_version_range};
use tsc_tspath::{
    EXTENSION_CJS, EXTENSION_CTS, EXTENSION_DCTS, EXTENSION_DMTS, EXTENSION_DTS, EXTENSION_JS,
    EXTENSION_JSON, EXTENSION_JSX, EXTENSION_MJS, EXTENSION_MTS, EXTENSION_TS, EXTENSION_TSX,
    RootedDirectoryPath, RootedFilePath, rooted_directory_path_from_absolute,
    try_get_extension_from_path,
};

use crate::resolver::move_to_next_directory_separator_if_available;
use crate::types::ResolvedModule;

// Go: `var typeScriptVersion = semver.MustParse(core.Version())` (package
// init). The port computes it lazily with a OnceLock (core.Version() is
// already ported in tsc-core, so no in-crate shim is needed).
fn type_script_version() -> &'static Version {
    static TYPE_SCRIPT_VERSION: OnceLock<Version> = OnceLock::new();
    TYPE_SCRIPT_VERSION.get_or_init(|| must_parse(tsc_core::version::version()))
}

#[allow(non_upper_case_globals)]
/// Go: `const InferredTypesContainingFile = "__inferred type names__.ts"`.
pub const InferredTypesContainingFile: &str = "__inferred type names__.ts";

// IsApplicableVersionedTypesKey returns true if a key from a
// typesVersions-style condition map applies to the running compiler version
// (e.g. "types@>=5.0").
pub fn is_applicable_versioned_types_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("types@") else {
        return false;
    };
    let (range, ok) = try_parse_version_range(rest);
    if !ok {
        return false;
    }
    range.test(Some(type_script_version()))
}

pub fn node_module_package_root_for_file(resolved: &RootedFilePath) -> RootedDirectoryPath {
    parse_node_module_package_root(resolved.as_string(), false /*isDirectory*/)
}

pub fn node_module_package_root_for_directory(resolved: &RootedDirectoryPath) -> RootedDirectoryPath {
    parse_node_module_package_root(resolved.as_string(), true /*isDirectory*/)
}

fn parse_node_module_package_root(path: &str, is_directory: bool) -> RootedDirectoryPath {
    let Some(idx) = path.rfind("/node_modules/") else {
        return RootedDirectoryPath::default();
    };

    let index_after_node_modules = idx + "/node_modules/".len();
    let mut index_after_package_name =
        move_to_next_directory_separator_if_available(path, index_after_node_modules, is_directory);
    if path.as_bytes()[index_after_node_modules] == b'@' {
        index_after_package_name = move_to_next_directory_separator_if_available(
            path,
            index_after_package_name,
            is_directory,
        );
    }
    // Go: `tspath.RootedDirectoryPathFromAbsolute` — normalizes the slice
    // (dropping any trailing separator).
    rooted_directory_path_from_absolute(&path[..index_after_package_name])
}

/// Go: `func ParsePackageName(moduleName string) (packageName, rest string)`.
pub fn parse_package_name(module_name: &str) -> (String, String) {
    let mut idx = module_name.find('/');
    if !module_name.is_empty() && module_name.as_bytes()[0] == b'@' {
        if let Some(i) = idx {
            let offset = i + 1;
            idx = module_name[offset..].find('/').map(|j| j + offset);
        }
    }
    match idx {
        None => (module_name.to_string(), String::new()),
        Some(idx) => (
            module_name[..idx].to_string(),
            module_name[idx + 1..].to_string(),
        ),
    }
}

pub fn mangle_scoped_package_name(package_name: &str) -> String {
    if !package_name.is_empty() && package_name.as_bytes()[0] == b'@' {
        match package_name.find('/') {
            None => return package_name.to_string(),
            Some(idx) => {
                return format!(
                    "{}__{}",
                    &package_name[1..idx],
                    &package_name[idx + 1..]
                );
            }
        }
    }
    package_name.to_string()
}

pub fn unmangle_scoped_package_name(package_name: &str) -> String {
    if let Some((before, after)) = package_name.split_once("__") {
        return format!("@{before}/{after}");
    }
    package_name.to_string()
}

pub fn get_types_package_name(package_name: &str) -> String {
    format!("@types/{}", mangle_scoped_package_name(package_name))
}

pub fn get_package_name_from_types_package_name(mangled_name: &str) -> String {
    match mangled_name.strip_prefix("@types/") {
        Some(without_at_types_prefix) => unmangle_scoped_package_name(without_at_types_prefix),
        None => mangled_name.to_string(),
    }
}

/// Go: `func ComparePatternKeys(a, b string) int` — the sort comparator for
/// exports/imports map pattern keys (port returns `Ordering`; Go's int
/// return maps -1/0/1).
pub fn compare_pattern_keys(a: &str, b: &str) -> Ordering {
    let a_pattern_index = a.find('*');
    let b_pattern_index = b.find('*');
    let base_len_a = match a_pattern_index {
        Some(i) => i + 1,
        None => a.len(),
    };
    let base_len_b = match b_pattern_index {
        Some(i) => i + 1,
        None => b.len(),
    };

    if base_len_a > base_len_b {
        return Ordering::Less;
    }
    if base_len_b > base_len_a {
        return Ordering::Greater;
    }
    if a_pattern_index.is_none() {
        return Ordering::Greater;
    }
    if b_pattern_index.is_none() {
        return Ordering::Less;
    }
    if a.len() > b.len() {
        return Ordering::Less;
    }
    if b.len() > a.len() {
        return Ordering::Greater;
    }
    Ordering::Equal
}

// Returns a DiagnosticMessage if we won't include a resolved module due to its extension.
// The DiagnosticMessage's parameters are the imported module name, and the filename it resolved to.
// This returns a diagnostic even if the module will be an untyped module.
//
// PORT: Go takes `file *ast.SourceFile` and reads `file.IsDeclarationFile`;
// the ast crate keeps that flag on the SourceFile payload
// (`file.payload().is_declaration_file`), so the port takes
// `Option<&SourceFile>` and reads the payload (Go's nil file maps to None).
pub fn get_resolution_diagnostic(
    options: &CompilerOptions,
    resolved_module: &ResolvedModule,
    file: Option<&SourceFile>,
) -> Option<&'static Message> {
    let need_jsx = || -> Option<&'static Message> {
        if options.jsx != JsxEmit::None {
            return None;
        }
        Some(&tsc_diagnostics::MODULE_0_WAS_RESOLVED_TO_1_BUT_JSX_IS_NOT_SET)
    };

    let need_allow_js = || -> Option<&'static Message> {
        if options.get_allow_js()
            || !options
                .no_implicit_any
                .default_if_unknown(options.strict)
                .is_true()
        {
            return None;
        }
        Some(&tsc_diagnostics::COULD_NOT_FIND_A_DECLARATION_FILE_FOR_MODULE_0_1_IMPLICITLY_HAS_AN_ANY_TYPE)
    };

    let need_resolve_json_module = || -> Option<&'static Message> {
        if options.get_resolve_json_module() {
            return None;
        }
        Some(&tsc_diagnostics::MODULE_0_WAS_RESOLVED_TO_1_BUT_RESOLVEJSONMODULE_IS_NOT_USED)
    };

    let need_allow_arbitrary_extensions = || -> Option<&'static Message> {
        if file.is_none_or(|f| f.payload().is_declaration_file)
            || options.allow_arbitrary_extensions.is_true()
        {
            return None;
        }
        Some(&tsc_diagnostics::MODULE_0_WAS_RESOLVED_TO_1_BUT_ALLOWARBITRARYEXTENSIONS_IS_NOT_SET)
    };

    if resolved_module.resolved_using_extra_extensions {
        return None;
    }

    match resolved_module.extension.as_str() {
        EXTENSION_TS | EXTENSION_DTS | EXTENSION_MTS | EXTENSION_DMTS | EXTENSION_CTS
        | EXTENSION_DCTS => {
            // These are always allowed.
            None
        }
        EXTENSION_TSX => need_jsx(),
        EXTENSION_JSX => {
            if let Some(message) = need_jsx() {
                return Some(message);
            }
            need_allow_js()
        }
        EXTENSION_JS | EXTENSION_MJS | EXTENSION_CJS => need_allow_js(),
        EXTENSION_JSON => need_resolve_json_module(),
        _ => need_allow_arbitrary_extensions(),
    }
}

/// TryGetJSExtensionForFile maps TS/JS/DTS extensions to the output JS-side
/// extension. Returns an empty string if the extension is unsupported.
///
/// PORT: Go returns a `string`; the port returns `String` for the same
/// reason (the mapped extension is a static, but the identity arms return the
/// borrowed input's extension text).
pub fn try_get_js_extension_for_file(file_name: &str, options: &CompilerOptions) -> String {
    let ext = try_get_extension_from_path(file_name);
    match ext {
        EXTENSION_TS | EXTENSION_DTS => EXTENSION_JS.to_string(),
        EXTENSION_TSX => {
            if options.jsx == JsxEmit::Preserve {
                EXTENSION_JSX.to_string()
            } else {
                EXTENSION_JS.to_string()
            }
        }
        EXTENSION_JS | EXTENSION_JSX | EXTENSION_JSON => ext.to_string(),
        EXTENSION_DMTS | EXTENSION_MTS | EXTENSION_MJS => EXTENSION_MJS.to_string(),
        EXTENSION_DCTS | EXTENSION_CTS | EXTENSION_CJS => EXTENSION_CJS.to_string(),
        _ => String::new(),
    }
}

pub fn try_get_js_extension_for_file_name(
    file_name: &RootedFilePath,
    options: &CompilerOptions,
) -> String {
    try_get_js_extension_for_file(file_name.as_string(), options)
}
