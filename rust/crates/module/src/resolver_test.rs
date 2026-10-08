// Ported from tsc/internal/module/resolver_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go test name mapping (Go name -> Rust test):
//   TestResolverSharedData -> SKIPPED (see note at the bottom of this file)
//   TestResolveModuleNameTrailingSlash -> resolve_module_name_trailing_slash
//   TestResolutionDataCaches -> resolution_data_caches
//   TestResolveDynamicModuleNameUsingRootDirs -> resolve_dynamic_module_name_using_root_dirs
//   TestRootDirsPreservesExceptionalDynamicSegments -> root_dirs_preserves_exceptional_dynamic_segments
//   TestRootDirsRejectsUnrepresentableDiskSegments -> root_dirs_rejects_unrepresentable_disk_segments
//   TestRootDirsEscapesRootLikeDynamicSegments -> root_dirs_escapes_root_like_dynamic_segments
//   TestRootDirsPreservesRelativeDiskSuffixes -> root_dirs_preserves_relative_disk_suffixes
//   TestRootDirsPreservesRelativeDirectoryIntent -> root_dirs_preserves_relative_directory_intent
//   TestResolveDynamicPackageJSONPath -> resolve_dynamic_package_json_path
//   TestResolveDynamicPackageSubpathFile -> resolve_dynamic_package_subpath_file
//   TestResolveDynamicESMPackageIndexFromReservedDirectory -> resolve_dynamic_esm_package_index_from_reserved_directory
//   TestResolveDynamicDottedDirectory -> resolve_dynamic_dotted_directory
//   TestResolvedModuleCarriesCaseInsensitivePath -> resolved_module_carries_case_insensitive_path
//   TestResolvedTypeReferenceDirectiveCarriesPath -> resolved_type_reference_directive_carries_path
//   TestResolvedEntrypointCarriesPath -> resolved_entrypoint_carries_path
//   TestDynamicExportPatternEntrypointUsesLogicalName -> dynamic_export_pattern_entrypoint_uses_logical_name
//   TestGeneratedDynamicEntrypointSpecifierResolvesEncodedFile -> generated_dynamic_entrypoint_specifier_resolves_encoded_file
//   TestDynamicEntrypointsEncodePhysicalPackageNameAndRootLikePath -> dynamic_entrypoints_encode_physical_package_name_and_root_like_path
//   TestDynamicExportPatternPreservesStaticSourcePrefix -> dynamic_export_pattern_preserves_static_source_prefix
//   TestDynamicExportPatternKeepsPackageForRootLikeMatch -> dynamic_export_pattern_keeps_package_for_root_like_match
//   TestDynamicEntrypointsPreserveCaseDistinctFiles -> dynamic_entrypoints_preserve_case_distinct_files
//   TestDiskExportPatternKeepsReservedPrefixLiteral -> disk_export_pattern_keeps_reserved_prefix_literal
//   TestDynamicExportPatternUnderExceptionalParent -> dynamic_export_pattern_under_exceptional_parent
//   TestDynamicExportPatternWithEmptyMatch -> dynamic_export_pattern_with_empty_match
//   TestUnaddressableEmptyExportPatternIsNotAnEntrypoint -> unaddressable_empty_export_pattern_is_not_an_entrypoint
//   TestResolverRejectsMismatchedPackageJsonCacheCaseSensitivity -> resolver_rejects_mismatched_package_json_cache_case_sensitivity
//   TestResolveModuleNameBlockedByNullExport -> resolve_module_name_blocked_by_null_export
//   TestResolveModuleNameExportTargetWithTrailingSlashDoesNotResolveAsFile -> resolve_module_name_export_target_with_trailing_slash_does_not_resolve_as_file
//   TestResolveModuleNameTrailingSlashRace -> resolve_module_name_trailing_slash_race
//   TestResolveSubpathNilContentsRace -> resolve_subpath_nil_contents_race
//   TestNodeModulePackageRoot -> node_module_package_root
//   TestResolvePeerDependencyNilContentsRace -> resolve_peer_dependency_nil_contents_race

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tsc_collections::OrderedMap;
use tsc_core::compileroptions::{
    ResolutionMode, ResolutionModeCommonJS, ResolutionModeESM, ResolutionModeNone,
};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind, ScriptTarget};
use tsc_core::tristate::Tristate;
use tsc_packagejson::{InfoCacheEntry, new_info_cache};
use tsc_tspath::{
    CaseSensitivity, EXTENSION_DTS, PathKey, RootedDirectoryPath, RootedFilePath, RootedPath,
};
use tsc_vfs::{Entries, FileInfo, Fs};

use crate::resolver::{
    DefaultResolver, ResolverOptions, new_resolver, new_resolver_with_options,
};
use crate::resolver_internal_test::ResolutionTestInputs;
use crate::types::{ResolutionHost, ResolvedModule};
use crate::util::{node_module_package_root_for_directory, node_module_package_root_for_file};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

fn test_fs(files: &[(&str, &str)]) -> Arc<dyn Fs> {
    tsc_vfs::vfstest::from_map(files.iter().copied(), CaseSensitivity::CaseSensitive)
}

fn test_host(fs: Arc<dyn Fs>, cwd: &str) -> Arc<dyn ResolutionHost> {
    Arc::new(ResolutionTestInputs {
        fs,
        cwd: RootedDirectoryPath::from(cwd),
    })
}

// opts mirrors the common `&core.CompilerOptions{ModuleResolution: Bundler,
// Module: ESNext, Target: ESNext}` literals of the Go tests.
fn bundler_opts(module: ModuleKind) -> CompilerOptions {
    CompilerOptions {
        module_resolution: ModuleResolutionKind::Bundler,
        module,
        target: ScriptTarget::ESNext,
        ..Default::default()
    }
}

fn new_test_resolver(
    host: Arc<dyn ResolutionHost>,
    compiler_options: CompilerOptions,
) -> DefaultResolver {
    new_resolver(ResolverOptions {
        host,
        compiler_options: Arc::new(compiler_options),
        typings_location: RootedDirectoryPath::default(),
        project_name: String::new(),
        extra_extensions: Vec::new(),
        package_json_cache: None,
    })
}

// resolve mirrors the tests' `resolver.ResolveModuleName(...)` calls and
// returns the (always present, possibly unresolved) result.
fn resolve_module(
    resolver: &DefaultResolver,
    module_name: &str,
    containing_file: &RootedFilePath,
    resolution_mode: ResolutionMode,
) -> Arc<ResolvedModule> {
    let (result, _diagnostics) = resolver.resolve_module_name(
        module_name,
        containing_file,
        resolution_mode,
        None,
    );
    result.expect("expected a resolution result (Go never returns nil here)")
}

// find_package_json mirrors the `PackageJsonCacheEntries` search pattern the
// entrypoint tests use to pick out the existing package.json cache entry.
fn find_package_json(resolver: &DefaultResolver) -> Option<Arc<InfoCacheEntry>> {
    let mut found: Option<Arc<InfoCacheEntry>> = None;
    resolver.package_json_cache_entries(|_key, entry| {
        if InfoCacheEntry::exists(Some(&entry)) {
            found = Some(entry);
            false
        } else {
            true
        }
    });
    found
}

// ---------------------------------------------------------------------------
// Concurrency primitives for the race regression tests
//
// PORT: Go uses close-once channels (`chan struct{}`) for gate and arrival
// signaling and `t.Context().Done()` for timeouts. The port expresses the
// same semantics with a `Mutex`/`Condvar` pair — `Gate::close` is the
// broadcast close, `Gate::wait_closed` is the timeout-guarded receive.
// ---------------------------------------------------------------------------

const TEST_TIMEOUT: Duration = Duration::from_secs(30);

