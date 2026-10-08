// Ported from tsc/internal/module/resolver_internal_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go test name mapping:
//   TestNormalizePathForCJSResolutionPreservesDirectoryIntent → normalize_path_for_cjs_resolution_preserves_directory_intent
//   TestPackageJSONPathWithTrailingSeparatorDoesNotResolveAsFile → package_json_path_with_trailing_separator_does_not_resolve_as_file
//   TestExtensionReplacementPreservesEmptyAndDotStems → extension_replacement_preserves_empty_and_dot_stems
//   TestOutputDirectoriesRemoveTrailingSeparators → output_directories_remove_trailing_separators

use std::sync::Arc;

use tsc_core::compileroptions::{ResolutionModeCommonJS, ResolutionModeESM};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use tsc_core::tristate::Tristate;
use tsc_tspath::{
    CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath, rooted_directory_path_from_absolute,
    rooted_directory_path_from_normalized, rooted_file_path_from_normalized,
};
use tsc_vfs::Entries;

use crate::cache::new_resolution_data;
use crate::resolver::{
    DefaultResolver, ResolutionCandidate, ResolutionState, normalize_path_for_cjs_resolution,
    resolve_resolution_candidate,
};
use crate::types::{Extensions, ResolutionHost, ResolverOptions};

// resolutionTestInputs mirrors the Go test host (declared in resolver_test.go,
// reused by this file's package).
pub(crate) struct ResolutionTestInputs {
    pub(crate) fs: Arc<dyn tsc_vfs::Fs>,
    pub(crate) cwd: RootedDirectoryPath,
}

impl ResolutionHost for ResolutionTestInputs {
    fn fs(&self) -> Arc<dyn tsc_vfs::Fs> {
        Arc::clone(&self.fs)
    }
    fn get_current_directory(&self) -> RootedDirectoryPath {
        self.cwd.clone()
    }
}

fn test_resolver_options(host: ResolutionTestInputs, opts: CompilerOptions) -> ResolverOptions {
    ResolverOptions {
        host: Arc::new(host),
        compiler_options: Arc::new(opts),
        typings_location: RootedDirectoryPath::default(),
        project_name: String::new(),
        extra_extensions: Vec::new(),
        package_json_cache: None,
    }
}

#[track_caller]
fn assert_candidate(
    candidate: &ResolutionCandidate,
    path: &str,
    directory_only: bool,
    text: &str,
) {
    assert_eq!(candidate.path, RootedPath::from(path), "path");
    assert_eq!(candidate.directory_only, directory_only, "directoryOnly");
    assert_eq!(candidate.as_string(), text, "AsString()");
    assert_eq!(
        candidate.has_trailing_directory_separator(),
        directory_only,
        "HasTrailingDirectorySeparator()"
    );
}

#[test]
fn normalize_path_for_cjs_resolution_preserves_directory_intent() {
    struct Case {
        name: &'static str,
        containing_directory: &'static str,
        module_name: &'static str,
        path: &'static str,
        text: &'static str,
        directory_only: bool,
    }
    let tests = [
        Case { name: "file", containing_directory: "/project", module_name: "file", path: "/project/file", text: "/project/file", directory_only: false },
        Case { name: "trailing separator", containing_directory: "/project", module_name: "directory/", path: "/project/directory", text: "/project/directory/", directory_only: true },
        Case { name: "current directory", containing_directory: "/project", module_name: ".", path: "/project", text: "/project/", directory_only: true },
        Case { name: "current directory with backslash", containing_directory: "/project", module_name: ".\\", path: "/project", text: "/project/", directory_only: true },
        Case { name: "nested parent directory", containing_directory: "/project/src", module_name: "../lib/", path: "/project/lib", text: "/project/lib/", directory_only: true },
        Case { name: "nested parent directory with backslashes", containing_directory: "/project/src", module_name: "..\\lib\\", path: "/project/lib", text: "/project/lib/", directory_only: true },
        Case { name: "posix root", containing_directory: "/project", module_name: "..", path: "/", text: "/", directory_only: true },
        Case { name: "drive root", containing_directory: "c:/project", module_name: "..", path: "c:/", text: "c:/", directory_only: true },
        Case { name: "drive root without separator", containing_directory: "/project", module_name: "c:", path: "c:/", text: "c:/", directory_only: true },
        Case { name: "UNC root without separator", containing_directory: "/project", module_name: "//server", path: "//server/", text: "//server/", directory_only: true },
        Case { name: "URL root", containing_directory: "file:///project", module_name: "..", path: "file:///", text: "file:///", directory_only: true },
        Case { name: "URL authority root without separator", containing_directory: "/project", module_name: "file://server", path: "file://server/", text: "file://server/", directory_only: true },
        Case {
            name: "dynamic reserved prefix",
            containing_directory: "^/~ts-uri~/custom/ts-nul-authority/folder",
            module_name: "./~ts-uri-escape~file",
            path: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~",
            text: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~",
            directory_only: false,
        },
        Case {
            name: "dynamic reserved prefix with extension",
            containing_directory: "^/~ts-uri~/custom/ts-nul-authority/folder",
            module_name: "./~ts-uri-escape~file.ts",
            path: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~.ts",
            text: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~.ts",
            directory_only: false,
        },
        Case {
            name: "dynamic reserved directory prefix",
            containing_directory: "^/~ts-uri~/custom/ts-nul-authority/folder",
            module_name: "./~ts-uri-escape~dir/file",
            path: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e646972~/file",
            text: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e646972~/file",
            directory_only: false,
        },
        Case {
            name: "dynamic dotted directory intent",
            containing_directory: "^/~ts-uri~/custom/ts-nul-authority/folder",
            module_name: "./~ts-uri-escape~dir.js/",
            path: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e6469722e6a73~",
            text: "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e6469722e6a73~/",
            directory_only: true,
        },
    ];

    for test in tests {
        let candidate = normalize_path_for_cjs_resolution(
            &rooted_directory_path_from_normalized(test.containing_directory),
            test.module_name,
        );
        assert_candidate(&candidate, test.path, test.directory_only, test.text);
    }
}

