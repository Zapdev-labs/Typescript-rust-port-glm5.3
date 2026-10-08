// Ported from tsc/internal/module/staticresolver_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go test name mapping:
//   TestStaticResolver → static_resolver

use std::sync::Arc;

use tsc_core::compileroptions::{ResolutionMode, ResolutionModeCommonJS, ResolutionModeESM};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use tsc_tspath::{
    CaseSensitivity, RootedDirectoryPath, RootedFilePath, rooted_directory_path_from_normalized,
};

use crate::resolver::{new_resolver, ResolverOptions};
use crate::resolver_internal_test::ResolutionTestInputs;
use crate::staticresolver::{new_static_resolver, new_static_resolutions, StaticResolutionEntry};
use crate::types::{ResolvedModule, Resolver};

#[test]
fn static_resolver() {
    let fs = tsc_vfs::vfstest::from_map(
        [
            ("/repo/node_modules/fallback/package.json", r#"{"name":"fallback","types":"index.d.ts"}"#),
            ("/repo/node_modules/fallback/index.d.ts", "export {};"),
        ],
        CaseSensitivity::CaseSensitive,
    );
    let fallback = new_resolver(ResolverOptions {
        host: Arc::new(ResolutionTestInputs {
            fs: Arc::clone(&fs),
            cwd: rooted_directory_path_from_normalized("/repo"),
        }),
        compiler_options: Arc::new(CompilerOptions {
            module: ModuleKind::ESNext,
            module_resolution: ModuleResolutionKind::Bundler,
            ..Default::default()
        }),
        typings_location: RootedDirectoryPath::default(),
        project_name: String::new(),
        extra_extensions: Vec::new(),
        package_json_cache: None,
    });
    let resolutions = new_static_resolutions(
        vec![
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                result: Some(Arc::new(ResolvedModule {
                    resolved_file_name: RootedFilePath::from("/global.d.ts"),
                    ..Default::default()
                })),
                ..Default::default()
            },
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                containing_directory: rooted_directory_path_from_normalized("/repo/src"),
                result: Some(Arc::new(ResolvedModule {
                    resolved_file_name: RootedFilePath::from("/directory.d.ts"),
                    ..Default::default()
                })),
                ..Default::default()
            },
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                resolution_mode: Some(ResolutionModeESM),
                result: Some(Arc::new(ResolvedModule {
                    resolved_file_name: RootedFilePath::from("/esm.d.ts"),
                    ..Default::default()
                })),
                ..Default::default()
            },
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                containing_directory: rooted_directory_path_from_normalized("/repo/src"),
                resolution_mode: Some(ResolutionModeESM),
                result: Some(Arc::new(ResolvedModule {
                    resolved_file_name: RootedFilePath::from("/directory-esm.d.ts"),
                    ..Default::default()
                })),
                ..Default::default()
            },
            StaticResolutionEntry {
                module_name: "unresolved".to_string(),
                result: None,
                ..Default::default()
            },
        ],
        true,
        rooted_directory_path_from_normalized("/repo"),
        CaseSensitivity::CaseSensitive,
    )
    .expect("NewStaticResolutions failed");
    let resolver = new_static_resolver(Arc::new(fallback) as Arc<dyn Resolver>, resolutions);

    struct Case {
        name: &'static str,
        containing_file: &'static str,
        mode: ResolutionMode,
        resolved_file_name: &'static str,
    }
    for test in [
        Case { name: "provided", containing_file: "/repo/src/index.ts", mode: ResolutionModeESM, resolved_file_name: "/directory-esm.d.ts" },
        Case { name: "provided", containing_file: "/repo/src/index.ts", mode: ResolutionModeCommonJS, resolved_file_name: "/directory.d.ts" },
        Case { name: "provided", containing_file: "/repo/other/index.ts", mode: ResolutionModeESM, resolved_file_name: "/esm.d.ts" },
        Case { name: "provided", containing_file: "/repo/other/index.ts", mode: ResolutionModeCommonJS, resolved_file_name: "/global.d.ts" },
        Case { name: "fallback", containing_file: "/repo/src/index.ts", mode: ResolutionModeESM, resolved_file_name: "/repo/node_modules/fallback/index.d.ts" },
        Case { name: "unresolved", containing_file: "/repo/src/index.ts", mode: ResolutionModeESM, resolved_file_name: "" },
    ] {
        let (result, _diagnostics) = resolver.resolve_module_name(
            test.name,
            &RootedFilePath::from(test.containing_file),
            test.mode,
            None,
        );
        if test.resolved_file_name.is_empty() {
            assert!(result.is_none(), "{}: expected no result", test.name);
        } else {
            let result = result
                .unwrap_or_else(|| panic!("{}: expected a result", test.name));
            assert_eq!(result.resolved_file_name, test.resolved_file_name.into());
        }
    }
}
