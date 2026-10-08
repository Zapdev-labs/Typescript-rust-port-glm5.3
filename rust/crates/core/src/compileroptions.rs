// Ported from tsc/internal/core/compileroptions.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use tsc_tspath::{RootedDirectoryPath, RootedFilePath};

use crate::options_generated::{
    CompilerOptions, JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind, NewLineKind,
    ScriptTarget,
};
use crate::tristate::Tristate;

// Go's `noCopy` (embedded by CompilerOptions/BuildOptions for the -copylocks
// vet check) has no Rust counterpart; dropped.

/// Go: `var EmptyCompilerOptions = &CompilerOptions{}`.
///
/// PORT: Go shares one pointer to the zero value; the port hands out a fresh
/// default instead (the struct is not const-constructible as a static).
pub fn empty_compiler_options() -> CompilerOptions {
    CompilerOptions::default()
}

impl CompilerOptions {
    pub fn get_emit_script_target(&self) -> ScriptTarget {
        if self.target != ScriptTarget::None {
            return self.target;
        }
        ScriptTarget::LatestStandard
    }

    pub fn get_emit_module_kind(&self) -> ModuleKind {
        if self.module != ModuleKind::None {
            return self.module;
        }

        let target = self.get_emit_script_target();
        if target == ScriptTarget::ESNext {
            return ModuleKind::ESNext;
        }
        if target >= ScriptTarget::ES2022 {
            return ModuleKind::ES2022;
        }
        if target >= ScriptTarget::ES2020 {
            return ModuleKind::ES2020;
        }
        if target >= ScriptTarget::ES2015 {
            return ModuleKind::ES2015;
        }
        ModuleKind::CommonJS
    }

    pub fn get_module_resolution_kind(&self) -> ModuleResolutionKind {
        match self.module_resolution {
            ModuleResolutionKind::Unknown
            | ModuleResolutionKind::Classic
            | ModuleResolutionKind::Node10 => match self.get_emit_module_kind() {
                ModuleKind::Node16 | ModuleKind::Node18 | ModuleKind::Node20 => {
                    ModuleResolutionKind::Node16
                }
                ModuleKind::NodeNext => ModuleResolutionKind::NodeNext,
                _ => ModuleResolutionKind::Bundler,
            },
            module_resolution => module_resolution,
        }
    }

    pub fn get_emit_module_detection_kind(&self) -> ModuleDetectionKind {
        if self.module_detection != ModuleDetectionKind::None {
            return self.module_detection;
        }
        let module_kind = self.get_emit_module_kind();
        // PORT: Go compares the numeric values `ModuleKindNode16 <=
        // moduleKind && moduleKind <= ModuleKindNodeNext`; the derived enum
        // ordering is validated to match the numeric ordering
        // (options_generated.rs).
        if ModuleKind::Node16 <= module_kind && module_kind <= ModuleKind::NodeNext {
            return ModuleDetectionKind::Force;
        }
        ModuleDetectionKind::Auto
    }

    pub fn get_resolve_package_json_exports(&self) -> bool {
        self.resolve_package_json_exports.is_true_or_unknown()
    }

    pub fn get_resolve_package_json_imports(&self) -> bool {
        self.resolve_package_json_imports.is_true_or_unknown()
    }

    pub fn get_allow_importing_ts_extensions(&self) -> bool {
        self.allow_importing_ts_extensions.is_true()
            || self.rewrite_relative_import_extensions.is_true()
    }

    pub fn allow_importing_ts_extensions_from(&self, file_name: &RootedFilePath) -> bool {
        self.get_allow_importing_ts_extensions() || file_name.is_declaration_file()
    }

    pub fn get_resolve_json_module(&self) -> bool {
        if self.resolve_json_module != Tristate::TSUnknown {
            return self.resolve_json_module == Tristate::TSTrue;
        }
        match self.get_emit_module_kind() {
            // TODO in 6.0: add Node16/Node18
            ModuleKind::Node20 | ModuleKind::NodeNext => true,
            _ => self.get_module_resolution_kind() == ModuleResolutionKind::Bundler,
        }
    }

