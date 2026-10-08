// Ported from tsc/internal/module/cache.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::sync::Arc;

use tsc_collections::{OrderedMap, SyncMap};
use tsc_core::compileroptions::ResolutionMode;
use tsc_core::options_generated::CompilerOptions;
use tsc_packagejson::{InfoCache, InfoCacheEntry, new_info_cache};
use tsc_tspath::{CaseSensitivity, PathKey, RootedDirectoryPath, RootedFilePath};

use crate::resolver::{ParsedPatterns, ResolverOptions, try_parse_patterns};
use crate::types::{ResolvedModule, ResolvedProjectReference, ResolvedTypeReferenceDirective};

// moduleResolutionCacheKey is the cache key for a module name resolution.
//
// Derives Hash/Eq — Go uses the struct as a sync.Map key (comparable).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ModuleResolutionCacheKey {
    pub(crate) containing_directory: RootedDirectoryPath,
    pub(crate) module_name: String,
    pub(crate) resolution_mode: ResolutionMode,
    pub(crate) redirect_config_name: RootedFilePath,
}

// moduleResolutionCache caches resolved modules by key.
//
// PORT: Go stores `*ResolvedModule` (nil-able) in a sync.Map; the SyncMap
// hands out owned clones, so values are `Option<Arc<ResolvedModule>>` —
// `Some(None)` is "cached an unresolved result" (Go's cached nil pointer).
pub(crate) struct ModuleResolutionCache {
    pub(crate) cache: SyncMap<ModuleResolutionCacheKey, Option<Arc<ResolvedModule>>>,
}

impl ModuleResolutionCache {
    pub(crate) fn get(&self, key: &ModuleResolutionCacheKey) -> Option<Option<Arc<ResolvedModule>>> {
        self.cache.load(key)
    }

    pub(crate) fn set(&self, key: ModuleResolutionCacheKey, value: Option<Arc<ResolvedModule>>) {
        // Go: LoadOrStore — first-writer-wins, return value discarded.
        self.cache.load_or_store(key, value);
    }
}

impl Default for ModuleResolutionCache {
    fn default() -> Self {
        Self {
            cache: SyncMap::new(),
        }
    }
}

// typeRefDirectiveResolutionCacheKey is the cache key for a type reference
// directive resolution.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TypeRefDirectiveResolutionCacheKey {
    pub(crate) containing_directory: RootedDirectoryPath,
    pub(crate) type_reference_name: String,
    pub(crate) resolution_mode: ResolutionMode,
    pub(crate) redirect_config_name: RootedFilePath,
    pub(crate) from_inferred_types_containing_file: bool,
}

pub(crate) struct TypeRefDirectiveResolutionCache {
    pub(crate) cache: SyncMap<TypeRefDirectiveResolutionCacheKey, Option<Arc<ResolvedTypeReferenceDirective>>>,
}

impl TypeRefDirectiveResolutionCache {
    pub(crate) fn get(
        &self,
        key: &TypeRefDirectiveResolutionCacheKey,
    ) -> Option<Option<Arc<ResolvedTypeReferenceDirective>>> {
        self.cache.load(key)
    }

    pub(crate) fn set(
        &self,
        key: TypeRefDirectiveResolutionCacheKey,
        value: Option<Arc<ResolvedTypeReferenceDirective>>,
    ) {
        self.cache.store(key, value);
    }
}

impl Default for TypeRefDirectiveResolutionCache {
    fn default() -> Self {
        Self {
            cache: SyncMap::new(),
        }
    }
}

// ParsedPatternsKey is the content snapshot of a `paths` mapping used to key
// the parsed-patterns cache.
//
// PORT: Go keys `parsedPatternsCache` by the `*collections.OrderedMap` pointer
// (CompilerOptions.Paths is shared by pointer). The Rust CompilerOptions owns
// the map by value, so there is no stable pointer to key on; the cache is
// keyed by the mapping's content instead. That is a behavioral superset
// (equal content from distinct pointers also hits) and the cached
// `ParsedPatterns` are a pure function of the content, so resolution results
// are identical.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ParsedPatternsKey(Vec<(String, Vec<String>)>);

