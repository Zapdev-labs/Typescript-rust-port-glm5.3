// Ported from tsc/internal/compiler/projectreferencefilemapper.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - `tsoptions.SourceOutputAndProjectReference` is not in the Rust tsoptions
//     crate (it comes with the ParseInputOutputNames wave); a crate-local
//     shape lives here until then (dedup candidate — PORTING-NOTES).
//   - `module.GetCompilerOptionsWithRedirect` takes
//     `Option<&Arc<dyn ResolvedProjectReference>>`; the orphan rule blocks
//     implementing that trait for `tsoptions::ParsedCommandLine` from this
//     crate, so resolved configs travel as `Arc<ResolvedProjectReference>`
//     (a newtype wrapper — the vfs crate's wrapper precedent).
//   - The maps are `FxHashMap`; none reach output ordering (the diagnostic
//     surfaces sort afterwards).

use std::sync::{Arc, Mutex};

use tsc_core::options_generated::CompilerOptions;
use tsc_tsoptions::ParsedCommandLine;
use tsc_tspath::{PathKey, RootedFilePath};

use crate::host::HasFileName;

/// Go: the `rangeResolvedProjectReference(f func(tspath.PathKey,
/// *tsoptions.ParsedCommandLine, *tsoptions.ParsedCommandLine, int) bool)`
/// callback shape, shared by the mapper's and Program's range surfaces
/// (alias mostly for clippy::type_complexity, the packagejson `TraceFn`
/// precedent; the lifetime carries the callback's captures).
pub type RangeResolvedProjectReferenceCallback<'a> = &'a mut dyn FnMut(
    &PathKey,
    Option<&Arc<ResolvedProjectReference>>,
    Option<&Arc<ResolvedProjectReference>>,
    usize,
) -> bool;

/// Go: `type SourceOutputAndProjectReference struct` (tsoptions) —
/// crate-local until the tsoptions wave lands it (see the file header).
#[derive(Clone, Debug, Default)]
pub struct SourceOutputAndProjectReference {
    /// Go: `Source tspath.RootedFilePath`.
    pub source: RootedFilePath,
    /// Go: `SourcePath tspath.PathKey`.
    pub source_path: PathKey,
    /// Go: `OutputDts tspath.RootedFilePath`.
    pub output_dts: RootedFilePath,
    /// Go: `OutputDtsPath tspath.PathKey`.
    pub output_dts_path: PathKey,
    /// Go: `Resolved *tsoptions.ParsedCommandLine`.
    pub resolved: Option<Arc<ResolvedProjectReference>>,
}

/// Go: the `*tsoptions.ParsedCommandLine` values the mapper hands to the
/// module crate's redirect surfaces — a newtype implementing
/// `module::ResolvedProjectReference` (the orphan-rule workaround; Go passes
/// the ParsedCommandLine directly because it implements the interface in its
/// own package).
pub struct ResolvedProjectReference {
    pub config: Arc<ParsedCommandLine>,
}

// PORT: a manual Debug impl (ParsedCommandLine derives no Debug, and the
// wrapping crate cannot derive through the newtype) printing the config-file
// name — the shape is never printed in output paths, only in test failure
// messages.
impl std::fmt::Debug for ResolvedProjectReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedProjectReference")
            .field("config", &self.config.config_file_name.as_string())
            .finish()
    }
}

impl tsc_module::ResolvedProjectReference for ResolvedProjectReference {
    fn config_name(&self) -> &RootedFilePath {
        &self.config.config_file_name
    }
    fn compiler_options(&self) -> Option<&CompilerOptions> {
        Some(self.config.compiler_options())
    }
}

/// Go: `type projectReferenceFileMapper struct`.
pub struct ProjectReferenceFileMapper {
    pub config: Arc<ParsedCommandLine>,
    pub use_source_of_project_reference: bool,
    pub dts_directories: rustc_hash::FxHashSet<PathKey>,

    /// Go: `configToProjectReference map[tspath.PathKey]*tsoptions.ParsedCommandLine`.
    pub config_to_project_reference: rustc_hash::FxHashMap<PathKey, Option<Arc<ResolvedProjectReference>>>,
    /// Go: `referencesInConfigFile map[tspath.PathKey][]tspath.PathKey`.
    pub references_in_config_file: rustc_hash::FxHashMap<PathKey, Vec<PathKey>>,
    /// Go: `sourceToProjectReference map[tspath.PathKey]*tsoptions.SourceOutputAndProjectReference`.
    pub source_to_project_reference: rustc_hash::FxHashMap<PathKey, Arc<SourceOutputAndProjectReference>>,
    /// Go: `outputDtsToProjectReference map[tspath.PathKey]*tsoptions.SourceOutputAndProjectReference`.
    pub output_dts_to_project_reference: rustc_hash::FxHashMap<PathKey, Arc<SourceOutputAndProjectReference>>,