    pub fn should_preserve_const_enums(&self) -> bool {
        self.preserve_const_enums == Tristate::TSTrue || self.get_isolated_modules()
    }

    pub fn get_allow_js(&self) -> bool {
        if self.allow_js != Tristate::TSUnknown {
            return self.allow_js == Tristate::TSTrue;
        }
        self.check_js == Tristate::TSTrue
    }

    pub fn get_jsx_transform_enabled(&self) -> bool {
        let jsx = self.jsx;
        jsx == JsxEmit::React || jsx == JsxEmit::ReactJSX || jsx == JsxEmit::ReactJSXDev
    }

    pub fn get_strict_option_value(&self, value: Tristate) -> bool {
        if value != Tristate::TSUnknown {
            return value == Tristate::TSTrue;
        }
        self.strict != Tristate::TSFalse
    }

    pub fn get_effective_type_roots(
        &self,
        current_directory: &RootedDirectoryPath,
    ) -> (Vec<RootedDirectoryPath>, bool) {
        // PORT: Go checks `options.TypeRoots != nil` (nil ⇒ not configured);
        // the port's Vec collapses nil and empty, so an explicitly empty
        // `typeRoots` array is treated as unset here.
        if !self.type_roots.is_empty() {
            return (self.type_roots.clone(), true);
        }
        let base_dir: RootedDirectoryPath = if !self.config_file_path.is_empty() {
            self.config_file_path.directory()
        } else {
            if current_directory.is_empty() {
                // This was accounted for in the TS codebase, but only for third-party API usage
                // where the module resolution host does not provide a getCurrentDirectory().
                panic!(
                    "cannot get effective type roots without a config file path or current directory"
                );
            }
            current_directory.clone()
        };

        let mut type_roots = Vec::with_capacity(base_dir.as_string().matches('/').count());
        base_dir.for_each_ancestor_directory(|dir| {
            type_roots.push(dir.resolve_directory("node_modules/@types"));
            ((), false)
        });
        (type_roots, false)
    }

    /// UsesWildcardTypes returns true if this option's types array includes "*".
    pub fn uses_wildcard_types(&self) -> bool {
        self.types.iter().any(|ty| ty == "*")
    }

    pub fn get_isolated_modules(&self) -> bool {
        self.isolated_modules == Tristate::TSTrue || self.verbatim_module_syntax == Tristate::TSTrue
    }

    pub fn is_incremental(&self) -> bool {
        self.incremental.is_true() || self.composite.is_true()
    }

    pub fn get_emit_standard_class_fields(&self) -> bool {
        self.use_define_for_class_fields != Tristate::TSFalse
            && self.get_emit_script_target() >= ScriptTarget::ES2022
    }

    pub fn get_use_define_for_class_fields(&self) -> bool {
        if self.use_define_for_class_fields == Tristate::TSUnknown {
            return self.get_emit_script_target() >= ScriptTarget::ES2022;
        }
        self.use_define_for_class_fields == Tristate::TSTrue
    }

    pub fn get_emit_declarations(&self) -> bool {
        self.declaration.is_true() || self.composite.is_true()
    }

    pub fn get_are_declaration_maps_enabled(&self) -> bool {
        self.declaration_map == Tristate::TSTrue && self.get_emit_declarations()
    }

    pub fn has_json_module_emit_enabled(&self) -> bool {
        !matches!(
            self.get_emit_module_kind(),
            ModuleKind::System | ModuleKind::UMD
        )
    }

    pub fn get_effective_root_dirs(&self) -> &[RootedDirectoryPath] {
        &self.root_dirs
    }

    pub fn get_paths_base_path(
        &self,
        current_directory: &RootedDirectoryPath,
    ) -> RootedDirectoryPath {
        if self.paths.as_ref().is_none_or(|paths| paths.size() == 0) {
            return RootedDirectoryPath::default();
        }
        if !self.paths_base_path.is_empty() {
            return self.paths_base_path.clone();
        }
        current_directory.clone()
    }
}

