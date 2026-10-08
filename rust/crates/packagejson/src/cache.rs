// Ported from tsc/internal/packagejson/cache.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   PackageJson / VersionPaths / PackageDirectory /
//   InfoCacheEntry / InfoCache                → same
//   NewPackageDirectory                       → new_package_directory
//   NewInfoCache                               → new_info_cache
//   ForEachAncestorDirectoryStoppingAtGlobalCache → for_each_ancestor_directory_stopping_at_global_cache
//   GetVersionPaths                            → get_version_paths
//   Exists / GetPaths                          → exists / get_paths
//   WithPackageDirectory                       → with_package_directory
//   String (PackageDirectory)                   → impl Display
//
// PORT notes:
//   - `InfoCache.cache` is `collections.SyncMap[PathKey, *InfoCacheEntry]` in
//     Go; the Rust SyncMap hands out owned clones, so entries are stored as
//     `Arc<InfoCacheEntry>` (pointer identity is preserved, matching Go).
//   - Go's nil-receiver methods (`(*InfoCacheEntry)(nil).Exists()`) are
//     `Option<&Self>` associated fns, following the collections crate's
//     nil-receiver convention.
//   - `PackageJson.versionPaths`/`versionTraces`/`once` collapse into one
//     `OnceLock<VersionPathsState>` (the trace list and result are produced by
//     the same single pass in Go).
//   - `VersionPaths.paths` (a per-copy lazy cache in Go — the cache never
//     survives the value copy Go returns) is an `Arc<OnceLock<_>>` shared by
//     every clone.

use std::fmt;
use std::sync::{Arc, OnceLock};

use tsc_collections::{OrderedMap, SyncMap};
use tsc_core::version::{version, version_major_minor};
use tsc_diagnostics::Message;
use tsc_diagnostics::{
    EXPECTED_TYPE_OF_0_FIELD_IN_PACKAGE_JSON_TO_BE_1_GOT_2,
    X_PACKAGE_JSON_DOES_NOT_HAVE_A_0_FIELD,
    X_PACKAGE_JSON_DOES_NOT_HAVE_A_TYPESVERSIONS_ENTRY_THAT_MATCHES_VERSION_0,
    X_PACKAGE_JSON_HAS_A_TYPESVERSIONS_ENTRY_0_THAT_IS_NOT_A_VALID_SEMVER_RANGE,
    X_PACKAGE_JSON_HAS_A_TYPESVERSIONS_FIELD_WITH_VERSION_SPECIFIC_PATH_MAPPINGS,
};
use tsc_semver::{Version, must_parse, try_parse_version_range};
use tsc_tspath::{CaseSensitivity, PathKey, RootedDirectoryPath, RootedFilePath};

use crate::jsonvalue::{JSONValue, JSONValueType};
use crate::packagejson::Fields;

// Go: `var typeScriptVersion = semver.MustParse(core.Version())` — computed at
// package init; the port computes lazily once.
fn type_script_version() -> &'static Version {
    static TYPE_SCRIPT_VERSION: OnceLock<Version> = OnceLock::new();
    TYPE_SCRIPT_VERSION.get_or_init(|| must_parse(version()))
}

// Go: `type diagnosticAndArgs struct` (unexported).
#[derive(Debug)]
struct DiagnosticAndArgs {
    message: &'static Message,
    args: Vec<String>,
}

/// Go: `trace func(m *diagnostics.Message, args ...any)` — every Go call site
/// passes string args, so the port pins them to `&[String]`. (Alias mostly for
/// clippy::type_complexity; the lifetime carries the closure's captures.)
pub type TraceFn<'a> = dyn FnMut(&'static Message, &[String]) + 'a;

/// Go: `type PackageJson struct` — parsed fields plus lazily-computed
/// typesVersions state. The version-paths/trace state is computed at most
/// once per PackageJson (Go: sync.Once).
#[derive(Debug, Default)]
pub struct PackageJson {
    pub fields: Fields,
    pub parseable: bool,
    version_paths_state: OnceLock<VersionPathsState>,
}

// Go: `type diagnosticAndArgs` + the two lazily-written PackageJson fields.
#[derive(Debug, Default)]
struct VersionPathsState {
    version_paths: VersionPaths,
    version_traces: Vec<DiagnosticAndArgs>,
}