struct Gate {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl Clone for Gate {
    fn clone(&self) -> Gate {
        Gate {
            state: Arc::clone(&self.state),
        }
    }
}

impl Gate {
    fn new() -> Gate {
        Gate {
            state: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    // close mirrors closing a Go channel: one-time, visible to every waiter.
    fn close(&self) {
        let mut closed = self.state.0.lock().unwrap();
        *closed = true;
        self.state.1.notify_all();
    }

    // wait blocks until the gate closes (no timeout; used from the FS hooks).
    fn wait(&self) {
        let mut closed = self.state.0.lock().unwrap();
        while !*closed {
            closed = self.state.1.wait(closed).unwrap();
        }
    }

    // wait_closed is the test-side `waitForSignal`: receive with a deadline,
    // converting a deadlock into a deterministic failure.
    fn wait_closed(&self, description: &str) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut closed = self.state.0.lock().unwrap();
        while !*closed {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for {description}");
            }
            let (guard, _timeout) = self
                .state
                .1
                .wait_timeout(closed, remaining)
                .unwrap();
            closed = guard;
        }
    }
}

struct Arrivals {
    state: Arc<(Mutex<u32>, Condvar)>,
}

impl Clone for Arrivals {
    fn clone(&self) -> Arrivals {
        Arrivals {
            state: Arc::clone(&self.state),
        }
    }
}

impl Arrivals {
    fn new() -> Arrivals {
        Arrivals {
            state: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    fn arrive(&self) {
        let mut count = self.state.0.lock().unwrap();
        *count += 1;
        self.state.1.notify_all();
    }

    // wait_for is the test-side wait for n signaled arrivals.
    fn wait_for(&self, n: u32, description: &str) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut count = self.state.0.lock().unwrap();
        while *count < n {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for {description}");
            }
            let (guard, _timeout) = self.state.1.wait_timeout(count, remaining).unwrap();
            count = guard;
        }
    }
}

// caseInsensitiveViewFS: Go embeds `vfs.FS` and overrides CaseSensitivity; the
// port writes out the delegation by hand.
struct CaseInsensitiveViewFs(Arc<dyn Fs>);

impl Fs for CaseInsensitiveViewFs {
    fn case_sensitivity(&self) -> CaseSensitivity {
        CaseSensitivity::CaseInsensitive
    }
    fn file_exists(&self, path: &RootedFilePath) -> bool {
        self.0.file_exists(path)
    }
    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        self.0.read_file(path)
    }
    fn write_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.0.write_file(path, data)
    }
    fn append_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.0.append_file(path, data)
    }
    fn remove(&self, path: &RootedPath) -> std::io::Result<()> {
        self.0.remove(path)
    }
    fn chtimes(
        &self,
        path: &RootedPath,
        atime: std::time::SystemTime,
        mtime: std::time::SystemTime,
    ) -> std::io::Result<()> {
        self.0.chtimes(path, atime, mtime)
    }
    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        self.0.directory_exists(path)
    }
    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        self.0.get_accessible_entries(path)
    }
    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        self.0.stat(path)
    }
    fn realpath(&self, path: &RootedPath) -> RootedPath {
        self.0.realpath(path)
    }
}

// blockingFS wraps a vfs.FS and forces FileExists calls for `targetPath` to
// block on `gate` until released. Each caller signals `arrivals` when it
// reaches the gate (Go: a send on a buffered channel).
struct BlockingFs {
    fs: Arc<dyn Fs>,
    target_path: RootedFilePath,
    gate: Gate,
    arrivals: Arrivals,
}

impl Fs for BlockingFs {
    fn case_sensitivity(&self) -> CaseSensitivity {
        self.fs.case_sensitivity()
    }
    fn file_exists(&self, path: &RootedFilePath) -> bool {
        if path == &self.target_path {
            self.arrivals.arrive();
            self.gate.wait();
        }
        self.fs.file_exists(path)
    }
    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        self.fs.read_file(path)
    }
    fn write_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.fs.write_file(path, data)
    }
    fn append_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.fs.append_file(path, data)
    }
    fn remove(&self, path: &RootedPath) -> std::io::Result<()> {
        self.fs.remove(path)
    }
    fn chtimes(
        &self,
        path: &RootedPath,
        atime: std::time::SystemTime,
        mtime: std::time::SystemTime,
    ) -> std::io::Result<()> {
        self.fs.chtimes(path, atime, mtime)
    }
    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        self.fs.directory_exists(path)
    }
    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        self.fs.get_accessible_entries(path)
    }
    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        self.fs.stat(path)
    }
    fn realpath(&self, path: &RootedPath) -> RootedPath {
        self.fs.realpath(path)
    }
}

// flipFileExistsFS wraps a vfs.FS and returns false for the first FileExists
// call to `targetPath`, then true for the second. Both calls signal arrival
// via their gate then block until released. ReadFile for the target path also
// signals arrival then blocks (Go: close-once channels).
struct FlipFileExistsFs {
    fs: Arc<dyn Fs>,
    target_path: RootedFilePath,
    call_count: AtomicI32,
    first_arrived: Gate,
    second_arrived: Gate,
    first_gate: Gate,
    second_gate: Gate,
    read_arrived: Gate,
    read_gate: Gate,
}