impl ModuleKind {
    pub fn is_non_node_esm(&self) -> bool {
        // PORT: Go compares numeric values `moduleKind >= ModuleKindES2015 &&
        // moduleKind <= ModuleKindESNext`; the derived enum ordering is
        // validated to match the numeric ordering (options_generated.rs).
        ModuleKind::ES2015 <= *self && *self <= ModuleKind::ESNext
    }

    pub fn supports_import_attributes(&self) -> bool {
        (ModuleKind::Node18 <= *self && *self <= ModuleKind::NodeNext)
            || *self == ModuleKind::Preserve
            || *self == ModuleKind::ESNext
    }

    pub fn supports_deferred_imports(&self) -> bool {
        *self == ModuleKind::ESNext || *self == ModuleKind::Preserve
    }

    pub fn supports_source_phase_imports(&self) -> bool {
        *self == ModuleKind::ESNext
            || *self == ModuleKind::NodeNext
            || *self == ModuleKind::Preserve
    }
}

/// Go: `type ResolutionMode = ModuleKind // ModuleKindNone | ModuleKindCommonJS | ModuleKindESNext`.
pub type ResolutionMode = ModuleKind;

// Go-parity constant names (ResolutionModeNone/CommonJS/ESM).
#[allow(non_upper_case_globals)]
/// Go: `ResolutionModeNone = ModuleKindNone`.
pub const ResolutionModeNone: ResolutionMode = ModuleKind::None;
#[allow(non_upper_case_globals)]
/// Go: `ResolutionModeCommonJS = ModuleKindCommonJS`.
pub const ResolutionModeCommonJS: ResolutionMode = ModuleKind::CommonJS;
#[allow(non_upper_case_globals)]
/// Go: `ResolutionModeESM = ModuleKindESNext`.
pub const ResolutionModeESM: ResolutionMode = ModuleKind::ESNext;

/// Go: `func GetNewLineKind(s string) NewLineKind`.
pub fn get_new_line_kind(s: &str) -> NewLineKind {
    match s {
        "\r\n" => NewLineKind::CRLF,
        "\n" => NewLineKind::LF,
        _ => NewLineKind::None,
    }
}

impl NewLineKind {
    pub fn get_new_line_character(&self) -> &'static str {
        match self {
            NewLineKind::CRLF => "\r\n",
            _ => "\n",
        }
    }
}

#[cfg(test)]
mod tests {
    // Ports of the behaviors in compileroptions.go (the Go file has no test
    // file; assertions here were derived from the Go method bodies, with the
    // enum values pinned by the options_generated baseline tests).

    use super::*;
    use crate::tristate::Tristate;
    use tsc_tspath::RootedFilePath;

    #[test]
    fn test_get_emit_script_target() {
        let options = CompilerOptions::default();
        // Target zero value => LatestStandard (ES2026 in the Go baseline).
        assert_eq!(options.get_emit_script_target(), ScriptTarget::ES2026);
        let options = CompilerOptions {
            target: ScriptTarget::ES2020,
            ..CompilerOptions::default()
        };
        assert_eq!(options.get_emit_script_target(), ScriptTarget::ES2020);
        let options = CompilerOptions {
            target: ScriptTarget::ESNext,
            ..CompilerOptions::default()
        };
        assert_eq!(options.get_emit_script_target(), ScriptTarget::ESNext);
    }