impl PackageJson {
    /// GetVersionPaths computes (once) the typesVersions entry matching the
    /// running TypeScript version and replays the recorded trace messages to
    /// `trace`, if provided.
    ///
    /// Go: `func (p *PackageJson) GetVersionPaths(trace func(m *diagnostics.Message, args ...any)) VersionPaths`.
    pub fn get_version_paths(&self, trace: Option<&mut TraceFn<'_>>) -> VersionPaths {
        let state = self
            .version_paths_state
            .get_or_init(|| self.compute_version_paths_state());
        if let Some(trace) = trace {
            for msg in &state.version_traces {
                trace(msg.message, &msg.args);
            }
        }
        state.version_paths.clone()
    }

    fn compute_version_paths_state(&self) -> VersionPathsState {
        let mut state = VersionPathsState::default();
        let types_versions = &self.fields.path_fields.types_versions;
        if types_versions.type_ == JSONValueType::NotPresent {
            state.version_traces.push(DiagnosticAndArgs {
                message: &X_PACKAGE_JSON_DOES_NOT_HAVE_A_0_FIELD,
                args: vec!["typesVersions".to_string()],
            });
            return state;
        }
        if types_versions.type_ != JSONValueType::Object {
            state.version_traces.push(DiagnosticAndArgs {
                message: &EXPECTED_TYPE_OF_0_FIELD_IN_PACKAGE_JSON_TO_BE_1_GOT_2,
                args: vec![
                    "typesVersions".to_string(),
                    "object".to_string(),
                    types_versions.type_.to_string(),
                ],
            });
            return state;
        }

        state.version_traces.push(DiagnosticAndArgs {
            message: &X_PACKAGE_JSON_HAS_A_TYPESVERSIONS_FIELD_WITH_VERSION_SPECIFIC_PATH_MAPPINGS,
            args: vec!["typesVersions".to_string()],
        });

        for (key, value) in types_versions.as_object().entries() {
            let (key_range, ok) = try_parse_version_range(key);
            if !ok {
                state.version_traces.push(DiagnosticAndArgs {
                    message: &X_PACKAGE_JSON_HAS_A_TYPESVERSIONS_ENTRY_0_THAT_IS_NOT_A_VALID_SEMVER_RANGE,
                    args: vec![key.clone()],
                });
                continue;
            }
            if key_range.test(Some(type_script_version())) {
                if value.type_ != JSONValueType::Object {
                    state.version_traces.push(DiagnosticAndArgs {
                        message: &EXPECTED_TYPE_OF_0_FIELD_IN_PACKAGE_JSON_TO_BE_1_GOT_2,
                        args: vec![
                            format!("typesVersions['{key}']"),
                            "object".to_string(),
                            value.type_.to_string(),
                        ],
                    });
                    return state;
                }
                state.version_paths = VersionPaths {
                    version: key.clone(),
                    paths_json: Some(Arc::new(value.as_object().clone())),
                    paths: Arc::new(OnceLock::new()),
                };
                return state;
            }
        }

        state.version_traces.push(DiagnosticAndArgs {
            message: &X_PACKAGE_JSON_DOES_NOT_HAVE_A_TYPESVERSIONS_ENTRY_THAT_MATCHES_VERSION_0,
            args: vec![version_major_minor().to_string()],
        });
        state
    }

    // Promoted from Fields (Go struct embedding).

    /// Promoted from Fields (Go struct embedding).
    pub fn has_dependency(&self, name: &str) -> bool {
        self.fields.has_dependency(name)
    }

    /// Promoted from Fields (Go struct embedding).
    pub fn range_dependencies(&self, f: impl FnMut(&str, &str, &str) -> bool) {
        self.fields.range_dependencies(f);
    }

    /// Promoted from Fields (Go struct embedding).
    pub fn get_runtime_dependency_names(&self) -> tsc_collections::Set<String> {
        self.fields.get_runtime_dependency_names()
    }
}

/// The computed `paths` cache shared by every clone of a VersionPaths
/// (Go caches into a plain per-copy field).
type ComputedPaths = Arc<OnceLock<Option<Arc<OrderedMap<String, Vec<String>>>>>>;

/// VersionPaths is the typesVersions entry that matched the running compiler
/// version, plus its raw path mappings.
#[derive(Clone, Debug, Default)]
pub struct VersionPaths {
    /// The matching typesVersions key (empty when nothing matched).
    pub version: String,
    paths_json: Option<Arc<OrderedMap<String, JSONValue>>>,
    paths: ComputedPaths,
}

