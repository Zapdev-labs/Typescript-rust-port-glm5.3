// Ported from tsc/internal/compiler/fileloader.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - `tracing` is not ported (no tracing crate yet): the loader's
//     `tracing.Push` hooks are dropped; the Trace callback stays on the
//     host.
//   - The content-mapper machinery is not ported (the contentmapper crate's
//     Host/project/transform pipeline): `parseContentMappedFile`, the
//     failure-budget bookkeeping, and the content-mapper diagnostics degrade
//     to Go's zero-config behavior (no content-mapper extensions are
//     configured), and `loader.contentMapperExtensions` is always empty.
//     See `ContentMapperSeam` at the bottom of this file.
//   - The module resolver's error channel (`moduleResolutionError`) is
//     dropped: the Rust module crate dropped the vestigial error (its
//     `ResolveModuleNameResult` note).
//   - `compilerResolutionHost` lives in host.rs (the Go file's other half).
//   - `parseTask`/`filesParser` live in files_parser.rs (Go splits them the
//     same way).
//   - The factory mutex + shared `ast.NodeFactory` (createSyntheticImport)
//     is per-file in the port: the port's NodeFactory borrows the owning
//     SourceFile's arena (crates/ast/src/factory.rs), so the synthetic nodes
//     are allocated in the parsed file's arena (same parent/loc semantics;
//     the cross-file factory pool is a Go GC optimization).

use std::sync::atomic::AtomicI32;
use std::sync::Arc;

use tsc_ast::{
    Diagnostic, Kind, NodeData, NodeFactory, NodeId, NodeStore, SourceFile, SourceFileMetaData,
    SourceFileParseOptions, node_text,
};
use tsc_core::compileroptions::{
    ResolutionMode, ResolutionModeCommonJS, ResolutionModeESM, ResolutionModeNone,
};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use tsc_core::scriptkind::ScriptKind;
use tsc_module::{
    DiagAndArgs, ModeAwareCache, ModeAwareCacheKey, PackageId, ResolutionHost, ResolvedModule,
    ResolvedTypeReferenceDirective, Resolver, ResolverOptions,
};
use tsc_tspath::{
    CaseSensitivity, PathKey, RootedDirectoryPath, RootedFilePath, RootedPath,
    EXTENSION_CJS, EXTENSION_CTS, EXTENSION_MJS, EXTENSION_MTS,
    SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT,
};

use crate::ast_shims::{
    get_emit_module_format_of_file_worker, get_external_module_indicator_options,
    get_implied_node_format_for_emit_worker, get_implied_node_format_for_file,
    get_jsx_implicit_import_base, get_jsx_runtime_import, is_exclusively_type_only_import_or_export,
    is_source_phase_import, is_source_phase_import_call, is_require_call, should_transform_import_call,
    walk_up_parenthesized_expressions,
};
use crate::file_include::{
    AutomaticTypeDirectiveFileData, FileIncludeKind, FileIncludeReason, ReferencedFileData,
};
use crate::files_parser::ParseTask;
use crate::host::CompilerResolutionHost;
use crate::processing_diagnostic::{
    IncludeExplainingDiagnostic, ProcessingDiagnostic, ProcessingDiagnosticKind,
};
use crate::program::{ProgramConfig, ProgramOptions};
use crate::project_reference_file_mapper::{
    ProjectReferenceFileMapper, ProjectReferenceFileMapperBuilder,
};

/// Go: `type libResolution struct`.
#[derive(Default)]
pub struct LibResolution {
    pub library_name: String,
    pub resolution: Option<Arc<ResolvedModule>>,
    pub trace: Vec<DiagAndArgs>,
}

/// Go: `type LibFile struct`.
#[derive(Clone, Debug, Default)]
pub struct LibFile {
    /// Go: `Name string`.
    pub name: String,
    pub(crate) path: RootedFilePath,
    pub(crate) path_key: PathKey,
    /// Go: `Replaced bool`.
    pub replaced: bool,
}

impl LibFile {
    pub fn path(&self) -> &RootedFilePath {
        &self.path
    }
    pub fn path_key(&self) -> &PathKey {
        &self.path_key
    }
}

/// Go: `type redirectsFile struct` (HasFileName).
#[derive(Clone, Debug, Default)]
pub struct RedirectsFile {
    /// Index of file at which this redirect file needs to be iterated.
    pub index: usize,
    pub file_name: RootedFilePath,
    pub path: PathKey,
    pub target: PathKey,
}

/// Go: `type DuplicateSourceFile struct`.
///
/// PORT: `Hash xxh3.Uint128`, `ContentMapper string`, and
/// `IsContentMapperFailureStub` are the content-mapper parse-cache surface —
/// the seam keeps the fields (empty/zero) with the machinery deferred (see
/// the file header).
#[derive(Clone, Debug, Default)]
pub struct DuplicateSourceFile {
    pub parse_options: SourceFileParseOptions,
    /// ContentMapperParseOptions are the acquire-time options for a
    /// content-mapped parse-cache entry (Go field; the surface is deferred).
    pub content_mapper_parse_options: SourceFileParseOptions,
    /// Go: `Hash xxh3.Uint128` — not computed in this wave.
    pub hash: (),
    pub script_kind: ScriptKind,
    /// ContentMapper is the identity of the content mapper that produced
    /// this file, or "" if the file is not content-mapped.
    pub content_mapper: String,
    /// IsContentMapperFailureStub reports whether the file is an empty
    /// placeholder from a failed transform.
    pub is_content_mapper_failure_stub: bool,
}

impl DuplicateSourceFile {
    /// The Go construction sites (casing duplicates, package dedup).
    pub(crate) fn of(file: &SourceFile) -> Self {
        DuplicateSourceFile {
            parse_options: file.parse_options().clone(),
            script_kind: file.payload().script_kind,
            ..Default::default()
        }
    }
}

/// Go: `type processedFiles struct` — the loader's output, moved into the
/// Program.
#[derive(Default)]
pub struct ProcessedFiles {
    /// Go: `files []*ast.SourceFile` — lib files first, then program files
    /// (the merged `allFiles` list).
    pub files: Vec<SourceFile>,
    /// duplicateSourceFiles tracks parsed files loaded during program
    /// construction that were later dropped from the final program, such as
    /// losing filename casing variants for the same path or files hidden
    /// behind package redirect deduplication.
    pub duplicate_source_files: Vec<DuplicateSourceFile>,
    /// Go: `filesByPath map[PathKey]*SourceFile` — indexes into `files`.
    pub files_by_path: rustc_hash::FxHashMap<PathKey, usize>,
    /// Go: `projectReferenceFileMapper *projectReferenceFileMapper`.
    pub project_reference_file_mapper: Option<Arc<ProjectReferenceFileMapper>>,
    /// Go: `missingFiles collections.Set[PathKey]`.
    pub missing_files: rustc_hash::FxHashSet<PathKey>,
    pub resolved_modules:
        rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedModule>>>>,
    pub type_resolutions_in_file:
        rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>>>,
    pub source_file_meta_datas: rustc_hash::FxHashMap<PathKey, SourceFileMetaData>,
    /// Go: `jsxRuntimeImportSpecifiers map[PathKey]*jsxRuntimeImportSpecifier`
    /// — the module reference text (see files_parser.rs).
    pub jsx_runtime_import_specifiers: rustc_hash::FxHashMap<PathKey, String>,
    /// Go: `importHelpersImportSpecifiers map[PathKey]*ast.StringLiteralNode`
    /// — the module reference text.
    pub import_helpers_import_specifiers: rustc_hash::FxHashMap<PathKey, String>,
    /// Go: `libFiles map[PathKey]*LibFile`.
    pub lib_files: rustc_hash::FxHashMap<PathKey, Arc<LibFile>>,
    /// List of present unsupported extensions (Go comment).
    pub source_files_found_searching_node_modules: rustc_hash::FxHashSet<PathKey>,
    /// fileIncludeData (flattened).
    pub file_include_reasons:
        rustc_hash::FxHashMap<PathKey, Vec<Arc<FileIncludeReason>>>,
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
    /// fatally (reported once per mapper) — the content-mapper seam.
    pub content_mapper_diagnostics: Vec<Arc<Diagnostic>>,
    pub finished_processing: bool,
}