    #[test]
    fn test_get_emit_module_kind() {
        // Default: LatestStandard (ES2026) >= ES2022 => ES2022.
        assert_eq!(
            CompilerOptions::default().get_emit_module_kind(),
            ModuleKind::ES2022
        );
        let mut options = CompilerOptions {
            target: ScriptTarget::ESNext,
            ..CompilerOptions::default()
        };
        assert_eq!(options.get_emit_module_kind(), ModuleKind::ESNext);
        options.target = ScriptTarget::ES2021;
        assert_eq!(options.get_emit_module_kind(), ModuleKind::ES2020);
        options.target = ScriptTarget::ES2017;
        assert_eq!(options.get_emit_module_kind(), ModuleKind::ES2015);
        options.target = ScriptTarget::ES5;
        assert_eq!(options.get_emit_module_kind(), ModuleKind::CommonJS);
        options.target = ScriptTarget::None;
        options.module = ModuleKind::NodeNext;
        assert_eq!(options.get_emit_module_kind(), ModuleKind::NodeNext);
    }

    #[test]
    fn test_get_module_resolution_kind() {
        // Default (unknown resolution, ES2022 emit kind) => Bundler.
        assert_eq!(
            CompilerOptions::default().get_module_resolution_kind(),
            ModuleResolutionKind::Bundler
        );
        let mut options = CompilerOptions {
            module: ModuleKind::Node16,
            ..CompilerOptions::default()
        };
        assert_eq!(
            options.get_module_resolution_kind(),
            ModuleResolutionKind::Node16
        );
        options.module = ModuleKind::NodeNext;
        assert_eq!(
            options.get_module_resolution_kind(),
            ModuleResolutionKind::NodeNext
        );
        // Classic/Node10 are also derived from the emit kind in Go (the
        // derive arm matches Unknown, Classic, and Node10); an explicit
        // non-legacy resolution wins over the emit kind.
        options.module_resolution = ModuleResolutionKind::Classic;
        assert_eq!(
            options.get_module_resolution_kind(),
            ModuleResolutionKind::NodeNext
        );
        options.module_resolution = ModuleResolutionKind::Bundler;
        assert_eq!(
            options.get_module_resolution_kind(),
            ModuleResolutionKind::Bundler
        );
    }

    #[test]
    fn test_get_emit_module_detection_kind() {
        // Default: ES2022 emit kind is outside Node16..=NodeNext => Auto.
        assert_eq!(
            CompilerOptions::default().get_emit_module_detection_kind(),
            ModuleDetectionKind::Auto
        );
        let mut options = CompilerOptions {
            module: ModuleKind::Node16,
            ..CompilerOptions::default()
        };
        assert_eq!(
            options.get_emit_module_detection_kind(),
            ModuleDetectionKind::Force
        );
        options.module_detection = ModuleDetectionKind::Legacy;
        assert_eq!(
            options.get_emit_module_detection_kind(),
            ModuleDetectionKind::Legacy
        );
    }

    #[test]
    fn test_unknown_tristate_defaults() {
        let options = CompilerOptions::default();
        assert!(options.get_resolve_package_json_exports());
        assert!(options.get_resolve_package_json_imports());
        assert!(!options.get_allow_importing_ts_extensions());
        assert!(!options.get_allow_js());
        assert!(options.get_strict_option_value(Tristate::TSUnknown));
        assert!(!options.get_emit_declarations());
        assert!(!options.is_incremental());
        assert!(!options.get_are_declaration_maps_enabled());
        assert!(options.get_use_define_for_class_fields());
        assert!(options.get_emit_standard_class_fields());
        assert!(options.get_resolve_json_module());
        assert!(options.has_json_module_emit_enabled());
    }