impl VersionPaths {
    /// Go: `func (v *VersionPaths) Exists() bool { return v != nil && v.Version != "" && v.pathsJSON != nil }`.
    ///
    /// PORT: the `v != nil` guard covers Go's nil-pointer method calls; the
    /// port is a value type — callers holding `Option<VersionPaths>` use
    /// `.is_some_and(VersionPaths::exists)`.
    pub fn exists(&self) -> bool {
        !self.version.is_empty() && self.paths_json.is_some()
    }

    /// GetPaths lazily flattens the entry's path mappings: every array-valued
    /// member becomes a `Vec<String>` (non-string elements collapse to "",
    /// as in Go), and the result is cached.
    ///
    /// Go: `func (v *VersionPaths) GetPaths() *collections.OrderedMap[string, []string]`
    /// (nil when `!Exists`).
    pub fn get_paths(&self) -> Option<&OrderedMap<String, Vec<String>>> {
        if !self.exists() {
            return None;
        }
        let computed = self.paths.get_or_init(|| {
            let paths_json = self
                .paths_json
                .as_deref()
                .expect("exists() implies paths_json");
            let mut paths = OrderedMap::with_size_hint(paths_json.size());
            for (key, value) in paths_json.entries() {
                if value.type_ != JSONValueType::Array {
                    continue;
                }
                let array = value.as_array();
                let mut slice = vec![String::new(); array.len()];
                for (i, path) in array.iter().enumerate() {
                    if path.type_ != JSONValueType::String {
                        continue;
                    }
                    slice[i] = path.as_string().to_string();
                }
                paths.set(key.clone(), slice);
            }
            Some(Arc::new(paths))
        });
        Some(computed.as_ref()?.as_ref())
    }
}

/// PackageDirectory pairs a package's directory path with its canonical path
/// key, so presentation and canonical identity advance together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDirectory {
    name: RootedDirectoryPath,
    key: PathKey,
}

/// Go: `func NewPackageDirectory(directory, caseSensitivity) PackageDirectory`.
pub fn new_package_directory(
    directory: RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
) -> PackageDirectory {
    let key = case_sensitivity.path_key(&directory.as_path());
    PackageDirectory { name: directory, key }
}

impl PackageDirectory {
    /// Go: `func (p PackageDirectory) AsDirectoryPath() RootedDirectoryPath`
    /// (returned by value there; by borrow here).
    pub fn as_directory_path(&self) -> &RootedDirectoryPath {
        &self.name
    }

    /// Go: `func (p PackageDirectory) PathKey() tspath.PathKey`
    /// (returned by value there; by borrow here).
    pub fn path_key(&self) -> &PathKey {
        &self.key
    }

    /// Go: `func (p PackageDirectory) Parent() PackageDirectory` — a root's
    /// parent is itself (a fixed point).
    pub fn parent(&self) -> PackageDirectory {
        let parent_name = self.name.as_path().directory();
        let parent_key = self.key.parent();
        if parent_name == self.name {
            return self.clone();
        }
        PackageDirectory {
            name: parent_name,
            key: parent_key,
        }
    }

    /// Go: `func (p PackageDirectory) ResolveFile(path string) tspath.RootedFilePath`.
    pub fn resolve_file(&self, path: &str) -> RootedFilePath {
        self.name.resolve_file(path)
    }
}

// Go: `func (p PackageDirectory) String() string`.
impl fmt::Display for PackageDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name.as_string())
    }
}

/// ForEachAncestorDirectoryStoppingAtGlobalCache calls callback for directory
/// and its ancestors while advancing presentation and canonical identity
/// together, stopping after the global cache directory is visited.
///
/// Go: `func ForEachAncestorDirectoryStoppingAtGlobalCache[T any](globalCache, directory, callback) T`.
///
/// PORT: Go's zero `T` on root exhaustion becomes `None`.
pub fn for_each_ancestor_directory_stopping_at_global_cache<T>(
    global_cache: &RootedDirectoryPath,
    mut directory: PackageDirectory,
    mut callback: impl FnMut(&PackageDirectory) -> (T, bool),
) -> Option<T> {
    loop {
        let (result, stop) = callback(&directory);
        if stop || directory.as_directory_path() == global_cache {
            return Some(result);
        }
        let parent = directory.parent();
        if parent == directory {
            return None;
        }
        directory = parent;
    }
}

