// Ported from tsc/internal/compiler/program.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go embeds `processedFiles` in `Program`; the port repeats the fields
//     flat (Rust has no embedding) — `new_program` moves the loader's output
//     in field by field.
//   - `lazyValue[T]`/`sync.Once` caches become `OnceLock` (interior
//     immutability); the mutation-needing ones (`programDiagnostics`,
//     appended by `blockEmittingOfFile`) are `Mutex<Vec<..>>`.
//   - The checker surfaces are the parallel checker-crate wave (see
//     checkerpool.rs): `initCheckerPool`/`compilerCheckerPool`,
//     `GetTypeChecker*`, `ForEachCheckerParallel`, `GetSemanticDiagnostics`/
//     `GetDeclarationDiagnostics`/`GetSuggestionDiagnostics`, and the
//     checker halves of `collectCheckerDiagnostics*` are TODO-shaped — the
//     ProgramLike implementations return empty diagnostics until that wave.
//   - `UpdateProgram`/`ReuseProgram` (the incremental watch surface) are not
//     ported: they need the content-mapper/cachedvfs machinery and Go's
//     shared `*ast.SourceFile` pointer graph (the port's files are owned).
//   - `verifyCompilerOptions`/`verifyProjectReferences` are degraded to the
//     option-syntax-free surface: Go positions every option diagnostic on the
//     tsconfig `compilerOptions` object literal (the AST-config wave, see
//     lib.rs); without it the checks would report unpositioned duplicates of
//     diagnostics the M6/M7 config waves will position. The
//     `hasEmitBlockingDiagnostics`/`blockEmittingOfFile` surface they feed is
//     ported so the waves plug in here (PORTING-NOTES).
//   - `collectContentMapperOptionDiagnostics` is the content-mapper seam
//     (always empty in this wave — see file_loader.rs).
//   - `tracing` is not ported (no tracing crate yet).
//   - `ExplainFiles` needs the locale surface; deferred with it.
//   - `GetSymlinkCache` needs symlinks/cachedvfs; deferred with them.
//   - `SymbolCount`/`TypeCount`/`InstantiationCount` are checker surfaces
//     (deferred); `LineCount`/`IdentifierCount` need the line-map/identifier
//     table surfaces (deferred with them).
//   - `FilterNoEmitSemanticDiagnostics` + `plainJSErrors` are the noEmit
//     semantic filter (execute-layer surface, checker wave) — deferred.
//   - `getDiagnosticsWithPrecedingDirectives` (the ts-expect-error filtering)
//     is deferred with the comment-directive surface; the M5 diagnostic
//     collectors return the unfiltered lists (PORTING-NOTES).
//   - `getAdditionalJSSyntacticDiagnostics` (the unchecked-JS decorator walk)
//     needs the visitor/SubtreeFacts surface — deferred with it.
//   - Go's diagnostic slices (`[]*ast.Diagnostic`) become
//     `Vec<FileDiagnostic>` here and in the collectors (the file-id pairing
//     the parser's `file_id = 0` gap forces — see diagnostics_collection.rs).

use std::sync::{Arc, Mutex, OnceLock};

use tsc_ast::{
    Diagnostic, FileReference, NodeId, NodeStore, SourceFile, SourceFileMetaData,
};
use tsc_core::compileroptions::{
    ResolutionMode, ResolutionModeCommonJS, ResolutionModeESM, ResolutionModeNone,
};
use tsc_core::options_generated::{CompilerOptions, ModuleKind};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_module::{
    ModeAwareCache, ModeAwareCacheKey, ResolutionHost, ResolvedModule, ResolverOptions,
    ResolvedTypeReferenceDirective,
};
use tsc_tspath::{CaseSensitivity, PathKey, RootedDirectoryPath, RootedFilePath, RootedPath};

use crate::diagnostics_collection::{
    FileDiagnostic, sort_and_deduplicate_diagnostics,
};
use crate::file_include::FileIncludeReason;
use crate::file_loader::{
    DuplicateSourceFile, LibFile, ProcessedFiles, RedirectsFile,
    get_default_resolution_mode_for_file as get_default_resolution_mode_for_file_worker,
    import_syntax_affects_module_resolution, process_all_program_files,
};
use crate::host::{CompilerHost, CompilerResolutionHost, HasFileName};
use crate::include_processor::IncludeProcessor;
use crate::processing_diagnostic::ProcessingDiagnostic;
use crate::project_reference_file_mapper::{
    ProjectReferenceFileMapper, SourceOutputAndProjectReference,
};

// ────────────────────────────────────────────────────────────────────────────
// ProgramOptions / ProgramConfig / ProgramHosts / ProgramFactories
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ProgramOptions struct` (the embedded structs become named
/// fields).
pub struct ProgramOptions {
    /// Go: the embedded `ProgramConfig`.
    pub config: ProgramConfig,
    /// Go: the embedded `ProgramHosts`.
    pub hosts: ProgramHosts,
    /// Go: the embedded `ProgramFactories`.
    pub factories: ProgramFactories,
}

/// Go: `type ProgramConfig struct`.
#[derive(Clone, Default)]
pub struct ProgramConfig {
    /// Go: `Config *tsoptions.ParsedCommandLine`.
    pub config: Arc<tsc_tsoptions::ParsedCommandLine>,
    /// Go: `UseSourceOfProjectReference bool`.
    pub use_source_of_project_reference: bool,
    /// Go: `SingleThreaded core.Tristate`.
    pub single_threaded: Tristate,
    /// Go: `TypingsLocation tspath.RootedDirectoryPath`.
    pub typings_location: RootedDirectoryPath,
    /// Go: `ProjectName string`.
    pub project_name: String,
    /// SkipModuleResolution avoids all module and type reference resolution
    /// while still collecting import metadata needed for emit.
    ///
    /// Go: `SkipModuleResolution bool`.
    pub skip_module_resolution: bool,
}

impl ProgramConfig {
    /// Go: `func (p *ProgramConfig) canUseProjectReferenceSource() bool`.
    pub fn can_use_project_reference_source(&self) -> bool {
        self.use_source_of_project_reference
            && !self
                .config
                .compiler_options()
                .disable_source_of_project_reference_redirect
                .is_true()
    }

    /// Go: `func (p *ProgramConfig) CompilerOptions()` — the parsed config's
    /// options.
    pub fn compiler_options(&self) -> &CompilerOptions {
        self.config.compiler_options()
    }

    /// The shared-options helper (Go hands the parsed config's
    /// `*core.CompilerOptions` pointer around; the port clones once per
    /// program construction — see `Program::compiler_options`).
    pub(crate) fn compiler_options_arc(&self) -> Arc<CompilerOptions> {
        Arc::new(self.compiler_options().clone())
    }

    fn resolved_single_threaded(&self) -> bool {
        let single_threaded = match self.single_threaded {
            Tristate::TSUnknown => self.compiler_options().single_threaded,
            value => value,
        };
        single_threaded == Tristate::TSTrue
    }
}

/// Go: `type ProgramHosts struct`.
///
/// PORT: `Tracing *tracing.Tracing` is dropped (the tracing crate is not
/// ported); `Host` is the nil-able interface (Go nil) → `Option<Arc<..>>`.
#[derive(Clone, Default)]
pub struct ProgramHosts {
    /// Go: `Host CompilerHost`.
    pub host: Option<Arc<dyn CompilerHost>>,
}

/// Go: `type ProgramFactories struct`.
#[derive(Default)]
pub struct ProgramFactories {
    /// Go: `CreateCheckerPool func(*Program) CheckerPool` — the checker wave
    /// seam (see checkerpool.rs); the field keeps the factory shape.
    /// (Go's function fields become `Option<Arc<dyn Fn>>` — the crate
    /// convention for Go's nil-able func fields; type-complexity is inherent
    /// to the Go shape.)
    #[allow(clippy::type_complexity)]
    pub create_checker_pool: Option<
        Arc<dyn Fn(&Program) -> Arc<dyn crate::checkerpool::CheckerPool>>,
    >,
    /// Go: `CreateModuleResolver func(module.ResolverOptions) module.Resolver`.
    #[allow(clippy::type_complexity)]
    pub create_module_resolver:
        Option<Arc<dyn Fn(ResolverOptions) -> Arc<dyn tsc_module::Resolver> + Send + Sync>>,
}

// ────────────────────────────────────────────────────────────────────────────
// Program
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type Program struct` (+ the embedded `processedFiles`).
pub struct Program {
    /// Go: `opts ProgramConfig`.
    pub opts: ProgramConfig,
    /// Go: `hosts ProgramHosts`.
    pub hosts: ProgramHosts,
    /// Go: `resolutionData *module.ResolutionData`.
    pub resolution_data: Arc<tsc_module::ResolutionData>,
    /// Go: `checkerPool CheckerPool` — the checker wave seam (checkerpool.rs);
    /// always `None` until that wave ports the pool constructor.
    pub checker_pool: Option<Arc<dyn crate::checkerpool::CheckerPool>>,
    /// Go: `compilerCheckerPool *checkerPool` — same seam.
    pub compiler_checker_pool: Option<Arc<dyn crate::checkerpool::CheckerPool>>,
    /// Go: `includeProcessor includeProcessor`.
    pub include_processor: IncludeProcessor,
    /// Go: `moduleResolutionError error` — dropped (the Rust module crate
    /// dropped the vestigial error; see file_loader.rs).
    /// Go: `caseSensitivity tspath.CaseSensitivity`.
    pub case_sensitivity: CaseSensitivity,
    /// The shared options (Go: `opts.Config.CompilerOptions()` pointer).
    pub(crate) compiler_options: Arc<CompilerOptions>,