/// Go: `type sourceFileFromReferenceDiagnostic struct`.
#[derive(Clone, Debug)]
pub(crate) struct SourceFileFromReferenceDiagnostic {
    pub message: &'static tsc_diagnostics::Message,
    pub args: Vec<String>,
}

/// Go: `type resolvedRef struct`.
#[derive(Default)]
pub(crate) struct ResolvedRef {
    pub file_name: RootedFilePath,
    pub path: PathKey,
    pub increase_depth: bool,
    pub elide_on_depth: bool,
    pub include_reason: Option<Arc<FileIncludeReason>>,
    pub package_id: PackageId,
}

/// Go: `type jsxRuntimeImportSpecifier struct` — the text form (see
/// files_parser.rs).
#[derive(Clone, Debug, Default)]
pub struct JsxRuntimeImportSpecifier {
    pub module_reference: String,
}

/// Go: `type fileLoader struct`.
pub(crate) struct FileLoader<'a> {
    pub opts: &'a ProgramConfig,
    pub host: Arc<dyn crate::host::CompilerHost>,
    /// Go: the loader has no singleThreaded field — the WorkGroups are built
    /// with the bool `processAllProgramFiles` receives; the port's
    /// `filesParser.parse` builds its WorkGroup from the loader, so the bool
    /// lives here (PORTING-NOTES).
    pub single_threaded: bool,
    pub resolver: Option<Arc<dyn Resolver>>,
    pub default_library_path: RootedDirectoryPath,
    pub case_sensitivity: CaseSensitivity,
    pub supported_extensions: Vec<Vec<String>>,
    pub supported_extensions_with_json_if_resolve_json_module: Vec<Vec<String>>,
    /// Go: `contentMapperExtensions []string` — the content-mapper seam; the
    /// list is carried (the parse-time extension check degrades — see
    /// parseSourceFile's PORT note) until that wave lands.
    #[allow(dead_code)]
    pub content_mapper_extensions: Vec<String>,

    pub files_parser: crate::files_parser::FilesParser,
    pub root_tasks: Vec<usize>,

    pub total_file_count: AtomicI32,
    pub lib_file_count: AtomicI32,

    pub project_references: Option<ProjectReferenceFileMapperBuilder>,

    pub path_for_lib_file_cache: tsc_collections::SyncMap<String, Arc<LibFile>>,
    pub path_for_lib_file_resolutions: tsc_collections::SyncMap<PathKey, Arc<LibResolution>>,
}

impl<'a> FileLoader<'a> {
    pub(crate) fn compiler_options(&self) -> &CompilerOptions {
        self.opts.compiler_options()
    }

    pub(crate) fn skip_module_resolution(&self) -> bool {
        self.opts.skip_module_resolution
    }

    pub(crate) fn can_use_project_reference_source(&self) -> bool {
        self.opts.can_use_project_reference_source()
    }

    pub(crate) fn supported_extensions_flattened(&self) -> Vec<String> {
        // Go: core.Flatten(loader.supportedExtensions).
        self.supported_extensions.concat()
    }

    /// Go: `func (p *fileLoader) toPath(file tspath.RootedPath) tspath.PathKey`.
    ///
    /// Kept for Go surface parity (the project-reference parser threads the
    /// loader's case sensitivity directly instead — see
    /// project_reference_parser.rs); used by the project-reference seam.
    #[allow(dead_code)]
    pub(crate) fn to_path(&self, file: &RootedPath) -> PathKey {
        self.case_sensitivity.path_key(file)
    }

    /// Go: `func (p *fileLoader) isSupportedExtension(canonicalFileName) bool`.
    pub(crate) fn is_supported_extension(&self, canonical_file_name: &PathKey) -> bool {
        // PORT: the extension groups are `Vec<Vec<String>>` (the tsoptions
        // shape); `extension_is_one_of` takes `&[&str]`, so each group is
        // mapped to borrowed slices first (Go: any-of-the-group check).
        self.supported_extensions_with_json_if_resolve_json_module
            .iter()
            .any(|group| {
                let extensions: Vec<&str> = group.iter().map(|e| e.as_str()).collect();
                canonical_file_name.extension_is_one_of(&extensions)
            })
    }

    fn project_references(&self) -> &ProjectReferenceFileMapperBuilder {
        self.project_references
            .as_ref()
            .expect("project references initialized before parsing")
    }

    pub(crate) fn project_references_mapper(&self) -> Option<Arc<ProjectReferenceFileMapper>> {
        self.project_references.as_ref().map(|builder| {
            Arc::clone(&builder.mapper)
        })
    }

    /// The builder's parse-file redirect (Go: `projectReferences.getParseFileRedirect`).
    pub(crate) fn get_parse_file_redirect(
        &self,
        file_name: &RootedFilePath,
        path: &PathKey,
    ) -> (RootedFilePath, PathKey) {
        let builder = self.project_references();
        builder.get_parse_file_redirect(&crate::host::HasFileName::new(
            file_name.clone(),
            path.clone(),
        ))
    }

    /// The builder's resolution redirect (Go:
    /// `projectReferences.getRedirectForResolution`).
    fn get_redirect_for_resolution(
        &self,
        file_name: &RootedFilePath,
        path: &PathKey,
    ) -> (Option<Arc<crate::project_reference_file_mapper::ResolvedProjectReference>>, RootedFilePath) {
        let builder = self.project_references();
        builder.get_redirect_for_resolution(&crate::host::HasFileName::new(
            file_name.clone(),
            path.clone(),
        ))
    }

    fn get_redirect_parsed_command_line_for_resolution(
        &self,
        file_name: &RootedFilePath,
        path: &PathKey,
    ) -> Option<Arc<crate::project_reference_file_mapper::ResolvedProjectReference>> {
        self.get_redirect_for_resolution(file_name, path).0
    }

    /// Go: `func (p *fileLoader) addRootTask(fileName, libFile, includeReason)`.
    fn add_root_task(
        &mut self,
        file_name: RootedFilePath,
        lib_file: Option<Arc<LibFile>>,
        include_reason: Option<Arc<FileIncludeReason>>,
    ) {
        if self.compiler_options().allow_non_ts_extensions.is_true()
            || file_name.has_extension()
        {
            let task = crate::files_parser::ParseTask {
                // Go: tspath.RootedPath(fileName) → toPath(fileName)
                path: self.case_sensitivity.path_key(&RootedPath::from(
                    file_name.as_string(),
                )),
                normalized_file_path: file_name,
                lib_file,
                include_reason,
                ..Default::default()
            };
            let index = self.files_parser.arena.push(task);
            self.root_tasks.push(index);
        }
    }

