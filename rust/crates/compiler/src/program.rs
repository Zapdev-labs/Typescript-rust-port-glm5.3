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
//   - `verifyCompilerOptions` ports its FULL arm set through the M6
//     config-node surface (the gap-5 wave): the removed-option arm
//     (TS5102/TS5108) through every remaining Go arm — the strict/
//     isolatedDeclarations/inlineSourceMap/composite/tsBuildInfo checks,
//     verifyProjectReferences + the composite root-files walk, the
//     `paths` validation, the sourcemap/declarationDir/lib/jsx/moduleKind
//     arms, and the emit-path-uniqueness loop (the
//     `OptionDiagnosticContext` machinery positions them on the config
//     file's AST nodes). The one residual: Go's per-key option
//     diagnostics on the config-file route are positioned by
//     parseOwnConfigOfJsonSourceFile's jsonConversionNotifier (the M6
//     notifier wave); the port's route-paired forms carry the zero range
//     until that lands (PORTING-NOTES). `getSourceFilesToEmit`'s
//     nil-target arm memoizes the FILTERED walk (Go's sourceFilesToEmit,
//     fixed this wave — it previously returned the unfiltered `files`).
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
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind, ScriptTarget};
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
use crate::processing_diagnostic::{
    IncludeExplainingDiagnostic, ProcessingDiagnostic, ProcessingDiagnosticKind,
};
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
    /// Go: `sourceFilesToEmitOnce sync.Once` + `sourceFilesToEmit
    /// []*ast.SourceFile` — the memoized FILTERED walk (Go's nil-target arm
    /// runs `getSourceFilesToEmit(p, nil, false, false)` — the
    /// `sourceFileMayBeEmitted` filter — NOT the raw `p.files`); the port
    /// memoizes the indexes into `files`.
    source_files_to_emit: OnceLock<Vec<usize>>,
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
            source_files_to_emit: OnceLock::new(),
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
        source_files_to_emit: OnceLock::new(),
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

    /// Go: `func (p *Program) GetConfigFileParsingDiagnostics()`
    /// — `slices.Clip(p.opts.Config.GetConfigFileParsingDiagnostics())`.
    ///
    /// PORT (file pairing): delegates to the parsed config's concat — the
    /// config file's parse diagnostics paired with the config file, `errors`
    /// with the file each was created against (see
    /// `ParsedCommandLine::error_files`; the unpaired command-line route
    /// keeps the pre-wave config-file pairing).
    pub fn get_config_file_parsing_diagnostics(&self) -> Vec<FileDiagnostic> {
        self.opts
            .config
            .get_config_file_parsing_diagnostics()
            .into_iter()
            .map(|(file_name, diagnostic)| FileDiagnostic {
                file_name,
                diagnostic,
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

    /// Go: `func (p *Program) BindSourceFiles()` — `wg :=
    /// core.NewWorkGroup(p.SingleThreaded())`, one queued bind per unbound
    /// file.
    ///
    /// PORT: the binds are disjoint `&mut` borrows of `files`, so the
    /// parallel WorkGroup maps onto scoped threads over contiguous
    /// chunks. Go's queue balances per-file work on the scheduler; static
    /// equal-count chunks would straggle on the big lib `.d.ts` files
    /// (lib.dom et al. dwarf the rest), so the chunk boundaries are cut by
    /// cumulative TEXT SIZE — the bind is O(nodes) in the file — keeping
    /// the disjoint-chunk borrows while approximating Go's per-file queue
    /// balance. Determinism is Go's own: the binder is per-file (no
    /// cross-file state), the `files` order is unchanged, and no twin
    /// output reads the raw lazily-assigned node/symbol ids (the binder's
    /// atomics — `symbols_seam` canonicalizes by encounter order and
    /// drops the ids on both sides). Sequential below the work-group's
    /// break-even and on the SingleThreaded option (Go's gate).
    pub fn bind_source_files(&mut self) {
        const MIN_PARALLEL_FILES: usize = 8;
        if self.single_threaded() || self.files.len() < MIN_PARALLEL_FILES {
            for file in &mut self.files {
                // Go: `if !file.IsBound()`.
                if !file.is_bound() {
                    tsc_binder::bind_source_file(file);
                }
            }
            return;
        }
        let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let threads = threads.min(self.files.len());
        // The balanced contiguous cut points: cumulative-bytes targets
        // (the bind is O(nodes) in the file; the big lib `.d.ts` files
        // otherwise straggle an equal-count chunking).
        let total: usize = self.files.iter().map(|file| file.text().len() + 1).sum();
        let target = total.div_ceil(threads);
        let mut cut_points: Vec<usize> = Vec::with_capacity(threads);
        let mut acc = 0usize;
        for (index, file) in self.files.iter().enumerate() {
            acc += file.text().len() + 1;
            if acc >= target && cut_points.len() + 1 < threads {
                cut_points.push(index + 1);
                acc = 0;
            }
        }
        cut_points.dedup();
        std::thread::scope(|scope| {
            let mut rest = self.files.as_mut_slice();
            let mut offset = 0usize;
            for boundary in cut_points {
                let (head, tail) = rest.split_at_mut(boundary - offset);
                offset = boundary;
                rest = tail;
                scope.spawn(move || {
                    for file in head {
                        if !file.is_bound() {
                            tsc_binder::bind_source_file(file);
                        }
                    }
                });
            }
            // The tail chunk absorbs the remainder.
            scope.spawn(move || {
                for file in rest {
                    if !file.is_bound() {
                        tsc_binder::bind_source_file(file);
                    }
                }
            });
        });
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
    ///
    /// PORT (fixed in the M6 verifyCompilerOptions wave, mirroring Go): the
    /// nil-target arm previously returned the UNFILTERED `p.files` — Go
    /// memoizes the FILTERED walk (`getSourceFilesToEmit(p, nil, false,
    /// false)` runs the `sourceFileMayBeEmitted` filter over
    /// `host.SourceFiles()`); the emit-path uniqueness loop depends on it.
    pub fn get_source_files_to_emit<'a>(
        &'a self,
        target_source_files: Option<&'a [&'a SourceFile]>,
        force_dts_emit: bool,
        force_js_emit: bool,
    ) -> Vec<&'a SourceFile> {
        if target_source_files.is_none() && !force_dts_emit && !force_js_emit {
            // Go: `p.sourceFilesToEmitOnce.Do(...)` — the filtered memoize.
            return self
                .get_source_files_to_emit_indexes()
                .iter()
                .map(|&index| &self.files[index])
                .collect();
        }
        crate::emitter::get_source_files_to_emit(
            self,
            target_source_files,
            force_dts_emit,
            force_js_emit,
        )
    }

    /// Go: the `p.sourceFilesToEmit` memoize — the indexes into `files` of
    /// the `sourceFileMayBeEmitted`-filtered walk (the emit-path loop's
    /// input; `getSourceFilesToEmit(nil, false, false)`).
    pub fn get_source_files_to_emit_indexes(&self) -> &[usize] {
        self.source_files_to_emit.get_or_init(|| {
            self.files
                .iter()
                .enumerate()
                .filter(|(_, file)| {
                    crate::emitter::source_file_may_be_emitted(file, self, false, false)
                })
                .map(|(index, _)| index)
                .collect()
        })
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

    /// Go: `func (p *Program) verifyCompilerOptions()` — the FULL arm set
    /// through the M6 config-node surface (the M6 verifyCompilerOptions
    /// wave): the removed-option arm plus every remaining Go arm — the
    /// strict/iso/inline/composite/tsBuildInfo checks, verifyProjectReferences
    /// plus the composite root-files walk, the `paths` validation, the
    /// sourcemap/declarationDir/lib/jsx/moduleKind checks, and the
    /// emit-path-uniqueness loop. Same diagnostics, messages/args,
    /// config-node positions and Go order.
    ///
    /// PORT (flush boundaries): Go appends into `programDiagnostics` across
    /// closures that borrow `p` every which way; the port collects per Go
    /// append-boundary — block A (removed + strict/iso/inline/composite/
    /// tsBuildInfo), verifyProjectReferences (its own collector — the range
    /// walk borrows `&self`), the composite walk (processing diagnostics),
    /// block B (paths + the remaining arms), then the emit-path findings
    /// (blockEmittingOfFile needs `&mut self`) — and extends in the same
    /// order.
    fn verify_compiler_options(&mut self) {
        let case_sensitivity = self.case_sensitivity;
        let mut collected: Vec<FileDiagnostic> = {
            let options = self.options();
            let config = &self.opts.config;
            let mut ctx = OptionDiagnosticContext::new(config);

            // Removed in TS7

            if !options.base_url.is_empty() {
                // BaseUrl will have been turned absolute by this point.
                let mut use_instead = String::new();
                let config_file_path = ctx.config_file_path();
                if !config_file_path.is_empty() {
                    let mut relative = tsc_tspath::get_relative_path_from_file(
                        config_file_path.as_string(),
                        options.base_url.as_string(),
                        case_sensitivity,
                    );
                    if !(relative.starts_with("./") || relative.starts_with("../")) {
                        relative = format!("./{relative}");
                    }
                    let suggestion = tsc_tspath::combine_paths(&relative, &["*"]);
                    use_instead =
                        format!("\"paths\": {{\"*\": [{}]}}", json_marshal_string(&suggestion));
                }
                ctx.create_removed_option_diagnostic("baseUrl", "", &use_instead);
            }

            if !options.out_file.is_empty() {
                ctx.create_removed_option_diagnostic("outFile", "", "");
            }

            if options.target == ScriptTarget::ES5 {
                ctx.create_removed_option_diagnostic("target", "ES5", "");
            }

            if options.module == ModuleKind::AMD {
                ctx.create_removed_option_diagnostic("module", "AMD", "");
            }
            if options.module == ModuleKind::System {
                ctx.create_removed_option_diagnostic("module", "System", "");
            }
            if options.module == ModuleKind::UMD {
                ctx.create_removed_option_diagnostic("module", "UMD", "");
            }

            if options.module_resolution == ModuleResolutionKind::Classic {
                ctx.create_removed_option_diagnostic("moduleResolution", "Classic", "");
            }

            if options.always_strict.is_false() {
                ctx.create_removed_option_diagnostic("alwaysStrict", "false", "");
            }

            if options.es_module_interop.is_false() {
                ctx.create_removed_option_diagnostic("esModuleInterop", "false", "");
            }

            if options.allow_synthetic_default_imports.is_false() {
                ctx.create_removed_option_diagnostic(
                    "allowSyntheticDefaultImports",
                    "false",
                    "",
                );
            }

            if options.module_resolution == ModuleResolutionKind::Node10 {
                ctx.create_removed_option_diagnostic("moduleResolution", "node10", "");
            }

            if !options.downlevel_iteration.is_unknown() {
                ctx.create_removed_option_diagnostic("downlevelIteration", "", "");
            }

            // ── Go: the strict/exactOptionalPropertyTypes checks ──
            if options.strict_property_initialization.is_true()
                && !options.get_strict_option_value(options.strict_null_checks)
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1,
                    "strictPropertyInitialization",
                    "strictNullChecks",
                    &[],
                );
            }
            if options.exact_optional_property_types.is_true()
                && !options.get_strict_option_value(options.strict_null_checks)
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1,
                    "exactOptionalPropertyTypes",
                    "strictNullChecks",
                    &[],
                );
            }

            if options.isolated_declarations.is_true() {
                if options.get_allow_js() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITH_OPTION_1,
                        "allowJs",
                        "isolatedDeclarations",
                        &[],
                    );
                }
                if !options.get_emit_declarations() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1_OR_OPTION_2,
                        "isolatedDeclarations",
                        "declaration",
                        &["composite".to_string()],
                    );
                }
            }

            if options.inline_source_map.is_true() {
                if options.source_map.is_true() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITH_OPTION_1,
                        "sourceMap",
                        "inlineSourceMap",
                        &[],
                    );
                }
                if !options.map_root.as_string().is_empty() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITH_OPTION_1,
                        "mapRoot",
                        "inlineSourceMap",
                        &[],
                    );
                }
            }

            if options.composite.is_true() {
                if options.declaration.is_false() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::COMPOSITE_PROJECTS_MAY_NOT_DISABLE_DECLARATION_EMIT,
                        "declaration",
                        "",
                        &[],
                    );
                }
                if options.incremental.is_false() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::COMPOSITE_PROJECTS_MAY_NOT_DISABLE_INCREMENTAL_COMPILATION,
                        "declaration",
                        "",
                        &[],
                    );
                }
            }

            if options.ts_build_info_file.as_string().is_empty()
                && options.incremental.is_true()
                && options.config_file_path.as_string().is_empty()
            {
                ctx.diagnostics.push(ctx.create_compiler_options_diagnostic(
                    &tsc_diagnostics::OPTION_INCREMENTAL_IS_ONLY_VALID_WITH_A_KNOWN_CONFIGURATION_FILE_LIKE_TSCONFIG_JSON_OR_WHEN_TSBUILDINFOFILE_IS_EXPLICITLY_PROVIDED,
                    &[],
                ));
            }

            ctx.diagnostics
        };

        // Go: `p.verifyProjectReferences()`.
        let (reference_diagnostics, emit_blocking_keys) = self.verify_project_references();
        collected.extend(reference_diagnostics);
        self.has_emit_blocking_diagnostics.extend(emit_blocking_keys);

        // Go: the composite root-files walk (addProcessingDiagnostic — the
        // explaining-file-include machinery, not programDiagnostics).
        self.processing_diagnostics
            .extend(self.composite_root_files_walk_diagnostics());

        // ── Go: block B — the paths validation + the remaining arms ──
        let (block_b_diagnostics, emit_findings) = {
            let options = self.options();
            let config = &self.opts.config;
            let mut ctx = OptionDiagnosticContext::new(config);

            // Go: forEachOptionPathsSyntax + createDiagnosticForOptionPaths +
            // createDiagnosticForOptionPathKeyValue over options.Paths.
            if let Some(paths) = &options.paths {
                for (key, value) in paths.entries() {
                    // !!! This code does not handle cases where the path
                    // mappings have the wrong types, as that information is
                    // mostly lost during the parsing process (Go comment).
                    if !has_zero_or_one_asterisk_character(key) {
                        ctx.create_diagnostic_for_option_paths(
                            true, /*onKey*/
                            key,
                            &tsc_diagnostics::PATTERN_0_CAN_HAVE_AT_MOST_ONE_ASTERISK_CHARACTER,
                            std::slice::from_ref(key),
                        );
                    }
                    // PORT (nil-vs-empty collapse): Go distinguishes a nil
                    // substitution list (`"paths": {"*": null}`) from an
                    // empty one — the port's `Vec<String>` cannot, so a
                    // non-array substitution value reports the
                    // "shouldn't be an empty array" message instead of
                    // "should be an array" (PORTING-NOTES).
                    if value.is_empty() {
                        ctx.create_diagnostic_for_option_paths(
                            false, /*onKey*/
                            key,
                            &tsc_diagnostics::SUBSTITUTIONS_FOR_PATTERN_0_SHOULDN_T_BE_AN_EMPTY_ARRAY,
                            std::slice::from_ref(key),
                        );
                    }
                    for (index, subst) in value.iter().enumerate() {
                        if !has_zero_or_one_asterisk_character(subst) {
                            ctx.create_diagnostic_for_option_path_key_value(
                                key,
                                index,
                                &tsc_diagnostics::SUBSTITUTION_0_IN_PATTERN_1_CAN_HAVE_AT_MOST_ONE_ASTERISK_CHARACTER,
                                &[subst.to_string(), key.to_string()],
                            );
                        }
                        if !tsc_tspath::path_is_relative(subst)
                            && !tsc_tspath::path_is_absolute(subst)
                        {
                            ctx.create_diagnostic_for_option_path_key_value(
                                key,
                                index,
                                &tsc_diagnostics::NON_RELATIVE_PATHS_ARE_NOT_ALLOWED_DID_YOU_FORGET_A_LEADING_SLASH,
                                &[],
                            );
                        }
                    }
                }
            }

            if options.source_map.is_false_or_unknown()
                && options.inline_source_map.is_false_or_unknown()
            {
                if options.inline_sources.is_true() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_USED_WHEN_EITHER_OPTION_INLINESOURCEMAP_OR_OPTION_SOURCEMAP_IS_PROVIDED,
                        "inlineSources",
                        "",
                        &[],
                    );
                }
                if !options.source_root.as_string().is_empty() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_USED_WHEN_EITHER_OPTION_INLINESOURCEMAP_OR_OPTION_SOURCEMAP_IS_PROVIDED,
                        "sourceRoot",
                        "",
                        &[],
                    );
                }
            }

            if !options.map_root.as_string().is_empty()
                && !(options.source_map.is_true() || options.declaration_map.is_true())
            {
                // Error to specify --mapRoot without --sourcemap
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1_OR_OPTION_2,
                    "mapRoot",
                    "sourceMap",
                    &["declarationMap".to_string()],
                );
            }

            if !options.declaration_dir.as_string().is_empty()
                && !options.get_emit_declarations()
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1_OR_OPTION_2,
                    "declarationDir",
                    "declaration",
                    &["composite".to_string()],
                );
            }

            if options.declaration_map.is_true() && !options.get_emit_declarations() {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1_OR_OPTION_2,
                    "declarationMap",
                    "declaration",
                    &["composite".to_string()],
                );
            }

            if options.lib.is_some() && options.no_lib.is_true() {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITH_OPTION_1,
                    "lib",
                    "noLib",
                    &[],
                );
            }

            if (options.isolated_modules.is_true() || options.verbatim_module_syntax.is_true())
                && options.preserve_const_enums.is_false()
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_PRESERVECONSTENUMS_CANNOT_BE_DISABLED_WHEN_0_IS_ENABLED,
                    if options.verbatim_module_syntax.is_true() {
                        "verbatimModuleSyntax"
                    } else {
                        "isolatedModules"
                    },
                    "preserveConstEnums",
                    &[],
                );
            }

            if !options.out_dir.as_string().is_empty()
                || !options.root_dir.as_string().is_empty()
                || !options.source_root.as_string().is_empty()
                || !options.map_root.as_string().is_empty()
                || (options.get_emit_declarations()
                    && !options.declaration_dir.as_string().is_empty())
            {
                // !!! sheetal checkSourceFilesBelongToPath - for root Dir and
                // configFile - explaining why file is in the program (Go
                // comment; the explaining machinery is the include processor's
                // surface).
                let dir = self.common_source_directory();
                if !options.out_dir.as_string().is_empty()
                    && dir.as_string().is_empty()
                    && self
                        .files
                        .iter()
                        .any(|file| file.file_name().root_length() > 1)
                {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::CANNOT_FIND_THE_COMMON_SUBDIRECTORY_PATH_FOR_THE_INPUT_FILES,
                        "outDir",
                        "",
                        &[],
                    );
                }
            }

            if !options.no_emit.is_true()
                && !options.composite.is_true()
                && options.root_dir.as_string().is_empty()
                && !options.config_file_path.as_string().is_empty()
                && (!options.out_dir.as_string().is_empty()
                    || (options.get_emit_declarations()
                        && !options.declaration_dir.as_string().is_empty())
                    || !options.out_file.is_empty())
            {
                // Check if rootDir inferred changed and issue diagnostic
                let dir = self.common_source_directory();
                let emitted_files: Vec<RootedFilePath> = self
                    .files
                    .iter()
                    .filter(|file| {
                        !file.payload().is_declaration_file
                            && crate::emitter::source_file_may_be_emitted(file, self, false, false)
                    })
                    .map(|file| file.file_name().clone())
                    .collect();
                let dir59 = tsc_outputpaths::get_computed_common_source_directory(
                    &emitted_files,
                    self.base_directory(),
                    self.case_sensitivity,
                );
                if !dir59.as_string().is_empty()
                    && self.case_sensitivity.compare_paths(&dir.as_path(), &dir59.as_path())
                        != 0
                {
                    // change in layout
                    let option1 = if !options.out_file.is_empty() {
                        "outFile"
                    } else if !options.out_dir.as_string().is_empty() {
                        "outDir"
                    } else {
                        "declarationDir"
                    };
                    let option2 = if options.out_file.is_empty()
                        && !options.out_dir.as_string().is_empty()
                    {
                        "declarationDir"
                    } else {
                        ""
                    };
                    let mut common_source_directory = dir59.as_string().to_owned();
                    if let Some(relative_path) = self.case_sensitivity
                        .relative_path_from_file_to_path(
                            &options.config_file_path,
                            &dir59.as_path(),
                        )
                    {
                        common_source_directory =
                            relative_path.as_module_specifier().as_string().to_owned();
                    }
                    let mut diag = ctx.create_diagnostic_for_option(
                        true, /*onKey*/
                        option1,
                        option2,
                        &tsc_diagnostics::THE_COMMON_SOURCE_DIRECTORY_OF_0_IS_1_THE_ROOTDIR_SETTING_MUST_BE_EXPLICITLY_SET_TO_THIS_OR_ANOTHER_PATH_TO_ADJUST_YOUR_OUTPUT_S_FILE_LAYOUT,
                        &[
                            options.config_file_path.base_name().to_owned(),
                            common_source_directory,
                        ],
                    );
                    diag.diagnostic.add_message_chain(
                        tsc_tsoptions::new_compiler_diagnostic(
                            &tsc_diagnostics::VISIT_HTTPS_COLON_SLASH_SLASHAKA_MS_SLASHTS6_FOR_MIGRATION_INFORMATION,
                            &[],
                        ),
                    );
                    // The pushed copy (Go mutates the shared diagnostic in
                    // place after the append).
                    if let Some(last) = ctx.diagnostics.last_mut() {
                        *last = diag;
                    }
                }
            }

            if options.check_js.is_true() && !options.get_allow_js() {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1,
                    "checkJs",
                    "allowJs",
                    &[],
                );
            }

            if options.emit_declaration_only.is_true() && !options.get_emit_declarations() {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1_OR_OPTION_2,
                    "emitDeclarationOnly",
                    "declaration",
                    &["composite".to_string()],
                );
            }

            if options.emit_decorator_metadata.is_true()
                && options.experimental_decorators.is_false_or_unknown()
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1,
                    "emitDecoratorMetadata",
                    "experimentalDecorators",
                    &[],
                );
            }

            if !options.jsx_factory.is_empty() {
                if !options.react_namespace.is_empty() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITH_OPTION_1,
                        "reactNamespace",
                        "jsxFactory",
                        &[],
                    );
                }
                if options.jsx == tsc_core::options_generated::JsxEmit::ReactJSX
                    || options.jsx == tsc_core::options_generated::JsxEmit::ReactJSXDev
                {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WHEN_OPTION_JSX_IS_1,
                        "jsxFactory",
                        &options.jsx.to_string(),
                        &[],
                    );
                }
                if tsc_parser::parse_isolated_entity_name(&options.jsx_factory).is_none() {
                    ctx.create_option_value_diagnostic(
                        "jsxFactory",
                        &tsc_diagnostics::INVALID_VALUE_FOR_JSXFACTORY_0_IS_NOT_A_VALID_IDENTIFIER_OR_QUALIFIED_NAME,
                        std::slice::from_ref(&options.jsx_factory),
                    );
                }
            } else if !options.react_namespace.is_empty()
                && !tsc_scanner::utilities::is_identifier_text(
                    &options.react_namespace,
                    tsc_core::languagevariant::LanguageVariant::Standard,
                )
            {
                ctx.create_option_value_diagnostic(
                    "reactNamespace",
                    &tsc_diagnostics::INVALID_VALUE_FOR_REACTNAMESPACE_0_IS_NOT_A_VALID_IDENTIFIER,
                    std::slice::from_ref(&options.react_namespace),
                );
            }

            if !options.jsx_fragment_factory.is_empty() {
                if options.jsx_factory.is_empty() {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WITHOUT_SPECIFYING_OPTION_1,
                        "jsxFragmentFactory",
                        "jsxFactory",
                        &[],
                    );
                }
                if options.jsx == tsc_core::options_generated::JsxEmit::ReactJSX
                    || options.jsx == tsc_core::options_generated::JsxEmit::ReactJSXDev
                {
                    ctx.create_diagnostic_for_option_name(
                        &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WHEN_OPTION_JSX_IS_1,
                        "jsxFragmentFactory",
                        &options.jsx.to_string(),
                        &[],
                    );
                }
                if tsc_parser::parse_isolated_entity_name(&options.jsx_fragment_factory)
                    .is_none()
                {
                    ctx.create_option_value_diagnostic(
                        "jsxFragmentFactory",
                        &tsc_diagnostics::INVALID_VALUE_FOR_JSXFRAGMENTFACTORY_0_IS_NOT_A_VALID_IDENTIFIER_OR_QUALIFIED_NAME,
                        std::slice::from_ref(&options.jsx_fragment_factory),
                    );
                }
            }

            if !options.react_namespace.is_empty()
                && (options.jsx == tsc_core::options_generated::JsxEmit::ReactJSX
                    || options.jsx == tsc_core::options_generated::JsxEmit::ReactJSXDev)
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WHEN_OPTION_JSX_IS_1,
                    "reactNamespace",
                    &options.jsx.to_string(),
                    &[],
                );
            }

            if !options.jsx_import_source.is_empty()
                && options.jsx == tsc_core::options_generated::JsxEmit::React
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CANNOT_BE_SPECIFIED_WHEN_OPTION_JSX_IS_1,
                    "jsxImportSource",
                    &options.jsx.to_string(),
                    &[],
                );
            }

            let module_kind = options.get_emit_module_kind();

            if options.allow_importing_ts_extensions.is_true()
                && !(options.no_emit.is_true()
                    || options.emit_declaration_only.is_true()
                    || options.rewrite_relative_import_extensions.is_true())
            {
                ctx.create_option_value_diagnostic(
                    "allowImportingTsExtensions",
                    &tsc_diagnostics::OPTION_ALLOWIMPORTINGTSEXTENSIONS_CAN_ONLY_BE_USED_WHEN_ONE_OF_NOEMIT_EMITDECLARATIONONLY_OR_REWRITERELATIVEIMPORTEXTENSIONS_IS_SET,
                    &[],
                );
            }

            let module_resolution = options.get_module_resolution_kind();
            if options.resolve_package_json_exports.is_true()
                && !module_resolution_supports_package_json_exports_and_imports(module_resolution)
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_USED_WHEN_MODULERESOLUTION_IS_SET_TO_NODE16_NODENEXT_OR_BUNDLER,
                    "resolvePackageJsonExports",
                    "",
                    &[],
                );
            }
            if options.resolve_package_json_imports.is_true()
                && !module_resolution_supports_package_json_exports_and_imports(module_resolution)
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_USED_WHEN_MODULERESOLUTION_IS_SET_TO_NODE16_NODENEXT_OR_BUNDLER,
                    "resolvePackageJsonImports",
                    "",
                    &[],
                );
            }
            if options.custom_conditions.is_some()
                && !module_resolution_supports_package_json_exports_and_imports(module_resolution)
            {
                ctx.create_diagnostic_for_option_name(
                    &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_USED_WHEN_MODULERESOLUTION_IS_SET_TO_NODE16_NODENEXT_OR_BUNDLER,
                    "customConditions",
                    "",
                    &[],
                );
            }

            if module_resolution == ModuleResolutionKind::Bundler
                && !emit_module_kind_is_non_node_esm(module_kind)
                && module_kind != ModuleKind::Preserve
                && module_kind != ModuleKind::CommonJS
            {
                ctx.create_option_value_diagnostic(
                    "moduleResolution",
                    &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_USED_WHEN_MODULE_IS_SET_TO_PRESERVE_COMMONJS_OR_ES2015_OR_LATER,
                    &["bundler".to_string()],
                );
            }

            if (ModuleKind::Node16..=ModuleKind::NodeNext).contains(&module_kind)
                && !(ModuleResolutionKind::Node16..=ModuleResolutionKind::NodeNext)
                    .contains(&module_resolution)
            {
                let module_kind_name = module_kind_string(module_kind);
                let module_resolution_name = match tsc_core::options_generated::module_kind_to_module_resolution_kind(module_kind) {
                    Some(resolution) => resolution.to_string(),
                    None => "Node16".to_string(),
                };
                ctx.create_option_value_diagnostic(
                    "moduleResolution",
                    &tsc_diagnostics::OPTION_MODULERESOLUTION_MUST_BE_SET_TO_0_OR_LEFT_UNSPECIFIED_WHEN_OPTION_MODULE_IS_SET_TO_1,
                    &[module_resolution_name, module_kind_name],
                );
            } else if (ModuleResolutionKind::Node16..=ModuleResolutionKind::NodeNext)
                .contains(&module_resolution)
                && !(ModuleKind::Node16..=ModuleKind::NodeNext).contains(&module_kind)
            {
                let module_resolution_name = module_resolution.to_string();
                ctx.create_option_value_diagnostic(
                    "module",
                    &tsc_diagnostics::OPTION_MODULE_MUST_BE_SET_TO_0_WHEN_OPTION_MODULERESOLUTION_IS_SET_TO_1,
                    &[module_resolution_name.clone(), module_resolution_name],
                );
            }

            // ── Go: the emit-path uniqueness loop (verifyEmitFilePath +
            // ForEachEmittedFile) — the findings collect here and apply
            // after the ctx borrows end (blockEmittingOfFile needs
            // `&mut self`). ──
            let mut emit_findings: Vec<EmitPathFinding> = Vec::new();
            if !options.no_emit.is_true() && !options.suppress_output_path_check.is_true() {
                let mut emit_files_seen: rustc_hash::FxHashSet<PathKey> =
                    rustc_hash::FxHashSet::default();
                let output_paths_host = ProgramOutputPathsHost { program: self };
                let verify_emit_file_path = |emit_file_name: &RootedFilePath,
                                             emit_files_seen: &mut rustc_hash::FxHashSet<PathKey>,
                                             emit_findings: &mut Vec<EmitPathFinding>| {
                    if emit_file_name.as_string().is_empty() {
                        return;
                    }
                    let emit_file_path = self.path_key_for_file_name(emit_file_name);
                    // Report error if the output overwrites input file
                    if self.files_by_path.contains_key(&emit_file_path) {
                        let mut diag = FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::CANNOT_WRITE_FILE_0_BECAUSE_IT_WOULD_OVERWRITE_INPUT_FILE,
                            &[emit_file_name.as_string().to_owned()],
                        );
                        if ctx.config_file_path().as_string().is_empty() {
                            // The program is from either an inferred project
                            // or an external project
                            diag.diagnostic.add_message_chain(
                                tsc_tsoptions::new_compiler_diagnostic(
                                    &tsc_diagnostics::ADDING_A_TSCONFIG_JSON_FILE_WILL_HELP_ORGANIZE_PROJECTS_THAT_CONTAIN_BOTH_TYPESCRIPT_AND_JAVASCRIPT_FILES_LEARN_MORE_AT_HTTPS_COLON_SLASH_SLASHAKA_MS_SLASHTSCONFIG,
                                    &[],
                                ),
                            );
                        }
                        emit_findings.push(EmitPathFinding {
                            emit_file_name: emit_file_name.clone(),
                            diagnostic: diag,
                        });
                    }

                    // Report error if multiple files write into same file
                    if emit_files_seen.contains(&emit_file_path) {
                        // Already seen the same emit file - report error
                        emit_findings.push(EmitPathFinding {
                            emit_file_name: emit_file_name.clone(),
                            diagnostic: FileDiagnostic::compiler_diagnostic(
                                &tsc_diagnostics::CANNOT_WRITE_FILE_0_BECAUSE_IT_WOULD_BE_OVERWRITTEN_BY_MULTIPLE_INPUT_FILES,
                                &[emit_file_name.as_string().to_owned()],
                            ),
                        });
                    } else {
                        emit_files_seen.insert(emit_file_path);
                    }
                };
                // PORT: outputpaths::for_each_emitted_file's `&[F]` cannot
                // express the memoized indexes (the orphan rule — F is
                // outputpaths' trait); the loop inlines over
                // `get_output_paths_for` exactly like Go's ForEachEmittedFile.
                for &index in self.get_source_files_to_emit_indexes() {
                    let output_paths = tsc_outputpaths::get_output_paths_for(
                        &self.files[index],
                        options,
                        &output_paths_host,
                        tsc_outputpaths::ForceEmitPaths::default(),
                    );
                    verify_emit_file_path(
                        output_paths.js_file_path(),
                        &mut emit_files_seen,
                        &mut emit_findings,
                    );
                    verify_emit_file_path(
                        output_paths.source_map_file_path(),
                        &mut emit_files_seen,
                        &mut emit_findings,
                    );
                    verify_emit_file_path(
                        output_paths.declaration_file_path(),
                        &mut emit_files_seen,
                        &mut emit_findings,
                    );
                    verify_emit_file_path(
                        output_paths.declaration_map_path(),
                        &mut emit_files_seen,
                        &mut emit_findings,
                    );
                }
                verify_emit_file_path(
                    &self.opts.config.get_build_info_file_name(),
                    &mut emit_files_seen,
                    &mut emit_findings,
                );
            }

            (ctx.diagnostics, emit_findings)
        };
        collected.extend(block_b_diagnostics);
        for finding in emit_findings {
            self.block_emitting_of_file(&finding.emit_file_name, finding.diagnostic);
        }
        self.program_diagnostics
            .lock()
            .expect("programDiagnostics poisoned")
            .extend(collected);
    }

    /// Go: `func (p *Program) verifyProjectReferences()`.
    ///
    /// PORT (borrows): Go appends into `programDiagnostics` and
    /// `hasEmitBlockingDiagnostics` inside the range walk; the walk borrows
    /// `&self`, so the diagnostics and the blocking keys collect locally
    /// and the caller applies them (same order).
    fn verify_project_references(
        &self,
    ) -> (Vec<FileDiagnostic>, Vec<PathKey>) {
        let build_info_file_name = if !self
            .options()
            .suppress_output_path_check
            .is_true()
        {
            self.opts.config.get_build_info_file_name()
        } else {
            RootedFilePath::default()
        };
        let mut diagnostics: Vec<FileDiagnostic> = Vec::new();
        let mut emit_blocking_keys: Vec<PathKey> = Vec::new();
        let mut callback = |_path: &PathKey,
                            config: Option<&Arc<crate::project_reference_file_mapper::ResolvedProjectReference>>,
                            parent: Option<&Arc<crate::project_reference_file_mapper::ResolvedProjectReference>>,
                            index: usize|
         -> bool {
                // Go: `ref := parent.ProjectReferences()[index]`.
                // !!! Deprecated in 5.0 and removed since 5.5
                // verifyRemovedProjectReference(ref, parent, index); (Go comment)
                let Some(parent) = parent else {
                    return true;
                };
                let ref_path = &parent
                    .config
                    .project_references()
                    .get(index)
                    .map(|reference| reference.path.clone())
                    .unwrap_or_default();
                let create_diagnostic_for_reference =
                    |message: &'static tsc_diagnostics::Message,
                     args: &[String],
                     diagnostics: &mut Vec<FileDiagnostic>| {
                        // Go: `createDiagnosticForReference(parent, index,
                        // ...)` — CreateDiagnosticAtReferenceSyntax reads
                        // the PARENT config's `references` array (the file
                        // the reference is WRITTEN in), the compiler-
                        // diagnostic form without a config file.
                        let diagnostic = match tsc_tsoptions::create_diagnostic_at_reference_syntax(
                            &parent.config, index, message, args,
                        ) {
                            Some(diagnostic) => diagnostic,
                            // Go: the compiler-diagnostic fallback (nil
                            // config file or no node — fileless).
                            None => tsc_tsoptions::new_compiler_diagnostic(message, args),
                        };
                        let file_name = if diagnostic.file().is_some() {
                            Some(parent.config.config_file_name.clone())
                        } else {
                            None
                        };
                        diagnostics.push(FileDiagnostic {
                            file_name,
                            diagnostic,
                        });
                    };
                if config.is_none() {
                    create_diagnostic_for_reference(
                        &tsc_diagnostics::FILE_0_NOT_FOUND,
                        &[ref_path.as_string().to_owned()],
                        &mut diagnostics,
                    );
                    return true;
                }
                let ref_options = config
                    .expect("config checked")
                    .config
                    .compiler_options();
                if !ref_options.composite.is_true() || ref_options.no_emit.is_true() {
                    // Go: `if len(parent.FileNames()) > 0`.
                    if !parent.config.file_names().is_empty() {
                        if !ref_options.composite.is_true() {
                            create_diagnostic_for_reference(
                                &tsc_diagnostics::REFERENCED_PROJECT_0_MUST_HAVE_SETTING_COMPOSITE_COLON_TRUE,
                                &[ref_path.as_string().to_owned()],
                                &mut diagnostics,
                            );
                        }
                        if ref_options.no_emit.is_true() {
                            create_diagnostic_for_reference(
                                &tsc_diagnostics::REFERENCED_PROJECT_0_MAY_NOT_DISABLE_EMIT,
                                &[ref_path.as_string().to_owned()],
                                &mut diagnostics,
                            );
                        }
                    }
                }
                if !build_info_file_name.as_string().is_empty()
                    && build_info_file_name.as_string()
                        == config
                            .expect("config checked")
                            .config
                            .get_build_info_file_name()
                            .as_string()
                {
                    create_diagnostic_for_reference(
                        &tsc_diagnostics::CANNOT_WRITE_FILE_0_BECAUSE_IT_WILL_OVERWRITE_TSBUILDINFO_FILE_GENERATED_BY_REFERENCED_PROJECT_1,
                        &[
                            build_info_file_name.as_string().to_owned(),
                            ref_path.as_string().to_owned(),
                        ],
                        &mut diagnostics,
                    );
                    emit_blocking_keys.push(self.path_key_for_file_name(&build_info_file_name));
                }
                true
        };
        self.range_resolved_project_reference(&mut callback);
        (diagnostics, emit_blocking_keys)
    }

    /// Go: verifyCompilerOptions' composite root-files walk — the
    /// explaining-file-include PROCESSING diagnostics for program files not
    /// listed in a composite project's root file list.
    fn composite_root_files_walk_diagnostics(&self) -> Vec<ProcessingDiagnostic> {
        if !self.options().composite.is_true() {
            return Vec::new();
        }
        let config_file_path = {
            // Go: the `configFilePath()` memoize — the config source file's
            // name ("" without one).
            match &self.opts.config.config_file {
                Some(config_file) => config_file.lock_source_file().file_name().clone(),
                None => RootedFilePath::default(),
            }
        };
        let mut root_paths: rustc_hash::FxHashSet<PathKey> = rustc_hash::FxHashSet::default();
        for file_name in self.opts.config.file_names() {
            root_paths.insert(self.case_sensitivity.path_key(&file_name.as_path()));
        }
        let mut diagnostics = Vec::new();
        for file in &self.files {
            // PORT (CanonicalSourceFile): Go prefers the canonical source
            // file's path key (`file.CanonicalSourceFile()`) — the
            // content-mapper surface, nil this wave; the port reads the
            // file's own key (PORTING-NOTES).
            let root_path = file.path_key();
            if crate::emitter::source_file_may_be_emitted(file, self, false, false)
                && !root_paths.contains(root_path)
            {
                diagnostics.push(ProcessingDiagnostic {
                    kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                    reason: None,
                    explanation: Some(IncludeExplainingDiagnostic {
                        file: file.path_key().clone(),
                        diagnostic_reason: None,
                        message: &tsc_diagnostics::FILE_0_IS_NOT_LISTED_WITHIN_THE_FILE_LIST_OF_PROJECT_1_PROJECTS_MUST_LIST_ALL_FILES_OR_USE_AN_INCLUDE_PATTERN,
                        args: vec![
                            file.file_name().as_string().to_owned(),
                            config_file_path.as_string().to_owned(),
                        ],
                    }),
                });
            }
        }
        diagnostics
    }

    /// Go: `func (p *Program) collectContentMapperOptionDiagnostics()` — the
    /// content-mapper seam (see file_loader.rs).
    fn collect_content_mapper_option_diagnostics(&mut self) {}

    /// Go: `func (p *Program) GetCheckerPool() CheckerPool` — the checker
    /// wave seam (checkerpool.rs).
    pub fn get_checker_pool(&self) -> Option<Arc<dyn crate::checkerpool::CheckerPool>> {
        self.checker_pool.clone()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// verifyCompilerOptions' option-diagnostic machinery (Go: program.go's
// verifyCompilerOptions closure bundle — the M6 config-node wave)
// ────────────────────────────────────────────────────────────────────────────

/// Go: the memoized closure bundle `verifyCompilerOptions` builds — the
/// option-diagnostic creators that position removed-option diagnostics on
/// the config file's AST nodes.
///
/// PORT: Go's closures capture `p` and append into `programDiagnostics`
/// inside `createOptionDiagnosticInObjectLiteralSyntax` /
/// `createCompilerOptionsDiagnostic`; the port collects into `diagnostics`
/// and `create_diagnostic_for_option` pushes exactly once per Go append
/// (each Go create* call appends at most one diagnostic, in call order —
/// same sequence, same dedup input).
struct OptionDiagnosticContext<'a> {
    /// Go: `p.opts.Config` (the memoizes read `Config.ConfigFile`).
    config: &'a tsc_tsoptions::ParsedCommandLine,
    /// Go: the `sourceFile` memoize (`ConfigFile.SourceFile`, nil-safe).
    /// PORT: the config AST rides behind tsoptions' `Mutex` (Send/Sync —
    /// see TsConfigSourceFile); the memo holds the guard for the whole
    /// verify pass (single-threaded, no nested lock).
    source_file: OnceLock<Option<std::sync::MutexGuard<'a, SourceFile>>>,
    /// Go: the `configFilePath` memoize.
    config_file_path: OnceLock<RootedFilePath>,
    /// Go: the `getCompilerOptionsPropertySyntax` memoize — the
    /// `compilerOptions` PropertyAssignment node.
    compiler_options_property_syntax: OnceLock<Option<NodeId>>,
    /// Go: the `getCompilerOptionsObjectLiteralSyntax` memoize — the
    /// `compilerOptions` value's ObjectLiteralExpression node.
    compiler_options_object_literal_syntax: OnceLock<Option<NodeId>>,
    /// The collected program diagnostics (appended by the creators; the
    /// caller extends `program_diagnostics` with them).
    diagnostics: Vec<FileDiagnostic>,
}