impl ParsedPatternsKey {
    pub(crate) fn of(paths: &OrderedMap<String, Vec<String>>) -> Self {
        Self(
            paths
                .entries()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }
}

pub(crate) struct ParsedPatternsCache {
    pub(crate) cache: SyncMap<ParsedPatternsKey, Arc<ParsedPatterns>>,
}

impl ParsedPatternsCache {
    pub(crate) fn get(&self, path_mappings: &OrderedMap<String, Vec<String>>) -> Arc<ParsedPatterns> {
        let key = ParsedPatternsKey::of(path_mappings);
        let (patterns, _) = self
            .cache
            .load_or_store(key, Arc::new(try_parse_patterns(path_mappings)));
        patterns
    }
}

impl Default for ParsedPatternsCache {
    fn default() -> Self {
        Self {
            cache: SyncMap::new(),
        }
    }
}

// ResolutionData is the resolver state that can be shared across resolvers
// (the package.json cache and configured options); resolvers derived from it
// each get their own resolution caches.
pub struct ResolutionData {
    compiler_options: Arc<CompilerOptions>,
    typings_location: RootedDirectoryPath,
    project_name: String,
    extra_extensions: Vec<String>,

    package_json_info_cache: Arc<InfoCache>,
}

impl Default for ResolutionData {
    /// Go has no default constructor; the port needs one so tests can build a
    /// bare `DefaultResolver` (Go tests write `&DefaultResolver{fs: fs}` with
    /// a zero `ResolutionData`). The cache defaults to case-insensitive, the
    /// zero `CaseSensitivity`.
    fn default() -> Self {
        Self {
            compiler_options: Arc::default(),
            typings_location: RootedDirectoryPath::default(),
            project_name: String::new(),
            extra_extensions: Vec::new(),
            package_json_info_cache: Arc::new(new_info_cache(CaseSensitivity::default())),
        }
    }
}

impl ResolutionData {
    pub(crate) fn compiler_options(&self) -> &Arc<CompilerOptions> {
        &self.compiler_options
    }

    pub(crate) fn typings_location(&self) -> &RootedDirectoryPath {
        &self.typings_location
    }

    pub(crate) fn project_name(&self) -> &str {
        &self.project_name
    }

    pub(crate) fn extra_extensions(&self) -> &[String] {
        &self.extra_extensions
    }

    pub(crate) fn package_json_info_cache(&self) -> &Arc<InfoCache> {
        &self.package_json_info_cache
    }

    /// Clone copies the package-json cache table without copying its entries.
    ///
    /// Go: `func (c *ResolutionData) Clone() *ResolutionData` — renamed to
    /// `cloned` because Rust reserves `clone` for the Clone trait (the Arc'd
    /// entries keep Go's pointer-sharing semantics).
    pub fn cloned(&self) -> Arc<Self> {
        Arc::new(Self {
            compiler_options: Arc::clone(&self.compiler_options),
            typings_location: self.typings_location.clone(),
            project_name: self.project_name.clone(),
            extra_extensions: self.extra_extensions.clone(),
            package_json_info_cache: Arc::new(self.package_json_info_cache.as_ref().clone()),
        })
    }

    /// Go: `func (c *ResolutionData) PackageJsonCacheEntries(f func(key, value) bool)`.
    pub fn package_json_cache_entries(
        &self,
        f: impl FnMut(PathKey, Arc<InfoCacheEntry>) -> bool,
    ) {
        self.package_json_info_cache.range(f);
    }
}

pub fn new_resolution_data(opts: &ResolverOptions) -> Arc<ResolutionData> {
    let package_json_info_cache = match &opts.package_json_cache {
        Some(cache) => Arc::new(cache.clone()),
        None => Arc::new(new_info_cache(opts.host.fs().case_sensitivity())),
    };
    Arc::new(ResolutionData {
        compiler_options: Arc::clone(&opts.compiler_options),
        typings_location: opts.typings_location.clone(),
        project_name: opts.project_name.clone(),
        extra_extensions: opts.extra_extensions.clone(),
        package_json_info_cache,
    })
}

pub fn get_redirect_config_name(redirect: Option<&Arc<dyn ResolvedProjectReference>>) -> RootedFilePath {
    match redirect {
        None => RootedFilePath::default(),
        Some(redirect) => redirect.config_name().clone(),
    }
}