    /// Go: `func (p *fileLoader) addRootFileTask(fileName, referenceText, libFile, includeReason)`.
    fn add_root_file_task(
        &mut self,
        file_name: RootedFilePath,
        reference_text: &str,
        lib_file: Option<Arc<LibFile>>,
        include_reason: Arc<FileIncludeReason>,
    ) {
        let (resolved_file, resolved_path, diagnostic) =
            self.get_source_file_from_reference(&file_name, reference_text);
        let mut normalized_file_path = resolved_file.clone();
        let mut path = resolved_path.clone();
        let mut failed_lookup = false;
        let mut processing_diagnostics: Vec<ProcessingDiagnostic> = Vec::new();
        if let Some(diagnostic) = diagnostic {
            normalized_file_path = file_name.clone();
            path = self.case_sensitivity.path_key(&RootedPath::from(file_name.as_string()));
            failed_lookup = true;
            processing_diagnostics.push(ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                reason: None,
                explanation: Some(IncludeExplainingDiagnostic {
                    file: PathKey::default(),
                    diagnostic_reason: Some(Arc::clone(&include_reason)),
                    message: diagnostic.message,
                    args: diagnostic.args,
                }),
            });
        }
        let task = ParseTask {
            normalized_file_path,
            path,
            lib_file,
            include_reason: Some(Arc::new(FileIncludeReason {
                kind: include_reason.kind,
                index: include_reason.index,
                is_default_lib: include_reason.is_default_lib,
                referenced_file: include_reason.referenced_file.clone(),
                automatic_type_directive: include_reason.automatic_type_directive.clone(),
                canonical_source_file: include_reason.canonical_source_file.clone(),
            })),
            failed_lookup,
            processing_diagnostics,
            ..Default::default()
        };
        let index = self.files_parser.arena.push(task);
        self.root_tasks.push(index);
    }

    /// Go: `func (p *fileLoader) addAutomaticTypeDirectiveTasks()`.
    fn add_automatic_type_directive_tasks(&mut self) {
        let containing_file_name = self
            .opts
            .config
            .base_directory()
            .resolve_file(tsc_module::InferredTypesContainingFile);
        let task = ParseTask {
            path: self.case_sensitivity.path_key(&RootedPath::from(
                containing_file_name.as_string(),
            )),
            normalized_file_path: containing_file_name,
            is_for_automatic_type_directive: true,
            ..Default::default()
        };
        let index = self.files_parser.arena.push(task);
        self.root_tasks.push(index);
    }

    /// Go: `func (p *fileLoader) addProjectReferenceTasks(singleThreaded bool)`.
    fn add_project_reference_tasks(&mut self, _single_threaded: bool) {
        let compiler_resolution_host: Arc<dyn ResolutionHost> =
            Arc::new(CompilerResolutionHost {
                host: Arc::clone(&self.host),
                base_directory: self.opts.config.base_directory().clone(),
            });
        let mut mapper = ProjectReferenceFileMapper::new(
            Arc::clone(&self.opts.config),
            self.opts.can_use_project_reference_source(),
        );
        // Go: the builder's host is the compilerResolutionHost.
        let project_reference_paths: Vec<RootedFilePath> = self
            .opts
            .config
            .project_references()
            .iter()
            .map(|r| r.path.clone())
            .collect();
        if !project_reference_paths.is_empty() {
            let host = Arc::clone(&self.host);
            let parser_tasks = {
                // Go: createProjectReferenceParseTasks + parser.parse.
                let mut tasks: Vec<crate::project_reference_parser::ProjectReferenceParseTask> =
                    project_reference_paths
                        .into_iter()
                        .map(|config_name| {
                            crate::project_reference_parser::ProjectReferenceParseTask {
                                config_name,
                                ..Default::default()
                            }
                        })
                        .collect();
                let root_task_indices: Vec<usize> = (0..tasks.len()).collect();
                let get_resolved = |file_name: &RootedFilePath, path: &PathKey| {
                    host.get_resolved_project_reference(file_name, path)
                };
                let parser = crate::project_reference_parser::ProjectReferenceParser::new(
                    &get_resolved,
                    self.case_sensitivity,
                );
                let parsed = parser.parse(&root_task_indices, &mut tasks);
                (parsed, tasks)
            };
            let (parsed, mut tasks) = parser_tasks;
            let root_config_path = mapper.root_config_path();
            parsed.init_mapper(&tasks, &root_config_path, &mut mapper);
            let _ = &mut tasks;
        }
        self.project_references = Some(ProjectReferenceFileMapperBuilder {
            mapper: Arc::new(mapper),
            host: compiler_resolution_host,
        });
    }

    /// Go: `func (p *fileLoader) sortLibs(libFiles []*ast.SourceFile)`.
    pub(crate) fn sort_libs(&self, lib_files: &mut [SourceFile]) {
        lib_files.sort_by(|f1, f2| {
            let a = self.get_default_lib_file_priority(f1);
            let b = self.get_default_lib_file_priority(f2);
            a.cmp(&b)
        });
    }

    /// Go: `func (p *fileLoader) getDefaultLibFilePriority(a *ast.SourceFile) int`.
    fn get_default_lib_file_priority(&self, a: &SourceFile) -> usize {
        if let Some(relative) = a
            .file_name()
            .relative_to(&self.default_library_path)
            .filter(|relative| !relative.is_empty())
        {
            let basename = relative.base_name();
            if basename == "lib.d.ts" || basename == "lib.es6.d.ts" {
                return 0;
            }
            let name = basename
                .strip_prefix("lib.")
                .unwrap_or(basename)
                .strip_suffix(".d.ts")
                .unwrap_or(basename);
            let libs = tsc_tsoptions::libs();
            if let Some(index) = libs.iter().position(|lib| lib == name) {
                return index + 1;
            }
        }
        tsc_tsoptions::libs().len() + 2
    }

    /// Go: `func (p *fileLoader) loadSourceFileMetaData(fileName) ast.SourceFileMetaData`.
    pub(crate) fn load_source_file_meta_data(
        &self,
        file_name: &RootedFilePath,
    ) -> SourceFileMetaData {
        if self.skip_module_resolution() {
            return SourceFileMetaData {
                implied_node_format: get_implied_node_format_for_file(file_name, ""),
                ..Default::default()
            };
        }

        let resolver = self
            .resolver
            .as_ref()
            .expect("resolver created before file loads");
        let resolution_data = resolver.get_resolution_data();
        let host: Arc<dyn ResolutionHost> = Arc::new(CompilerResolutionHost {
            host: Arc::clone(&self.host),
            base_directory: self.opts.config.base_directory().clone(),
        });
        let new_resolver = resolution_data.new_resolver(host);
        let package_json_scope =
            new_resolver.get_package_scope_for_path(&file_name.directory());
        let module_resolution_kind = self.compiler_options().get_module_resolution_kind();

        let mut package_json_type = String::new();
        let mut package_json_directory = RootedDirectoryPath::default();
        if let Some(package_json_scope) = package_json_scope {
            if tsc_packagejson::InfoCacheEntry::exists(Some(&package_json_scope)) {
                package_json_directory = package_json_scope
                    .package_directory
                    .as_directory_path()
                    .clone();
                if let Some(value) = package_json_scope
                    .contents
                    .as_ref()
                    .and_then(|contents| contents.fields.header_fields.type_.get_value())
                {
                    if !file_name.extension_is_one_of(&[
                        EXTENSION_MTS, EXTENSION_CTS, EXTENSION_MJS, EXTENSION_CJS,
                    ]) && (ModuleResolutionKind::Node16 <= module_resolution_kind
                        && module_resolution_kind <= ModuleResolutionKind::NodeNext
                        || file_name.contains_lowercase_directory_sequence("/node_modules/"))
                    {
                        package_json_type = value.clone();
                    }
                }
            }
        }

        let implied_node_format = get_implied_node_format_for_file(file_name, &package_json_type);
        SourceFileMetaData {
            package_json_type,
            package_json_directory,
            implied_node_format,
        }
    }

    /// Go: `func (p *fileLoader) parseSourceFile(t *parseTask) *ast.SourceFile`.
    pub(crate) fn parse_source_file(&self, task: &ParseTask) -> Option<SourceFile> {
        let redirect = self.get_redirect_parsed_command_line_for_resolution(
            &task.normalized_file_path,
            &task.path,
        );
        // Go: `options := p.projectReferences.getCompilerOptionsForFile(t)` —
        // the redirected compiler options (the parse options only consume
        // GetExternalModuleIndicatorOptions' inputs through metadata + jsx).
        let options_for_file =
            crate::project_reference_file_mapper::compiler_options_with_redirect(
                &self.opts.compiler_options_arc(),
                redirect,
            );
        let parse_options = SourceFileParseOptions {
            file_name: task.normalized_file_path.clone(),
            path_key: task.path.clone(),
            external_module_indicator_options: get_external_module_indicator_options(
                &task.normalized_file_path,
                &options_for_file,
                &task.metadata,
            ),
        };
        // Go: the content-mapper extension check routes content-mapper files
        // through `parseContentMappedFile` — the seam is empty in this wave
        // (see the file header), so every parse is a plain load.
        self.host.get_source_file(&parse_options)
    }

    /// Go: `func (p *fileLoader) getSourceFileFromReference(fileName, referenceText)`.
    fn get_source_file_from_reference(
        &self,
        file_name: &RootedFilePath,
        reference_text: &str,
    ) -> (RootedFilePath, PathKey, Option<SourceFileFromReferenceDiagnostic>) {
        let options = self.compiler_options();
        let allow_non_ts_extensions = options.allow_non_ts_extensions.is_true();
        let diagnostic_file_name = tsc_tspath::normalize_slashes(reference_text).into_owned();

        if file_name.has_extension() {
            let canonical_file_name = self.case_sensitivity.path_key(&RootedPath::from(
                file_name.as_string(),
            ));
            if !allow_non_ts_extensions && !self.is_supported_extension(&canonical_file_name) {
                if canonical_file_name.has_js_file_extension() {
                    return (
                        RootedFilePath::default(),
                        PathKey::default(),
                        Some(SourceFileFromReferenceDiagnostic {
                            message: &tsc_diagnostics::FILE_0_IS_A_JAVASCRIPT_FILE_DID_YOU_MEAN_TO_ENABLE_THE_ALLOWJS_OPTION,
                            args: vec![diagnostic_file_name],
                        }),
                    );
                }
                return (
                    RootedFilePath::default(),
                    PathKey::default(),
                    Some(SourceFileFromReferenceDiagnostic {
                        message: &tsc_diagnostics::FILE_0_HAS_AN_UNSUPPORTED_EXTENSION_THE_ONLY_SUPPORTED_EXTENSIONS_ARE_1,
                        args: vec![
                            diagnostic_file_name.clone(),
                            format!("'{}'", self.supported_extensions_flattened().join("', '")),
                        ],
                    }),
                );
            }

            if !self.host.fs().file_exists(file_name) {
                return (
                    RootedFilePath::default(),
                    PathKey::default(),
                    Some(SourceFileFromReferenceDiagnostic {
                        message: &tsc_diagnostics::FILE_0_NOT_FOUND,
                        args: vec![diagnostic_file_name],
                    }),
                );
            }
            return (file_name.clone(), canonical_file_name, None);
        }

        if allow_non_ts_extensions && self.host.fs().file_exists(file_name) {
            return (
                file_name.clone(),
                self.case_sensitivity.path_key(&RootedPath::from(file_name.as_string())),
                None,
            );
        }

        if allow_non_ts_extensions {
            return (
                RootedFilePath::default(),
                PathKey::default(),
                Some(SourceFileFromReferenceDiagnostic {
                    message: &tsc_diagnostics::FILE_0_NOT_FOUND,
                    args: vec![diagnostic_file_name],
                }),
            );
        }

        // Go: only try adding extensions from the first supported group
        // (which should be .ts/.tsx/.d.ts).
        for ext in &self.supported_extensions[0] {
            let candidate = file_name.append_suffix(ext);
            if self.host.fs().file_exists(&candidate) {
                return (
                    candidate.clone(),
                    self.case_sensitivity.path_key(&RootedPath::from(candidate.as_string())),
                    None,
                );
            }
        }

        (
            RootedFilePath::default(),
            PathKey::default(),
            Some(SourceFileFromReferenceDiagnostic {
                message: &tsc_diagnostics::COULD_NOT_RESOLVE_THE_PATH_0_WITH_THE_EXTENSIONS_COLON_1,
                args: vec![
                    diagnostic_file_name.clone(),
                    format!("'{}'", self.supported_extensions_flattened().join("', '")),
                ],
            }),
        )
    }

    /// Go: `func (p *fileLoader) resolveTripleslashPathReference(moduleName, containingFile, index)`.
    pub(crate) fn resolve_tripleslash_path_reference(
        &self,
        module_name: &str,
        containing_file: &RootedFilePath,
        index: usize,
    ) -> Result<ResolvedRef, ProcessingDiagnostic> {
        let normalized_file_name = containing_file.directory().resolve_file(module_name);
        let containing_path = self
            .case_sensitivity
            .path_key(&RootedPath::from(containing_file.as_string()));
        let include_reason = Arc::new(FileIncludeReason {
            kind: FileIncludeKind::ReferenceFile,
            index,
            is_default_lib: false,
            referenced_file: Some(ReferencedFileData {
                file: containing_path.clone(),
                index: index as i64,
                synthetic: None,
            }),
            automatic_type_directive: None,
            canonical_source_file: PathKey::default(),
        });

        let (resolved_file_name, resolved_path, diagnostic) =
            self.get_source_file_from_reference(&normalized_file_name, module_name);
        if let Some(diagnostic) = diagnostic {
            return Err(ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                reason: None,
                explanation: Some(IncludeExplainingDiagnostic {
                    file: PathKey::default(),
                    diagnostic_reason: Some(Arc::clone(&include_reason)),
                    message: diagnostic.message,
                    args: diagnostic.args,
                }),
            });
        }
        if containing_path == resolved_path {
            return Err(ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                reason: None,
                explanation: Some(IncludeExplainingDiagnostic {
                    file: PathKey::default(),
                    diagnostic_reason: Some(Arc::clone(&include_reason)),
                    message: &tsc_diagnostics::A_FILE_CANNOT_HAVE_A_REFERENCE_TO_ITSELF,
                    args: Vec::new(),
                }),
            });
        }

        Ok(ResolvedRef {
            file_name: resolved_file_name,
            path: resolved_path,
            include_reason: Some(include_reason),
            ..Default::default()
        })
    }

    /// Go: `func (p *fileLoader) resolveAutomaticTypeDirectives(containingFileName)`.
    #[allow(clippy::type_complexity)]
    pub(crate) fn resolve_automatic_type_directives(
        &self,
        containing_file_name: &RootedFilePath,
    ) -> (
        Vec<ResolvedRef>,
        ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>>,
        Vec<DiagAndArgs>,
        Vec<ProcessingDiagnostic>,
    ) {
        let mut to_parse: Vec<ResolvedRef> = Vec::new();
        let mut type_resolutions_in_file: ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>>;
        let mut type_resolutions_trace: Vec<DiagAndArgs> = Vec::new();
        let mut p_diagnostics: Vec<ProcessingDiagnostic> = Vec::new();

        let automatic_type_directive_names = tsc_module::get_automatic_type_directive_names(
            self.compiler_options(),
            self.opts.config.base_directory(),
            self.host.fs().as_ref(),
        );
        type_resolutions_in_file = ModeAwareCache::default();
        if !automatic_type_directive_names.is_empty() {
            let resolver = self
                .resolver
                .as_ref()
                .expect("resolver created before file loads");
            for name in automatic_type_directive_names {
                // Under node16/nodenext module resolution, load `types`/ata
                // include names as cjs resolution results by passing an
                // `undefined` mode. Under bundler module resolution, this also
                // triggers the "import" condition to be used.
                let resolution_mode = ResolutionModeNone;
                let (resolved, trace) = resolver.resolve_type_reference_directive(
                    &name,
                    containing_file_name,
                    resolution_mode,
                    None,
                );
                type_resolutions_in_file.insert(
                    ModeAwareCacheKey {
                        name: name.clone(),
                        mode: resolution_mode,
                    },
                    resolved.clone(),
                );
                type_resolutions_trace.extend(trace);
                if resolved.as_deref().is_some_and(tsc_module::ResolvedTypeReferenceDirective::is_resolved) {
                    let resolved = resolved.expect("resolved");
                    to_parse.push(ResolvedRef {
                        file_name: resolved.resolved_file_name.clone(),
                        path: resolved.resolved_path.clone(),
                        increase_depth: resolved.is_external_library_import,
                        elide_on_depth: false,
                        include_reason: Some(Arc::new(FileIncludeReason {
                            kind: FileIncludeKind::AutomaticTypeDirectiveFile,
                            index: 0,
                            is_default_lib: false,
                            referenced_file: None,
                            automatic_type_directive: Some(AutomaticTypeDirectiveFileData {
                                type_reference: name.clone(),
                                package_id: resolved.package_id.clone(),
                            }),
                            canonical_source_file: PathKey::default(),
                        })),
                        package_id: resolved.package_id.clone(),
                    });
                } else {
                    p_diagnostics.push(ProcessingDiagnostic {
                        kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                        reason: None,
                        explanation: Some(IncludeExplainingDiagnostic {
                            file: PathKey::default(),
                            diagnostic_reason: Some(Arc::new(FileIncludeReason {
                                kind: FileIncludeKind::AutomaticTypeDirectiveFile,
                                index: 0,
                                is_default_lib: false,
                                referenced_file: None,
                                automatic_type_directive: Some(
                                    AutomaticTypeDirectiveFileData {
                                        type_reference: name.clone(),
                                        package_id: PackageId::default(),
                                    },
                                ),
                                canonical_source_file: PathKey::default(),
                            })),
                            message: &tsc_diagnostics::CANNOT_FIND_TYPE_DEFINITION_FILE_FOR_0,
                            args: vec![name.clone()],
                        }),
                    });
                }
            }
        }
        (to_parse, type_resolutions_in_file, type_resolutions_trace, p_diagnostics)
    }

    /// Go: `func (p *fileLoader) resolveTypeReferenceDirectives(t *parseTask)`.
    pub(crate) fn resolve_type_reference_directives(
        &self,
        files_parser: &crate::files_parser::FilesParser,
        task: &mut ParseTask,
    ) {
        let Some(file) = &task.file else {
            return;
        };
        if file.payload().type_reference_directives.is_empty() {
            return;
        }
        // Go: loader.tracing.Push — not ported.

        let meta = task.metadata.clone();

        let mut type_resolutions_in_file: ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>> =
            ModeAwareCache::default();
        let mut type_resolutions_trace: Vec<DiagAndArgs> = Vec::new();
        let resolver = self
            .resolver
            .as_ref()
            .expect("resolver created before file loads");

        let type_reference_directives = file.payload().type_reference_directives.clone();
        let file_name = file.file_name().clone();
        let file_path = task.path.clone();
        for (index, r#ref) in type_reference_directives.iter().enumerate() {
            let (redirect, redirect_file_name) =
                self.get_redirect_for_resolution(&file_name, &file_path);
            let options_for_file =
                crate::project_reference_file_mapper::compiler_options_with_redirect(
                    &self.opts.compiler_options_arc(),
                    redirect.clone(),
                );
            let resolution_mode = get_mode_for_type_reference_directive_in_file(
                r#ref,
                &file_name,
                &meta,
                &options_for_file,
            );
            // PORT: Go passes `redirect` (nil-able `*tsoptions.ParsedCommandLine`)
            // straight through; the port's newtype unsizes to the trait object
            // via the mapper's helper (Option<Arc<_>> → Option<Arc<dyn _>> is
            // not a coercion site).
            let redirect_dyn: Option<Arc<dyn tsc_module::ResolvedProjectReference>> =
                crate::project_reference_file_mapper::redirect_as_dyn(redirect.clone());
            let (resolved, trace) = resolver.resolve_type_reference_directive(
                &r#ref.file_name,
                &redirect_file_name,
                resolution_mode,
                redirect_dyn.as_ref(),
            );
            type_resolutions_in_file.insert(
                ModeAwareCacheKey {
                    name: r#ref.file_name.clone(),
                    mode: resolution_mode,
                },
                resolved.clone(),
            );
            let include_reason = Arc::new(FileIncludeReason {
                kind: FileIncludeKind::TypeReferenceDirective,
                index,
                is_default_lib: false,
                referenced_file: Some(ReferencedFileData {
                    file: task.path.clone(),
                    index: index as i64,
                    synthetic: None,
                }),
                automatic_type_directive: None,
                canonical_source_file: PathKey::default(),
            });
            type_resolutions_trace.extend(trace);

            if resolved.as_deref().is_some_and(tsc_module::ResolvedTypeReferenceDirective::is_resolved) {
                let resolved = resolved.expect("resolved");
                crate::files_parser::push_sub_task(
                    files_parser,
                    task,
                    ResolvedRef {
                        file_name: resolved.resolved_file_name.clone(),
                        path: resolved.resolved_path.clone(),
                        increase_depth: resolved.is_external_library_import,
                        elide_on_depth: false,
                        include_reason: Some(include_reason),
                        package_id: resolved.package_id.clone(),
                    },
                    None,
                );
            } else {
                task.processing_diagnostics.push(ProcessingDiagnostic {
                    kind: ProcessingDiagnosticKind::UnknownReference,
                    reason: Some(include_reason),
                    explanation: None,
                });
            }
        }

        task.type_resolutions_in_file = type_resolutions_in_file;
        task.type_resolutions_trace = type_resolutions_trace;
    }

    /// Go: `func (p *fileLoader) resolveImportsAndModuleAugmentations(t *parseTask)`.
    pub(crate) fn resolve_imports_and_module_augmentations(
        &self,
        files_parser: &crate::files_parser::FilesParser,
        task: &mut ParseTask,
    ) {
        let Some(file) = task.file.as_mut() else {
            return;
        };
        let meta = task.metadata.clone();

        let is_javascript_file = crate::ast_shims::is_source_file_js_by_file(file);
        let is_external_module_file = crate::ast_shims::is_external_module(file);

        let file_name = file.file_name().clone();
        let file_path = task.path.clone();
        let (redirect, redirect_file_name) =
            self.get_redirect_for_resolution(&file_name, &file_path);
        let options_for_file =
            crate::project_reference_file_mapper::compiler_options_with_redirect(
                &self.opts.compiler_options_arc(),
                redirect.clone(),
            );
        let mut module_names: Vec<NodeId> = Vec::with_capacity(
            file.imports().len() + file.payload().module_augmentations.len() + 2,
        );

        if (is_javascript_file
            || (!file.payload().is_declaration_file
                && (options_for_file.get_isolated_modules() || is_external_module_file)))
            && options_for_file.import_helpers.is_true()
        {
            let specifier = create_synthetic_import(file, EXTERNAL_HELPERS_MODULE_NAME_TEXT);
            module_names.push(specifier);
            task.import_helpers_import_specifier =
                Some(EXTERNAL_HELPERS_MODULE_NAME_TEXT.to_string());
        }

        if is_javascript_file || file.payload().script_kind == ScriptKind::TSX {
            let jsx_import = get_jsx_runtime_import(
                &get_jsx_implicit_import_base(&options_for_file, file),
                &options_for_file,
            );
            if !jsx_import.is_empty() {
                let specifier = create_synthetic_import(file, &jsx_import);
                module_names.push(specifier);
                task.jsx_runtime_import_specifier = Some(jsx_import);
            }
        }

        let imports_start = module_names.len();

        module_names.extend(file.imports().iter().copied());
        for imp in &file.payload().module_augmentations {
            if file.node(*imp).kind == Kind::StringLiteral {
                module_names.push(*imp);
            }
            // Do nothing if it's an Identifier; we don't need to do module
            // resolution for `declare global`.
        }

        if self.skip_module_resolution() {
            return;
        }

        if !module_names.is_empty() {
            let resolver = self
                .resolver
                .as_ref()
                .expect("resolver created before file loads");
            let mut resolutions_in_file: ModeAwareCache<Option<Arc<ResolvedModule>>> =
                ModeAwareCache::default();
            let mut resolutions_trace: Vec<DiagAndArgs> = Vec::new();
            // PORT: Go pushes sub-tasks onto `t.subTasks` as it walks; the
            // port's `push_sub_task` needs `&mut ParseTask` (the whole task)
            // while the file is borrowed out of `task.file`, so the refs are
            // collected here and pushed after the walk (same order).
            let mut pending_sub_tasks: Vec<ResolvedRef> = Vec::new();

            for (index, entry) in module_names.iter().enumerate() {
                let module_name = node_text(file as &dyn tsc_ast::NodeStore, *entry).to_string();
                if module_name.is_empty()
                    || is_source_phase_import(file as &dyn tsc_ast::NodeStore, file.node(*entry).parent.get())
                {
                    continue;
                }

                let mode = get_mode_for_usage_location(
                    file,
                    &file_name,
                    &meta,
                    *entry,
                    &options_for_file,
                );
                let redirect_dyn: Option<Arc<dyn tsc_module::ResolvedProjectReference>> =
                    crate::project_reference_file_mapper::redirect_as_dyn(redirect.clone());
                let (resolved_module, trace) = resolver.resolve_module_name(
                    &module_name,
                    &redirect_file_name,
                    mode,
                    redirect_dyn.as_ref(),
                );
                let resolved_module = resolved_module.or_else(|| {
                    // Go: `if resolvedModule == nil { resolvedModule = &module.ResolvedModule{} }`.
                    Some(Arc::new(ResolvedModule::default()))
                });
                resolutions_in_file.insert(
                    ModeAwareCacheKey {
                        name: module_name.clone(),
                        mode,
                    },
                    resolved_module.clone(),
                );
                resolutions_trace.extend(trace);

                let Some(resolved_module) = resolved_module else {
                    continue;
                };
                if !ResolvedModule::is_resolved(Some(resolved_module.as_ref())) {
                    continue;
                }

                let resolved_file_name = resolved_module.resolved_file_name.clone();
                let is_from_node_modules_search =
                    resolved_module.is_external_library_import;
                // Don't treat redirected files as JS files.
                let redirect_parsed_command_line = self
                    .get_redirect_parsed_command_line_for_resolution(
                        &resolved_file_name,
                        &resolved_module.resolved_path,
                    );
                let is_js_file = !resolved_module.resolved_using_extra_extensions
                    && !resolved_file_name
                        .extension_is_one_of(&SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT)
                    && redirect_parsed_command_line.is_none();
                let is_js_file_from_node_modules = is_from_node_modules_search
                    && is_js_file
                    && resolved_file_name.contains_lowercase_directory_sequence("/node_modules/");

                // add file to program only if:
                // - resolution was successful
                // - noResolve is falsy
                // - module name comes from the list of imports
                // - it's not a top level JavaScript module that exceeded the search max

                let import_index = index as i64 - imports_start as i64;

                let should_add_file = !module_name.is_empty()
                    && tsc_module::get_resolution_diagnostic(
                        &options_for_file,
                        resolved_module.as_ref(),
                        Some(&*file),
                    )
                    .is_none()
                    && !options_for_file.no_resolve.is_true()
                    && !(is_js_file && !options_for_file.get_allow_js())
                    && (import_index < 0
                        || (import_index < file.imports().len() as i64
                            && (crate::ast_shims::is_in_js_file_of(file, file.imports()[import_index as usize])
                                || file.node(file.imports()[import_index as usize]).flags
                                    & tsc_ast::NodeFlags::JSDOC
                                    == tsc_ast::NodeFlags::NONE)));

                if should_add_file {
                    let synthetic = (import_index < 0).then(|| module_name.clone());
                    pending_sub_tasks.push(ResolvedRef {
                        file_name: resolved_file_name.clone(),
                        path: resolved_module.resolved_path.clone(),
                        increase_depth: resolved_module.is_external_library_import,
                        elide_on_depth: is_js_file_from_node_modules,
                        include_reason: Some(Arc::new(FileIncludeReason {
                            kind: FileIncludeKind::Import,
                            index: 0,
                            is_default_lib: false,
                            referenced_file: Some(ReferencedFileData {
                                file: task.path.clone(),
                                index: import_index,
                                synthetic,
                            }),
                            automatic_type_directive: None,
                            canonical_source_file: PathKey::default(),
                        })),
                        package_id: resolved_module.package_id.clone(),
                    });
                }
            }

            task.resolutions_in_file = resolutions_in_file;
            task.resolutions_trace = resolutions_trace;

            for resolved_ref in pending_sub_tasks {
                crate::files_parser::push_sub_task(files_parser, task, resolved_ref, None);
            }
        }
    }

    /// Go: `func (p *fileLoader) pathForLibFile(name string) *LibFile`.
    pub(crate) fn path_for_lib_file(&self, name: &str) -> Arc<LibFile> {
        if let Some(cached) = self.path_for_lib_file_cache.load(name) {
            return cached;
        }

        let mut path = self.default_library_path.resolve_file(name);
        let mut path_key = self.case_sensitivity.path_key(&RootedPath::from(path.as_string()));
        let mut replaced = false;
        if !self.skip_module_resolution()
            && self.compiler_options().lib_replacement.is_true()
            && name != "lib.d.ts"
        {
            let library_name = get_library_name_from_lib_file_name(name);
            let resolve_from = get_inferred_library_name_resolve_from(
                self.opts.config.base_directory(),
                name,
            );
            let (resolution, trace) = self.resolve_library(&library_name, &resolve_from);
            if ResolvedModule::is_resolved(resolution.as_deref()) {
                let resolved = resolution.as_deref().expect("resolved");
                path = resolved.resolved_file_name.clone();
                path_key = resolved.resolved_path.clone();
                replaced = true;
            }
            self.path_for_lib_file_resolutions.load_or_store(
                self.case_sensitivity.path_key(&RootedPath::from(resolve_from.as_string())),
                Arc::new(LibResolution {
                    library_name,
                    resolution,
                    trace,
                }),
            );
        }

        let (lib_path, _) = self.path_for_lib_file_cache.load_or_store(
            name.to_string(),
            Arc::new(LibFile {
                name: name.to_string(),
                path,
                path_key,
                replaced,
            }),
        );
        lib_path
    }

    /// Go: `func (p *fileLoader) resolveLibrary(libraryName, resolveFrom)`.
    fn resolve_library(
        &self,
        library_name: &str,
        resolve_from: &RootedFilePath,
    ) -> (Option<Arc<ResolvedModule>>, Vec<DiagAndArgs>) {
        // Go: loader.tracing.Push — not ported.
        let resolver = self
            .resolver
            .as_ref()
            .expect("resolver created before file loads");
        let (resolved, trace) = resolver.resolve_module_name(
            library_name,
            resolve_from,
            ResolutionModeCommonJS,
            None,
        );
        (resolved, trace)
    }
}