impl Fs for FlipFileExistsFs {
    fn case_sensitivity(&self) -> CaseSensitivity {
        self.fs.case_sensitivity()
    }
    fn file_exists(&self, path: &RootedFilePath) -> bool {
        if path == &self.target_path {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                self.first_arrived.close();
                self.first_gate.wait();
                return false; // first caller: simulate "file not yet visible"
            }
            if n == 2 {
                self.second_arrived.close();
                self.second_gate.wait();
                return self.fs.file_exists(path); // second caller: file is visible
            }
        }
        self.fs.file_exists(path)
    }
    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        if path == &self.target_path {
            self.read_arrived.close();
            self.read_gate.wait();
        }
        self.fs.read_file(path)
    }
    fn write_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.fs.write_file(path, data)
    }
    fn append_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.fs.append_file(path, data)
    }
    fn remove(&self, path: &RootedPath) -> std::io::Result<()> {
        self.fs.remove(path)
    }
    fn chtimes(
        &self,
        path: &RootedPath,
        atime: std::time::SystemTime,
        mtime: std::time::SystemTime,
    ) -> std::io::Result<()> {
        self.fs.chtimes(path, atime, mtime)
    }
    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        self.fs.directory_exists(path)
    }
    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        self.fs.get_accessible_entries(path)
    }
    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        self.fs.stat(path)
    }
    fn realpath(&self, path: &RootedPath) -> RootedPath {
        self.fs.realpath(path)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// Regression test for https://github.com/microsoft/TypeScript/tsc/issues/3526.
//
// Resolving a node_modules import with a trailing slash (e.g. `pkg/`) must
// produce the same result as without one.
#[test]
fn resolve_module_name_trailing_slash() {
    let fs = test_fs(&[
        ("/repo/node_modules/pkg/package.json", r#"{"name":"pkg","main":"main.js","types":"main.d.ts"}"#),
        ("/repo/node_modules/pkg/main.d.ts", "export const x: number;"),
        ("/repo/node_modules/pkg/main.js", "exports.x = 1;"),
        ("/repo/src/file.ts", ""),
    ]);
    let host = test_host(fs, "/repo");
    let opts = CompilerOptions {
        trace_resolution: Tristate::TSTrue,
        ..bundler_opts(ModuleKind::ESNext)
    };
    let resolver = new_test_resolver(host, opts);

    for name in ["pkg", "pkg/"] {
        let resolved = resolve_module(
            &resolver,
            name,
            &RootedFilePath::from("/repo/src/file.ts"),
            ResolutionModeESM,
        );
        assert!(
            ResolvedModule::is_resolved(Some(&resolved)),
            "{name:?} failed to resolve"
        );
        let expected = CaseSensitivity::CaseSensitive
            .path_key(&RootedPath::from(resolved.resolved_file_name.as_string()));
        assert_eq!(
            resolved.resolved_path, expected,
            "{name:?} resolved path mismatch"
        );
    }
}

#[test]
fn resolution_data_caches() {
    let old_host = test_host(
        test_fs(&[
            ("/src/node_modules/pkg/package.json", r#"{"name":"pkg","types":"index.d.ts"}"#),
            ("/src/node_modules/pkg/index.d.ts", "export const value: number;"),
        ]),
        "/",
    );
    let new_host = test_host(
        test_fs(&[
            ("/src/node_modules/pkg/package.json", r#"{"name":"pkg","types":"index.d.ts"}"#),
            ("/src/node_modules/pkg/index.d.ts", "export const value: number;"),
            ("/new/package.json", r#"{"name":"new"}"#),
            ("/missing-first/package.json", r#"{"name":"missing-first"}"#),
        ]),
        "/",
    );
    let resolver = new_test_resolver(
        old_host.clone(),
        CompilerOptions {
            module: ModuleKind::NodeNext,
            ..Default::default()
        },
    );
    let resolved = resolve_module(
        &resolver,
        "pkg",
        &RootedFilePath::from("/src/index.ts"),
        ResolutionModeCommonJS,
    );
    assert!(ResolvedModule::is_resolved(Some(&resolved)));
    let cached = resolve_module(
        &resolver,
        "pkg",
        &RootedFilePath::from("/src/index.ts"),
        ResolutionModeCommonJS,
    );
    assert!(Arc::ptr_eq(&cached, &resolved));

    let _ = resolver.resolve_type_reference_directive(
        "missing",
        &RootedFilePath::from("/src/index.ts"),
        ResolutionModeCommonJS,
        None,
    );
    let mut paths = OrderedMap::with_size_hint(1);
    paths.set("alias/*".to_string(), vec!["./*".to_string()]);
    resolver.get_parsed_patterns_for_paths(&CompilerOptions {
        paths: Some(paths),
        ..Default::default()
    });
    assert_eq!(resolver.module_resolution_cache.cache.size(), 1);
    assert_eq!(resolver.type_ref_directive_resolution_cache.cache.size(), 1);
    assert_eq!(resolver.parsed_patterns_for_paths.cache.size(), 1);

    let data = resolver.get_resolution_data();
    let rebound = data.new_resolver(old_host);
    assert_eq!(rebound.module_resolution_cache.cache.size(), 0);
    assert_eq!(rebound.type_ref_directive_resolution_cache.cache.size(), 0);
    assert_eq!(rebound.parsed_patterns_for_paths.cache.size(), 0);

    let clone = data.cloned().new_resolver(new_host);
    assert_eq!(clone.module_resolution_cache.cache.size(), 0);
    assert_eq!(clone.type_ref_directive_resolution_cache.cache.size(), 0);
    assert_eq!(clone.parsed_patterns_for_paths.cache.size(), 0);
    let pkg_directory = RootedDirectoryPath::from("/src/node_modules/pkg");
    let scope_from_clone = clone.get_package_scope_for_path(&pkg_directory);
    let scope_from_resolver = resolver.get_package_scope_for_path(&pkg_directory);
    // Go compares the entries with assert.Equal (deep equality); the clone
    // shares the cached entry Arcs, so pointer equality is equivalent here.
    assert!(matches!(
        (&scope_from_clone, &scope_from_resolver),
            (Some(a), Some(b)) if Arc::ptr_eq(a, b)
    ));
    assert!(InfoCacheEntry::exists(clone.get_package_scope_for_path(&RootedDirectoryPath::from("/new")).as_deref()));
    assert!(!InfoCacheEntry::exists(
        resolver.get_package_scope_for_path(&RootedDirectoryPath::from("/new")).as_deref()
    ));
    assert!(!InfoCacheEntry::exists(
        resolver
            .get_package_scope_for_path(&RootedDirectoryPath::from("/missing-first"))
            .as_deref()
    ));
    assert!(InfoCacheEntry::exists(
        clone
            .get_package_scope_for_path(&RootedDirectoryPath::from("/missing-first"))
            .as_deref()
    ));
}

#[test]
fn resolve_dynamic_module_name_using_root_dirs() {
    struct Case {
        // Go uses this only for the subtest name.
        #[allow(dead_code)]
        name: &'static str,
        target_root: &'static str,
        target_file: &'static str,
    }
    for test in [
        Case {
            name: "dynamic roots",
            target_root: "^/~ts-uri~/custom/ts-nul-authority/generated",
            target_file: "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~.ts",
        },
        Case {
            name: "dynamic to disk",
            target_root: "c:/generated",
            target_file: "c:/generated/~ts-uri-escape~file.ts",
        },
    ] {
        let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
        let fs = test_fs(&[
            (source_file, ""),
            (test.target_file, "export const value = 1;"),
        ]);
        let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
        let opts = CompilerOptions {
            root_dirs: Some(vec![
                RootedDirectoryPath::from("^/~ts-uri~/custom/ts-nul-authority/src"),
                RootedDirectoryPath::from(test.target_root),
            ]),
            ..bundler_opts(ModuleKind::ESNext)
        };
        let resolver = new_test_resolver(host, opts);
        let resolved = resolve_module(
            &resolver,
            "./~ts-uri-escape~file",
            &RootedFilePath::from(source_file),
            ResolutionModeESM,
        );
        assert!(
            ResolvedModule::is_resolved(Some(&resolved)),
            "expected dynamic module to resolve through rootDirs"
        );
        assert_eq!(resolved.resolved_file_name, RootedFilePath::from(test.target_file));
    }
}

#[test]
fn root_dirs_preserves_exceptional_dynamic_segments() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~2e2e~/main.ts";
    let target_file = "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~2e2e~/dep.ts";
    let fs = test_fs(&[
        (source_file, ""),
        (target_file, "export const value = 1;"),
        ("^/~ts-uri~/custom/ts-nul-authority/dep.ts", "export const wrong = 1;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let opts = CompilerOptions {
        root_dirs: Some(vec![
            RootedDirectoryPath::from("^/~ts-uri~/custom/ts-nul-authority/src"),
            RootedDirectoryPath::from("^/~ts-uri~/custom/ts-nul-authority/generated"),
        ]),
        ..bundler_opts(ModuleKind::ESNext)
    };
    let resolver = new_test_resolver(host, opts);
    let resolved = resolve_module(
        &resolver,
        "./dep",
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved))
            && resolved.resolved_file_name == RootedFilePath::from(target_file),
        "resolved file = {:?}, expected {target_file:?}",
        resolved.resolved_file_name
    );
}

#[test]
fn root_dirs_rejects_unrepresentable_disk_segments() {
    for source_file in [
        "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~2e2e~/main.ts",
        "^/~ts-uri~/custom/ts-nul-authority/src/c:/main.ts",
        "^/~ts-uri~/custom/ts-nul-authority/src/^/main.ts",
    ] {
        let fs = test_fs(&[(source_file, ""), ("c:/dep.ts", "export const wrong = 1;")]);
        let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
        let opts = CompilerOptions {
            root_dirs: Some(vec![
                RootedDirectoryPath::from("^/~ts-uri~/custom/ts-nul-authority/src"),
                RootedDirectoryPath::from("c:/generated"),
            ]),
            ..bundler_opts(ModuleKind::ESNext)
        };
        let resolver = new_test_resolver(host, opts);
        let resolved = resolve_module(
            &resolver,
            "./dep",
            &RootedFilePath::from(source_file),
            ResolutionModeESM,
        );
        assert!(
            !ResolvedModule::is_resolved(Some(&resolved)),
            "unexpectedly resolved unrepresentable disk path to {:?}",
            resolved.resolved_file_name
        );
    }
}

#[test]
fn root_dirs_escapes_root_like_dynamic_segments() {
    struct Case {
        source_file: &'static str,
        target_file: &'static str,
    }
    for test in [
        Case {
            source_file: "^/~ts-uri~/custom/ts-nul-authority/src/c:/main.ts",
            target_file: "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~633a~/dep.ts",
        },
        Case {
            source_file: "^/~ts-uri~/custom/ts-nul-authority/src/^/main.ts",
            target_file: "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~5e~/dep.ts",
        },
    ] {
        let fs = test_fs(&[
            (test.source_file, ""),
            (test.target_file, "export const value = 1;"),
            ("c:/dep.ts", "export const wrong = 1;"),
            ("^/dep.ts", "export const wrong = 1;"),
        ]);
        let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
        let opts = CompilerOptions {
            root_dirs: Some(vec![
                RootedDirectoryPath::from("^/~ts-uri~/custom/ts-nul-authority/src"),
                RootedDirectoryPath::from("^/~ts-uri~/custom/ts-nul-authority/generated"),
            ]),
            ..bundler_opts(ModuleKind::ESNext)
        };
        let resolver = new_test_resolver(host, opts);
        let resolved = resolve_module(
            &resolver,
            "./dep",
            &RootedFilePath::from(test.source_file),
            ResolutionModeESM,
        );
        assert!(
            ResolvedModule::is_resolved(Some(&resolved))
                && resolved.resolved_file_name == RootedFilePath::from(test.target_file),
            "resolved file = {:?}, expected {:?}",
            resolved.resolved_file_name,
            test.target_file
        );
    }
}

#[test]
fn root_dirs_preserves_relative_disk_suffixes() {
    struct Case {
        source_file: &'static str,
        target_root: &'static str,
        target_file: &'static str,
    }
    for test in [
        Case {
            source_file: "c:/src/^/main.ts",
            target_root: "^/~ts-uri~/custom/ts-nul-authority/generated",
            target_file: "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~5e~/dep.ts",
        },
        Case {
            source_file: "c:/src/c:/main.ts",
            target_root: "d:/generated",
            target_file: "d:/generated/c:/dep.ts",
        },
    ] {
        let fs = test_fs(&[
            (test.source_file, ""),
            (test.target_file, "export const value = 1;"),
            ("^/dep.ts", "export const wrong = 1;"),
            ("c:/dep.ts", "export const wrong = 1;"),
        ]);
        let host = test_host(fs, "c:/");
        let opts = CompilerOptions {
            root_dirs: Some(vec![
                RootedDirectoryPath::from("c:/src"),
                RootedDirectoryPath::from(test.target_root),
            ]),
            ..bundler_opts(ModuleKind::ESNext)
        };
        let resolver = new_test_resolver(host, opts);
        let resolved = resolve_module(
            &resolver,
            "./dep",
            &RootedFilePath::from(test.source_file),
            ResolutionModeESM,
        );
        assert!(
            ResolvedModule::is_resolved(Some(&resolved))
                && resolved.resolved_file_name == RootedFilePath::from(test.target_file),
            "resolved file = {:?}, expected {:?}",
            resolved.resolved_file_name,
            test.target_file
        );
    }
}

#[test]
fn root_dirs_preserves_relative_directory_intent() {
    struct Case {
        name: &'static str,
        source_root: &'static str,
        source_file: &'static str,
        target_root: &'static str,
        target_file: &'static str,
    }
    for test in [
        Case { name: "disk to disk", source_root: "c:/src", source_file: "c:/src/^/main.ts", target_root: "d:/generated", target_file: "d:/generated/^/pkg/index.ts" },
        Case { name: "disk to dynamic", source_root: "c:/src", source_file: "c:/src/^/main.ts", target_root: "^/~ts-uri~/custom/ts-nul-authority/generated", target_file: "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~5e~/pkg/index.ts" },
        Case { name: "empty suffix to dynamic", source_root: "c:/src", source_file: "c:/src/main.ts", target_root: "^/~ts-uri~/custom/ts-nul-authority/generated", target_file: "^/~ts-uri~/custom/ts-nul-authority/generated/index.ts" },
        Case { name: "empty suffix to disk", source_root: "^/~ts-uri~/custom/ts-nul-authority/src", source_file: "^/~ts-uri~/custom/ts-nul-authority/src/main.ts", target_root: "d:/generated", target_file: "d:/generated/index.ts" },
        Case { name: "literal empty segment to dynamic", source_root: "^/~ts-uri~/custom/ts-nul-authority/src", source_file: "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~~/main.ts", target_root: "^/~ts-uri~/custom/ts-nul-authority/generated", target_file: "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~~/index.ts" },
    ] {
        let fs = test_fs(&[
            (test.source_file, ""),
            (test.target_file, "export const value = 1;"),
            ("^/pkg/index.ts", "export const wrong = 1;"),
            ("d:/generated.ts", "export const wrong = 1;"),
        ]);
        let host = test_host(fs, "c:/");
        let opts = CompilerOptions {
            root_dirs: Some(vec![
                RootedDirectoryPath::from(test.source_root),
                RootedDirectoryPath::from(test.target_root),
            ]),
            ..bundler_opts(ModuleKind::CommonJS)
        };

        let mut module_name = "./pkg/";
        if test.name.contains("empty suffix") || test.name == "literal empty segment to dynamic" {
            module_name = "./";
        }
        let resolver = new_test_resolver(host, opts);
        let resolved = resolve_module(
            &resolver,
            module_name,
            &RootedFilePath::from(test.source_file),
            ResolutionModeCommonJS,
        );
        assert!(
            ResolvedModule::is_resolved(Some(&resolved))
                && resolved.resolved_file_name == RootedFilePath::from(test.target_file),
            "resolved file = {:?}, expected {:?}",
            resolved.resolved_file_name,
            test.target_file
        );
    }
}

#[test]
fn resolve_dynamic_package_json_path() {
    let fallback_file = "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/~ts-uri-escape~7e74732d7572692d6573636170657e7479706573~.d.ts";
    let target_file = "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/ts3.1/~ts-uri-escape~7e74732d7572692d6573636170657e7479706573~.d.ts";
    let fs = test_fs(&[
        ("^/~ts-uri~/custom/ts-nul-authority/src/main.ts", ""),
        (
            "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/package.json",
            r#"{"name":"pkg","types":"~ts-uri-escape~types.d.ts","typesVersions":{"*":{"*":["ts3.1/*"]}}}"#,
        ),
        (fallback_file, "export const fallback: number;"),
        (target_file, "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));
    let resolved = resolve_module(
        &resolver,
        "pkg",
        &RootedFilePath::from("^/~ts-uri~/custom/ts-nul-authority/src/main.ts"),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected dynamic package path to resolve"
    );
    assert_eq!(resolved.resolved_file_name, RootedFilePath::from(target_file));
    assert!(
        resolved.extension == EXTENSION_DTS && resolved.resolved_file_name.is_declaration_file(),
        "resolved declaration was classified as extension {:?}",
        resolved.extension
    );
}

#[test]
fn resolve_dynamic_package_subpath_file() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let target_file = "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/~ts-uri-escape~7e74732d7572692d6573636170657e3636366636667e~.ts";
    let fs = test_fs(&[
        (source_file, ""),
        ("^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/package.json", r#"{"name":"pkg"}"#),
        (target_file, "export const value = 1;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));
    let resolved = resolve_module(
        &resolver,
        "pkg/~ts-uri-escape~666f6f~.ts",
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved))
            && resolved.resolved_file_name == RootedFilePath::from(target_file),
        "resolved file = {:?}, expected {target_file:?}",
        resolved.resolved_file_name
    );
}

#[test]
fn resolve_dynamic_esm_package_index_from_reserved_directory() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let package_name = "~ts-uri-escape~pkg.js";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/node_modules/~ts-uri-escape~7e74732d7572692d6573636170657e706b672e6a73~";
    let target_file = format!("{package_directory}/index.js");
    let fs = test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            format!(r#"{{"name":"{package_name}"}}"#).as_str(),
        ),
        (target_file.as_str(), "exports.value = 1;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));
    let resolved = resolve_module(
        &resolver,
        package_name,
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved))
            && resolved.resolved_file_name == RootedFilePath::from(target_file.as_str()),
        "resolved file = {:?}, expected {target_file:?}",
        resolved.resolved_file_name
    );
}

#[test]
fn resolve_dynamic_dotted_directory() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let target_file = "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~7e74732d7572692d6573636170657e6469722e6a73~/index.ts";
    let fs = test_fs(&[
        (source_file, ""),
        (target_file, "export const value = 1;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::CommonJS));
    let resolved = resolve_module(
        &resolver,
        "./~ts-uri-escape~dir.js",
        &RootedFilePath::from(source_file),
        ResolutionModeCommonJS,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected dotted dynamic directory to resolve"
    );
    assert_eq!(resolved.resolved_file_name, RootedFilePath::from(target_file));
}

#[test]
fn resolved_module_carries_case_insensitive_path() {
    let fs = tsc_vfs::vfstest::from_map(
        [
            ("/Repo/node_modules/Pkg/package.json", r#"{"name":"pkg","types":"Index.d.ts"}"#),
            ("/Repo/node_modules/Pkg/Index.d.ts", "export const x: number;"),
            ("/Repo/src/file.ts", ""),
        ],
        CaseSensitivity::CaseInsensitive,
    );
    let host = test_host(fs, "/Repo");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        "Pkg",
        &RootedFilePath::from("/Repo/src/file.ts"),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected module to resolve"
    );
    let assert_path = CaseSensitivity::CaseInsensitive
        .path_key(&RootedPath::from(resolved.resolved_file_name.as_string()));
    assert_eq!(resolved.resolved_path, assert_path);
}

#[test]
fn resolved_type_reference_directive_carries_path() {
    let fs = tsc_vfs::vfstest::from_map(
        [
            ("/repo/node_modules/@types/pkg/package.json", r#"{"name":"@types/pkg","types":"index.d.ts"}"#),
            ("/repo/node_modules/@types/pkg/index.d.ts", "export const x: number;"),
            ("/repo/src/file.ts", ""),
        ],
        CaseSensitivity::CaseInsensitive,
    );
    let host = test_host(fs, "/repo");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let (resolved, _diagnostics) = resolver.resolve_type_reference_directive(
        "pkg",
        &RootedFilePath::from("/repo/src/file.ts"),
        ResolutionModeNone,
        None,
    );
    let resolved = resolved.expect("expected a type reference directive result");
    assert!(resolved.is_resolved(), "expected type reference directive to resolve");
    let expected = CaseSensitivity::CaseInsensitive
        .path_key(&RootedPath::from(resolved.resolved_file_name.as_string()));
    assert_eq!(resolved.resolved_path, expected);
}

#[test]
fn resolved_entrypoint_carries_path() {
    let fs = tsc_vfs::vfstest::from_map(
        [
            ("/Repo/node_modules/Pkg/package.json", r#"{"name":"pkg","types":"Index.d.ts"}"#),
            ("/Repo/node_modules/Pkg/Index.d.ts", "export const x: number;"),
            ("/Repo/src/file.ts", ""),
        ],
        CaseSensitivity::CaseInsensitive,
    );
    let host = test_host(fs, "/Repo");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        "Pkg",
        &RootedFilePath::from("/Repo/src/file.ts"),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected module to resolve"
    );
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", false);
    assert_eq!(entrypoints.len(), 1, "entrypoint count");
    let expected = CaseSensitivity::CaseInsensitive
        .path_key(&RootedPath::from(entrypoints[0].resolved_file_name.as_string()));
    assert_eq!(entrypoints[0].resolved_path, expected);
}

#[test]
fn dynamic_export_pattern_entrypoint_uses_logical_name() {
    struct Case {
        name: &'static str,
        package_json: &'static str,
        import_name: &'static str,
        module_specifier: &'static str,
    }
    for test in [
        Case {
            name: "logical wildcard match",
            package_json: r#"{"name":"Pkg","exports":{"./*":"./*.d.ts"}}"#,
            import_name: "Pkg/~ts-uri-escape~value",
            module_specifier: "Pkg/~ts-uri-spec~7e74732d7572692d6573636170657e76616c7565~",
        },
        Case {
            name: "reserved target pattern",
            package_json: r#"{"name":"Pkg","exports":{"./*":"./~ts-uri-escape~*.d.ts"}}"#,
            import_name: "Pkg/value",
            module_specifier: "Pkg/value",
        },
        Case {
            name: "reserved wildcard with trailer",
            package_json: r#"{"name":"Pkg","exports":{"./*":"./*-impl.d.ts"}}"#,
            import_name: "Pkg/~ts-uri-escape~value",
            module_specifier: "Pkg/~ts-uri-spec~7e74732d7572692d6573636170657e76616c7565~",
        },
    ] {
        let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
        let target_file = if test.name == "reserved wildcard with trailer" {
            "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/~ts-uri-escape~7e74732d7572692d6573636170657e76616c75652d696d706c~.d.ts"
        } else {
            "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/~ts-uri-escape~7e74732d7572692d6573636170657e76616c7565~.d.ts"
        };
        let fs = test_fs(&[
            (source_file, ""),
            ("^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/package.json", test.package_json),
            (target_file, "export const value: number;"),
        ]);
        let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
        let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

        let resolved = resolve_module(
            &resolver,
            test.import_name,
            &RootedFilePath::from(source_file),
            ResolutionModeESM,
        );
        assert!(
            ResolvedModule::is_resolved(Some(&resolved)),
            "expected dynamic package export to resolve"
        );
        let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");

        let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", false);
        assert_eq!(entrypoints.len(), 1, "entrypoint count");
        assert_eq!(
            entrypoints[0].module_specifier.as_string(),
            test.module_specifier,
            "module specifier"
        );
    }
}

#[test]
fn generated_dynamic_entrypoint_specifier_resolves_encoded_file() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let package_file = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/~ts-uri-escape~7e74732d7572692d6573636170657e76616c7565~.d.ts";
    let module_specifier = "Pkg/~ts-uri-spec~7e74732d7572692d6573636170657e76616c7565~.d.ts";
    let fs = test_fs(&[
        (source_file, ""),
        ("^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/package.json", r#"{"name":"Pkg"}"#),
        (package_file, "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let _ = resolve_module(&resolver, "Pkg", &RootedFilePath::from(source_file), ResolutionModeESM);
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", true);
    assert!(
        entrypoints.len() == 1
            && entrypoints[0].module_specifier.as_string() == module_specifier,
        "entrypoints = {:?}, expected {module_specifier:?}",
        entrypoints
    );

    let resolved = resolve_module(
        &resolver,
        module_specifier,
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved))
            && resolved.resolved_file_name == RootedFilePath::from(package_file),
        "resolved file = {:?}, expected {package_file:?}",
        resolved.resolved_file_name
    );
}

#[test]
fn dynamic_entrypoints_encode_physical_package_name_and_root_like_path() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let physical_package_name = "~ts-uri-escape~7e74732d7572692d6573636170657e706b672e6a73~";
    let source_package_name = "~ts-uri-spec~7e74732d7572692d6573636170657e706b672e6a73~";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/node_modules/";
    let package_directory = format!("{package_directory}{physical_package_name}");
    let target_file = format!("{package_directory}/~ts-uri-escape~633a~/value.d.ts");
    let module_specifier = format!("{source_package_name}/~ts-uri-spec~633a~/value.d.ts");
    let fs = test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            r#"{"name":"~ts-uri-escape~pkg.js"}"#,
        ),
        (target_file.as_str(), "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let _ = resolve_module(
        &resolver,
        source_package_name,
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(
        &package_json,
        physical_package_name,
        true,
    );
    assert!(
        entrypoints.len() == 1
            && entrypoints[0].module_specifier.as_string() == module_specifier,
        "entrypoints = {:?}, expected {module_specifier:?}",
        entrypoints
    );
    let resolved = resolve_module(
        &resolver,
        module_specifier.as_str(),
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved))
            && resolved.resolved_file_name == RootedFilePath::from(target_file.as_str()),
        "generated module specifier resolved to {:?}, expected {target_file:?}",
        resolved.resolved_file_name
    );
}

#[test]
fn dynamic_export_pattern_preserves_static_source_prefix() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let target_file = format!("{package_directory}/foo.d.ts");
    let module_specifier = "Pkg/~ts-uri-escape~prefix/foo";
    let fs = test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            r#"{"name":"Pkg","exports":{"./~ts-uri-escape~prefix/*":"./*.d.ts"}}"#,
        ),
        (target_file.as_str(), "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        module_specifier,
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected static reserved-prefix export to resolve"
    );
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", false);
    assert!(
        entrypoints.len() == 1
            && entrypoints[0].module_specifier.as_string() == module_specifier,
        "entrypoints = {:?}, expected {module_specifier:?}",
        entrypoints
    );
}

#[test]
fn dynamic_export_pattern_keeps_package_for_root_like_match() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let target_file = format!("{package_directory}/~ts-uri-escape~633a~/value.d.ts");
    let module_specifier = "Pkg/~ts-uri-spec~633a~/value";
    let fs = test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            r#"{"name":"Pkg","exports":{"./*":"./*.d.ts"}}"#,
        ),
        (target_file.as_str(), "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        module_specifier,
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected root-like dynamic export to resolve"
    );
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", false);
    assert!(
        entrypoints.len() == 1
            && entrypoints[0].module_specifier.as_string() == module_specifier,
        "entrypoints = {:?}, expected {module_specifier:?}",
        entrypoints
    );
}

#[test]
fn dynamic_entrypoints_preserve_case_distinct_files() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let fs = CaseInsensitiveViewFs(test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            r#"{"name":"Pkg","types":"Foo.d.ts"}"#,
        ),
        (format!("{package_directory}/Foo.d.ts").as_str(), "export const upper: number;"),
        (format!("{package_directory}/foo.d.ts").as_str(), "export const lower: number;"),
    ]));
    let host = test_host(Arc::new(fs), "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let _ = resolve_module(&resolver, "Pkg", &RootedFilePath::from(source_file), ResolutionModeESM);
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", true);
    assert_eq!(entrypoints.len(), 2, "entrypoint count");
}

#[test]
fn disk_export_pattern_keeps_reserved_prefix_literal() {
    let target_file = "/repo/node_modules/pkg/~ts-uri-escape~666f6f~.d.ts";
    let fs = test_fs(&[
        ("/repo/src/main.ts", ""),
        ("/repo/node_modules/pkg/package.json", r#"{"name":"pkg","exports":{"./*":"./*.d.ts"}}"#),
        (target_file, "export const value: number;"),
    ]);
    let host = test_host(fs, "/repo");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        "pkg/~ts-uri-escape~666f6f~",
        &RootedFilePath::from("/repo/src/main.ts"),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected disk package export to resolve"
    );
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "pkg", false);
    assert!(
        entrypoints.len() == 1
            && entrypoints[0].module_specifier.as_string() == "pkg/~ts-uri-escape~666f6f~",
        "entrypoints = {:?}, expected literal reserved-prefix entrypoint",
        entrypoints
    );
}