    #[test]
    fn test_tristate_overrides() {
        let mut options = CompilerOptions {
            allow_js: Tristate::TSFalse,
            ..CompilerOptions::default()
        };
        assert!(!options.get_allow_js());
        // checkJs only takes effect when allowJs is Unknown (Go GetAllowJS).
        options.check_js = Tristate::TSTrue;
        assert!(!options.get_allow_js());
        options.allow_js = Tristate::TSUnknown;
        assert!(options.get_allow_js());

        options.strict = Tristate::TSFalse;
        assert!(!options.get_strict_option_value(Tristate::TSUnknown));
        assert!(options.get_strict_option_value(Tristate::TSTrue));
        assert!(!options.get_strict_option_value(Tristate::TSFalse));

        options.use_define_for_class_fields = Tristate::TSFalse;
        assert!(!options.get_use_define_for_class_fields());
        assert!(!options.get_emit_standard_class_fields());

        options.resolve_json_module = Tristate::TSFalse;
        assert!(!options.get_resolve_json_module());

        options.composite = Tristate::TSTrue;
        assert!(options.is_incremental());
        assert!(options.get_emit_declarations());
        options.declaration_map = Tristate::TSTrue;
        assert!(options.get_are_declaration_maps_enabled());

        options.rewrite_relative_import_extensions = Tristate::TSTrue;
        assert!(options.get_allow_importing_ts_extensions());

        options.isolated_modules = Tristate::TSTrue;
        assert!(options.get_isolated_modules());
        assert!(options.should_preserve_const_enums());
    }

    #[test]
    fn test_get_jsx_transform_enabled() {
        let options = CompilerOptions::default();
        assert!(!options.get_jsx_transform_enabled());
        let mut options = CompilerOptions {
            jsx: JsxEmit::React,
            ..CompilerOptions::default()
        };
        assert!(options.get_jsx_transform_enabled());
        for jsx in [JsxEmit::ReactJSX, JsxEmit::ReactJSXDev] {
            options.jsx = jsx;
            assert!(options.get_jsx_transform_enabled());
        }
        options.jsx = JsxEmit::Preserve;
        assert!(!options.get_jsx_transform_enabled());
    }

    #[test]
    fn test_module_kind_predicates() {
        // PORT note: derived enum ordering == numeric ordering (validated by
        // the generator), matching Go's int32 comparisons.
        assert!(ModuleKind::ES2015.is_non_node_esm());
        assert!(ModuleKind::ES2022.is_non_node_esm());
        assert!(ModuleKind::ESNext.is_non_node_esm());
        assert!(!ModuleKind::CommonJS.is_non_node_esm());
        assert!(!ModuleKind::Node16.is_non_node_esm());
        assert!(!ModuleKind::Preserve.is_non_node_esm());

        assert!(ModuleKind::Node18.supports_import_attributes());
        assert!(ModuleKind::Node20.supports_import_attributes());
        assert!(ModuleKind::NodeNext.supports_import_attributes());
        assert!(ModuleKind::Preserve.supports_import_attributes());
        assert!(ModuleKind::ESNext.supports_import_attributes());
        assert!(!ModuleKind::CommonJS.supports_import_attributes());
        assert!(!ModuleKind::Node16.supports_import_attributes());

        assert!(ModuleKind::ESNext.supports_deferred_imports());
        assert!(ModuleKind::Preserve.supports_deferred_imports());
        assert!(!ModuleKind::NodeNext.supports_deferred_imports());

        assert!(ModuleKind::NodeNext.supports_source_phase_imports());
        assert!(!ModuleKind::Node20.supports_source_phase_imports());
    }

    #[test]
    fn test_resolution_mode_consts() {
        assert_eq!(ResolutionModeNone, ModuleKind::None);
        assert_eq!(ResolutionModeCommonJS, ModuleKind::CommonJS);
        assert_eq!(ResolutionModeESM, ModuleKind::ESNext);
    }

    #[test]
    fn test_get_new_line_kind() {
        assert_eq!(get_new_line_kind("\r\n"), NewLineKind::CRLF);
        assert_eq!(get_new_line_kind("\n"), NewLineKind::LF);
        assert_eq!(get_new_line_kind(""), NewLineKind::None);
        assert_eq!(NewLineKind::CRLF.get_new_line_character(), "\r\n");
        assert_eq!(NewLineKind::LF.get_new_line_character(), "\n");
        // The Go default arm covers every non-CRLF value, including zero.
        assert_eq!(NewLineKind::None.get_new_line_character(), "\n");
    }