/// Go: `const externalHelpersModuleNameText = "tslib"`.
const EXTERNAL_HELPERS_MODULE_NAME_TEXT: &str = "tslib";

/// Go: `func getLibraryNameFromLibFileName(libFileName string) string`.
fn get_library_name_from_lib_file_name(lib_file_name: &str) -> String {
    // Support resolving to lib.dom.d.ts -> @typescript/lib-dom, and
    //                      lib.dom.iterable.d.ts -> @typescript/lib-dom/iterable
    //                      lib.es2015.symbol.wellknown.d.ts -> @typescript/lib-es2015/symbol-wellknown
    let components: Vec<&str> = lib_file_name.split('.').collect();
    let mut path = String::from("@typescript/lib-");
    if components.len() > 1 {
        path.push_str(components[1]);
    }
    let mut i = 2;
    while i < components.len() && !components[i].is_empty() && components[i] != "d" {
        if i == 2 {
            path.push('/');
        } else {
            path.push('-');
        }
        path.push_str(components[i]);
        i += 1;
    }
    path
}

/// Go: `func getInferredLibraryNameResolveFrom(baseDirectory, libFileName)`.
fn get_inferred_library_name_resolve_from(
    base_directory: &RootedDirectoryPath,
    lib_file_name: &str,
) -> RootedFilePath {
    base_directory.resolve_file(&format!(
        "__lib_node_modules_lookup_{lib_file_name}__.ts"
    ))
}

