// Ported from tsc/internal/module/staticresolver.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::fmt;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use tsc_core::compileroptions::ResolutionMode;
use tsc_tspath::{CaseSensitivity, PathKey, RootedDirectoryPath, RootedFilePath};

use crate::types::{ResolvedModule, ResolvedProjectReference, Resolver};

/// StaticResolutionEntry is one pre-computed resolution for a module name.
#[derive(Clone, Default)]
pub struct StaticResolutionEntry {
    pub module_name: String,
    pub containing_directory: RootedDirectoryPath,
    pub resolution_mode: Option<ResolutionMode>,
    pub result: Option<Arc<ResolvedModule>>,
}

// staticResolutionKey identifies a static resolution, with explicit
// "was this field set" bits so lookups can distinguish a query that omitted a
// field from one that set it to the zero value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct StaticResolutionKey {
    module_name: String,
    directory: PathKey,
    mode: ResolutionMode,
    has_directory: bool,
    has_mode: bool,
}

/// StaticResolutions is a table of pre-computed resolutions consulted before
/// (optionally) falling back to a full resolver.
pub struct StaticResolutions {
    fallback_to_resolver: bool,
    entries: FxHashMap<StaticResolutionKey, Option<Arc<ResolvedModule>>>,
    current_directory: RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
}

/// Go: `func NewStaticResolutions(...) (*StaticResolutions, error)`.
///
/// PORT: Go's error values are plain messages ("module name is empty",
/// "duplicate static module resolution for %q"); the port carries them as a
/// `String` error payload (`NewStaticResolutionsResult`'s Err arm).
pub type NewStaticResolutionsResult = Result<Arc<StaticResolutions>, String>;

pub fn new_static_resolutions(
    entries: Vec<StaticResolutionEntry>,
    fallback_to_resolver: bool,
    current_directory: RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
) -> NewStaticResolutionsResult {
    let entries_count = entries.len();
    let mut resolutions = StaticResolutions {
        fallback_to_resolver,
        entries: FxHashMap::default(),
        current_directory,
        case_sensitivity,
    };
    resolutions.entries.reserve(entries_count);
    for entry in entries {
        if entry.module_name.is_empty() {
            return Err("module name is empty".to_string());
        }
        let mut key = StaticResolutionKey {
            module_name: entry.module_name,
            directory: PathKey::default(),
            mode: ResolutionMode::default(),
            has_directory: false,
            has_mode: false,
        };
        if !entry.containing_directory.is_empty() {
            key.directory = resolutions
                .case_sensitivity
                .path_key(&entry.containing_directory.as_path());
            key.has_directory = true;
        }
        if let Some(mode) = entry.resolution_mode {
            key.mode = mode;
            key.has_mode = true;
        }
        if resolutions.entries.contains_key(&key) {
            return Err(format!(
                "duplicate static module resolution for {:?}",
                key.module_name
            ));
        }
        resolutions.entries.insert(key, entry.result);
    }
    Ok(Arc::new(resolutions))
}

impl StaticResolutions {
    pub fn current_directory(&self) -> &RootedDirectoryPath {
        &self.current_directory
    }

    fn lookup(
        &self,
        module_name: &str,
        containing_directory: &RootedDirectoryPath,
        resolution_mode: ResolutionMode,
    ) -> Option<&Option<Arc<ResolvedModule>>> {
        let directory = self
            .case_sensitivity
            .path_key(&containing_directory.as_path());
        let keys = [
            StaticResolutionKey {
                module_name: module_name.to_string(),
                directory: directory.clone(),
                mode: resolution_mode,
                has_directory: true,
                has_mode: true,
            },
            StaticResolutionKey {
                module_name: module_name.to_string(),
                directory,
                mode: ResolutionMode::default(),
                has_directory: true,
                has_mode: false,
            },
            StaticResolutionKey {
                module_name: module_name.to_string(),
                directory: PathKey::default(),
                mode: resolution_mode,
                has_directory: false,
                has_mode: true,
            },
            StaticResolutionKey {
                module_name: module_name.to_string(),
                directory: PathKey::default(),
                mode: ResolutionMode::default(),
                has_directory: false,
                has_mode: false,
            },
        ];
        for key in keys {
            if let Some(result) = self.entries.get(&key) {
                return Some(result);
            }
        }
        None
    }
}

// Go: the resolver surfaces StaticResolutions' entries only through
// lookups; the Debug impl below is a test aid.
impl fmt::Debug for StaticResolutions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticResolutions")
            .field("fallback_to_resolver", &self.fallback_to_resolver)
            .field("current_directory", &self.current_directory)
            .field("case_sensitivity", &self.case_sensitivity)
            .field("entries", &self.entries.len())
            .finish()
    }
}

// StaticResolver consults a table of static resolutions first, falling back
// to a full resolver for misses.
pub struct StaticResolver {
    fallback: Arc<dyn Resolver>,
    resolutions: Arc<StaticResolutions>,
}

pub fn new_static_resolver(
    fallback: Arc<dyn Resolver>,
    resolutions: Arc<StaticResolutions>,
) -> StaticResolver {
    StaticResolver {
        fallback,
        resolutions,
    }
}

impl StaticResolver {
    fn resolve_module_name_inner(
        &self,
        module_name: &str,
        containing_file: &RootedFilePath,
        containing_directory: &RootedDirectoryPath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> crate::types::ResolveModuleNameResult {
        if let Some(result) = self
            .resolutions
            .lookup(module_name, containing_directory, resolution_mode)
        {
            return (result.clone(), Vec::new());
        }
        if !self.resolutions.fallback_to_resolver {
            return (None, Vec::new());
        }
        self.fallback
            .resolve_module_name(module_name, containing_file, resolution_mode, redirected_reference)
    }
}

impl Resolver for StaticResolver {
    fn base_directory(&self) -> RootedDirectoryPath {
        self.fallback.base_directory()
    }

    fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> crate::types::ResolveModuleNameResult {
        self.resolve_module_name_inner(
            module_name,
            containing_file,
            &containing_file.directory(),
            resolution_mode,
            redirected_reference,
        )
    }

    fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &RootedDirectoryPath,
        resolution_mode: ResolutionMode,
    ) -> crate::types::ResolveModuleNameResult {
        if let Some(result) = self
            .resolutions
            .lookup(module_name, containing_directory, resolution_mode)
        {
            return (result.clone(), Vec::new());
        }
        if !self.resolutions.fallback_to_resolver {
            return (None, Vec::new());
        }
        self.fallback
            .resolve_module_name_from_directory(module_name, containing_directory, resolution_mode)
    }

    fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> crate::types::ResolveTypeReferenceDirectiveResult {
        self.fallback.resolve_type_reference_directive(
            type_reference_directive_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    fn get_resolution_data(&self) -> Arc<crate::cache::ResolutionData> {
        self.fallback.get_resolution_data()
    }
}