    /// Go: `realpathDtsToSource collections.SyncMap[tspath.PathKey, *tsoptions.SourceOutputAndProjectReference]`.
    realpath_dts_to_source: Mutex<rustc_hash::FxHashMap<PathKey, Option<Arc<SourceOutputAndProjectReference>>>>,
    /// Go: `mapper.config` as the range walks' root parent — Go hands the
    /// root `*tsoptions.ParsedCommandLine` directly as the callback's
    /// `parent` argument (rangeResolvedProjectReference/
    /// rangeResolvedProjectReferenceInChildConfig); the port wraps it in
    /// the ResolvedProjectReference newtype (the orphan-rule shape — see
    /// the file header), memoized (the wrap is a pure function of config).
    root_resolved_reference: std::sync::OnceLock<Arc<ResolvedProjectReference>>,
}

impl ProjectReferenceFileMapper {
    pub fn new(
        config: Arc<ParsedCommandLine>,
        use_source_of_project_reference: bool,
    ) -> Self {
        ProjectReferenceFileMapper {
            config,
            use_source_of_project_reference,
            dts_directories: rustc_hash::FxHashSet::default(),
            config_to_project_reference: rustc_hash::FxHashMap::default(),
            references_in_config_file: rustc_hash::FxHashMap::default(),
            source_to_project_reference: rustc_hash::FxHashMap::default(),
            output_dts_to_project_reference: rustc_hash::FxHashMap::default(),
            realpath_dts_to_source: Mutex::new(rustc_hash::FxHashMap::default()),
            root_resolved_reference: std::sync::OnceLock::new(),
        }
    }

    /// The `mapper.config` root parent (Go's direct `*tsoptions.ParsedCommandLine`
    /// handoff), wrapped in the newtype once and memoized.
    fn root_resolved_reference(&self) -> Arc<ResolvedProjectReference> {
        self.root_resolved_reference
            .get_or_init(|| {
                Arc::new(ResolvedProjectReference {
                    config: Arc::clone(&self.config),
                })
            })
            .clone()
    }

    /// Go: `func (mapper *projectReferenceFileMapper) resolutionHost(host)`.
    ///
    /// PORT: Go wraps the host in `newProjectReferenceDtsFakingHost` when
    /// source-redirect mode is on with known outputs; the dts-faking host
    /// needs cachedvfs + symlinks (not ported), so the port returns the host
    /// unchanged — see project_reference_dts_faking_host.rs.
    pub fn resolution_host(
        &self,
        host: Arc<dyn tsc_module::ResolutionHost>,
    ) -> Arc<dyn tsc_module::ResolutionHost> {
        host
    }