#[test]
fn dynamic_export_pattern_under_exceptional_parent() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/work/~ts-uri-escape~2e2e~/src/main.ts";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/work/~ts-uri-escape~2e2e~/node_modules/Pkg";
    let target_file = format!("{package_directory}/foo.d.ts");
    let fs = test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            r#"{"name":"Pkg","exports":{"./*":"./*.d.ts"}}"#,
        ),
        (target_file.as_str(), "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        "Pkg/foo",
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected dynamic package export to resolve"
    );
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", false);
    assert!(
        entrypoints.len() == 1
            && entrypoints[0].module_specifier.as_string() == "Pkg/foo",
        "entrypoints = {:?}, expected logical Pkg/foo entrypoint",
        entrypoints
    );
}

#[test]
fn dynamic_export_pattern_with_empty_match() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let target_file = format!("{package_directory}/index.d.ts");
    let fs = test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            r#"{"name":"Pkg","exports":{"./foo/*":"./index*.d.ts"}}"#,
        ),
        (target_file.as_str(), "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        "Pkg/foo/",
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved)),
        "expected empty wildcard package export to resolve"
    );
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", false);
    assert_eq!(entrypoints.len(), 1, "entrypoint count");
    let generated = entrypoints[0].module_specifier.as_string().to_string();
    let resolved = resolve_module(
        &resolver,
        generated.as_str(),
        &RootedFilePath::from(source_file),
        ResolutionModeESM,
    );
    assert!(
        ResolvedModule::is_resolved(Some(&resolved))
            && resolved.resolved_file_name == RootedFilePath::from(target_file.as_str()),
        "generated module specifier {generated:?} resolved to {:?}, expected {target_file:?}",
        resolved.resolved_file_name
    );
}