/// InfoCacheEntry is one cached package.json lookup result.
#[derive(Clone, Debug)]
pub struct InfoCacheEntry {
    pub package_directory: PackageDirectory,
    pub directory_exists: bool,
    /// Go: `Contents *PackageJson` (nil-able pointer) — `Option<Arc<PackageJson>>`
    /// because the SyncMap hands out owned clones.
    pub contents: Option<Arc<PackageJson>>,
}

impl InfoCacheEntry {
    /// Go: `func (p *InfoCacheEntry) Exists() bool { return p != nil && p.Contents != nil }`.
    ///
    /// PORT: the nil receiver is an `Option` argument (collections crate
    /// convention); callers holding `Option<Arc<InfoCacheEntry>>` pass
    /// `.as_deref()`.
    pub fn exists(this: Option<&Self>) -> bool {
        this.is_some_and(|entry| entry.contents.is_some())
    }

    /// Go: `func (p *InfoCacheEntry) GetContents() *PackageJson` — nil unless
    /// both the entry and its contents exist.
    pub fn get_contents(this: Option<&Self>) -> Option<Arc<PackageJson>> {
        this.and_then(|entry| entry.contents.clone())
    }

    /// WithPackageDirectory returns an entry whose PackageDirectory matches the
    /// caller's value. The package.json info cache is keyed by the canonical
    /// package directory, but multiple callers may use directory paths whose
    /// presentation differs while their canonical identity is the same. Because
    /// the cache uses first-writer-wins semantics, a later caller may receive an
    /// entry whose PackageDirectory doesn't match its own candidate path.
    /// Downstream code compares the candidate against PackageDirectory, so
    /// return a corrected shallow copy when they diverge.
    /// See https://github.com/microsoft/TypeScript/pull/50740.
    ///
    /// Go: `func (p *InfoCacheEntry) WithPackageDirectory(packageDirectory) *InfoCacheEntry`
    /// — returns the same pointer when they already match; the Arc receiver
    /// preserves that identity.
    pub fn with_package_directory(self: Arc<Self>, package_directory: PackageDirectory) -> Arc<Self> {
        if self.package_directory == package_directory {
            return self;
        }
        Arc::new(InfoCacheEntry {
            package_directory,
            directory_exists: self.directory_exists,
            contents: self.contents.clone(),
        })
    }
}

/// InfoCache caches package.json lookups by canonical package directory.
///
/// Go: `func NewInfoCache(caseSensitivity) *InfoCache` + pointer-receiver
/// methods; the port is an owned value (the interior-mutability SyncMap makes
/// sharing work the same) and Go's `Clone` method is the derived `Clone`.
#[derive(Clone)]
pub struct InfoCache {
    cache: SyncMap<PathKey, Arc<InfoCacheEntry>>,
    case_sensitivity: CaseSensitivity,
}

/// Go: `func NewInfoCache(caseSensitivity tspath.CaseSensitivity) *InfoCache`.
pub fn new_info_cache(case_sensitivity: CaseSensitivity) -> InfoCache {
    InfoCache {
        cache: SyncMap::new(),
        case_sensitivity,
    }
}

impl InfoCache {
    /// Go: `func (p *InfoCache) PackageDirectory(directory) PackageDirectory`.
    pub fn package_directory(&self, directory: RootedDirectoryPath) -> PackageDirectory {
        new_package_directory(directory, self.case_sensitivity)
    }

    /// Go: `func (p *InfoCache) CaseSensitivity() tspath.CaseSensitivity`.
    pub fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
    }

    /// Go: `func (p *InfoCache) Get(directory) *InfoCacheEntry` — Go's typed-nil
    /// result becomes `None`.
    pub fn get(&self, directory: &PackageDirectory) -> Option<Arc<InfoCacheEntry>> {
        self.cache.load(directory.path_key())
    }

    /// Go: `func (p *InfoCache) Set(directory, info) *InfoCacheEntry` —
    /// first-writer-wins: returns the entry now stored for the key.
    pub fn set(
        &self,
        directory: &PackageDirectory,
        info: Arc<InfoCacheEntry>,
    ) -> Arc<InfoCacheEntry> {
        self.cache
            .load_or_store(directory.path_key().clone(), info)
            .0
    }

    /// Go: `func (p *InfoCache) Range(f func(key, value) bool)`.
    pub fn range(&self, f: impl FnMut(PathKey, Arc<InfoCacheEntry>) -> bool) {
        self.cache.range(f)
    }
}