    /// Go: `func (mapper *projectReferenceFileMapper) rootConfigPath() tspath.PathKey`.
    ///
    /// PORT: Go reads `config.ConfigFile.SourceFile.PathKey()`; the Rust
    /// ParsedCommandLine has no parsed ConfigFile (the AST-config wave), so
    /// the root config path is `config_file_name`'s path key (Go's
    /// ConfigFile.SourceFile.PathKey is the same file's key).
    pub(crate) fn root_config_path(&self) -> PathKey {
        if self.config.config_file_name.as_string().is_empty() {
            return PathKey::default();
        }
        self.config.case_sensitivity.path_key_str(self.config.config_file_name.as_string())
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getParseFileRedirect(file ast.HasFileName)`.
    pub fn get_parse_file_redirect(&self, file: &HasFileName) -> (RootedFilePath, PathKey) {
        if self.use_source_of_project_reference {
            // Map to source file from project reference
            let source = self
                .get_project_reference_from_output_dts(&file.path)
                .or_else(|| {
                    self.realpath_dts_to_source
                        .lock()
                        .expect("realpathDtsToSource poisoned")
                        .get(&file.path)
                        .cloned()
                        .flatten()
                });
            if let Some(source) = source {
                return (source.source.clone(), source.source_path.clone());
            }
        } else {
            // Map to dts file from project reference
            if let Some(output) = self.get_project_reference_from_source(&file.path) {
                if !output.output_dts.is_empty() {
                    return (output.output_dts.clone(), output.output_dts_path.clone());
                }
            }
        }
        (RootedFilePath::default(), PathKey::default())
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getResolvedProjectReferences()`.
    pub fn get_resolved_project_references(&self) -> Vec<Arc<ResolvedProjectReference>> {
        let refs = self.references_in_config_file.get(&self.root_config_path());
        let mut result = Vec::new();
        if let Some(refs) = refs {
            result.reserve(refs.len());
            for ref_path in refs {
                if let Some(Some(ref_config)) = self.config_to_project_reference.get(ref_path) {
                    result.push(Arc::clone(ref_config));
                }
            }
        }
        result
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getProjectReferenceFromSource(path)`.
    pub fn get_project_reference_from_source(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        self.source_to_project_reference.get(path).cloned()
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getProjectReferenceFromOutputDts(path)`.
    pub fn get_project_reference_from_output_dts(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        self.output_dts_to_project_reference.get(path).cloned()
    }

    /// Go: `func (mapper *projectReferenceFileMapper) isSourceFromProjectReference(path)`.
    pub fn is_source_from_project_reference(&self, path: &PathKey) -> bool {
        self.use_source_of_project_reference
            && self.get_project_reference_from_source(path).is_some()
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getCompilerOptionsForFile(file)`.
    pub fn get_compiler_options_for_file(
        &self,
        compiler_options: &Arc<CompilerOptions>,
        file: &HasFileName,
    ) -> Arc<CompilerOptions> {
        let redirect = self.get_redirect_parsed_command_line_for_resolution(file);
        compiler_options_with_redirect(compiler_options, redirect)
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getRedirectParsedCommandLineForResolution(file)`.
    pub fn get_redirect_parsed_command_line_for_resolution(
        &self,
        file: &HasFileName,
    ) -> Option<Arc<ResolvedProjectReference>> {
        self.get_redirect_for_resolution(file).0
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getRedirectForResolution(file)`.
    pub fn get_redirect_for_resolution(
        &self,
        file: &HasFileName,
    ) -> (Option<Arc<ResolvedProjectReference>>, RootedFilePath) {
        let path = &file.path;
        // Check if outputdts of source file from project reference
        if let Some(output) = self.get_project_reference_from_source(path) {
            return (output.resolved.clone(), output.source.clone());
        }

        // Source file from project reference
        if let Some(result_from_dts) = self.get_project_reference_from_output_dts(path) {
            return (result_from_dts.resolved.clone(), result_from_dts.source.clone());
        }

        let realpath_dts_to_source = self
            .realpath_dts_to_source
            .lock()
            .expect("realpathDtsToSource poisoned")
            .get(path)
            .cloned()
            .flatten();
        if let Some(realpath_dts_to_source) = realpath_dts_to_source {
            return (realpath_dts_to_source.resolved.clone(), realpath_dts_to_source.source.clone());
        }
        (None, file.file_name.clone())
    }

    /// Go: `func (mapper *projectReferenceFileMapper) getResolvedReferenceFor(path)`.
    pub fn get_resolved_reference_for(
        &self,
        path: &PathKey,
    ) -> Option<Arc<ResolvedProjectReference>> {
        self.config_to_project_reference
            .get(path)
            .cloned()
            .flatten()
    }

    /// Go: `func (mapper *projectReferenceFileMapper) rangeResolvedProjectReference(f)`.
    pub fn range_resolved_project_reference(
        &self,
        f: crate::project_reference_file_mapper::RangeResolvedProjectReferenceCallback<'_>,
    ) -> bool {
        if self.config.project_references().is_empty() {
            return false;
        }
        let mut seen_ref: rustc_hash::FxHashSet<PathKey> =
            rustc_hash::FxHashSet::with_capacity_and_hasher(
                self.references_in_config_file.len(),
                Default::default(),
            );
        let root_config_path = self.root_config_path();
        seen_ref.insert(root_config_path.clone());
        let refs = self
            .references_in_config_file
            .get(&root_config_path)
            .cloned()
            .unwrap_or_default();
        // Go: `mapper.rangeResolvedReferenceWorker(refs, f, mapper.config,
        // seenRef)` — the root ParsedCommandLine is the parent.
        let root_parent = self.root_resolved_reference();
        self.range_resolved_reference_worker(
            &refs,
            f,
            Some(&root_parent),
            &mut seen_ref,
        )
    }

    /// Go: `func (mapper *projectReferenceFileMapper) rangeResolvedReferenceWorker(...)`.
    fn range_resolved_reference_worker(
        &self,
        references: &[PathKey],
        f: crate::project_reference_file_mapper::RangeResolvedProjectReferenceCallback<'_>,
        parent: Option<&Arc<ResolvedProjectReference>>,
        seen_ref: &mut rustc_hash::FxHashSet<PathKey>,
    ) -> bool {
        for (index, path) in references.iter().enumerate() {
            if !seen_ref.insert(path.clone()) {
                continue;
            }
            let config = self
                .config_to_project_reference
                .get(path)
                .cloned()
                .flatten();
            let root_config: Option<Arc<ResolvedProjectReference>> = None;
            let _ = root_config;
            let parent_ref = parent.cloned();
            if !f(path, config.as_ref(), parent_ref.as_ref(), index) {
                return false;
            }
            let sub_refs = self
                .references_in_config_file
                .get(path)
                .cloned()
                .unwrap_or_default();
            let config_arc = config;
            if !self.range_resolved_reference_worker(
                &sub_refs,
                f,
                config_arc.as_ref(),
                seen_ref,
            ) {
                return false;
            }
        }
        true
    }

    /// Go: `func (mapper *projectReferenceFileMapper) rangeResolvedProjectReferenceInChildConfig(childConfig, f)`.
    ///
    /// PORT: Go keys off `childConfig.ConfigFile.SourceFile.PathKey()`; the
    /// Rust ParsedCommandLine has no parsed ConfigFile, so the child root is
    /// the command line's own config_file_name key.
    pub fn range_resolved_project_reference_in_child_config(
        &self,
        child_config: &Arc<ParsedCommandLine>,
        f: crate::project_reference_file_mapper::RangeResolvedProjectReferenceCallback<'_>,
    ) -> bool {
        if child_config.config_file_name.as_string().is_empty() {
            return false;
        }
        let child_path = child_config
            .case_sensitivity
            .path_key_str(child_config.config_file_name.as_string());
        let mut seen_ref: rustc_hash::FxHashSet<PathKey> =
            rustc_hash::FxHashSet::with_capacity_and_hasher(
                self.references_in_config_file.len(),
                Default::default(),
            );
        seen_ref.insert(child_path.clone());
        let refs = self
            .references_in_config_file
            .get(&child_path)
            .cloned()
            .unwrap_or_default();
        // Go: `mapper.rangeResolvedReferenceWorker(refs, f, mapper.config,
        // seenRef)` — the ROOT config (not the child) is the parent.
        let root_parent = self.root_resolved_reference();
        self.range_resolved_reference_worker(&refs, f, Some(&root_parent), &mut seen_ref)
    }
}

/// Go: `type projectReferenceFileMapperBuilder struct`.
pub struct ProjectReferenceFileMapperBuilder {
    /// Go: the embedded `*projectReferenceFileMapper`.
    pub mapper: Arc<ProjectReferenceFileMapper>,
    pub host: Arc<dyn tsc_module::ResolutionHost>,
}

/// The shared `Option<Arc<ResolvedProjectReference>>` → module redirect
/// conversion (Go passes `*tsoptions.ParsedCommandLine` directly; the port
/// unsizes the newtype — see the file header). `Option<Arc<_>>` →
/// `Option<Arc<dyn _>>` is not a coercion site, so the unsizing happens
/// per-entry at a binding.
pub(crate) fn redirect_as_dyn(
    redirect: Option<Arc<ResolvedProjectReference>>,
) -> Option<Arc<dyn tsc_module::ResolvedProjectReference>> {
    redirect.map(|redirect| {
        let dyn_redirect: Arc<dyn tsc_module::ResolvedProjectReference> = redirect;
        dyn_redirect
    })
}

/// The shared `Option<Arc<ResolvedProjectReference>>` → module redirect
/// conversion (Go passes `*tsoptions.ParsedCommandLine` directly; the port
/// unsizes the newtype — see the file header).
pub(crate) fn compiler_options_with_redirect(
    compiler_options: &Arc<CompilerOptions>,
    redirect: Option<Arc<ResolvedProjectReference>>,
) -> Arc<CompilerOptions> {
    match redirect {
        Some(redirect) => {
            let dyn_redirect: Arc<dyn tsc_module::ResolvedProjectReference> = redirect;
            tsc_module::get_compiler_options_with_redirect(compiler_options, Some(&dyn_redirect))
        }
        None => tsc_module::get_compiler_options_with_redirect(compiler_options, None),
    }
}

impl ProjectReferenceFileMapperBuilder {
    /// Go: `func (builder *projectReferenceFileMapperBuilder) getParseFileRedirect(file)`.
    pub fn get_parse_file_redirect(&self, file: &HasFileName) -> (RootedFilePath, PathKey) {
        if self.mapper.use_source_of_project_reference
            && self
                .mapper
                .get_project_reference_from_output_dts(&file.path)
                .is_none()
        {
            self.resolve_symlink(file);
        }
        self.mapper.get_parse_file_redirect(file)
    }

    /// Go: `func (builder *projectReferenceFileMapperBuilder) getRedirectForResolution(file)`.
    pub fn get_redirect_for_resolution(
        &self,
        file: &HasFileName,
    ) -> (Option<Arc<ResolvedProjectReference>>, RootedFilePath) {
        if self.mapper.get_project_reference_from_source(&file.path).is_none()
            && self
                .mapper
                .get_project_reference_from_output_dts(&file.path)
                .is_none()
        {
            self.resolve_symlink(file);
        }
        self.mapper.get_redirect_for_resolution(file)
    }

    /// Go: `func (builder *projectReferenceFileMapperBuilder) getCompilerOptionsForFile(file)`.
    pub fn get_compiler_options_for_file(
        &self,
        compiler_options: &Arc<CompilerOptions>,
        file: &HasFileName,
    ) -> Arc<CompilerOptions> {
        let redirect = self.get_redirect_parsed_command_line_for_resolution(file);
        compiler_options_with_redirect(compiler_options, redirect)
    }

    /// Go: `func (builder *projectReferenceFileMapperBuilder) getRedirectParsedCommandLineForResolution(file)`.
    pub fn get_redirect_parsed_command_line_for_resolution(
        &self,
        file: &HasFileName,
    ) -> Option<Arc<ResolvedProjectReference>> {
        self.get_redirect_for_resolution(file).0
    }

    /// Go: `func (builder *projectReferenceFileMapperBuilder) resolveSymlink(file)`.
    ///
    /// PORT: the cachedvfs/symlinks-dependent realpath arm degrades to the
    /// `not a symlink` caching (Store(path, nil)) — the resolved-output map is
    /// empty in this wave anyway (ParseInputOutputNames is not ported).
    pub fn resolve_symlink(&self, file: &HasFileName) {
        let path = &file.path;
        let cache = self
            .mapper
            .realpath_dts_to_source
            .lock()
            .expect("realpathDtsToSource poisoned");
        if cache.contains_key(path) {
            return;
        }
        let has_project_references = !self.config_resolved_project_reference_paths().is_empty();
        drop(cache);
        if has_project_references
            && self.mapper.config.compiler_options().preserve_symlinks
                == tsc_core::tristate::Tristate::TSTrue
        {
            let file_name = &file.file_name;
            if !file_name.contains_lowercase_directory_sequence("/node_modules/") {
                self.mapper
                    .realpath_dts_to_source
                    .lock()
                    .expect("realpathDtsToSource poisoned")
                    .insert(path.clone(), None);
            } else {
                // Go: realpath the file through the fs and re-map through the
                // output-dts table; symlinks.KnownSymlinks + cachedvfs are not
                // ported — the direct comparison arm only (PORTING-NOTES).
                let real_declaration_path = self.host.fs().case_sensitivity().path_key_str(
                    self.host
                        .fs()
                        .realpath(&tsc_tspath::RootedPath::from(file_name.as_string()))
                        .as_string(),
                );
                let entry = if real_declaration_path == *path {
                    None
                } else {
                    self.mapper
                        .get_project_reference_from_output_dts(&real_declaration_path)
                };
                self.mapper
                    .realpath_dts_to_source
                    .lock()
                    .expect("realpathDtsToSource poisoned")
                    .insert(path.clone(), entry);
            }
        }
    }

    /// Go: `config.ResolvedProjectReferencePaths()` — the resolved reference
    /// config paths (the tsoptions method this wave ports).
    fn config_resolved_project_reference_paths(&self) -> Vec<RootedFilePath> {
        self.mapper.config.resolved_project_reference_paths()
    }

    /// Go: `mapper.rootConfigPath()` through the embedded mapper.
    pub fn root_config_path(&self) -> PathKey {
        self.mapper.root_config_path()
    }
}