    // processedFiles (Go: embedded).
    /// Go: `files []*ast.SourceFile` — lib files first, then program files.
    pub files: Vec<SourceFile>,
    /// Go: `filesByPath map[tspath.PathKey]*ast.SourceFile` — indexes into
    /// `files`.
    pub files_by_path: rustc_hash::FxHashMap<PathKey, usize>,
    /// Go: `duplicateSourceFiles []*DuplicateSourceFile`.
    pub duplicate_source_files: Vec<DuplicateSourceFile>,
    /// Go: `projectReferenceFileMapper *projectReferenceFileMapper`.
    pub project_reference_file_mapper: Option<Arc<ProjectReferenceFileMapper>>,
    /// Go: `missingFiles collections.Set[tspath.PathKey]`.
    pub missing_files: rustc_hash::FxHashSet<PathKey>,
    /// Go: `resolvedModules map[...]module.ModeAwareCache[*module.ResolvedModule]`.
    pub resolved_modules:
        rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedModule>>>>,
    /// Go: `typeResolutionsInFile`.
    pub type_resolutions_in_file:
        rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>>>,
    /// Go: `sourceFileMetaDatas map[tspath.PathKey]ast.SourceFileMetaData`.
    pub source_file_meta_datas: rustc_hash::FxHashMap<PathKey, SourceFileMetaData>,
    /// Go: `jsxRuntimeImportSpecifiers map[PathKey]*jsxRuntimeImportSpecifier`
    /// — the module reference text.
    pub jsx_runtime_import_specifiers: rustc_hash::FxHashMap<PathKey, String>,
    /// Go: `importHelpersImportSpecifiers map[PathKey]*ast.StringLiteralNode`
    /// — the module reference text.
    pub import_helpers_import_specifiers: rustc_hash::FxHashMap<PathKey, String>,
    /// Go: `libFiles map[tspath.PathKey]*LibFile`.
    pub lib_files: rustc_hash::FxHashMap<PathKey, Arc<LibFile>>,
    /// List of present unsupported extensions (Go comment).
    pub source_files_found_searching_node_modules: rustc_hash::FxHashSet<PathKey>,
    /// Go: `fileIncludeReasons map[PathKey][]*FileIncludeReason`.
    pub file_include_reasons:
        rustc_hash::FxHashMap<PathKey, Vec<Arc<FileIncludeReason>>>,
    /// Go: `processingDiagnostics []*processingDiagnostic`.
    pub processing_diagnostics: Vec<ProcessingDiagnostic>,
    /// if file was included using source file and its output is actually part
    /// of program this contains mapping from output to source file.
    pub output_file_to_project_reference_source:
        Option<rustc_hash::FxHashMap<PathKey, RootedFilePath>>,
    /// Key is a file path. Value is the list of files that redirect to it
    /// (same package, different install location).
    pub redirect_targets_map: rustc_hash::FxHashMap<PathKey, Vec<RootedFilePath>>,
    /// filesByPath for redirect files.
    pub redirect_files_by_path: rustc_hash::FxHashMap<PathKey, RedirectsFile>,
    /// Program-level diagnostics reported when a content mapper fails
    /// fatally — the content-mapper seam (always empty in this wave).
    pub content_mapper_diagnostics: Vec<Arc<Diagnostic>>,
    /// Go: `finishedProcessing bool`.
    pub finished_processing: bool,

    // Program-only state.
    /// Go: `programDiagnostics []*ast.Diagnostic` (verifyCompilerOptions) +
    /// `hasEmitBlockingDiagnostics collections.Set[tspath.PathKey]`.
    pub program_diagnostics: Mutex<Vec<FileDiagnostic>>,
    pub has_emit_blocking_diagnostics: rustc_hash::FxHashSet<PathKey>,
    /// Go: `usesUriStyleNodeCoreModules core.Tristate` — the loader's
    /// module-resolution surface sets it; the seam keeps the Go zero value.
    pub uses_uri_style_node_core_modules: Tristate,
    /// Go: `commonSourceDirectory` + `commonSourceDirectoryOnce`.
    common_source_directory: OnceLock<RootedDirectoryPath>,
}

impl Default for Program {
    /// The Go zero-Program shape (constructed for tests that populate the
    /// file maps directly — see program_test.rs).
    fn default() -> Self {
        Program {
            opts: ProgramConfig::default(),
            hosts: ProgramHosts::default(),
            resolution_data: Arc::new(tsc_module::ResolutionData::default()),
            checker_pool: None,
            compiler_checker_pool: None,
            include_processor: IncludeProcessor::default(),
            case_sensitivity: CaseSensitivity::default(),
            compiler_options: Arc::new(CompilerOptions::default()),
            files: Vec::new(),
            files_by_path: rustc_hash::FxHashMap::default(),
            duplicate_source_files: Vec::new(),
            project_reference_file_mapper: None,
            missing_files: rustc_hash::FxHashSet::default(),
            resolved_modules: rustc_hash::FxHashMap::default(),
            type_resolutions_in_file: rustc_hash::FxHashMap::default(),
            source_file_meta_datas: rustc_hash::FxHashMap::default(),
            jsx_runtime_import_specifiers: rustc_hash::FxHashMap::default(),
            import_helpers_import_specifiers: rustc_hash::FxHashMap::default(),
            lib_files: rustc_hash::FxHashMap::default(),
            source_files_found_searching_node_modules: rustc_hash::FxHashSet::default(),
            file_include_reasons: rustc_hash::FxHashMap::default(),
            processing_diagnostics: Vec::new(),
            output_file_to_project_reference_source: None,
            redirect_targets_map: rustc_hash::FxHashMap::default(),
            redirect_files_by_path: rustc_hash::FxHashMap::default(),
            content_mapper_diagnostics: Vec::new(),
            finished_processing: false,
            program_diagnostics: Mutex::new(Vec::new()),
            has_emit_blocking_diagnostics: rustc_hash::FxHashSet::default(),
            uses_uri_style_node_core_modules: Tristate::TSUnknown,
            common_source_directory: OnceLock::new(),
        }
    }
}

/// Go: `func NewProgram(opts ProgramOptions) *Program`.
pub fn new_program(opts: ProgramOptions) -> Program {
    let host = opts
        .hosts
        .host
        .clone()
        .expect("NewProgram: ProgramOptions.Hosts.Host must be set");
    let case_sensitivity = host.fs().case_sensitivity();
    let single_threaded = opts.config.resolved_single_threaded();
    let compiler_options = opts.config.compiler_options_arc();

    let (processed, resolution_data) = process_all_program_files(&opts, single_threaded);
    let ProcessedFiles {
        files,
        duplicate_source_files,
        files_by_path,
        project_reference_file_mapper,
        missing_files,
        resolved_modules,
        type_resolutions_in_file,
        source_file_meta_datas,
        jsx_runtime_import_specifiers,
        import_helpers_import_specifiers,
        lib_files,
        source_files_found_searching_node_modules,
        file_include_reasons,
        processing_diagnostics,
        output_file_to_project_reference_source,
        redirect_targets_map,
        redirect_files_by_path,
        content_mapper_diagnostics,
        finished_processing,
    } = processed;

    let mut program = Program {
        opts: opts.config,
        hosts: opts.hosts,
        resolution_data,
        // Go: initCheckerPool — the checker wave seam (checkerpool.rs); the
        // pools stay None until that wave ports the pool constructor.
        checker_pool: None,
        compiler_checker_pool: None,
        include_processor: IncludeProcessor::default(),
        case_sensitivity,
        compiler_options,
        files,
        files_by_path,
        duplicate_source_files,
        project_reference_file_mapper,
        missing_files,
        resolved_modules,
        type_resolutions_in_file,
        source_file_meta_datas,
        jsx_runtime_import_specifiers,
        import_helpers_import_specifiers,
        lib_files,
        source_files_found_searching_node_modules,
        file_include_reasons,
        processing_diagnostics,
        output_file_to_project_reference_source,
        redirect_targets_map,
        redirect_files_by_path,
        content_mapper_diagnostics,
        finished_processing,
        program_diagnostics: Mutex::new(Vec::new()),
        has_emit_blocking_diagnostics: rustc_hash::FxHashSet::default(),
        uses_uri_style_node_core_modules: Tristate::TSUnknown,
        common_source_directory: OnceLock::new(),
    };

    // Go: verifyCompilerOptions + collectContentMapperOptionDiagnostics —
    // the degraded seams (see the file header).
    program.verify_compiler_options();
    program.collect_content_mapper_option_diagnostics();
    program
}