#[test]
fn unaddressable_empty_export_pattern_is_not_an_entrypoint() {
    let source_file = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    let package_directory = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let fs = test_fs(&[
        (source_file, ""),
        (
            format!("{package_directory}/package.json").as_str(),
            r#"{"name":"Pkg","exports":{"./*":"./index*.d.ts"}}"#,
        ),
        (format!("{package_directory}/index.d.ts").as_str(), "export const value: number;"),
    ]);
    let host = test_host(fs, "^/~ts-uri~/custom/ts-nul-authority/");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let _ = resolve_module(&resolver, "Pkg", &RootedFilePath::from(source_file), ResolutionModeESM);
    let package_json = find_package_json(&resolver).expect("expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", false);
    assert!(
        entrypoints.is_empty(),
        "entrypoints = {:?}, expected unaddressable empty wildcard match to be omitted",
        entrypoints
    );
}

#[test]
fn resolver_rejects_mismatched_package_json_cache_case_sensitivity() {
    let fs = test_fs(&[]);
    let host = test_host(fs, "/repo");
    // Go expects NewResolverWithOptions to panic when the supplied package
    // JSON cache's case sensitivity differs from the host filesystem's.
    let result = catch_unwind(AssertUnwindSafe(|| {
        new_resolver_with_options(
            host,
            Arc::new(CompilerOptions::default()),
            RootedDirectoryPath::default(),
            String::new(),
            ResolverOptions {
                host: Arc::new(NoopHost),
                compiler_options: Arc::new(CompilerOptions::default()),
                typings_location: RootedDirectoryPath::default(),
                project_name: String::new(),
                extra_extensions: Vec::new(),
                package_json_cache: Some(new_info_cache(CaseSensitivity::CaseInsensitive)),
            },
        )
    }));
    assert!(
        result.is_err(),
        "expected mismatched package JSON cache case sensitivity to panic"
    );
}