impl<'a> OptionDiagnosticContext<'a> {
    fn new(config: &'a tsc_tsoptions::ParsedCommandLine) -> Self {
        OptionDiagnosticContext {
            config,
            source_file: OnceLock::new(),
            config_file_path: OnceLock::new(),
            compiler_options_property_syntax: OnceLock::new(),
            compiler_options_object_literal_syntax: OnceLock::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Go: `sourceFile := core.Memoize(func() *ast.SourceFile { configFile
    /// := p.opts.Config.ConfigFile; if configFile == nil { return nil };
    /// return configFile.SourceFile })`.
    fn source_file(&self) -> Option<&SourceFile> {
        self.source_file
            .get_or_init(|| {
                self.config
                    .config_file
                    .as_ref()
                    .map(|config_file| config_file.lock_source_file())
            })
            .as_deref()
    }

    /// Go: `configFilePath := core.Memoize(...)` — the config source file's
    /// name, "" without one.
    fn config_file_path(&self) -> RootedFilePath {
        self.config_file_path
            .get_or_init(|| {
                self.source_file()
                    .map_or_else(RootedFilePath::default, |file| file.file_name().clone())
            })
            .clone()
    }

    /// Go: `getCompilerOptionsPropertySyntax := core.Memoize(func() *ast.PropertyAssignment {
    /// return tsoptions.ForEachTsConfigPropArray(sourceFile(), "compilerOptions", core.Identity) })`.
    fn get_compiler_options_property_syntax(&self) -> Option<NodeId> {
        *self.compiler_options_property_syntax.get_or_init(|| {
            tsc_tsoptions::for_each_ts_config_prop_array(
                self.source_file(),
                "compilerOptions",
                |_store, property| Some(property),
            )
        })
    }

    /// Go: `getCompilerOptionsObjectLiteralSyntax := core.Memoize(...)`.
    fn get_compiler_options_object_literal_syntax(&self) -> Option<NodeId> {
        *self
            .compiler_options_object_literal_syntax
            .get_or_init(|| {
                let property = self.get_compiler_options_property_syntax()?;
                let source_file = self
                    .source_file()
                    .expect("compilerOptions property syntax implies a source file");
                let initializer = source_file
                    .node(property)
                    .as_property_assignment()
                    .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
                    .initializer;
                if initializer.is_none() {
                    // Go: `compilerOptionsProperty.Initializer != nil`.
                    return None;
                }
                if tsc_ast::is_object_literal_expression(source_file.node(initializer)) {
                    return Some(initializer);
                }
                None
            })
    }

    /// Go: `ast.NewDiagnostic(file, loc, message, args...)` paired with the
    /// config file's name (the port's FileDiagnostic pairing —
    /// diagnostics_collection.rs).
    fn diagnostic_for_node(
        &self,
        source_file: &SourceFile,
        node: NodeId,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) -> FileDiagnostic {
        FileDiagnostic {
            file_name: Some(source_file.file_name().clone()),
            diagnostic: tsc_tsoptions::create_diagnostic_for_node_in_source_file(
                source_file, node, message, args,
            ),
        }
    }

    /// Go: `createOptionDiagnosticInObjectLiteralSyntax := func(objectLiteral, onKey, key1, key2, message, args...) *ast.Diagnostic`.
    ///
    /// PORT: returns the diagnostic WITHOUT appending (the port appends in
    /// `create_diagnostic_for_option`, once per Go append — see the struct
    /// header note).
    fn create_option_diagnostic_in_object_literal_syntax(
        &self,
        object_literal: Option<NodeId>,
        on_key: bool,
        key1: &str,
        key2: &str,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) -> Option<FileDiagnostic> {
        let source_file = self.source_file()?;
        tsc_tsoptions::for_each_property_assignment(
            source_file,
            object_literal,
            key1,
            key2,
            |store, property| {
                let data = store
                    .node(property)
                    .as_property_assignment()
                    .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
                    .clone();
                // Go: `core.IfElse(onKey, property.Name(), property.Initializer)`.
                let node = if on_key { data.name } else { data.initializer };
                if node.is_none() {
                    // Go would nil-deref here; the JSON parser never mints a
                    // PropertyAssignment without a name/initializer.
                    return None;
                }
                Some(self.diagnostic_for_node(source_file, node, message, args))
            },
        )
    }

    /// Go: `createCompilerOptionsDiagnostic := func(message, args...) *ast.Diagnostic` —
    /// positioned on the `compilerOptions` property's NAME when the config
    /// has one, else the fileless compiler diagnostic.
    fn create_compiler_options_diagnostic(
        &self,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) -> FileDiagnostic {
        match (self.source_file(), self.get_compiler_options_property_syntax()) {
            (Some(source_file), Some(property)) => {
                let name = source_file
                    .node(property)
                    .as_property_assignment()
                    .expect("IsPropertyAssignment guaranteed by the walk")
                    .name;
                self.diagnostic_for_node(source_file, name, message, args)
            }
            _ => FileDiagnostic::compiler_diagnostic(message, args),
        }
    }

    /// Go: `createDiagnosticForOption := func(onKey, option1, option2, message, args...) *ast.Diagnostic`.
    fn create_diagnostic_for_option(
        &mut self,
        on_key: bool,
        option1: &str,
        option2: &str,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) -> FileDiagnostic {
        let diag = self.create_option_diagnostic_in_object_literal_syntax(
            self.get_compiler_options_object_literal_syntax(),
            on_key,
            option1,
            option2,
            message,
            args,
        );
        let diag = diag
            .unwrap_or_else(|| self.create_compiler_options_diagnostic(message, args));
        self.diagnostics.push(diag.clone());
        diag
    }

    /// Go: `createRemovedOptionDiagnostic := func(name, value, useInstead)` —
    /// TS5102 (no value) / TS5108 (with value), plus the `Use_0_instead`
    /// message chain for baseUrl's paths suggestion.
    fn create_removed_option_diagnostic(&mut self, name: &str, value: &str, use_instead: &str) {
        let (message, args): (&'static tsc_diagnostics::Message, Vec<String>) =
            if value.is_empty() {
                (
                    &tsc_diagnostics::OPTION_0_HAS_BEEN_REMOVED_PLEASE_REMOVE_IT_FROM_YOUR_CONFIGURATION,
                    vec![name.to_string()],
                )
            } else {
                (
                    &tsc_diagnostics::OPTION_0_1_HAS_BEEN_REMOVED_PLEASE_REMOVE_IT_FROM_YOUR_CONFIGURATION,
                    vec![name.to_string(), value.to_string()],
                )
            };
        let mut diag = self.create_diagnostic_for_option(
            value.is_empty(), /*onKey*/
            name,
            "",
            message,
            &args,
        );
        if !use_instead.is_empty() {
            // Go: `diag.AddMessageChain(ast.NewCompilerDiagnostic(diagnostics.Use_0_instead, useInstead))`.
            diag.diagnostic
                .add_message_chain(tsc_tsoptions::new_compiler_diagnostic(
                    &tsc_diagnostics::USE_0_INSTEAD,
                    &[use_instead.to_string()],
                ));
            // The pushed copy (Go mutates the shared diagnostic in place).
            if let Some(last) = self.diagnostics.last_mut() {
                *last = diag;
            }
        }
    }

    /// Go: `createDiagnosticForOptionName := func(message, option1, option2, args...)`
    /// — `option1, option2` become the leading args; onKey = true.
    fn create_diagnostic_for_option_name(
        &mut self,
        message: &'static tsc_diagnostics::Message,
        option1: &str,
        option2: &str,
        args: &[String],
    ) {
        let mut new_args = Vec::with_capacity(args.len() + 2);
        new_args.push(option1.to_string());
        new_args.push(option2.to_string());
        new_args.extend(args.iter().cloned());
        self.create_diagnostic_for_option(true, option1, option2, message, &new_args);
    }

    /// Go: `createOptionValueDiagnostic := func(option1, message, args...)`
    /// — onKey = false, no second option key.
    fn create_option_value_diagnostic(
        &mut self,
        option1: &str,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) {
        self.create_diagnostic_for_option(false, option1, "", message, args);
    }

    /// Go: `forEachOptionPathsSyntax := func(callback func(*ast.PropertyAssignment) *ast.Diagnostic) *ast.Diagnostic`
    /// — the `paths` property assignment of the compilerOptions object.
    fn for_each_option_paths_syntax(
        &self,
        callback: impl FnOnce(&dyn tsc_ast::NodeStore, NodeId) -> Option<FileDiagnostic>,
    ) -> Option<FileDiagnostic> {
        let source_file = self.source_file()?;
        tsc_tsoptions::for_each_property_assignment(
            source_file,
            self.get_compiler_options_object_literal_syntax(),
            "paths",
            "",
            callback,
        )
    }

    /// Go: `createDiagnosticForOptionPaths := func(onKey, key, message, args...) *ast.Diagnostic`
    /// — the paths object's `key` property (positioned), the
    /// compilerOptions-diagnostic fallback.
    fn create_diagnostic_for_option_paths(
        &mut self,
        on_key: bool,
        key: &str,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) {
        let diag = self.for_each_option_paths_syntax(|store, property| {
            let initializer = store
                .node(property)
                .as_property_assignment()
                .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
                .initializer;
            if initializer.is_none() {
                return None;
            }
            if tsc_ast::is_object_literal_expression(store.node(initializer)) {
                return self.create_option_diagnostic_in_object_literal_syntax(
                    Some(initializer),
                    on_key,
                    key,
                    "",
                    message,
                    args,
                );
            }
            None
        });
        let diag = diag
            .unwrap_or_else(|| self.create_compiler_options_diagnostic(message, args));
        self.diagnostics.push(diag);
    }

    /// Go: `createDiagnosticForOptionPathKeyValue := func(key, valueIndex, message, args...) *ast.Diagnostic`
    /// — the `key` property's array-literal `valueIndex`-th element
    /// (positioned), the compilerOptions-diagnostic fallback.
    fn create_diagnostic_for_option_path_key_value(
        &mut self,
        key: &str,
        value_index: usize,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) {
        let diag = self.for_each_option_paths_syntax(|store, property| {
            let initializer = store
                .node(property)
                .as_property_assignment()
                .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
                .initializer;
            if initializer.is_none() {
                return None;
            }
            if !tsc_ast::is_object_literal_expression(store.node(initializer)) {
                return None;
            }
            tsc_tsoptions::for_each_property_assignment(
                store,
                Some(initializer),
                key,
                "",
                |store, key_props| {
                    let initializer = store
                        .node(key_props)
                        .as_property_assignment()
                        .expect("IsPropertyAssignment guaranteed by ForEachPropertyAssignment")
                        .initializer;
                    if initializer.is_none() || !tsc_ast::is_array_literal_expression(store.node(initializer)) {
                        return None;
                    }
                    let elements = store
                        .node(initializer)
                        .as_array_literal_expression()
                        .expect("IsArrayLiteralExpression checked")
                        .elements
                        .clone()?;
                    let value = *elements.nodes.get(value_index)?;
                    // Go appends INSIDE the walk callback here (the inner
                    // createOptionDiagnosticInObjectLiteralSyntax appends
                    // when it returns non-nil); the fallback below appends
                    // only when the whole walk returned nil — one push in
                    // total, mirroring Go's single append.
                    let source_file = self.source_file()?;
                    Some(self.diagnostic_for_node(source_file, value, message, args))
                },
            )
        });
        let diag = diag
            .unwrap_or_else(|| self.create_compiler_options_diagnostic(message, args));
        self.diagnostics.push(diag);
    }
}

/// Go: `func hasZeroOrOneAsteriskCharacter(str string) bool` (program.go).
fn has_zero_or_one_asterisk_character(string: &str) -> bool {
    let mut seen_asterisk = false;
    for ch in string.chars() {
        if ch == '*' {
            if !seen_asterisk {
                seen_asterisk = true;
            } else {
                // have already seen asterisk
                return false;
            }
        }
    }
    true
}

/// Go: `func moduleResolutionSupportsPackageJsonExportsAndImports(moduleResolution) bool`.
fn module_resolution_supports_package_json_exports_and_imports(
    module_resolution: ModuleResolutionKind,
) -> bool {
    (ModuleResolutionKind::Node16..=ModuleResolutionKind::NodeNext).contains(&module_resolution)
        || module_resolution == ModuleResolutionKind::Bundler
}

/// Go: `func emitModuleKindIsNonNodeESM(moduleKind core.ModuleKind) bool`.
fn emit_module_kind_is_non_node_esm(module_kind: ModuleKind) -> bool {
    (ModuleKind::ES2015..=ModuleKind::ESNext).contains(&module_kind)
}

/// Go: `func (i ModuleKind) String() string` (core's generated stringer) —
/// a crate-local helper: the Rust core carries no Display for ModuleKind
/// (the value-type stringers landed only where the Go package exposes
/// String publicly beyond the stringer — PORTING-NOTES dedup candidate).
fn module_kind_string(module_kind: ModuleKind) -> String {
    match module_kind {
        ModuleKind::None => "None",
        ModuleKind::CommonJS => "CommonJS",
        ModuleKind::AMD => "AMD",
        ModuleKind::UMD => "UMD",
        ModuleKind::System => "System",
        ModuleKind::ES2015 => "ES2015",
        ModuleKind::ES2020 => "ES2020",
        ModuleKind::ES2022 => "ES2022",
        ModuleKind::ESNext => "ESNext",
        ModuleKind::Node16 => "Node16",
        ModuleKind::Node18 => "Node18",
        ModuleKind::Node20 => "Node20",
        ModuleKind::NodeNext => "NodeNext",
        ModuleKind::Preserve => "Preserve",
    }
    .to_string()
}

/// One emit-path-uniqueness finding — Go's `verifyEmitFilePath` appends into
/// `programDiagnostics` + `hasEmitBlockingDiagnostics` through
/// `blockEmittingOfFile` (needs `&mut self`); the findings collect during
/// the `&self` walk and apply after.
struct EmitPathFinding {
    emit_file_name: RootedFilePath,
    diagnostic: FileDiagnostic,
}

/// Go: the Program's `OutputPathsHost` implementation (outputpaths.go's
/// interface — ForEachEmittedFile's `p` argument).
///
/// PORT (content-mapper seam): `ContentMapperExtensions` reads the config's
/// content mappers — always empty this wave (see file_loader.rs); the
/// adapter keeps the Go shape with the empty slice.
struct ProgramOutputPathsHost<'a> {
    program: &'a Program,
}

impl tsc_outputpaths::OutputPathsHost for ProgramOutputPathsHost<'_> {
    fn common_source_directory(&self) -> RootedDirectoryPath {
        self.program.common_source_directory()
    }

    fn content_mapper_extensions(&self) -> &[String] {
        &[]
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.program.case_sensitivity
    }
}

/// Go: `core.Must(json.Marshal(suggestion))` — encoding/json's string
/// marshal with HTML escaping ON (the default): `"`, `\`, control
/// characters, `<`, `>`, `&`, U+2028/U+2029 escape exactly as Go's encoder
/// does (lowercase hex). PORT: hand-rolled — the compiler crate has no
/// serde_json dependency, and the escape table is encoding/json's.
fn json_marshal_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