impl Program {
    /// Go: `func (p *Program) Options() *core.CompilerOptions`.
    pub fn options(&self) -> &CompilerOptions {
        &self.compiler_options
    }

    /// Go: `func (p *Program) CommandLine() *tsoptions.ParsedCommandLine`.
    pub fn command_line(&self) -> &tsc_tsoptions::ParsedCommandLine {
        &self.opts.config
    }

    /// Go: `func (p *Program) Host() CompilerHost`.
    pub fn host(&self) -> &Arc<dyn CompilerHost> {
        self.hosts
            .host
            .as_ref()
            .expect("program host must be set")
    }

    /// Go: `func (p *Program) SourceFiles() []*ast.SourceFile`.
    pub fn source_files(&self) -> &[SourceFile] {
        &self.files
    }

    /// Go: `func (p *Program) GetSourceFiles() []*ast.SourceFile`.
    pub fn get_source_files(&self) -> &[SourceFile] {
        &self.files
    }

    /// Go: `func (p *Program) DuplicateSourceFiles() []*DuplicateSourceFile`.
    pub fn duplicate_source_files(&self) -> &[DuplicateSourceFile] {
        &self.duplicate_source_files
    }

    /// Go: `func (p *Program) BaseDirectory() tspath.RootedDirectoryPath`.
    pub fn base_directory(&self) -> &RootedDirectoryPath {
        self.opts.config.base_directory()
    }

    /// Go: `func (p *Program) GetCurrentDirectory()`.
    pub fn get_current_directory(&self) -> RootedDirectoryPath {
        self.base_directory().clone()
    }

    /// Go: `func (p *Program) GetGlobalTypingsCacheLocation()`.
    pub fn get_global_typings_cache_location(&self) -> &RootedDirectoryPath {
        &self.opts.typings_location
    }