// The ResolverOptions passed to NewResolverWithOptions is mostly overridden by
// the function; Go's literal embeds a nil host. `Arc<dyn ResolutionHost>` has
// no nil, so the port uses a stub that is never consulted.
struct NoopHost;

impl ResolutionHost for NoopHost {
    fn fs(&self) -> Arc<dyn Fs> {
        unreachable!("NoopHost.fs is never consulted")
    }
    fn get_current_directory(&self) -> RootedDirectoryPath {
        unreachable!("NoopHost.get_current_directory is never consulted")
    }
}

#[test]
fn resolve_module_name_blocked_by_null_export() {
    let fs = test_fs(&[
        ("/repo/node_modules/pkg/package.json", r#"{"name":"pkg","exports":{".":null}}"#),
        ("/repo/src/file.ts", ""),
    ]);
    let host = test_host(fs, "/repo");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        "pkg",
        &RootedFilePath::from("/repo/src/file.ts"),
        ResolutionModeESM,
    );
    assert!(
        !ResolvedModule::is_resolved(Some(&resolved)),
        "expected null export to remain unresolved, got {:?}",
        resolved.resolved_file_name
    );
    assert_eq!(
        resolved.resolved_path,
        PathKey::default(),
        "unresolved module has a path"
    );
}

#[test]
fn resolve_module_name_export_target_with_trailing_slash_does_not_resolve_as_file() {
    let fs = test_fs(&[
        ("/repo/node_modules/pkg/package.json", r#"{"name":"pkg","exports":{".":"./index.d.ts/"}}"#),
        ("/repo/node_modules/pkg/index.d.ts", "export const x: number;"),
        ("/repo/src/file.ts", ""),
    ]);
    let host = test_host(fs, "/repo");
    let resolver = new_test_resolver(host, bundler_opts(ModuleKind::ESNext));

    let resolved = resolve_module(
        &resolver,
        "pkg",
        &RootedFilePath::from("/repo/src/file.ts"),
        ResolutionModeESM,
    );
    assert!(
        !ResolvedModule::is_resolved(Some(&resolved)),
        "expected directory-only export target to remain unresolved, got {:?}",
        resolved.resolved_file_name
    );
}

// Regression test for https://github.com/microsoft/TypeScript/tsc/issues/3526.
//
// Two threads resolve the same package via specifiers that differ only by a
// trailing slash (`pkg` and `pkg/`). A blocking FS holds both at the
// FileExists check for `package.json` — after each has confirmed a
// `package.json` info-cache miss but before either has called `Set`. When
// released, both proceed to `LoadOrStore` and one of them loses. Without the
// fix, the loser receives the winner's `InfoCacheEntry` whose
// `PackageDirectory` doesn't match its own `candidate` (because one spelling
// has a trailing slash and the other doesn't), and
// `loadNodeModuleFromDirectoryWorker`'s `ComparePaths` check skips loading
// the package's `main`/`types`. With no `index.*` present, resolution falls
// through to "unresolved" — the phantom TS2307 the issue describes. This
// test deterministically fails when the fix is reverted.
#[test]
fn resolve_module_name_trailing_slash_race() {
    let pkg_json_path = "/repo/node_modules/pkg/package.json";
    let files: Vec<(String, String)> = vec![
        // `types` points at a file that is not discoverable through any
        // fallback path: there is no `index.*` and no `main`. The only way to
        // resolve `pkg` (or `pkg/`) is via the package.json `types` field
        // inside `loadNodeModuleFromDirectoryWorker`, which is exactly the
        // step that the bug skips when `candidate` and
        // `packageInfo.PackageDirectory` mismatch.
        (pkg_json_path.to_string(), r#"{"name":"pkg","types":"./typings/index.d.ts"}"#.to_string()),
        ("/repo/node_modules/pkg/typings/index.d.ts".to_string(), "export const x: number;".to_string()),
        // Distinct containing files so each `ResolveModuleName` call has a
        // unique module-resolution-cache key.
        ("/repo/src/a/file.ts".to_string(), String::new()),
        ("/repo/src/b/file.ts".to_string(), String::new()),
    ];
    let inner: Vec<(&str, &str)> = files.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let fs = Arc::new(BlockingFs {
        fs: tsc_vfs::vfstest::from_map(inner, CaseSensitivity::CaseSensitive),
        target_path: RootedFilePath::from(pkg_json_path),
        gate: Gate::new(),
        arrivals: Arrivals::new(),
    });
    let host = test_host(Arc::clone(&fs) as Arc<dyn Fs>, "/repo");
    let resolver = Arc::new(new_test_resolver(host, bundler_opts(ModuleKind::ESNext)));

    let (tx, rx) = mpsc::channel::<(&'static str, bool)>();
    let mut handles = Vec::new();
    for name in ["pkg", "pkg/"] {
        let containing_file = if name.ends_with('/') {
            "/repo/src/b/file.ts"
        } else {
            "/repo/src/a/file.ts"
        };
        let resolver = Arc::clone(&resolver);
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            let (result, _) = resolver.resolve_module_name(
                name,
                &RootedFilePath::from(containing_file),
                ResolutionModeESM,
                None,
            );
            let resolved = ResolvedModule::is_resolved(result.as_deref());
            tx.send((name, resolved)).unwrap();
        }));
    }

    // Wait for both threads to reach the FileExists gate, guaranteeing both
    // have observed a package.json info-cache miss.
    fs.arrivals.wait_for(2, "first FileExists gate arrivals");
    fs.gate.close();

    for handle in handles {
        handle.join().unwrap();
    }
    drop(tx);
    for (name, resolved) in rx {
        assert!(resolved, "{name:?} failed to resolve");
    }
}