    #[test]
    fn test_uses_wildcard_types() {
        let mut options = CompilerOptions::default();
        assert!(!options.uses_wildcard_types());
        options.types = vec!["node".to_string()];
        assert!(!options.uses_wildcard_types());
        options.types.push("*".to_string());
        assert!(options.uses_wildcard_types());
    }

    #[test]
    fn test_get_effective_root_dirs_and_paths_base_path() {
        let mut options = CompilerOptions::default();
        assert!(options.get_effective_root_dirs().is_empty());
        // Paths unset => empty base path, even when a current directory is given.
        let cwd = RootedDirectoryPath::from("/proj");
        assert!(options.get_paths_base_path(&cwd).is_empty());

        let mut paths = tsc_collections::OrderedMap::new();
        paths.set("*".to_string(), vec!["out/*".to_string()]);
        options.paths = Some(paths);
        options.paths_base_path = RootedDirectoryPath::from("/config");
        assert_eq!(
            options.get_paths_base_path(&cwd),
            RootedDirectoryPath::from("/config")
        );
        options.paths_base_path = RootedDirectoryPath::default();
        assert_eq!(options.get_paths_base_path(&cwd), cwd);

        options.root_dirs = vec![
            RootedDirectoryPath::from("/a"),
            RootedDirectoryPath::from("/b"),
        ];
        assert_eq!(options.get_effective_root_dirs().len(), 2);
    }

    #[test]
    fn test_get_effective_type_roots_from_config() {
        let options = CompilerOptions {
            type_roots: vec![RootedDirectoryPath::from("/custom/types")],
            ..CompilerOptions::default()
        };
        let (roots, from_config) =
            options.get_effective_type_roots(&RootedDirectoryPath::from("/proj"));
        assert!(from_config);
        assert_eq!(roots, vec![RootedDirectoryPath::from("/custom/types")]);
    }

    #[test]
    fn test_get_effective_type_roots_walks_ancestors() {
        let options = CompilerOptions {
            config_file_path: RootedFilePath::from("/home/user/proj/tsconfig.json"),
            ..CompilerOptions::default()
        };
        let (roots, from_config) =
            options.get_effective_type_roots(&RootedDirectoryPath::default());
        assert!(!from_config);
        assert_eq!(
            roots,
            vec![
                RootedDirectoryPath::from("/home/user/proj/node_modules/@types"),
                RootedDirectoryPath::from("/home/user/node_modules/@types"),
                RootedDirectoryPath::from("/home/node_modules/@types"),
                RootedDirectoryPath::from("/node_modules/@types"),
            ]
        );

        // No config file path: the current directory is the base.
        let options = CompilerOptions::default();
        let (roots, from_config) =
            options.get_effective_type_roots(&RootedDirectoryPath::from("/proj"));
        assert!(!from_config);
        assert_eq!(
            roots,
            vec![
                RootedDirectoryPath::from("/proj/node_modules/@types"),
                RootedDirectoryPath::from("/node_modules/@types"),
            ]
        );
    }

    #[test]
    #[should_panic(
        expected = "cannot get effective type roots without a config file path or current directory"
    )]
    fn test_get_effective_type_roots_panics_without_base() {
        CompilerOptions::default().get_effective_type_roots(&RootedDirectoryPath::default());
    }

    #[test]
    fn test_allow_importing_ts_extensions_from() {
        let options = CompilerOptions::default();
        assert!(!options.allow_importing_ts_extensions_from(&RootedFilePath::from("/x/foo.ts")));
        assert!(options.allow_importing_ts_extensions_from(&RootedFilePath::from("/x/foo.d.ts")));
        let options = CompilerOptions {
            allow_importing_ts_extensions: Tristate::TSTrue,
            ..CompilerOptions::default()
        };
        assert!(options.allow_importing_ts_extensions_from(&RootedFilePath::from("/x/foo.ts")));
    }

    #[test]
    fn test_empty_compiler_options() {
        // Go: EmptyCompilerOptions is the zero value.
        assert!(empty_compiler_options().equals(&CompilerOptions::default()));
    }
}