    /// Go: `func (p *Program) CaseSensitivity() tspath.CaseSensitivity`.
    pub fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
    }

    /// Go: `func (p *Program) UseCaseSensitiveFileNames() bool`.
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitivity.is_case_sensitive()
    }

    /// Go: `func (p *Program) UsesUriStyleNodeCoreModules() core.Tristate`.
    pub fn uses_uri_style_node_core_modules(&self) -> Tristate {
        self.uses_uri_style_node_core_modules
    }

    /// Go: `func (p *Program) SingleThreaded() bool`.
    pub fn single_threaded(&self) -> bool {
        self.opts.resolved_single_threaded()
    }

    /// Go: `func (p *Program) FileExists(path tspath.RootedFilePath) bool`.
    pub fn file_exists(&self, path: &RootedFilePath) -> bool {
        self.host().fs().file_exists(path)
    }

    /// Go: `func (p *Program) PathKeyForFileName(fileName) tspath.PathKey`.
    pub fn path_key_for_file_name(&self, file_name: &RootedFilePath) -> PathKey {
        self.case_sensitivity
            .path_key(&RootedPath::from(file_name.as_string()))
    }

    /// Go: `func (p *Program) GetSourceFile(fileName) *ast.SourceFile`.
    pub fn get_source_file(&self, file_name: &RootedFilePath) -> Option<&SourceFile> {
        self.get_source_file_by_path(&self.path_key_for_file_name(file_name))
    }

    /// Go: `func (p *Program) GetSourceFileByPath(path) *ast.SourceFile`.
    pub fn get_source_file_by_path(&self, path: &PathKey) -> Option<&SourceFile> {
        self.files_by_path
            .get(path)
            .map(|&index| &self.files[index])
    }

    /// Go: `func (p *Program) FilesByPath() map[...]` — the merged-list
    /// indexes (PORT: Go's `*ast.SourceFile` values).
    pub fn files_by_path(&self) -> &rustc_hash::FxHashMap<PathKey, usize> {
        &self.files_by_path
    }

    /// The `filesByPath` lookup as an index (the port's file-pair shape for
    /// the include machinery — see file_include.rs).
    pub fn file_index_by_path(&self, path: &PathKey) -> Option<usize> {
        self.files_by_path.get(path).copied()
    }

    /// Go: `func (p *Program) GetSourceFileForResolvedModule(resolved)`.
    pub fn get_source_file_for_resolved_module(
        &self,
        resolved: &ResolvedModule,
    ) -> Option<&SourceFile> {
        self.get_source_file_with_redirect(&resolved.resolved_file_name, &resolved.resolved_path)
    }

    /// Go: `func (p *Program) getSourceFileWithRedirect(fileName, path)`.
    pub fn get_source_file_with_redirect(
        &self,
        file_name: &RootedFilePath,
        path: &PathKey,
    ) -> Option<&SourceFile> {
        let file = self.get_source_file_by_path(path);
        if file.is_none() {
            let filename = self.get_parse_file_redirect_by_file_name(file_name);
            if !filename.as_string().is_empty() {
                return self.get_source_file(&filename);
            }
        }
        file
    }

    /// Go: `func (p *Program) GetParseFileRedirect(fileName)`.
    pub fn get_parse_file_redirect(&self, file_name: &RootedFilePath) -> RootedFilePath {
        self.get_parse_file_redirect_by_file_name(file_name)
    }

    /// Go: `func (p *Program) getParseFileRedirectByFileName(fileName)`.
    fn get_parse_file_redirect_by_file_name(&self, file_name: &RootedFilePath) -> RootedFilePath {
        self.project_reference_file_mapper
            .as_ref()
            .map(|mapper| {
                mapper.get_parse_file_redirect(&HasFileName::new(
                    file_name.clone(),
                    self.path_key_for_file_name(file_name),
                ))
            })
            .map(|(redirect, _)| redirect)
            .unwrap_or_default()
    }

    /// Go: `func (p *Program) HasSameFileNames(other *Program) bool`.
    pub fn has_same_file_names(&self, other: &Program) -> bool {
        self.files_by_path.len() == other.files_by_path.len()
            && self.files_by_path.iter().all(|(path, &index)| {
                other
                    .files_by_path
                    .get(path)
                    .is_some_and(|&other_index| {
                        self.files[index].file_name() == other.files[other_index].file_name()
                    })
            })
            && self.redirect_files_by_path.len() == other.redirect_files_by_path.len()
            && self.redirect_files_by_path.iter().all(|(path, file)| {
                other
                    .redirect_files_by_path
                    .get(path)
                    .is_some_and(|other_file| file.file_name == other_file.file_name)
            })
    }

    // ── resolution surfaces ────────────────────────────────────────────────

    /// Go: `func (p *Program) newResolver() *module.DefaultResolver`.
    fn new_resolver(&self) -> tsc_module::DefaultResolver {
        let host: Arc<dyn ResolutionHost> = Arc::new(CompilerResolutionHost {
            host: self.host().clone(),
            base_directory: self.base_directory().clone(),
        });
        let host = match &self.project_reference_file_mapper {
            // Go: p.projectReferenceFileMapper.resolutionHost(host) — the
            // dts-faking host is the identity in this port (see
            // project_reference_dts_faking_host.rs).
            Some(mapper) => mapper.resolution_host(host),
            None => host,
        };
        self.resolution_data.new_resolver(host)
    }

    /// Go: `func (p *Program) GetNearestAncestorDirectoryWithPackageJson(dirname)`.
    pub fn get_nearest_ancestor_directory_with_package_json(
        &self,
        dirname: &RootedDirectoryPath,
    ) -> RootedDirectoryPath {
        let scoped = self.new_resolver().get_package_scope_for_path(dirname);
        if let Some(scoped) = scoped {
            if tsc_packagejson::InfoCacheEntry::exists(Some(&scoped)) {
                return scoped.package_directory.as_directory_path().clone();
            }
        }
        RootedDirectoryPath::default()
    }

    /// Go: `func (p *Program) GetResolvedModule(file, moduleReference, mode)`.
    pub fn get_resolved_module(
        &self,
        file: &SourceFile,
        module_reference: &str,
        mode: ResolutionMode,
    ) -> Option<&Arc<ResolvedModule>> {
        if let Some(resolutions) = self.resolved_modules.get(file.path_key()) {
            return resolutions
                .get(&ModeAwareCacheKey {
                    name: module_reference.to_string(),
                    mode,
                })
                .and_then(Option::as_ref);
        }
        None
    }

    /// Go: `func (p *Program) GetResolvedModuleFromModuleSpecifier(file, moduleSpecifier)`.
    pub fn get_resolved_module_from_module_specifier(
        &self,
        file: &SourceFile,
        module_specifier: NodeId,
    ) -> Option<&Arc<ResolvedModule>> {
        let node = file.node(module_specifier);
        if !matches!(
            node.kind,
            tsc_ast::Kind::StringLiteral
                | tsc_ast::Kind::NoSubstitutionTemplateLiteral
                | tsc_ast::Kind::TemplateHead
        ) {
            return None;
        }
        let parent = node.parent.get();
        if crate::ast_shims::is_source_phase_import(file as &dyn tsc_ast::NodeStore, parent) {
            return None;
        }
        let mode = self.get_mode_for_usage_location(file, module_specifier);
        let module_reference = tsc_ast::node_text(file as &dyn tsc_ast::NodeStore, module_specifier);
        self.get_resolved_module(file, module_reference, mode)
    }

    /// Go: `func (p *Program) GetResolvedModules()`.
    pub fn get_resolved_modules(
        &self,
    ) -> &rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedModule>>>> {
        &self.resolved_modules
    }

    /// Go: `func (p *Program) GetResolvedTypeReferenceDirective(file, typeDirectiveName, mode)`.
    pub fn get_resolved_type_reference_directive(
        &self,
        file: &SourceFile,
        type_directive_name: &str,
        mode: ResolutionMode,
    ) -> Option<&Arc<ResolvedTypeReferenceDirective>> {
        if let Some(resolutions) = self.type_resolutions_in_file.get(file.path_key()) {
            return resolutions
                .get(&ModeAwareCacheKey {
                    name: type_directive_name.to_string(),
                    mode,
                })
                .and_then(Option::as_ref);
        }
        None
    }

    /// Go: `func (p *Program) GetResolvedTypeReferenceDirectives()`.
    pub fn get_resolved_type_reference_directives(
        &self,
    ) -> &rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>>>
    {
        &self.type_resolutions_in_file
    }

    /// Go: `func (p *Program) GetResolvedTypeReferenceDirectiveFromTypeReferenceDirective(typeRef, sourceFile)`.
    pub fn get_resolved_type_reference_directive_from_type_reference_directive(
        &self,
        type_ref: &FileReference,
        source_file: &SourceFile,
    ) -> Option<&Arc<ResolvedTypeReferenceDirective>> {
        let mode = if type_ref.mode != ResolutionModeNone {
            type_ref.mode
        } else {
            self.get_default_resolution_mode_for_file(source_file)
        };
        self.get_resolved_type_reference_directive(source_file, &type_ref.file_name, mode)
    }

    /// Go: `func (p *Program) GetSourceFileMetaData(path) ast.SourceFileMetaData`
    /// (the missing-key arm is the zero value).
    pub fn get_source_file_meta_data(&self, path: &PathKey) -> SourceFileMetaData {
        self.source_file_meta_datas
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// The compiler options for a file (Go:
    /// `p.projectReferenceFileMapper.getCompilerOptionsForFile(file)`).
    fn get_compiler_options_for_file(&self, file: &SourceFile) -> Arc<CompilerOptions> {
        self.get_compiler_options_for_file_by(file.path_key(), file.file_name())
    }

    fn get_compiler_options_for_file_by(
        &self,
        path: &PathKey,
        file_name: &RootedFilePath,
    ) -> Arc<CompilerOptions> {
        match &self.project_reference_file_mapper {
            Some(mapper) => {
                let redirect = mapper.get_redirect_parsed_command_line_for_resolution(
                    &HasFileName::new(file_name.clone(), path.clone()),
                );
                crate::project_reference_file_mapper::compiler_options_with_redirect(
                    &self.compiler_options,
                    redirect,
                )
            }
            None => Arc::clone(&self.compiler_options),
        }
    }

    /// Go: `func (p *Program) GetEmitModuleFormatOfFile(sourceFile) core.ModuleKind`.
    pub fn get_emit_module_format_of_file(&self, source_file: &SourceFile) -> ModuleKind {
        crate::ast_shims::get_emit_module_format_of_file_worker(
            source_file.file_name(),
            &self.get_compiler_options_for_file(source_file),
            &self.get_source_file_meta_data(source_file.path_key()),
        )
    }

    /// Go: `func (p *Program) GetDefaultResolutionModeForFile(sourceFile)`.
    pub fn get_default_resolution_mode_for_file(&self, source_file: &SourceFile) -> ResolutionMode {
        get_default_resolution_mode_for_file_worker(
            source_file.file_name(),
            &self.get_source_file_meta_data(source_file.path_key()),
            &self.get_compiler_options_for_file(source_file),
        )
    }

    /// Go: `func (p *Program) GetModeForUsageLocation(sourceFile, location)`.
    pub fn get_mode_for_usage_location(
        &self,
        source_file: &SourceFile,
        location: NodeId,
    ) -> ResolutionMode {
        crate::file_loader::get_mode_for_usage_location(
            source_file,
            source_file.file_name(),
            &self.get_source_file_meta_data(source_file.path_key()),
            location,
            &self.get_compiler_options_for_file(source_file),
        )
    }

    /// Go: `func (p *Program) GetImpliedNodeFormatForEmit(sourceFile)`.
    ///
    /// PORT: keyed by path (the include machinery's call shape — see
    /// include_processor.rs).
    pub fn get_implied_node_format_for_emit_by_path(&self, path: &PathKey) -> ResolutionMode {
        let file_name = self.file_name_for_path(path);
        crate::ast_shims::get_implied_node_format_for_emit_worker(
            &file_name,
            self.get_compiler_options_for_file_by(path, &file_name)
                .get_emit_module_kind(),
            &self.get_source_file_meta_data(path),
        )
    }

    /// The file name for a path key (the program file or the redirect file;
    /// Go's HasFileName carries both).
    fn file_name_for_path(&self, path: &PathKey) -> RootedFilePath {
        self.get_source_file_by_path(path)
            .map(|file| file.file_name().clone())
            .or_else(|| {
                self.redirect_files_by_path
                    .get(path)
                    .map(|redirect| redirect.file_name.clone())
            })
            .unwrap_or_default()
    }

    /// The synthetic import's mode (Go:
    /// `GetModeForUsageLocation(file, syntheticImportHelpersSpecifier)`).
    ///
    /// PORT: the synthetic specifier nodes are carried as text (see
    /// files_parser.rs), so the mode is the file's emit syntax for the
    /// usage — the synthetic import's parent is the synthesized
    /// ImportDeclaration (no attributes, not a require/import call), which is
    /// exactly the fall-through arm of `getModeForUsageLocation`.
    pub fn get_mode_for_synthetic_import(&self, file_index: usize) -> ResolutionMode {
        let file = &self.files[file_index];
        let options_for_file = self.get_compiler_options_for_file(file);
        if import_syntax_affects_module_resolution(&options_for_file) {
            let file_emit_mode = crate::ast_shims::get_emit_module_format_of_file_worker(
                file.file_name(),
                &options_for_file,
                &self.get_source_file_meta_data(file.path_key()),
            );
            if file_emit_mode == ModuleKind::CommonJS {
                return ResolutionModeCommonJS;
            }
            if file_emit_mode.is_non_node_esm() || file_emit_mode == ModuleKind::Preserve {
                return ResolutionModeESM;
            }
        }
        ResolutionModeNone
    }

    // ── project-reference surfaces ──────────────────────────────────────────

    /// Go: `func (p *Program) GetRedirectTargets(path) []tspath.RootedFilePath`.
    pub fn get_redirect_targets(&self, path: &PathKey) -> &[RootedFilePath] {
        self.redirect_targets_map
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Go: `func (p *Program) GetSourceOfProjectReferenceIfOutputIncluded(file)`.
    ///
    /// PORT: Go takes `ast.HasFileName`; the port takes the path key + the
    /// fallback file name (the HasFileName pair).
    pub fn get_source_of_project_reference_if_output_included(
        &self,
        path: &PathKey,
        file_name: &RootedFilePath,
    ) -> RootedFilePath {
        if let Some(source) = self
            .output_file_to_project_reference_source
            .as_ref()
            .and_then(|map| map.get(path))
        {
            return source.clone();
        }
        file_name.clone()
    }

    /// Go: `func (p *Program) GetProjectReferenceFromSource(path)`.
    pub fn get_project_reference_from_source(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        self.project_reference_file_mapper
            .as_ref()
            .and_then(|mapper| mapper.get_project_reference_from_source(path))
    }

    /// Go: `func (p *Program) IsSourceFromProjectReference(path) bool`.
    pub fn is_source_from_project_reference(&self, path: &PathKey) -> bool {
        self.project_reference_file_mapper
            .as_ref()
            .is_some_and(|mapper| mapper.is_source_from_project_reference(path))
    }

    /// Go: `func (p *Program) GetProjectReferenceFromOutputDts(path)`.
    pub fn get_project_reference_from_output_dts(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        self.project_reference_file_mapper
            .as_ref()
            .and_then(|mapper| mapper.get_project_reference_from_output_dts(path))
    }

    /// Go: `func (p *Program) GetResolvedProjectReferenceFor(path)`.
    pub fn get_resolved_project_reference_for(
        &self,
        path: &PathKey,
    ) -> Option<Arc<crate::project_reference_file_mapper::ResolvedProjectReference>> {
        self.project_reference_file_mapper
            .as_ref()
            .and_then(|mapper| mapper.get_resolved_reference_for(path))
    }

    /// Go: `func (p *Program) GetRedirectForResolution(file)`.
    pub fn get_redirect_for_resolution(
        &self,
        file_name: &RootedFilePath,
        path: &PathKey,
    ) -> Option<Arc<crate::project_reference_file_mapper::ResolvedProjectReference>> {
        self.project_reference_file_mapper
            .as_ref()
            .and_then(|mapper| {
                mapper
                    .get_redirect_for_resolution(&HasFileName::new(
                        file_name.clone(),
                        path.clone(),
                    ))
                    .0
            })
    }

    /// Go: `func (p *Program) GetResolvedProjectReferences()`.
    pub fn get_resolved_project_references(
        &self,
    ) -> Vec<Arc<crate::project_reference_file_mapper::ResolvedProjectReference>> {
        self.project_reference_file_mapper
            .as_ref()
            .map(|mapper| mapper.get_resolved_project_references())
            .unwrap_or_default()
    }

    /// Go: `func (p *Program) RangeResolvedProjectReference(f)`.
    pub fn range_resolved_project_reference(
        &self,
        f: crate::project_reference_file_mapper::RangeResolvedProjectReferenceCallback<'_>,
    ) -> bool {
        self.project_reference_file_mapper
            .as_ref()
            .map(|mapper| mapper.range_resolved_project_reference(f))
            .unwrap_or(false)
    }

    /// Go: `func (p *Program) RangeResolvedProjectReferenceInChildConfig(childConfig, f)`.
    pub fn range_resolved_project_reference_in_child_config(
        &self,
        child_config: &Arc<tsc_tsoptions::ParsedCommandLine>,
        f: crate::project_reference_file_mapper::RangeResolvedProjectReferenceCallback<'_>,
    ) -> bool {
        self.project_reference_file_mapper
            .as_ref()
            .map(|mapper| {
                mapper.range_resolved_project_reference_in_child_config(child_config, f)
            })
            .unwrap_or(false)
    }

    // ── include / lib surfaces ──────────────────────────────────────────────

    /// Go: `func (p *Program) IsSourceFileDefaultLibrary(path) bool`.
    pub fn is_source_file_default_library(&self, path: &PathKey) -> bool {
        self.lib_files.contains_key(path)
    }

    /// Go: `func (p *Program) IsLibFile(sourceFile) bool`.
    pub fn is_lib_file(&self, source_file: &SourceFile) -> bool {
        self.lib_files.contains_key(source_file.path_key())
    }

    /// Go: `func (p *Program) IsMissingPath(path) bool` (testing only).
    pub fn is_missing_path(&self, path: &PathKey) -> bool {
        self.missing_files.contains(path)
    }

    /// Go: `func (p *Program) GetIncludeReasons()` (testing only).
    pub fn get_include_reasons(
        &self,
    ) -> &rustc_hash::FxHashMap<PathKey, Vec<Arc<FileIncludeReason>>> {
        &self.file_include_reasons
    }

    /// Go: `func (p *Program) GetDefaultLibFile(path) *LibFile`.
    pub fn get_default_lib_file(&self, path: &PathKey) -> Option<Arc<LibFile>> {
        self.lib_files.get(path).cloned()
    }

    /// Go: `func (p *Program) GetLibFileFromReference(ref) *ast.SourceFile`.
    pub fn get_lib_file_from_reference(&self, r#ref: &FileReference) -> Option<&SourceFile> {
        let name = tsc_tsoptions::get_lib_file_name(&r#ref.file_name)?;
        for (path, lib_file) in &self.lib_files {
            if lib_file.name == name {
                return self.get_source_file_by_path(path);
            }
        }
        None
    }

    /// Go: `func (p *Program) IsSourceFileFromExternalLibrary(file) bool`.
    pub fn is_source_file_from_external_library(&self, file: &SourceFile) -> bool {
        self.source_files_found_searching_node_modules
            .contains(file.path_key())
    }

    /// Go: `func (p *Program) IsGlobalTypingsFile(fileName) bool`.
    pub fn is_global_typings_file(&self, file_name: &RootedFilePath) -> bool {
        if !file_name.is_declaration_file() {
            return false;
        }
        self.case_sensitivity.contains_path(
            self.get_global_typings_cache_location(),
            &file_name.as_path(),
        )
    }

    /// Go: `func (p *Program) GetJSXRuntimeImportSpecifier(path)`.
    pub fn get_jsx_runtime_import_specifier(&self, path: &PathKey) -> Option<&String> {
        self.jsx_runtime_import_specifiers.get(path)
    }

    /// Go: `func (p *Program) GetImportHelpersImportSpecifier(path)`.
    pub fn get_import_helpers_import_specifier(&self, path: &PathKey) -> Option<&String> {
        self.import_helpers_import_specifiers.get(path)
    }

    /// Go: `func (p *Program) GetSourceFileFromReference(origin, ref)`.
    pub fn get_source_file_from_reference(
        &self,
        origin: &SourceFile,
        r#ref: &FileReference,
    ) -> Option<&SourceFile> {
        // TODO (Go): The module loader in corsa is fairly different than
        // strada, it should probably be able to expose this functionality at
        // some point, rather than redoing logic approximately here, since most
        // of the related logic now lives in module.Resolver. Still, without
        // the failed lookup reporting that only the loader does, this isn't
        // terribly complicated.
        if let Some(name) = tsc_tsoptions::get_lib_file_name(&r#ref.file_name) {
            // Handling lib file references (e.g. /// <reference lib="es2015"/>)
            for (path, lib_file) in &self.lib_files {
                if lib_file.name == name {
                    return self.get_source_file_by_path(path);
                }
            }
            return None;
        }

        let file_name = origin.file_name().directory().resolve_file(&r#ref.file_name);
        let supported_extensions_base = tsc_tsoptions::tsconfigparsing::get_supported_extensions(
            self.options(),
            // Go: p.CommandLine().ContentMapperExtensions() — the seam is
            // empty in this wave.
            &[],
        );
        let supported_extensions = tsc_tsoptions::tsconfigparsing::get_supported_extensions_with_json_if_resolve_json_module(
            self.options(),
            supported_extensions_base,
        );
        let allow_non_ts_extensions = self.options().allow_non_ts_extensions.is_true();
        if file_name.has_extension() {
            if !allow_non_ts_extensions {
                let canonical_file_name = self.path_key_for_file_name(&file_name);
                let supported = supported_extensions.iter().any(|group| {
                    // PORT: the groups are Vec<Vec<String>> (the tsoptions
                    // shape); extension_is_one_of takes &[&str].
                    let extensions: Vec<&str> = group.iter().map(|e| e.as_str()).collect();
                    canonical_file_name.extension_is_one_of(&extensions)
                });
                if !supported {
                    return None; // unsupported extensions are forced to fail
                }
            }

            return self.get_source_file_with_redirect(&file_name, &self.path_key_for_file_name(&file_name));
        }
        if allow_non_ts_extensions {
            let extensionless =
                self.get_source_file_with_redirect(&file_name, &self.path_key_for_file_name(&file_name));
            if extensionless.is_some() {
                return extensionless;
            }
        }

        // Only try adding extensions from the first supported group (which
        // should be .ts/.tsx/.d.ts)
        for ext in &supported_extensions[0] {
            let file_name_with_extension = file_name.append_suffix(ext);
            let result = self.get_source_file_with_redirect(
                &file_name_with_extension,
                &self.path_key_for_file_name(&file_name_with_extension),
            );
            if result.is_some() {
                return result;
            }
        }
        None
    }

    // ── diagnostics ─────────────────────────────────────────────────────────

    /// Go: `func (p *Program) GetConfigFileParsingDiagnostics()`.
    pub fn get_config_file_parsing_diagnostics(&self) -> Vec<FileDiagnostic> {
        // PORT (file ids): the parsed config's diagnostics carry the config
        // file's identity; the port pairs them with the config file's name.
        self.opts
            .config
            .errors
            .iter()
            .map(|diagnostic| FileDiagnostic {
                file_name: Some(self.opts.config.config_file_name.clone()),
                diagnostic: diagnostic.clone(),
            })
            .collect()
    }

    /// Go: `func (p *Program) collectDiagnostics(sourceFile, concurrent, collect)`
    /// — the shared collector shape.
    fn collect_diagnostics(
        &self,
        source_file: Option<&SourceFile>,
        _concurrent: bool,
        collect: impl Fn(&SourceFile) -> Vec<FileDiagnostic>,
    ) -> Vec<FileDiagnostic> {
        let result: Vec<FileDiagnostic> = match source_file {
            Some(source_file) => collect(source_file),
            // Go: collectDiagnosticsFromFiles on a WorkGroup — PORT: the
            // parallel arm is the checker-collection path (deferred with the
            // checker wave); the sequential walk is identical.
            None => self.files.iter().flat_map(collect).collect(),
        };
        // Go: filterAndSortDiagnostics — the ReportsUnnecessary/spanmap
        // filter is deferred with the spanmap surface (see the file header).
        sort_and_deduplicate_diagnostics(result)
    }

    /// Go: `func (p *Program) GetSyntacticDiagnostics(ctx, sourceFile)`.
    pub fn get_syntactic_diagnostics(
        &self,
        source_file: Option<&SourceFile>,
    ) -> Vec<FileDiagnostic> {
        let options = self.options();
        self.collect_diagnostics(source_file, false, |file| {
            let mut diags: Vec<Diagnostic> = file.diagnostics().to_vec();
            diags.extend(file.js_diagnostics().iter().cloned());
            // Go: for JS files that won't be checked by the checker (no
            // checkJs/ts-check), program-level syntactic checks that require
            // compiler options (getAdditionalJSSyntacticDiagnostics) —
            // deferred with the visitor/SubtreeFacts surface (see the file
            // header).
            let _ = options;
            diags
                .into_iter()
                .map(|diagnostic| FileDiagnostic::for_file(file, diagnostic))
                .collect()
        })
    }

    /// Go: `func (p *Program) GetBindDiagnostics(ctx, sourceFile)`.
    ///
    /// PORT: Go binds here (lazily, through the shared pointers); the port's
    /// binding is explicit — `bind_source_files` (or per-file bind through
    /// the owned `files`) must run before the bind diagnostics are read.
    pub fn get_bind_diagnostics(
        &self,
        source_file: Option<&SourceFile>,
    ) -> Vec<FileDiagnostic> {
        self.collect_diagnostics(source_file, false, |file| {
            file.payload()
                .bind_diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| FileDiagnostic::for_file(file, diagnostic))
                .collect()
        })
    }

    /// Go: `func (p *Program) GetBindDiagnostics`'s binding half.
    pub fn get_bind_diagnostics_binding(&mut self, source_file: Option<usize>) {
        match source_file {
            Some(index) => {
                tsc_binder::bind_source_file(&mut self.files[index]);
            }
            None => self.bind_source_files(),
        }
    }

    /// Go: `func (p *Program) BindSourceFiles()`.
    ///
    /// PORT: Go queues the binds on a WorkGroup; the binds are disjoint
    /// `&mut` borrows of `files` — the sequential loop is the same dataflow
    /// (parallelism returns with the checker wave's grouped iteration).
    pub fn bind_source_files(&mut self) {
        for file in &mut self.files {
            tsc_binder::bind_source_file(file);
        }
    }

    /// Go: `func (p *Program) GetProgramDiagnostics()`.
    ///
    /// PORT (fixed during the M5 program-twin parity run, mirroring Go):
    /// the port previously folded the include processor's FULL collection
    /// (`get_diagnostics()` — global + per-file) where Go folds only
    /// `GetGlobalDiagnostics()`, and concatenated in a different order.
    /// File-scoped include diagnostics (referenced-location diagnostics
    /// attached through `processingDiagnostic.toDiagnostic`'s
    /// `diagnosticAt` path) leaked into the program set — Go keeps them
    /// out. Mirrored Go exactly: programDiagnostics, then
    /// contentMapperDiagnostics, then the include processor's GLOBAL
    /// diagnostics, all through `sort_and_deduplicate_diagnostics`
    /// (Go: `SortAndDeduplicateDiagnostics(slices.Concat(...))`;
    /// `contentMapperOptionDiagnostics` is the not-ported content-mapper
    /// seam — empty here, matching Go's nil-project case).
    pub fn get_program_diagnostics(&self) -> Vec<FileDiagnostic> {
        let mut all: Vec<FileDiagnostic> = self
            .program_diagnostics
            .lock()
            .expect("programDiagnostics poisoned")
            .clone();
        all.extend(
            self.content_mapper_diagnostics
                .iter()
                .map(|diagnostic| FileDiagnostic {
                    file_name: None,
                    diagnostic: (**diagnostic).clone(),
                }),
        );
        all.extend(self.include_processor.get_diagnostics(self).get_global_diagnostics());
        sort_and_deduplicate_diagnostics(all)
    }

    /// Go: `func (p *Program) GetIncludeProcessorDiagnostics(sourceFile)`.
    ///
    /// PORT: the `getDiagnosticsWithPrecedingDirectives` filter (the
    /// ts-expect-error suppression) is deferred with the comment-directive
    /// surface — see the file header.
    pub fn get_include_processor_diagnostics(
        &self,
        source_file: Option<&SourceFile>,
    ) -> Vec<FileDiagnostic> {
        let Some(source_file) = source_file else {
            return Vec::new();
        };
        if self.skip_type_checking(source_file, false) {
            return Vec::new();
        }
        self.include_processor
            .get_diagnostics(self)
            .get_diagnostics_for_file(source_file.file_name())
    }

    /// Go: `func (p *Program) SkipTypeChecking(sourceFile, ignoreNoCheck) bool`.
    pub fn skip_type_checking(&self, source_file: &SourceFile, ignore_no_check: bool) -> bool {
        (!ignore_no_check && self.options().no_check.is_true())
            || (self.options().skip_lib_check.is_true()
                && source_file.payload().is_declaration_file)
            || (self.options().skip_default_lib_check.is_true()
                && self.is_source_file_default_library(source_file.path_key()))
            || self.is_source_from_project_reference(source_file.path_key())
            || !self.can_include_bind_and_check_diagnostics(source_file)
    }

    /// Go: `func (p *Program) canIncludeBindAndCheckDiagnostics(sourceFile) bool`.
    fn can_include_bind_and_check_diagnostics(&self, source_file: &SourceFile) -> bool {
        let payload = source_file.payload();
        if let Some(check_js_directive) = &payload.check_js_directive {
            if !check_js_directive.enabled {
                return false;
            }
        }

        if payload.script_kind == ScriptKind::TS || payload.script_kind == ScriptKind::TSX {
            return true;
        }

        let is_js = payload.script_kind == ScriptKind::JS || payload.script_kind == ScriptKind::JSX;
        let is_check_js = is_js
            && crate::ast_shims::is_check_js_enabled_for_file(source_file, self.options());
        let is_plain_js = crate::ast_shims::is_plain_js_file(
            source_file,
            self.options().check_js,
        );

        // By default, only type-check .ts, .tsx, plain JS, and checked JS
        is_plain_js || is_check_js
    }

    /// Go: `func (p *Program) GetUnresolvedImports() *collections.Set[string]`
    /// (the lazy `extractUnresolvedImports` surface).
    ///
    /// PORT (fixed during the M5 program-twin parity run, mirroring Go):
    /// the port previously guarded the sweep with
    /// `!uses_uri_style_node_core_modules` and `!skip_type_checking`,
    /// neither of which exists in Go's `extractUnresolvedImports` — and
    /// Go never SETS `p.usesUriStyleNodeCoreModules` (it stays the zero
    /// `Tristate::Unknown`), so the port's guard made the whole sweep
    /// dead and `get_unresolved_imports` always returned `[]`. Mirrored
    /// Go: every file, unfiltered.
    pub fn get_unresolved_imports(&self) -> Vec<String> {
        self.extract_unresolved_imports()
    }

    fn extract_unresolved_imports(&self) -> Vec<String> {
        // Go: a `collections.Set` dedup; the sweep is order-insensitive and
        // the result is deduplicated here.
        let mut unresolved: Vec<String> = Vec::new();
        for source_file in &self.files {
            unresolved.extend(self.extract_unresolved_imports_from_source_file(source_file));
        }
        unresolved.sort();
        unresolved.dedup();
        unresolved
    }

    /// Go: `func (p *Program) extractUnresolvedImportsFromSourceFile(file)`.
    fn extract_unresolved_imports_from_source_file(&self, file: &SourceFile) -> Vec<String> {
        let mut unresolved_imports: Vec<String> = Vec::new();
        let Some(resolved_modules) = self.resolved_modules.get(file.path_key()) else {
            return unresolved_imports;
        };
        for (cache_key, resolution) in resolved_modules {
            let resolved = tsc_module::ResolvedModule::is_resolved(resolution.as_deref());
            // Go: tspath.ExtensionIsOneOf(resolution.Extension, ...)
            if (!resolved
                || !tsc_tspath::extension_is_one_of(
                    resolution.as_deref().map_or("", |r| r.extension.as_str()),
                    &tsc_tspath::SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT,
                ))
                && !tsc_tspath::is_external_module_name_relative(&cache_key.name)
            {
                unresolved_imports.push(cache_key.name.clone());
            }
        }
        unresolved_imports
    }

    // ── emit-blocking + common source directory ─────────────────────────────

    /// Go: `func (p *Program) blockEmittingOfFile(emitFileName, diag)`.
    pub fn block_emitting_of_file(&mut self, emit_file_name: &RootedFilePath, diag: FileDiagnostic) {
        self.has_emit_blocking_diagnostics
            .insert(self.path_key_for_file_name(emit_file_name));
        self.program_diagnostics
            .lock()
            .expect("programDiagnostics poisoned")
            .push(diag);
    }

    /// Go: `func (p *Program) IsEmitBlocked(emitFileName) bool`.
    pub fn is_emit_blocked(&self, emit_file_name: &RootedFilePath) -> bool {
        self.has_emit_blocking_diagnostics
            .contains(&self.path_key_for_file_name(emit_file_name))
    }

    /// Go: `func (p *Program) getSourceFilesToEmit(targetSourceFiles, forceDtsEmit, forceJsEmit)`.
    pub fn get_source_files_to_emit<'a>(
        &'a self,
        target_source_files: Option<&'a [&'a SourceFile]>,
        force_dts_emit: bool,
        force_js_emit: bool,
    ) -> Vec<&'a SourceFile> {
        if target_source_files.is_none() && !force_dts_emit && !force_js_emit {
            // Go: `if targetSourceFiles == nil && !forceDtsEmit && !forceJsEmit { return p.files }`.
            return self.files.iter().collect();
        }
        crate::emitter::get_source_files_to_emit(
            self,
            target_source_files,
            force_dts_emit,
            force_js_emit,
        )
    }

    /// Go: `func (p *Program) SourceFileMayBeEmitted(sourceFile, forceDtsEmit) bool`.
    pub fn source_file_may_be_emitted(
        &self,
        source_file: &SourceFile,
        force_dts_emit: bool,
    ) -> bool {
        crate::emitter::source_file_may_be_emitted(source_file, self, force_dts_emit, false)
    }

    /// Go: `func (p *Program) CommonSourceDirectory()`.
    pub fn common_source_directory(&self) -> RootedDirectoryPath {
        self.common_source_directory
            .get_or_init(|| {
                let files: Vec<RootedFilePath> = self
                    .files
                    .iter()
                    .filter(|file| {
                        crate::emitter::source_file_may_be_emitted(file, self, false, false)
                            && !file.payload().is_declaration_file
                    })
                    .map(|file| file.file_name().clone())
                    .collect();
                let mut check = |source_files: &[RootedFilePath],
                                 root_directory: &RootedDirectoryPath|
                 -> bool {
                    self.check_source_files_belong_to_path(source_files, root_directory)
                };
                tsc_outputpaths::get_common_source_directory(
                    self.options(),
                    &|| files.clone(),
                    self.base_directory(),
                    self.case_sensitivity,
                    Some(&mut check),
                )
            })
            .clone()
    }

    /// Go: `func (p *Program) checkSourceFilesBelongToPath(sourceFiles, rootDirectory) bool`.
    ///
    /// PORT: Go appends the not-under-rootDir processing diagnostics here;
    /// the port's `processing_diagnostics` is owned (`&self` walk), so the
    /// diagnostics land with the interior-mutability pass — the boolean
    /// result (which drives the common-source-directory computation) is
    /// identical (PORTING-NOTES).
    fn check_source_files_belong_to_path(
        &self,
        source_files: &[RootedFilePath],
        root_directory: &RootedDirectoryPath,
    ) -> bool {
        let mut all_files_belong_to_path = true;
        for file in source_files {
            if !self
                .case_sensitivity
                .contains_file_path(root_directory, file)
            {
                all_files_belong_to_path = false;
            }
        }
        all_files_belong_to_path
    }

    // ── deferred seams ──────────────────────────────────────────────────────

    /// Go: `func (p *Program) verifyCompilerOptions()` + `verifyProjectReferences`
    /// — the degraded option-syntax-free surface (see the file header).
    fn verify_compiler_options(&mut self) {}

    /// Go: `func (p *Program) collectContentMapperOptionDiagnostics()` — the
    /// content-mapper seam (see file_loader.rs).
    fn collect_content_mapper_option_diagnostics(&mut self) {}

    /// Go: `func (p *Program) GetCheckerPool() CheckerPool` — the checker
    /// wave seam (checkerpool.rs).
    pub fn get_checker_pool(&self) -> Option<Arc<dyn crate::checkerpool::CheckerPool>> {
        self.checker_pool.clone()
    }
}