#[test]
fn package_json_path_with_trailing_separator_does_not_resolve_as_file() {
    let fs = tsc_vfs::vfstest::from_map(
        [("/project/index.d.ts", "export {};")],
        CaseSensitivity::CaseSensitive,
    );
    let resolver = DefaultResolver::bare(new_resolution_data(&bare_options()), Arc::clone(&fs) as Arc<dyn tsc_vfs::Fs>);
    let mut state = ResolutionState::new_minimal(&resolver);
    state.compiler_options = Arc::new(CompilerOptions::default());

    let candidate = resolve_resolution_candidate("/project", "index.d.ts/");
    let result = state.load_file_name_from_package_json_field(
        Extensions::extensionsDeclaration,
        &candidate,
        "./index.d.ts/",
    );
    assert!(
        result.should_continue_searching(),
        "directory-only candidate resolved as {:?}",
        result.map(|r| r.path)
    );
}

fn bare_options() -> ResolverOptions {
    ResolverOptions {
        host: Arc::new(BareHost),
        compiler_options: Arc::new(CompilerOptions::default()),
        typings_location: RootedDirectoryPath::default(),
        project_name: String::new(),
        extra_extensions: Vec::new(),
        package_json_cache: None,
    }
}

// Go writes `&DefaultResolver{fs: fs}` with a zero ResolutionData; the port's
// DefaultResolver::bare needs *some* host to construct the default
// ResolutionData (the case-sensitivity of its package.json cache).
struct BareHost;

impl ResolutionHost for BareHost {
    fn fs(&self) -> Arc<dyn tsc_vfs::Fs> {
        tsc_vfs::vfstest::from_map([], CaseSensitivity::default())
    }
    fn get_current_directory(&self) -> RootedDirectoryPath {
        RootedDirectoryPath::default()
    }
}

#[test]
fn extension_replacement_preserves_empty_and_dot_stems() {
    struct Case {
        import_name: &'static str,
        expected: &'static str,
    }
    for test in [
        Case { import_name: ".js", expected: "/project/.native.ts" },
        Case { import_name: "..js", expected: "/project/..native.ts" },
        Case { import_name: "...js", expected: "/project/...native.ts" },
        Case { import_name: ".js", expected: "/project/.native.d.ts" },
    ] {
        let fs = tsc_vfs::vfstest::from_map(
            [(test.expected, "export {};")],
            CaseSensitivity::CaseSensitive,
        );
        let resolver = DefaultResolver::bare(
            new_resolution_data(&bare_options()),
            Arc::clone(&fs) as Arc<dyn tsc_vfs::Fs>,
        );
        let mut state = ResolutionState::new_minimal(&resolver);
        state.compiler_options = Arc::new(CompilerOptions {
            module_suffixes: vec![".native".to_string(), String::new()],
            ..Default::default()
        });
        let result = state.load_module_from_file_no_implicit_extensions(
            Extensions::extensionsTypeScript | Extensions::extensionsDeclaration,
            &resolve_resolution_candidate("/project", test.import_name),
        );
        let result = result
            .unwrap_or_else(|| panic!("got no result, expected {}", test.expected));
        assert_eq!(result.path, rooted_file_path_from_normalized(test.expected));
    }
}

#[test]
fn output_directories_remove_trailing_separators() {
    let fs =
        tsc_vfs::vfstest::from_map([], CaseSensitivity::CaseSensitive);
    let resolver = DefaultResolver::bare(
        new_resolution_data(&bare_options()),
        Arc::clone(&fs) as Arc<dyn tsc_vfs::Fs>,
    );
    let mut state = ResolutionState::new_minimal(&resolver);
    state.compiler_options = Arc::new(CompilerOptions {
        declaration_dir: rooted_directory_path_from_absolute("/project/types/"),
        out_dir: rooted_directory_path_from_absolute("/project/dist/"),
        ..Default::default()
    });

    let directories = state.get_output_directories();
    let expected = [
        rooted_directory_path_from_absolute("/project/types"),
        rooted_directory_path_from_absolute("/project/dist"),
    ];
    assert_eq!(directories.len(), expected.len());
    for (i, directory) in directories.iter().enumerate() {
        assert_eq!(directory, &expected[i], "directory {i}");
    }
    // Silence unused warnings for ported-but-unused imports.
    let _ = (
        ResolutionModeCommonJS,
        ResolutionModeESM,
        ModuleKind::ESNext,
        ModuleResolutionKind::Bundler,
        Tristate::TSTrue,
        RootedFilePath::default(),
        Entries::default(),
        test_resolver_options,
    );
}