// Regression test for https://github.com/microsoft/TypeScript/tsc/issues/1290.
//
// Two threads resolve `pkg/sub` concurrently. Both miss the package.json
// info-cache for the root package directory. A `FlipFileExistsFs` forces the
// first thread's `FileExists` to return false (simulating the file not yet
// being visible), so it stores a nil-Contents cache entry. The second
// thread's `FileExists` returns true, but its `Set` call (`LoadOrStore`)
// returns the first thread's nil-Contents entry. Without the `Exists()`
// guard on the `typesVersions` lookup, `packageInfo.Contents.GetVersionPaths`
// dereferences nil and panics. With the guard the nil-Contents entry is safely
// skipped.
#[test]
fn resolve_subpath_nil_contents_race() {
    let root_pkg_json = "/repo/node_modules/pkg/package.json";
    let files = [
        (root_pkg_json.to_string(), r#"{"name":"pkg","version":"1.0.0"}"#.to_string()),
        ("/repo/node_modules/pkg/sub/index.d.ts".to_string(), "export declare const sub: number;".to_string()),
        ("/repo/node_modules/pkg/sub/index.js".to_string(), "exports.sub = 1;".to_string()),
        ("/repo/src/a/file.ts".to_string(), String::new()),
        ("/repo/src/b/file.ts".to_string(), String::new()),
    ];
    let inner: Vec<(&str, &str)> = files.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let fs = Arc::new(FlipFileExistsFs {
        fs: tsc_vfs::vfstest::from_map(inner, CaseSensitivity::CaseSensitive),
        target_path: RootedFilePath::from(root_pkg_json),
        call_count: AtomicI32::new(0),
        first_arrived: Gate::new(),
        second_arrived: Gate::new(),
        first_gate: Gate::new(),
        second_gate: Gate::new(),
        read_arrived: Gate::new(),
        read_gate: Gate::new(),
    });
    let host = test_host(Arc::clone(&fs) as Arc<dyn Fs>, "/repo");
    let resolver = Arc::new(new_test_resolver(host, bundler_opts(ModuleKind::ESNext)));

    let panicked = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(&'static str, bool)>();
    let mut handles = Vec::new();
    // Two threads both resolve "pkg/sub". Each calls getPackageJsonInfo for
    // the root package directory, reaching FileExists for rootPkgJSON.
    for containing_file in ["/repo/src/a/file.ts", "/repo/src/b/file.ts"] {
        let resolver = Arc::clone(&resolver);
        let panicked = Arc::clone(&panicked);
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                let (result, _) = resolver.resolve_module_name(
                    "pkg/sub",
                    &RootedFilePath::from(containing_file),
                    ResolutionModeESM,
                    None,
                );
                ResolvedModule::is_resolved(result.as_deref())
            }));
            match result {
                Ok(resolved) => tx.send((containing_file, resolved)).unwrap(),
                Err(payload) => {
                    panicked.store(true, Ordering::SeqCst);
                    tx.send((containing_file, false)).unwrap();
                    resume_unwind(payload);
                }
            }
        }));
    }

    // Phase 1: Wait for both threads to reach FileExists for the root
    // package.json, guaranteeing both have observed a cache miss.
    fs.first_arrived.wait_closed("first root package.json FileExists arrival");
    fs.second_arrived.wait_closed("second root package.json FileExists arrival");

    // Phase 2: Release the first FileExists caller (returns false). It enters
    // the "file not found" branch and stores a nil-Contents entry via Set —
    // this is nearly instant (no ReadFile).
    fs.first_gate.close();

    // Phase 3: Release the second FileExists caller (returns true). It
    // proceeds to ReadFile, which we gate separately to ensure the first
    // thread's nil-Contents Set has completed.
    fs.second_gate.close();

    // Phase 4: Wait for the second thread to reach ReadFile, then release.
    // By this point the first thread has stored its nil-Contents entry. The
    // second thread's Set (LoadOrStore) will return that stale entry.
    fs.read_arrived.wait_closed("root package.json ReadFile arrival");
    fs.read_gate.close();

    for handle in handles {
        handle.join().expect("resolver thread panicked due to nil Contents dereference");
    }
    drop(tx);
    assert!(
        !panicked.load(Ordering::SeqCst),
        "resolver panicked due to nil Contents dereference in loadModuleFromSpecificNodeModulesDirectory"
    );
    for (containing_file, resolved) in rx {
        assert!(resolved, "{containing_file:?} failed to resolve pkg/sub");
    }
}