impl crate::emitter::SourceFileMayBeEmittedHost for Program {
    fn options(&self) -> &CompilerOptions {
        self.options()
    }

    fn get_project_reference_from_source(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        Program::get_project_reference_from_source(self, path)
    }

    fn is_source_file_from_external_library(&self, file: &SourceFile) -> bool {
        self.source_files_found_searching_node_modules
            .contains(file.path_key())
    }

    fn base_directory(&self) -> RootedDirectoryPath {
        self.base_directory().clone()
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
    }

    fn source_files(&self) -> &[SourceFile] {
        &self.files
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Emit result shapes (Go: program.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type WriteFileData struct`.
pub struct WriteFileData {
    /// Go: `SourceMapUrlPos int`.
    pub source_map_url_pos: i32,
    /// Go: `BuildInfo any` — the build-info surface (M7).
    pub build_info: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    /// Go: `Diagnostics []*ast.Diagnostic`.
    pub diagnostics: Vec<FileDiagnostic>,
    /// Go: `SkippedDtsWrite bool`.
    pub skipped_dts_write: bool,
    /// Go: `SourceFile *ast.SourceFile`.
    pub source_file: Option<usize>,
}

/// Go: `type WriteFile func(fileName, text, data) error`.
pub type WriteFile =
    Arc<dyn Fn(&RootedFilePath, &str, Option<&mut WriteFileData>) -> std::io::Result<()> + Send + Sync>;

/// Go: `type EmitOptions struct`.
pub struct EmitOptions {
    /// Source files to emit. If `None`, emits all files.
    ///
    /// Go: `TargetSourceFiles []*ast.SourceFile` (nil = all).
    pub target_source_files: Option<Vec<usize>>,
    /// Go: `EmitOnly EmitOnly` (emitter.rs).
    pub emit_only: crate::emitter::EmitOnly,
    /// Go: `ForceEmit bool`.
    pub force_emit: bool,
    /// Go: `WriteFile WriteFile`.
    pub write_file: Option<WriteFile>,
}

/// Go: `type EmitResult struct`.
#[derive(Default)]
pub struct EmitResult {
    /// Go: `EmitSkipped bool`.
    pub emit_skipped: bool,
    /// Contains declaration emit diagnostics.
    ///
    /// Go: `Diagnostics []*ast.Diagnostic`.
    pub diagnostics: Vec<FileDiagnostic>,
    /// Array of files the compiler wrote to disk.
    ///
    /// Go: `EmittedFiles []tspath.RootedFilePath`.
    pub emitted_files: Vec<RootedFilePath>,
    /// Array of sourceMapData if compiler emitted sourcemaps.
    ///
    /// Go: `SourceMaps []*SourceMapEmitResult`.
    pub source_maps: Vec<SourceMapEmitResult>,
}

/// Go: `type SourceMapEmitResult struct`.
pub struct SourceMapEmitResult {
    /// Input source file (which one can use on program to get the file), 1:1
    /// mapping with the sourceMap.sources list.
    ///
    /// Go: `InputSourceFileNames []string`.
    pub input_source_file_names: Vec<String>,
    /// Go: `SourceMap *sourcemap.RawSourceMap` — the sourcemap surface (M7).
    pub source_map: Option<()>,
    /// Go: `GeneratedFile string`.
    pub generated_file: String,
}

/// Go: `func CombineEmitResults(results) *EmitResult`.
pub fn combine_emit_results(results: Vec<EmitResult>) -> EmitResult {
    let mut result = EmitResult::default();
    for emit_result in results {
        if emit_result.emit_skipped {
            result.emit_skipped = true;
        }
        result.diagnostics.extend(emit_result.diagnostics);
        result.emitted_files.extend(emit_result.emitted_files);
        if !emit_result.source_maps.is_empty() {
            result.source_maps.extend(emit_result.source_maps);
        }
    }
    result
}

// ────────────────────────────────────────────────────────────────────────────
// ProgramLike / HandleNoEmitOptions / GetDiagnosticsOfAnyProgram
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ProgramLike interface`.
///
/// PORT: Go's `Emit(ctx, options) *EmitResult` method is the M7 emit boundary
/// (see emitter.rs) — it is not on the trait yet; `HandleNoEmitOptions` does
/// not reach it (its callers are the emit paths).
pub trait ProgramLike {
    /// Go: `Options() *core.CompilerOptions`.
    fn options(&self) -> &CompilerOptions;
    /// Go: `GetSourceFile(fileName) *ast.SourceFile`.
    fn get_source_file(&self, file_name: &RootedFilePath) -> Option<&SourceFile>;
    /// Go: `GetSourceFiles() []*ast.SourceFile`.
    fn get_source_files(&self) -> &[SourceFile];
    /// Go: `GetConfigFileParsingDiagnostics() []*ast.Diagnostic`.
    fn get_config_file_parsing_diagnostics(&self) -> Vec<FileDiagnostic>;
    /// Go: `GetSyntacticDiagnostics(ctx, file) []*ast.Diagnostic`.
    fn get_syntactic_diagnostics(&self, file: Option<&SourceFile>) -> Vec<FileDiagnostic>;
    /// Go: `GetBindDiagnostics(ctx, file) []*ast.Diagnostic`.
    fn get_bind_diagnostics(&self, file: Option<&SourceFile>) -> Vec<FileDiagnostic>;
    /// Go: `GetProgramDiagnostics() []*ast.Diagnostic`.
    fn get_program_diagnostics(&self) -> Vec<FileDiagnostic>;
    /// Go: `GetGlobalDiagnostics(ctx) []*ast.Diagnostic`.
    fn get_global_diagnostics(&self) -> Vec<FileDiagnostic>;
    /// Go: `GetSemanticDiagnostics(ctx, file) []*ast.Diagnostic`.
    fn get_semantic_diagnostics(&self, file: Option<&SourceFile>) -> Vec<FileDiagnostic>;
    /// Go: `GetDeclarationDiagnostics(ctx, file) []*ast.Diagnostic`.
    fn get_declaration_diagnostics(&self, file: Option<&SourceFile>) -> Vec<FileDiagnostic>;
    /// Go: `GetSuggestionDiagnostics(ctx, file) []*ast.Diagnostic`.
    fn get_suggestion_diagnostics(&self, file: Option<&SourceFile>) -> Vec<FileDiagnostic>;
    /// Go: `CommonSourceDirectory() tspath.RootedDirectoryPath`.
    fn common_source_directory(&self) -> RootedDirectoryPath;
    /// Go: `IsSourceFileDefaultLibrary(path) bool`.
    fn is_source_file_default_library(&self, path: &PathKey) -> bool;
    /// Go: `Program() *Program`.
    fn program(&self) -> &Program;
}

impl ProgramLike for Program {
    fn options(&self) -> &CompilerOptions {
        self.options()
    }

    fn get_source_file(&self, file_name: &RootedFilePath) -> Option<&SourceFile> {
        Program::get_source_file(self, file_name)
    }

    fn get_source_files(&self) -> &[SourceFile] {
        &self.files
    }

    fn get_config_file_parsing_diagnostics(&self) -> Vec<FileDiagnostic> {
        self.get_config_file_parsing_diagnostics()
    }

    fn get_syntactic_diagnostics(&self, file: Option<&SourceFile>) -> Vec<FileDiagnostic> {
        self.get_syntactic_diagnostics(file)
    }

    fn get_bind_diagnostics(&self, file: Option<&SourceFile>) -> Vec<FileDiagnostic> {
        self.get_bind_diagnostics(file)
    }

    fn get_program_diagnostics(&self) -> Vec<FileDiagnostic> {
        self.get_program_diagnostics()
    }

    fn get_global_diagnostics(&self) -> Vec<FileDiagnostic> {
        // Go: the checker pool's global diagnostics — the checker wave seam
        // (see the file header).
        Vec::new()
    }

    fn get_semantic_diagnostics(&self, _file: Option<&SourceFile>) -> Vec<FileDiagnostic> {
        // The checker wave seam (see the file header).
        Vec::new()
    }

    fn get_declaration_diagnostics(&self, _file: Option<&SourceFile>) -> Vec<FileDiagnostic> {
        // Go: the declarations transform diagnostics — the M7 boundary.
        Vec::new()
    }

    fn get_suggestion_diagnostics(&self, _file: Option<&SourceFile>) -> Vec<FileDiagnostic> {
        // The checker wave seam (see the file header).
        Vec::new()
    }

    fn common_source_directory(&self) -> RootedDirectoryPath {
        Program::common_source_directory(self)
    }

    fn is_source_file_default_library(&self, path: &PathKey) -> bool {
        self.lib_files.contains_key(path)
    }

    fn program(&self) -> &Program {
        self
    }
}

/// Go: `func HandleNoEmitOptions(ctx, program, files, emitBuildInfo) *EmitResult`
/// — mirrors tsc's handleNoEmitOptions.
pub fn handle_no_emit_options(
    program: &dyn ProgramLike,
    files: Option<&[&SourceFile]>,
    emit_build_info: Option<&mut dyn FnMut() -> Option<EmitResult>>,
) -> Option<EmitResult> {
    if !program.options().no_emit.is_true() {
        if !program.options().no_emit_on_error.is_true() {
            // NoEmit is false and NoEmitOnError is also false, so we can
            // proceed with normal emit.
            return None;
        }

        let mut get_bind_diagnostics =
            |file: Option<&SourceFile>| -> Vec<FileDiagnostic> {
                program.get_bind_diagnostics(file)
            };
        let mut get_semantic_diagnostics =
            |file: Option<&SourceFile>| -> Vec<FileDiagnostic> {
                program.get_semantic_diagnostics(file)
            };
        let diagnostics = get_diagnostics_of_any_program(
            program,
            files,
            true, // skipNoEmitCheckForDtsDiagnostics
            &mut get_bind_diagnostics,
            &mut get_semantic_diagnostics,
        );
        if diagnostics.is_empty() {
            // NoEmitOnError is enabled, but no diagnostics were found, so we
            // can proceed with emitting.
            return None;
        }
        return Some(EmitResult {
            diagnostics,
            emit_skipped: true,
            ..Default::default()
        });
    }
    if files.is_some() {
        return Some(EmitResult {
            emit_skipped: true,
            ..Default::default()
        });
    }
    if let Some(emit_build_info) = emit_build_info {
        if let Some(result) = emit_build_info() {
            return Some(result);
        }
    }
    Some(EmitResult::default())
}

/// Go: `func GetDiagnosticsOfAnyProgram(ctx, program, files, skipNoEmitCheckForDtsDiagnostics, getBindDiagnostics, getSemanticDiagnostics) []*ast.Diagnostic`.
pub fn get_diagnostics_of_any_program(
    program: &dyn ProgramLike,
    files: Option<&[&SourceFile]>,
    skip_no_emit_check_for_dts_diagnostics: bool,
    get_bind_diagnostics: &mut dyn FnMut(Option<&SourceFile>) -> Vec<FileDiagnostic>,
    get_semantic_diagnostics: &mut dyn FnMut(Option<&SourceFile>) -> Vec<FileDiagnostic>,
) -> Vec<FileDiagnostic> {
    let mut all_diagnostics: Vec<FileDiagnostic> = program.get_config_file_parsing_diagnostics();
    let config_file_parsing_diagnostics_length = all_diagnostics.len();

    let append_diagnostics_for_all_files =
        |mut diagnostics: Vec<FileDiagnostic>,
         get_diagnostics: &mut dyn FnMut(Option<&SourceFile>) -> Vec<FileDiagnostic>|
         -> Vec<FileDiagnostic> {
            match files {
                None => {
                    diagnostics.extend(get_diagnostics(None));
                    diagnostics
                }
                Some(files) => {
                    for file in files {
                        diagnostics.extend(get_diagnostics(Some(file)));
                    }
                    diagnostics
                }
            }
        };

    let mut get_syntactic_diagnostics =
        |file: Option<&SourceFile>| -> Vec<FileDiagnostic> {
            program.get_syntactic_diagnostics(file)
        };
    let syntactic_diagnostics =
        append_diagnostics_for_all_files(Vec::new(), &mut get_syntactic_diagnostics);
    if !syntactic_diagnostics.is_empty() {
        // Per-file content mapper failures are syntactic diagnostics, but the
        // locationless diagnostic that disables a repeatedly failing mapper
        // must still be reported.
        all_diagnostics.extend(
            program
                .program()
                .content_mapper_diagnostics
                .iter()
                .map(|diagnostic| FileDiagnostic {
                    file_name: None,
                    diagnostic: (**diagnostic).clone(),
                }),
        );
    }
    all_diagnostics.extend(syntactic_diagnostics);

    // If we didn't have any syntactic errors, then also try getting the
    // program (options), global and semantic errors.
    if all_diagnostics.len() == config_file_parsing_diagnostics_length {
        all_diagnostics.extend(program.get_program_diagnostics());

        // Do binding early so we can track the time.
        // Go: appendDiagnosticsForAllFiles(nil, getBindDiagnostics) — the
        // results are dropped (the collection is the side effect in Go;
        // the port's binding is explicit — see get_bind_diagnostics).
        let _ = append_diagnostics_for_all_files(Vec::new(), get_bind_diagnostics);

        if program.options().list_files_only.is_false() {
            all_diagnostics.extend(program.get_global_diagnostics());

            if all_diagnostics.len() == config_file_parsing_diagnostics_length {
                all_diagnostics =
                    append_diagnostics_for_all_files(all_diagnostics, get_semantic_diagnostics);
                // Incremental programs cache checking globals with file
                // diagnostics; a late sweep would also collect incidental
                // signature-generation globals (Go's *Program type arm).
                all_diagnostics.extend(program.get_global_diagnostics());
            }

            if (skip_no_emit_check_for_dts_diagnostics || program.options().no_emit.is_true())
                && program.options().get_emit_declarations()
                && all_diagnostics.len() == config_file_parsing_diagnostics_length
            {
                let mut get_declaration_diagnostics =
                    |file: Option<&SourceFile>| -> Vec<FileDiagnostic> {
                        program.get_declaration_diagnostics(file)
                    };
                all_diagnostics = append_diagnostics_for_all_files(
                    all_diagnostics,
                    &mut get_declaration_diagnostics,
                );
            }
        }
    }
    all_diagnostics
}