// ────────────────────────────────────────────────────────────────────────────
// Mode-for-usage-location helpers (Go: fileloader.go bottom)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getModeForTypeReferenceDirectiveInFile(ref, file, meta, options)`.
fn get_mode_for_type_reference_directive_in_file(
    r#ref: &tsc_ast::FileReference,
    file_name: &RootedFilePath,
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ResolutionMode {
    if r#ref.mode != ResolutionModeNone {
        r#ref.mode
    } else {
        get_default_resolution_mode_for_file(file_name, meta, options)
    }
}

/// Go: `func getDefaultResolutionModeForFile(fileName, meta, options)`.
pub(crate) fn get_default_resolution_mode_for_file(
    file_name: &RootedFilePath,
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ResolutionMode {
    if import_syntax_affects_module_resolution(options) {
        get_implied_node_format_for_emit_worker(
            file_name,
            options.get_emit_module_kind(),
            meta,
        )
    } else {
        ResolutionModeNone
    }
}

/// Go: `func getModeForUsageLocation(fileName, meta, usage, options)`.
pub(crate) fn get_mode_for_usage_location(
    file: &SourceFile,
    file_name: &RootedFilePath,
    meta: &SourceFileMetaData,
    usage: NodeId,
    options: &CompilerOptions,
) -> ResolutionMode {
    let store = file as &dyn tsc_ast::NodeStore;
    let parent = file.node(usage).parent.get();
    let parent_kind = file.node(parent).kind;
    let is_import_or_export = matches!(
        parent_kind,
        Kind::ImportDeclaration
            | Kind::JSImportDeclaration
            | Kind::ExportDeclaration
            | Kind::JSDocImportTag
    );
    if is_import_or_export {
        // Go: IsImportDeclaration(usage.Parent) || KindJSImportDeclaration ||
        // IsExportDeclaration(usage.Parent) || IsJSDocImportTag(usage.Parent)
        let is_type_only = is_exclusively_type_only_import_or_export(store, parent);
        if is_type_only {
            let attributes = match &file.node(parent).data {
                NodeData::ImportDeclaration(d) => d.attributes,
                NodeData::JSDocImportTag(d) => d.attributes,
                NodeData::ExportDeclaration(d) => d.attributes,
                _ => None,
            };
            if let Some(attributes) = attributes {
                let (mode_override, ok) =
                    crate::ast_shims::get_resolution_mode_override(store, attributes);
                if ok {
                    return mode_override;
                }
            }
        }
    }
    if parent_kind == Kind::LiteralType
        && file.node(file.node(parent).parent.get()).kind == Kind::ImportType
    {
        let grandparent = file.node(parent).parent.get();
        if let NodeData::ImportTypeNode(import_type) = &file.node(grandparent).data {
            if let Some(attributes) = import_type.attributes {
                let (mode_override, ok) =
                    crate::ast_shims::get_resolution_mode_override(store, attributes);
                if ok {
                    return mode_override;
                }
            }
        }
    }

    if import_syntax_affects_module_resolution(options) {
        return get_emit_syntax_for_usage_location_worker(file, file_name, meta, usage, options);
    }

    ResolutionModeNone
}

/// Go: `func importSyntaxAffectsModuleResolution(options) bool`.
pub(crate) fn import_syntax_affects_module_resolution(options: &CompilerOptions) -> bool {
    let module_resolution = options.get_module_resolution_kind();
    (ModuleResolutionKind::Node16 <= module_resolution
        && module_resolution <= ModuleResolutionKind::NodeNext)
        || options.get_resolve_package_json_exports()
        || options.get_resolve_package_json_imports()
}

/// Go: `func getEmitSyntaxForUsageLocationWorker(fileName, meta, usage, options)`.
fn get_emit_syntax_for_usage_location_worker(
    file: &SourceFile,
    file_name: &RootedFilePath,
    meta: &SourceFileMetaData,
    usage: NodeId,
    options: &CompilerOptions,
) -> ResolutionMode {
    let store = file as &dyn tsc_ast::NodeStore;
    let parent = file.node(usage).parent.get();
    let parent_kind = file.node(parent).kind;
    let is_import_equals_require = parent_kind == Kind::ExternalModuleReference
        && file.node(file.node(parent).parent.get()).kind == Kind::ImportEqualsDeclaration;
    if is_require_call(store, parent, false) || is_import_equals_require {
        return ResolutionModeCommonJS;
    }
    let file_emit_mode = get_emit_module_format_of_file_worker(file_name, options, meta);
    let call = walk_up_parenthesized_expressions(store, parent);
    if call != NodeId::NONE
        && file.node(call).kind == Kind::CallExpression
        && is_import_call(store, call)
    {
        if is_source_phase_import_call(store, call) {
            return ResolutionModeESM;
        }
        if should_transform_import_call(file_name, options, file_emit_mode) {
            return ResolutionModeCommonJS;
        }
        return ResolutionModeESM;
    }
    // If we're in --module preserve on an input file, we know that an import
    // is an import. But if this is a declaration file, we'd prefer to use the
    // impliedNodeFormat. Since we want things to be consistent between the
    // two, we need to issue errors when the user writes ESM syntax in a
    // definitely-CJS file, until/unless declaration emit can indicate a true
    // ESM import. On the other hand, writing CJS syntax in a definitely-ESM
    // file is fine, since declaration emit preserves the CJS syntax.
    if file_emit_mode == ModuleKind::CommonJS {
        return ResolutionModeCommonJS;
    }
    if file_emit_mode.is_non_node_esm() || file_emit_mode == ModuleKind::Preserve {
        return ResolutionModeESM;
    }
    ResolutionModeNone
}

/// Go: `func IsImportCall(node *Node) bool` (ast) —
/// `IsCallExpression(node) && IsImportKeyword(node.AsCallExpression().Expression)`.
fn is_import_call(store: &dyn tsc_ast::NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind != Kind::CallExpression {
        return false;
    }
    match &n.data {
        NodeData::CallExpression(call) => {
            store.node(call.expression).kind == Kind::ImportKeyword
        }
        _ => false,
    }
}

/// Go: `func (p *fileLoader) createSyntheticImport(text, file) *ast.StringLiteralNode`.
fn create_synthetic_import(file: &mut SourceFile, text: &str) -> NodeId {
    // PORT: the synthetic nodes live in the parsed file's arena (see the file
    // header); the parent wiring matches Go's (importDecl ← moduleReference,
    // importDecl ← file node).
    let file_node = file.as_node_id();
    let external_helpers_module_reference = {
        let mut factory = NodeFactory::new(file);
        factory.new_string_literal(text, tsc_ast::TokenFlags::NONE)
    };
    let import_decl = {
        let mut factory = NodeFactory::new(file);
        factory.new_import_declaration(None, None, external_helpers_module_reference, None)
    };
    file.node_mut(import_decl)
        .parent
        .set(file_node);
    file.node_mut(external_helpers_module_reference)
        .parent
        .set(import_decl);
    external_helpers_module_reference
}

// ────────────────────────────────────────────────────────────────────────────
// processAllProgramFiles
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func processAllProgramFiles(opts ProgramOptions, singleThreaded bool)
/// (processedFiles, *module.ResolutionData, error)`.
///
/// PORT: the module-resolution error channel is dropped (the Rust module
/// crate dropped the vestigial error — see its `ResolveModuleNameResult`).
pub fn process_all_program_files(
    opts: &ProgramOptions,
    single_threaded: bool,
) -> (ProcessedFiles, Arc<tsc_module::ResolutionData>) {
    let compiler_options = opts.config.compiler_options_arc();
    let root_files = opts.config.config.file_names().to_vec();
    let supported_extensions = tsc_tsoptions::tsconfigparsing::get_supported_extensions(
        &compiler_options,
        // Go: opts.Config.ContentMapperExtensions() — the seam is empty.
        &[],
    );
    let supported_extensions_with_json_if_resolve_json_module =
        tsc_tsoptions::tsconfigparsing::get_supported_extensions_with_json_if_resolve_json_module(
            &compiler_options,
            supported_extensions.clone(),
        );
    let max_node_module_js_depth = compiler_options
        .max_node_module_js_depth
        .unwrap_or(0);
    let host = opts
        .hosts
        .host
        .clone()
        .expect("processAllProgramFiles: ProgramOptions.Hosts.Host must be set");
    let default_library_path = host.default_library_path();
    let case_sensitivity = host.fs().case_sensitivity();
    let mut loader = FileLoader {
        opts: &opts.config,
        host,
        // Go: the WorkGroups are built with the bool processAllProgramFiles
        // receives — see the struct field.
        single_threaded,
        resolver: None,
        default_library_path,
        case_sensitivity,
        supported_extensions,
        supported_extensions_with_json_if_resolve_json_module,
        // Go: opts.Config.ContentMapperExtensions() — the content-mapper
        // seam is empty in this wave.
        content_mapper_extensions: Vec::new(),
        files_parser: crate::files_parser::FilesParser::new(max_node_module_js_depth),
        root_tasks: Vec::with_capacity(root_files.len() + compiler_options.lib.as_ref().map_or(0, Vec::len)),
        total_file_count: AtomicI32::new(0),
        lib_file_count: AtomicI32::new(0),
        project_references: None,
        path_for_lib_file_cache: tsc_collections::SyncMap::new(),
        path_for_lib_file_resolutions: tsc_collections::SyncMap::new(),
    };
    loader.add_project_reference_tasks(single_threaded);
    let resolver_options = ResolverOptions {
        host: loader
            .project_references
            .as_ref()
            .expect("project references initialized")
            .host
            .clone(),
        compiler_options: Arc::clone(&compiler_options),
        typings_location: opts.config.typings_location.clone(),
        project_name: opts.config.project_name.clone(),
        extra_extensions: Vec::new(),
        // Go: NewResolverOptions receives no package-json cache here — the
        // resolver builds a fresh one over the host (None).
        package_json_cache: None,
    };
    loader.resolver = Some(match &opts.factories.create_module_resolver {
        Some(create_module_resolver) => create_module_resolver(resolver_options),
        None => Arc::new(tsc_module::new_resolver(resolver_options)),
    });

    // Go: opts.Tracing.Push — not ported.
    for (index, root_file) in root_files.iter().enumerate() {
        // Go: `opts.Config.RootFileNameForDiagnostic(index)` — the un-rooted
        // name string (present when root files came from the command line).
        let reference_text = opts
            .config
            .config
            .root_file_names_for_diagnostics
            .get(index)
            .cloned()
            .unwrap_or_else(|| root_file.as_string().to_string());
        loader.add_root_file_task(
            root_file.clone(),
            &reference_text,
            None,
            Arc::new(FileIncludeReason {
                kind: FileIncludeKind::RootFile,
                index,
                is_default_lib: false,
                referenced_file: None,
                automatic_type_directive: None,
                canonical_source_file: PathKey::default(),
            }),
        );
    }
    let no_lib = compiler_options.no_lib.is_false_or_unknown();
    if !root_files.is_empty() && no_lib {
        if let Some(libs) = compiler_options.lib.as_ref() {
            for (index, lib) in libs.iter().enumerate() {
                if let Some(name) = tsc_tsoptions::get_lib_file_name(lib) {
                    let lib_file = loader.path_for_lib_file(&name);
                    loader.add_root_task(
                        lib_file.path.clone(),
                        Some(lib_file),
                        Some(Arc::new(FileIncludeReason {
                            kind: FileIncludeKind::LibFile,
                            index,
                            is_default_lib: false,
                            referenced_file: None,
                            automatic_type_directive: None,
                            canonical_source_file: PathKey::default(),
                        })),
                    );
                }
                // !!! error on unknown name
            }
        } else {
            let name = tsc_tsoptions::get_default_lib_file_name(&compiler_options);
            let lib_file = loader.path_for_lib_file(&name);
            loader.add_root_task(
                lib_file.path.clone(),
                Some(lib_file),
                Some(Arc::new(FileIncludeReason {
                    kind: FileIncludeKind::LibFile,
                    index: 0,
                    is_default_lib: true,
                    referenced_file: None,
                    automatic_type_directive: None,
                    canonical_source_file: PathKey::default(),
                })),
            );
        }
    }

    if !root_files.is_empty() && !opts.config.skip_module_resolution {
        loader.add_automatic_type_directive_tasks();
    }

    let root_tasks = loader.root_tasks.clone();
    loader.files_parser.parse(&loader, &root_tasks);
    let resolver = loader.resolver.as_ref().expect("resolver").clone();
    let resolution_data = resolver.get_resolution_data();
    let processed = loader.files_parser.get_processed_files(&loader);
    (processed, resolution_data)
}

/// The crate-root M5 seam (Go: the loader is package-private; this is the
/// orchestrator-facing entry the milestone gate describes: create program
/// state from a file set + CompilerOptions, load all files, collect
/// parse-level diagnostics — see program.rs `new_program`).
pub use crate::program::new_program as build_program;