#[test]
fn node_module_package_root() {
    struct Case {
        name: &'static str,
        path: &'static str,
        is_folder: bool,
        want: &'static str,
    }
    for test in [
        Case { name: "file in package", path: "/a/node_modules/b/lib/index.d.ts", is_folder: false, want: "/a/node_modules/b" },
        Case { name: "file in scoped package", path: "/a/node_modules/@scope/b/lib/index.d.ts", is_folder: false, want: "/a/node_modules/@scope/b" },
        Case { name: "folder subpath", path: "/a/node_modules/b/lib/File", is_folder: true, want: "/a/node_modules/b" },
        Case { name: "folder subpath scoped", path: "/a/node_modules/@scope/b/lib/File", is_folder: true, want: "/a/node_modules/@scope/b" },
        Case { name: "package root folder", path: "/a/node_modules/b", is_folder: true, want: "/a/node_modules/b" },
        Case { name: "scoped package root folder", path: "/a/node_modules/@scope/b", is_folder: true, want: "/a/node_modules/@scope/b" },
        Case { name: "package root interpreted as file", path: "/a/node_modules/b", is_folder: false, want: "/a/node_modules" },
        Case { name: "scoped package root interpreted as file", path: "/a/node_modules/@scope/b", is_folder: false, want: "/a/node_modules/@scope" },
        // A bare scope directory has no package name; must not panic
        // (https://github.com/microsoft/TypeScript/tsc/issues/4373).
        Case { name: "scope-only folder", path: "/a/node_modules/@scope", is_folder: true, want: "/a/node_modules/@scope" },
        Case { name: "types scope-only folder", path: "/a/node_modules/@types", is_folder: true, want: "/a/node_modules/@types" },
        Case { name: "not in node_modules", path: "/a/src/index.ts", is_folder: false, want: "" },
    ] {
        let got = if test.is_folder {
            node_module_package_root_for_directory(&RootedDirectoryPath::from(test.path))
        } else {
            node_module_package_root_for_file(&RootedFilePath::from(test.path))
        };
        assert_eq!(got, RootedDirectoryPath::from(test.want), "{}", test.name);
    }
}

// Regression test for https://github.com/microsoft/TypeScript/tsc/issues/4478.
//
// While resolving a package with peerDependencies, two threads look up the
// peer package's package.json concurrently. A `FlipFileExistsFs` forces the
// first lookup to cache a nil-Contents entry and the second lookup to receive
// that stale entry from `Set`. The resolver must not dereference the peer
// package.json contents unless the entry actually exists.
#[test]
fn resolve_peer_dependency_nil_contents_race() {
    let peer_pkg_json = "/repo/node_modules/peer/package.json";
    let files = [
        (
            "/repo/node_modules/pkg/package.json".to_string(),
            r#"{"name":"pkg","version":"1.0.0","types":"index.d.ts","peerDependencies":{"peer":"*"}}"#.to_string(),
        ),
        ("/repo/node_modules/pkg/index.d.ts".to_string(), "export declare const x: number;".to_string()),
        (peer_pkg_json.to_string(), r#"{"name":"peer","version":"2.0.0"}"#.to_string()),
        ("/repo/src/a/file.ts".to_string(), String::new()),
        ("/repo/src/b/file.ts".to_string(), String::new()),
    ];
    let inner: Vec<(&str, &str)> = files.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let fs = Arc::new(FlipFileExistsFs {
        fs: tsc_vfs::vfstest::from_map(inner, CaseSensitivity::CaseSensitive),
        target_path: RootedFilePath::from(peer_pkg_json),
        call_count: AtomicI32::new(0),
        first_arrived: Gate::new(),
        second_arrived: Gate::new(),
        first_gate: Gate::new(),
        second_gate: Gate::new(),
        read_arrived: Gate::new(),
        read_gate: Gate::new(),
    });
    let host = test_host(Arc::clone(&fs) as Arc<dyn Fs>, "/repo");
    let resolver = Arc::new(new_test_resolver(host, bundler_opts(ModuleKind::ESNext)));

    let panicked = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(&'static str, bool)>();
    let mut handles = Vec::new();
    for containing_file in ["/repo/src/a/file.ts", "/repo/src/b/file.ts"] {
        let resolver = Arc::clone(&resolver);
        let panicked = Arc::clone(&panicked);
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                let (result, _) = resolver.resolve_module_name(
                    "pkg",
                    &RootedFilePath::from(containing_file),
                    ResolutionModeESM,
                    None,
                );
                ResolvedModule::is_resolved(result.as_deref())
            }));
            match result {
                Ok(resolved) => tx.send((containing_file, resolved)).unwrap(),
                Err(payload) => {
                    panicked.store(true, Ordering::SeqCst);
                    tx.send((containing_file, false)).unwrap();
                    resume_unwind(payload);
                }
            }
        }));
    }

    fs.first_arrived.wait_closed("first peer package.json FileExists arrival");
    fs.second_arrived.wait_closed("second peer package.json FileExists arrival");
    fs.first_gate.close();
    let first_result = rx
        .recv_timeout(TEST_TIMEOUT)
        .expect("timed out waiting for first peer package.json lookup to finish");
    fs.second_gate.close();
    fs.read_arrived.wait_closed("peer package.json ReadFile arrival");
    fs.read_gate.close();

    for handle in handles {
        handle.join().expect("resolver thread panicked due to nil Contents dereference");
    }
    drop(tx);
    assert!(
        !panicked.load(Ordering::SeqCst),
        "resolver panicked due to nil Contents dereference in readPackageJsonPeerDependencies"
    );
    assert!(
        first_result.1,
        "{:?} failed to resolve pkg",
        first_result.0
    );
    for (containing_file, resolved) in rx {
        assert!(resolved, "{containing_file:?} failed to resolve pkg");
    }
}

// TestResolverSharedData is SKIPPED in the port: it uses Go reflection
// (`testutil.CheckDataOnly(reflect.TypeFor[ResolutionData](), ...)`) to assert
// that ResolutionData contains no data-only dependencies (ast.Diagnostic and
// packagejson.PackageJson must be shared by pointer). Rust has no runtime
// reflection over struct fields, and ResolutionData's shape is enforced by
// its Rust type definition (`Arc<CompilerOptions>`, `Arc<InfoCache>`), so
// there is nothing to test portably. 32 of the 33 Go tests are ported.

