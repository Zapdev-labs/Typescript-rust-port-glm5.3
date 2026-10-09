// Ported from tsc/internal/compiler/filesparser.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes (the task graph):
//   - Go's `*parseTask` graph (subTasks / redirectedParseTask / loadedTask
//     pointers, `parseTaskData` groups shared across goroutines) becomes an
//     arena (`TaskArena`) + stable indices. Every access to a task happens
//     under its path group's mutex (`ParseTaskData` behind
//     `SyncMap<PathKey, Arc<Mutex<..>>>`) exactly as Go's `data.mu`
//     serializes a path's casings; `with_task` checks a task out of the
//     arena and puts it back inside the critical section.
//   - Go's `parseTaskDataPool` (a sync.Pool) is dropped — pooling is a Go GC
//     optimization; the port allocates the group map directly.
//   - The queued group closure captures `&FileLoader`/`&FilesParser`/the
//     group borrows, matching Go's closure captures; `core.WorkFn` is
//     `Box<dyn FnOnce() + Send>` (whose trait-object default is `'static`),
//     so the group is the crate-local `ScopedWorkGroup` running the closures
//     on `std::thread::scope` (see the ScopedWorkGroup PORT note).
//   - `data.tasks` map iteration: Go iterates the (randomized) map; the port
//     iterates a snapshot in insertion order — loading is idempotent and the
//     collection walk (which fixes program order) is a separate
//     deterministic walk, so the iteration order is not observable
//     (PORTING-NOTES).
//   - The collection walk (`getProcessedFiles`/`collectFiles`) moves the
//     owned `SourceFile`s out of the arena into `ProcessedFiles`; the
//     `filesByPath` map values are slots (`Lib(i)`/`File(i)`) resolved to
//     merged-list indexes at construction (Go stores shared `*SourceFile`
//     pointers, including the package-dedup aliasing).

use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};

use tsc_ast::{SourceFile, SourceFileMetaData};
use tsc_module::{
    DiagAndArgs, ModeAwareCache, PackageId, ResolvedModule, ResolvedTypeReferenceDirective,
};
use tsc_tspath::{PathKey, RootedFilePath};

use crate::file_include::{FileIncludeKind, FileIncludeReason, ReferencedFileData};
use crate::file_loader::{FileLoader, LibFile, ProcessedFiles, RedirectsFile, ResolvedRef};
use crate::processing_diagnostic::{
    IncludeExplainingDiagnostic, ProcessingDiagnostic, ProcessingDiagnosticKind,
};

/// Go: `type parseTask struct`.
#[derive(Default)]
pub(crate) struct ParseTask {
    pub normalized_file_path: RootedFilePath,
    pub path: PathKey,
    pub file: Option<SourceFile>,
    /// Go: `libFile *LibFile`.
    pub lib_file: Option<Arc<LibFile>>,
    /// Go: `redirectedParseTask *parseTask` — arena index.
    pub redirected_parse_task: Option<usize>,
    /// Go: `subTasks []*parseTask` — arena indices.
    pub sub_tasks: Vec<usize>,
    pub loaded: bool,
    pub started_sub_tasks: bool,
    pub is_for_automatic_type_directive: bool,
    pub is_content_mapper_supplemental: bool,
    pub failed_lookup: bool,
    pub include_reason: Option<Arc<FileIncludeReason>>,
    pub package_id: PackageId,

    pub metadata: SourceFileMetaData,
    pub resolutions_in_file: ModeAwareCache<Option<Arc<ResolvedModule>>>,
    pub resolutions_trace: Vec<DiagAndArgs>,
    pub type_resolutions_in_file: ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>>,
    pub type_resolutions_trace: Vec<DiagAndArgs>,
    /// Go: `resolutionDiagnostics []*ast.Diagnostic` — carried for the
    /// checker wave (the program surface reads it via the resolutions map
    /// until then; PORTING-NOTES).
    #[allow(dead_code)]
    pub resolution_diagnostics: Vec<Arc<tsc_ast::Diagnostic>>,
    pub processing_diagnostics: Vec<ProcessingDiagnostic>,
    /// Go: `importHelpersImportSpecifier *ast.StringLiteralNode` — the
    /// synthetic module reference text (see file_include.rs).
    pub import_helpers_import_specifier: Option<String>,
    /// Go: `jsxRuntimeImportSpecifier *jsxRuntimeImportSpecifier` — the
    /// module reference text (the specifier node is synthetic).
    pub jsx_runtime_import_specifier: Option<String>,

    pub increase_depth: bool,
    pub elide_on_depth: bool,

    /// Go: `loadedTask *parseTask` — arena index.
    pub loaded_task: Option<usize>,
    /// Go: `allIncludeReasons []*FileIncludeReason` — carried for the
    /// include-reason explain surface (`getIncludeReasons` reads the
    /// program-level reason map; PORTING-NOTES).
    #[allow(dead_code)]
    pub all_include_reasons: Vec<Arc<FileIncludeReason>>,
}

/// Go: `type parseTaskData struct` — the per-path group (map of tasks by file
/// casing + the group's depth/package state), `mu`-guarded.
#[derive(Default)]
pub(crate) struct ParseTaskData {
    /// Go: `tasks map[tspath.RootedFilePath]*parseTask` — file name → task
    /// (arena index).
    pub tasks: rustc_hash::FxHashMap<RootedFilePath, usize>,
    /// Go: `lowestDepth int` — starts at `math.MaxInt`.
    pub lowest_depth: i64,
    pub started_sub_tasks: bool,
    pub package_id: PackageId,
}

/// The task arena (Go: the goroutine-shared `*parseTask` graph).
#[derive(Default)]
pub(crate) struct TaskArena {
    tasks: Mutex<Vec<ParseTask>>,
}

impl TaskArena {
    pub fn push(&self, task: ParseTask) -> usize {
        let mut tasks = self.tasks.lock().expect("task arena poisoned");
        tasks.push(task);
        tasks.len() - 1
    }

    /// Checks a task out, runs `f`, puts it back. Every caller holds the
    /// task's path-group mutex (the group serializes all access to its
    /// tasks; see the file header).
    pub fn with_task<R>(&self, index: usize, f: impl FnOnce(&mut ParseTask) -> R) -> R {
        let mut task = {
            let mut tasks = self.tasks.lock().expect("task arena poisoned");
            std::mem::take(&mut tasks[index])
        };
        let result = f(&mut task);
        self.tasks
            .lock()
            .expect("task arena poisoned")[index] = task;
        result
    }

    /// The read-only variant of [`TaskArena::with_task`] (the same
    /// checkout/put-back; the closure takes `&ParseTask`). PORT: Go reads
    /// the shared task through the group mutex; the checkout is the port's
    /// equivalent — no unsafe aliasing (the task is owned here).
    pub fn with_task_read<R>(&self, index: usize, f: impl FnOnce(&ParseTask) -> R) -> R {
        let task = {
            let mut tasks = self.tasks.lock().expect("task arena poisoned");
            std::mem::take(&mut tasks[index])
        };
        let result = f(&task);
        self.tasks
            .lock()
            .expect("task arena poisoned")[index] = task;
        result
    }
}

/// Go: `type filesParser struct`.
pub(crate) struct FilesParser {
    /// Go: `taskDataByPath collections.SyncMap[tspath.PathKey, *parseTaskData]`.
    task_data_by_path: tsc_collections::SyncMap<PathKey, Arc<Mutex<ParseTaskData>>>,
    /// Go: `maxDepth int` (MaxNodeModuleJsDepth).
    max_depth: i64,
    /// The task arena (Go: the shared task graph).
    pub(crate) arena: TaskArena,
}

impl FilesParser {
    pub(crate) fn new(max_depth: i64) -> Self {
        FilesParser {
            task_data_by_path: tsc_collections::SyncMap::new(),
            max_depth,
            arena: TaskArena::default(),
        }
    }

    /// The group data for a path (collection walk + loader surfaces).
    pub(crate) fn group_data(&self, path: &PathKey) -> Option<Arc<Mutex<ParseTaskData>>> {
        self.task_data_by_path.load(path)
    }

    /// Go: `func (w *filesParser) parse(loader, tasks)`.
    pub(crate) fn parse(&self, loader: &FileLoader, tasks: &[usize]) {
        // Go: core.NewWorkGroup(loader.singleThreaded) — the scoped group
        // (see ScopedWorkGroup's PORT note; the Arc is the shared handle the
        // queued closures clone for the recursive `start`).
        let wg = Arc::new(ScopedWorkGroup::new(loader.single_threaded));
        self.start(loader, &wg, tasks, 0);
        wg.run_and_wait();
    }

    /// Go: `func (w *filesParser) start(loader, tasks, depth)`.
    ///
    /// `'s`/`'l` are the capture lifetimes of the queued closure (the group
    /// runs it within `parse`'s scope, so they cover `'env`).
    pub(crate) fn start<'s, 'l, 'env>(
        &'s self,
        loader: &'l FileLoader,
        wg: &Arc<ScopedWorkGroup<'env>>,
        tasks: &[usize],
        depth: i64,
    ) where
        's: 'env,
        'l: 'env,
    {
        for &task_index in tasks {
            let (path, normalized_file_path, increase_depth, elide_on_depth, package_id) =
                self.arena.with_task_read(task_index, |task| {
                    if task.path.as_string().is_empty() {
                        panic!(
                            "parse task must have a path key: {}",
                            task.normalized_file_path.as_string()
                        );
                    }
                    (
                        task.path.clone(),
                        task.normalized_file_path.clone(),
                        task.increase_depth,
                        task.elide_on_depth,
                        task.package_id.clone(),
                    )
                });

            // Go: getParseTaskData(task) + taskDataByPath.LoadOrStore (the
            // sync.Pool is dropped — PORTING-NOTES).
            let (data, loaded) = self.task_data_by_path.load_or_store(
                path.clone(),
                Arc::new(Mutex::new(ParseTaskData::new(task_index, &normalized_file_path))),
            );
            if loaded {
                // Go: putParseTaskData(candidate) — pool return, dropped.
            }

            let max_depth = self.max_depth;
            let files_parser = self;
            // The closure clones the Arc handle (see the group's PORT note).
            let wg_handle = Arc::clone(wg);
            wg.queue(Box::new(move || {
                run_task_group(
                    files_parser,
                    loader,
                    data,
                    loaded,
                    task_index,
                    normalized_file_path,
                    increase_depth,
                    elide_on_depth,
                    package_id,
                    depth,
                    max_depth,
                    &wg_handle,
                );
            }));
        }
    }

    /// Go: `func (w *filesParser) getProcessedFiles(loader) processedFiles`.
    pub(crate) fn get_processed_files(&self, loader: &FileLoader) -> ProcessedFiles {
        CollectState::new(self, loader).collect(loader.root_tasks.clone())
    }

    /// Go: `func (w *filesParser) addIncludeReason(includeProcessor, task, reason)`.
    fn add_include_reason(
        &self,
        state: &mut CollectState,
        task_index: usize,
        reason: Arc<FileIncludeReason>,
    ) {
        let redirected = self
            .arena
            .with_task_read(task_index, |task| task.redirected_parse_task);
        if let Some(redirected) = redirected {
            self.add_include_reason(state, redirected, reason);
            return;
        }
        let (loaded, path) = self
            .arena
            .with_task_read(task_index, |task| (task.loaded, task.path.clone()));
        if loaded {
            state
                .file_include_reasons
                .entry(path)
                .or_default()
                .push(reason);
        }
    }
}

impl ParseTaskData {
    fn new(task_index: usize, normalized_file_path: &RootedFilePath) -> Self {
        let mut data = ParseTaskData {
            lowest_depth: i64::MAX,
            ..Default::default()
        };
        data.tasks
            .insert(normalized_file_path.clone(), task_index);
        data
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The queued group closure (Go: the `w.wg.Queue(func(){ data.mu.Lock() ... })` body)
// ────────────────────────────────────────────────────────────────────────────

/// Go: the queued closure body: `data.mu.Lock(); ... task loads ...; start(subTasks)`.
#[allow(clippy::too_many_arguments)]
fn run_task_group<'s, 'l, 'env>(
    files_parser: &'s FilesParser,
    loader: &'l FileLoader,
    data: Arc<Mutex<ParseTaskData>>,
    loaded: bool,
    task_index: usize,
    normalized_file_path: RootedFilePath,
    increase_depth: bool,
    elide_on_depth: bool,
    package_id: PackageId,
    depth: i64,
    max_depth: i64,
    wg: &Arc<ScopedWorkGroup<'env>>,
) where
    's: 'env,
    'l: 'env,
{
    let mut data = data.lock().expect("parseTaskData poisoned");

    let mut start_subtasks = false;
    if loaded {
        // Go: an existing task for this path — record the alias or register
        // the new casing.
        if let Some(&existing) = data.tasks.get(&normalized_file_path) {
            files_parser
                .arena
                .with_task(task_index, |task| task.loaded_task = Some(existing));
        } else {
            data.tasks.insert(normalized_file_path.clone(), task_index);
            // This is new task for file name - so load subtasks if there was
            // loading for any other casing
            start_subtasks = data.started_sub_tasks;
        }
    }

    // Propagate packageId to data if we have one and data doesn't yet
    if data.package_id.name.is_empty() && !package_id.name.is_empty() {
        data.package_id = package_id;
    }

    let current_depth = if increase_depth { depth + 1 } else { depth };
    let lowered = current_depth < data.lowest_depth;
    if lowered {
        // If we're seeing this task at a lower depth than before,
        // reprocess its subtasks to ensure they are loaded.
        data.lowest_depth = current_depth;
        start_subtasks = true;
        data.started_sub_tasks = true;
    }

    if elide_on_depth && current_depth > max_depth {
        return;
    }

    let group_task_indexes: Vec<usize> = data.tasks.values().copied().collect();
    for task_by_file_name in group_task_indexes {
        let mut load_sub_tasks = start_subtasks;
        let (is_loaded, started_sub_tasks) = files_parser.arena.with_task_read(
            task_by_file_name,
            |task| (task.loaded, task.started_sub_tasks),
        );
        if !is_loaded {
            files_parser
                .arena
                .with_task(task_by_file_name, |task| load(loader, files_parser, task));
            // Go: `if taskByFileName.redirectedParseTask != nil` — always load
            // redirected task.
            let has_redirect = files_parser.arena.with_task_read(
                task_by_file_name,
                |task| task.redirected_parse_task.is_some(),
            );
            if has_redirect {
                load_sub_tasks = true;
                data.started_sub_tasks = true;
            }
        }
        if load_sub_tasks && (lowered || !started_sub_tasks) {
            files_parser
                .arena
                .with_task(task_by_file_name, |task| task.started_sub_tasks = true);
            let sub_tasks = files_parser
                .arena
                .with_task_read(task_by_file_name, |task| task.sub_tasks.clone());
            files_parser.start(loader, wg, &sub_tasks, data.lowest_depth);
        }
    }
}

/// Go: `func (t *parseTask) load(loader *fileLoader)`.
pub(crate) fn load(loader: &FileLoader, files_parser: &FilesParser, task: &mut ParseTask) {
    task.loaded = true;
    if task.is_for_automatic_type_directive {
        load_automatic_type_directives(loader, files_parser, task);
        return;
    }
    if task.failed_lookup {
        // The root file name did not resolve to a supported extension; the
        // task exists only to carry its processing diagnostic, so nothing is
        // parsed.
        return;
    }

    // Go: loader.tracing.Push — not ported (no tracing crate).

    let (redirect, redirect_path) =
        loader.get_parse_file_redirect(&task.normalized_file_path, &task.path);
    if !redirect.as_string().is_empty() {
        redirect_task(files_parser, task, redirect, redirect_path);
        return;
    }

    if !task.is_content_mapper_supplemental && !task.normalized_file_path.has_extension() {
        let allow_non_ts_extensions = loader
            .compiler_options()
            .allow_non_ts_extensions
            .is_true();
        if !allow_non_ts_extensions {
            let canonical_file_name = task.path.clone();
            if !loader.is_supported_extension(&canonical_file_name) {
                if canonical_file_name.has_js_file_extension() {
                    task.processing_diagnostics.push(ProcessingDiagnostic {
                        kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                        reason: None,
                        explanation: Some(IncludeExplainingDiagnostic {
                            file: PathKey::default(),
                            diagnostic_reason: task.include_reason.clone(),
                            message:
                                &tsc_diagnostics::FILE_0_IS_A_JAVASCRIPT_FILE_DID_YOU_MEAN_TO_ENABLE_THE_ALLOWJS_OPTION,
                            args: vec![task.normalized_file_path.as_string().to_string()],
                        }),
                    });
                } else {
                    task.processing_diagnostics.push(ProcessingDiagnostic {
                        kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                        reason: None,
                        explanation: Some(IncludeExplainingDiagnostic {
                            file: PathKey::default(),
                            diagnostic_reason: task.include_reason.clone(),
                            message:
                                &tsc_diagnostics::FILE_0_HAS_AN_UNSUPPORTED_EXTENSION_THE_ONLY_SUPPORTED_EXTENSIONS_ARE_1,
                            args: vec![
                                task.normalized_file_path.as_string().to_string(),
                                format!("'{}'", loader.supported_extensions_flattened().join("', '")),
                            ],
                        }),
                    });
                }
                return;
            }
        }
    }

    loader.total_file_count.fetch_add(1, Ordering::Relaxed);
    if task.lib_file.is_some() {
        loader.lib_file_count.fetch_add(1, Ordering::Relaxed);
        // Default lib files are all scripts; we can safely skip looking up
        // their package.json to avoid adding spurious lookups to file watcher
        // tracking.
        task.metadata = SourceFileMetaData {
            implied_node_format: tsc_core::options_generated::ModuleKind::CommonJS,
            ..Default::default()
        };
    } else {
        task.metadata = loader.load_source_file_meta_data(&task.normalized_file_path);
    }

    if task.file.is_none() {
        task.file = loader.parse_source_file(task);
    }
    if task.file.is_none() {
        return;
    }

    // Go: `if virtualFileName := file.VirtualFileName(); ...` — the
    // content-mapper virtual-file surface (not ported); the metadata stays
    // as loaded.

    let no_resolve = loader.compiler_options().no_resolve.is_true();
    if !no_resolve && !loader.skip_module_resolution() {
        let referenced_files = task
            .file
            .as_ref()
            .expect("file")
            .payload()
            .referenced_files
            .clone();
        let containing_file_name = task.file.as_ref().expect("file").file_name().clone();
        for (index, r#ref) in referenced_files.iter().enumerate() {
            match loader.resolve_tripleslash_path_reference(
                &r#ref.file_name,
                &containing_file_name,
                index,
            ) {
                Err(processing_diagnostic) => {
                    task.processing_diagnostics.push(processing_diagnostic);
                    continue;
                }
                Ok(resolved_ref) => {
                    push_sub_task(files_parser, task, resolved_ref, None);
                }
            }
        }

        loader.resolve_type_reference_directives(files_parser, task);
    }

    let no_lib = loader.compiler_options().no_lib == tsc_core::tristate::Tristate::TSTrue;
    if !no_lib && !loader.skip_module_resolution() {
        let lib_reference_directives = task
            .file
            .as_ref()
            .expect("file")
            .payload()
            .lib_reference_directives
            .clone();
        for (index, lib) in lib_reference_directives.iter().enumerate() {
            let include_reason = Arc::new(FileIncludeReason {
                kind: FileIncludeKind::LibReferenceDirective,
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
            if let Some(name) = tsc_tsoptions::get_lib_file_name(&lib.file_name) {
                let lib_file = loader.path_for_lib_file(&name);
                push_sub_task(
                    files_parser,
                    task,
                    ResolvedRef {
                        file_name: lib_file.path.clone(),
                        path: lib_file.path_key.clone(),
                        increase_depth: false,
                        elide_on_depth: false,
                        include_reason: Some(include_reason),
                        package_id: PackageId::default(),
                    },
                    Some(lib_file),
                );
            } else {
                task.processing_diagnostics.push(ProcessingDiagnostic {
                    kind: ProcessingDiagnosticKind::UnknownReference,
                    reason: Some(include_reason),
                    explanation: None,
                });
            }
        }
    }

    loader.resolve_imports_and_module_augmentations(files_parser, task);

    // Go: `for _, supplemental := range file.SupplementalSourceFiles()` —
    // the content-mapper supplemental surface (not ported): no supplemental
    // files exist in this wave.
}

/// Go: `func (t *parseTask) redirect(loader, fileName, path)`.
fn redirect_task(
    files_parser: &FilesParser,
    task: &mut ParseTask,
    file_name: RootedFilePath,
    path: PathKey,
) {
    let redirected = files_parser.arena.push(ParseTask {
        normalized_file_path: file_name,
        path,
        lib_file: task.lib_file.clone(),
        include_reason: task.include_reason.clone(),
        // increaseDepth and elideOnDepth are not copied to redirects,
        // otherwise their depth would be double counted.
        ..Default::default()
    });
    task.redirected_parse_task = Some(redirected);
    task.sub_tasks = vec![redirected];
}

/// Go: `func (t *parseTask) loadAutomaticTypeDirectives(loader)`.
fn load_automatic_type_directives(loader: &FileLoader, files_parser: &FilesParser, task: &mut ParseTask) {
    let (to_parse_type_refs, type_resolutions_in_file, type_resolutions_trace, p_diagnostics) =
        loader.resolve_automatic_type_directives(&task.normalized_file_path);
    task.type_resolutions_in_file = type_resolutions_in_file;
    task.type_resolutions_trace = type_resolutions_trace;
    task.processing_diagnostics.extend(p_diagnostics);
    for type_resolution in to_parse_type_refs {
        push_sub_task(files_parser, task, type_resolution, None);
    }
}

/// Go: `func (t *parseTask) addSubTask(ref resolvedRef, libFile *LibFile)` —
/// the in-task queue for sub-tasks created during load() (the arena push
/// happens when the loader can reach it; see `SubTaskSeam`).
pub(crate) fn push_sub_task(
    files_parser: &FilesParser,
    task: &mut ParseTask,
    r#ref: ResolvedRef,
    lib_file: Option<Arc<LibFile>>,
) {
    let index = files_parser.arena.push(ParseTask {
        normalized_file_path: r#ref.file_name,
        path: r#ref.path,
        lib_file,
        increase_depth: r#ref.increase_depth,
        elide_on_depth: r#ref.elide_on_depth,
        include_reason: r#ref.include_reason,
        package_id: r#ref.package_id,
        ..Default::default()
    });
    task.sub_tasks.push(index);
}

// ────────────────────────────────────────────────────────────────────────────
// getProcessedFiles / collectFiles
// ────────────────────────────────────────────────────────────────────────────

/// Where a path's file lives during the collection walk (Go: the shared
/// `*SourceFile` pointer; package dedup aliases earlier files).
#[derive(Clone, Copy)]
enum FileSlot {
    /// An index into the lib-file list.
    Lib(usize),
    /// An index into the program-file list.
    File(usize),
}

/// The `package_id_to_source_file` map key (see the field's PORT note): a
/// composite string of every `module.PackageId` field.
fn package_id_key(id: &PackageId) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        id.name, id.sub_module_name, id.version, id.peer_dependencies
    )
}

// ────────────────────────────────────────────────────────────────────────────
// ScopedWorkGroup
// ────────────────────────────────────────────────────────────────────────────

/// Go: the `core.WorkGroup` the parser builds in `parse` (`NewWorkGroup`) —
/// `queue`/`run_and_wait` (Go's `Queue`/`RunAndWait`).
///
/// PORT: `core.WorkFn` is `Box<dyn FnOnce() + Send>`, whose trait-object
/// default carries a `'static` bound — the parse-task closures capture
/// `&FilesParser`/`&FileLoader`/the group (matching Go's closure captures;
/// `processAllProgramFiles` borrows `opts`, so no `'static` data exists to
/// capture), so a crate-local scoped group with the same queue/run_and_wait
/// surface runs them on `std::thread::scope`. The group is shared as an
/// `Arc` (the queued closures clone it for the recursive `start` — Go's
/// closures capture the group pointer the same way); queue-during-run works,
/// `run_and_wait` is the only barrier, and the executed closures drop their
/// `Arc` clones as they run, so the group is freed when `parse` returns. No
/// unsafe code.
pub(crate) struct ScopedWorkGroup<'env> {
    single_threaded: bool,
    state: Mutex<ScopedState<'env>>,
    cv: Condvar,
}

#[derive(Default)]
struct ScopedState<'env> {
    fns: Vec<Box<dyn FnOnce() + Send + 'env>>,
    active: usize,
    done: bool,
}

impl<'env> ScopedWorkGroup<'env> {
    pub(crate) fn new(single_threaded: bool) -> Self {
        ScopedWorkGroup {
            single_threaded,
            state: Mutex::new(ScopedState::default()),
            cv: Condvar::new(),
        }
    }

    /// Go: `Queue(f func())`.
    pub(crate) fn queue(&self, f: Box<dyn FnOnce() + Send + 'env>) {
        let mut state = self.state.lock().expect("work group poisoned");
        if state.done {
            panic!("Queue called after RunAndWait returned");
        }
        state.fns.push(f);
        drop(state);
        self.cv.notify_one();
    }

    /// Go: `RunAndWait()`.
    pub(crate) fn run_and_wait(&self) {
        if self.single_threaded {
            loop {
                let f = self.state.lock().expect("work group poisoned").fns.pop();
                match f {
                    Some(f) => f(),
                    None => break,
                }
            }
            let mut state = self.state.lock().expect("work group poisoned");
            state.fns.clear();
            state.done = true;
            return;
        }
        let this = self;
        std::thread::scope(|scope| {
            let mut state = this.state.lock().expect("work group poisoned");
            loop {
                if let Some(f) = state.fns.pop() {
                    state.active += 1;
                    drop(state);
                    scope.spawn(move || {
                        f();
                        let mut state = this.state.lock().expect("work group poisoned");
                        state.active -= 1;
                        drop(state);
                        this.cv.notify_all();
                    });
                    state = this.state.lock().expect("work group poisoned");
                    continue;
                }
                if state.active == 0 {
                    break;
                }
                state = this.cv.wait(state).expect("work group poisoned");
            }
        });
        let mut state = self.state.lock().expect("work group poisoned");
        state.fns.clear();
        state.done = true;
    }
}

/// The collection-walk state (Go: the closure locals of `collectFiles`).
pub(crate) struct CollectState<'a> {
    files_parser: &'a FilesParser,
    loader: &'a FileLoader<'a>,
    /// The walked program files (Go: `files`).
    files: Vec<SourceFile>,
    /// The walked lib files (Go: `libFiles`).
    lib_files: Vec<SourceFile>,
    /// path → slot (Go: `filesByPath`).
    files_by_path: rustc_hash::FxHashMap<PathKey, FileSlot>,
    duplicate_source_files: Vec<crate::file_loader::DuplicateSourceFile>,
    missing_files: rustc_hash::FxHashSet<PathKey>,
    /// Go: `fileIncludeData.fileIncludeReasons`.
    file_include_reasons: rustc_hash::FxHashMap<PathKey, Vec<Arc<FileIncludeReason>>>,
    /// Go: `fileIncludeData.processingDiagnostics`.
    processing_diagnostics: Vec<ProcessingDiagnostic>,
    /// Go: `tasksSeenByNameIgnoreCase` (case-sensitive file systems only).
    tasks_seen_by_name_ignore_case: Option<rustc_hash::FxHashMap<PathKey, usize>>,
    output_file_to_project_reference_source: Option<rustc_hash::FxHashMap<PathKey, RootedFilePath>>,
    resolved_modules: rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedModule>>>>,
    type_resolutions_in_file:
        rustc_hash::FxHashMap<PathKey, ModeAwareCache<Option<Arc<ResolvedTypeReferenceDirective>>>>,
    source_file_meta_datas: rustc_hash::FxHashMap<PathKey, SourceFileMetaData>,
    jsx_runtime_import_specifiers: rustc_hash::FxHashMap<PathKey, String>,
    import_helpers_import_specifiers: rustc_hash::FxHashMap<PathKey, String>,
    source_files_found_searching_node_modules: rustc_hash::FxHashSet<PathKey>,
    lib_files_map: rustc_hash::FxHashMap<PathKey, Arc<LibFile>>,
    redirect_targets_map: rustc_hash::FxHashMap<PathKey, Vec<RootedFilePath>>,
    redirect_files_by_path: rustc_hash::FxHashMap<PathKey, RedirectsFile>,
    /// Go: `packageIdToSourceFile map[module.PackageId]*parseTask`. PORT:
    /// `module.PackageId` derives no `Hash` (the module crate is outside
    /// this wave's touch scope), so the map is keyed by a composite string
    /// of the id's fields (same identity semantics as Go's map key).
    package_id_to_source_file: Option<rustc_hash::FxHashMap<String, usize>>,
    /// seen: group identity (Arc ptr) → checked name.
    seen: rustc_hash::FxHashMap<usize, RootedFilePath>,
    /// recordedDuplicates: group identity → recorded casings.
    recorded_duplicates: rustc_hash::FxHashMap<usize, rustc_hash::FxHashSet<RootedFilePath>>,
    deduplicate_packages: bool,
}

impl<'a> CollectState<'a> {
    fn new(files_parser: &'a FilesParser, loader: &'a FileLoader) -> Self {
        CollectState {
            files_parser,
            loader,
            files: Vec::new(),
            lib_files: Vec::new(),
            files_by_path: rustc_hash::FxHashMap::default(),
            duplicate_source_files: Vec::new(),
            missing_files: rustc_hash::FxHashSet::default(),
            file_include_reasons: rustc_hash::FxHashMap::default(),
            processing_diagnostics: Vec::new(),
            // Go: allocates only when the file system is case-sensitive.
            tasks_seen_by_name_ignore_case: loader
                .case_sensitivity
                .is_case_sensitive()
                .then(rustc_hash::FxHashMap::default),
            output_file_to_project_reference_source: (!loader
                .can_use_project_reference_source())
            .then(rustc_hash::FxHashMap::default),
            resolved_modules: rustc_hash::FxHashMap::default(),
            type_resolutions_in_file: rustc_hash::FxHashMap::default(),
            source_file_meta_datas: rustc_hash::FxHashMap::default(),
            jsx_runtime_import_specifiers: rustc_hash::FxHashMap::default(),
            import_helpers_import_specifiers: rustc_hash::FxHashMap::default(),
            source_files_found_searching_node_modules: rustc_hash::FxHashSet::default(),
            lib_files_map: rustc_hash::FxHashMap::default(),
            redirect_targets_map: rustc_hash::FxHashMap::default(),
            redirect_files_by_path: rustc_hash::FxHashMap::default(),
            // Go: allocates unless DeduplicatePackages is false.
            package_id_to_source_file: (!loader
                .compiler_options()
                .deduplicate_packages
                .is_false())
            .then(rustc_hash::FxHashMap::default),
            seen: rustc_hash::FxHashMap::default(),
            recorded_duplicates: rustc_hash::FxHashMap::default(),
            deduplicate_packages: !loader
                .compiler_options()
                .deduplicate_packages
                .is_false(),
        }
    }

    /// Go: `collectFiles(loader.rootTasks, seen)` + the merge + the
    /// pathForLibFileResolutions sweep.
    fn collect(mut self, root_tasks: Vec<usize>) -> ProcessedFiles {
        let root_tasks_clone = root_tasks.clone();
        self.collect_files(root_tasks);

        let lib_file_count = self.lib_files.len();
        self.loader.sort_libs(&mut self.lib_files);

        // Go: allFiles := append(libFiles, files...) — lib files first; the
        // redirect indexes shift by len(libFiles).
        let mut all_files = std::mem::take(&mut self.lib_files);
        let files = std::mem::take(&mut self.files);
        let file_base = all_files.len();
        for redirect_file in self.redirect_files_by_path.values_mut() {
            redirect_file.index += lib_file_count;
        }
        all_files.extend(files);

        // filesByPath slots → merged indexes (Lib first, then File).
        let files_by_path: rustc_hash::FxHashMap<PathKey, usize> = self
            .files_by_path
            .iter()
            .map(|(path, slot)| {
                let index = match slot {
                    FileSlot::Lib(i) => *i,
                    FileSlot::File(i) => file_base + i,
                };
                (path.clone(), index)
            })
            .collect();

        // Go: the pathForLibFileResolutions sweep (sorted keys).
        let mut keys = self.loader.path_for_lib_file_resolutions.keys();
        keys.sort();
        for key in keys {
            if let Some(value) = self.loader.path_for_lib_file_resolutions.load(&key) {
                self.resolved_modules.entry(key).or_default().insert(
                    tsc_module::ModeAwareCacheKey {
                        name: value.library_name.clone(),
                        mode: tsc_core::options_generated::ModuleKind::CommonJS,
                    },
                    value.resolution.clone(),
                );
                for trace in &value.trace {
                    self.loader.host.trace(trace.message, &trace.args);
                }
            }
        }

        let _ = root_tasks_clone;
        ProcessedFiles {
            finished_processing: true,
            files: all_files,
            duplicate_source_files: self.duplicate_source_files,
            files_by_path,
            project_reference_file_mapper: self.loader.project_references_mapper(),
            missing_files: self.missing_files,
            resolved_modules: self.resolved_modules,
            type_resolutions_in_file: self.type_resolutions_in_file,
            source_file_meta_datas: self.source_file_meta_datas,
            jsx_runtime_import_specifiers: self.jsx_runtime_import_specifiers,
            import_helpers_import_specifiers: self.import_helpers_import_specifiers,
            lib_files: self.lib_files_map,
            source_files_found_searching_node_modules: self
                .source_files_found_searching_node_modules,
            file_include_reasons: self.file_include_reasons,
            processing_diagnostics: self.processing_diagnostics,
            output_file_to_project_reference_source: self.output_file_to_project_reference_source,
            redirect_targets_map: self.redirect_targets_map,
            redirect_files_by_path: self.redirect_files_by_path,
            // Go: loader.contentMapperDiagnostics — the content-mapper seam
            // (always empty in this wave).
            content_mapper_diagnostics: Vec::new(),
        }
    }

    /// Go: the recursive `collectFiles` closure.
    fn collect_files(&mut self, tasks: Vec<usize>) {
        for mut task_index in tasks {
            let include_reason = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.include_reason.clone());
            // Exclude automatic type directive tasks from include reason
            // processing, as these are internal implementation details and
            // should not contribute to the reasons for including files.
            let (redirected, is_for_automatic_type_directive, loaded_task) = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| {
                    (
                        task.redirected_parse_task,
                        task.is_for_automatic_type_directive,
                        task.loaded_task,
                    )
                });
            if redirected.is_none() && !is_for_automatic_type_directive {
                // Go: `if task.loadedTask != nil { task = task.loadedTask }` —
                // the CLAIM task is the group's loaded winner, so the
                // subtree claims at the FIRST DFS site. The pre-follow
                // form read the alias's own `loaded` (false) and skipped,
                // leaving the claim to whichever task won the parallel
                // load race — the program order (and the twin's
                // `program_files` fold) was run-to-run nondeterministic.
                if let Some(loaded_task) = loaded_task {
                    task_index = loaded_task;
                }
                if let Some(include_reason) = &include_reason {
                    self.files_parser
                        .add_include_reason(self, task_index, include_reason.clone());
                }
            }
            let path = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.path.clone());
            let data_group = self
                .files_parser
                .group_data(&path)
                .map(|data| Arc::as_ptr(&data) as usize);
            let is_loaded = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.loaded);
            if !is_loaded {
                continue;
            }
            let Some(data_group) = data_group else {
                continue;
            };

            // ensure we only walk each task once
            if let Some(checked_name) = self.seen.get(&data_group).cloned() {
                let normalized_file_path = self
                    .files_parser
                    .arena
                    .with_task_read(task_index, |task| task.normalized_file_path.clone());
                let has_file = self
                    .files_parser
                    .arena
                    .with_task_read(task_index, |task| task.file.is_some());
                if has_file && checked_name != normalized_file_path {
                    let dups = self.recorded_duplicates.entry(data_group).or_default();
                    if dups.insert(normalized_file_path.clone()) {
                        let duplicate = self.files_parser.arena.with_task_read(task_index, |task| {
                            task.file.as_ref().map(|file| {
                                crate::file_loader::DuplicateSourceFile::of(file)
                            })
                        });
                        if let Some(duplicate) = duplicate {
                            self.duplicate_source_files.push(duplicate);
                        }
                    }
                }
                if !self
                    .loader
                    .compiler_options()
                    .force_consistent_casing_in_file_names
                    .is_false()
                {
                    // Check if it differs only in drive letters its ok to
                    // ignore that error:
                    let checked_absolute_path = checked_name.without_root().to_string();
                    let input_absolute_path = normalized_file_path.without_root().to_string();
                    if checked_absolute_path != input_absolute_path {
                        self.add_processing_diagnostics_for_file_casing(
                            path.clone(),
                            checked_name.as_string(),
                            normalized_file_path.as_string(),
                            include_reason.clone(),
                        );
                    }
                }
                continue;
            } else {
                let normalized_file_path = self
                    .files_parser
                    .arena
                    .with_task_read(task_index, |task| task.normalized_file_path.clone());
                self.seen.insert(data_group, normalized_file_path);
            }

            if let Some(seen_by_ignore_case) = self.tasks_seen_by_name_ignore_case.as_mut() {
                let path_lower_case = path.case_insensitive_key();
                if let Some(&task_by_ignore_case) = seen_by_ignore_case.get(&path_lower_case) {
                    let (by_ignore_case_path, by_ignore_case_name) = self
                        .files_parser
                        .arena
                        .with_task_read(task_by_ignore_case, |task| {
                            (task.path.clone(), task.normalized_file_path.clone())
                        });
                    let normalized_file_path = self
                        .files_parser
                        .arena
                        .with_task_read(task_index, |task| {
                            task.normalized_file_path.clone()
                        });
                    self.add_processing_diagnostics_for_file_casing(
                        by_ignore_case_path,
                        by_ignore_case_name.as_string(),
                        normalized_file_path.as_string(),
                        include_reason.clone(),
                    );
                } else {
                    seen_by_ignore_case.insert(path_lower_case, task_index);
                }
            }

            for trace in self.files_parser.arena.with_task_read(task_index, |task| {
                task.type_resolutions_trace.clone()
            }) {
                self.loader.host.trace(trace.message, &trace.args);
            }
            for trace in self.files_parser.arena.with_task_read(task_index, |task| {
                task.resolutions_trace.clone()
            }) {
                self.loader.host.trace(trace.message, &trace.args);
            }

            // The group state (package id + lowest depth), read under the
            // group lock.
            let (group_package_id, group_lowest_depth) = {
                let data = self
                    .files_parser
                    .group_data(&path)
                    .expect("group data for walked task");
                let data = data.lock().expect("parseTaskData poisoned");
                (data.package_id.clone(), data.lowest_depth)
            };

            if self.deduplicate_packages && !group_package_id.name.is_empty() {
                if let Some(&package_id_file) = self
                    .package_id_to_source_file
                    .as_ref()
                    .and_then(|map| map.get(&package_id_key(&group_package_id)))
                {
                    // Package deduplication keeps the first package instance
                    // in the program; this task's file is recorded as a
                    // duplicate and the path redirects to the first one.
                    let duplicate = self.files_parser.arena.with_task(task_index, |task| {
                        task.file.take().map(|file| {
                            crate::file_loader::DuplicateSourceFile::of(&file)
                        })
                    });
                    if let Some(duplicate) = duplicate {
                        self.duplicate_source_files.push(duplicate);
                    }
                    let normalized_file_path = self
                        .files_parser
                        .arena
                        .with_task_read(task_index, |task| {
                            task.normalized_file_path.clone()
                        });
                    let package_id_file_path = self
                        .files_parser
                        .arena
                        .with_task_read(package_id_file, |task| task.path.clone());
                    self.redirect_targets_map
                        .entry(package_id_file_path.clone())
                        .or_default()
                        .push(normalized_file_path.clone());
                    self.redirect_files_by_path.insert(
                        path.clone(),
                        RedirectsFile {
                            index: self.files.len() + self.redirect_files_by_path.len(),
                            file_name: normalized_file_path,
                            path: path.clone(),
                            target: package_id_file_path.clone(),
                        },
                    );
                    let package_slot = self.files_by_path.get(&package_id_file_path).copied();
                    if let Some(package_slot) = package_slot {
                        self.files_by_path.insert(path.clone(), package_slot);
                    }
                    if group_lowest_depth > 0 {
                        self.source_files_found_searching_node_modules
                            .insert(path.clone());
                    }
                    continue;
                } else if self
                    .files_parser
                    .arena
                    .with_task_read(task_index, |task| task.file.is_some())
                {
                    self.package_id_to_source_file
                        .get_or_insert_with(rustc_hash::FxHashMap::default)
                        .insert(package_id_key(&group_package_id), task_index);
                }
            }

            let sub_tasks = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.sub_tasks.clone());
            if !sub_tasks.is_empty() {
                self.collect_files(sub_tasks);
            }

            // Exclude automatic type directive tasks from include reason
            // processing (see above).
            if let Some(redirected) = redirected {
                if let Some(output_map) = self.output_file_to_project_reference_source.as_mut() {
                    let redirected_path = self
                        .files_parser
                        .arena
                        .with_task_read(redirected, |task| task.path.clone());
                    let file_name = self
                        .files_parser
                        .arena
                        .with_task_read(task_index, |task| task.normalized_file_path.clone());
                    output_map.insert(redirected_path, file_name);
                }
                continue;
            }

            if is_for_automatic_type_directive {
                let (type_resolutions, processing_diagnostics) =
                    self.files_parser.arena.with_task_read(task_index, |task| {
                        (
                            task.type_resolutions_in_file.clone(),
                            task.processing_diagnostics.clone(),
                        )
                    });
                self.type_resolutions_in_file
                    .insert(path.clone(), type_resolutions);
                if !processing_diagnostics.is_empty() {
                    self.processing_diagnostics.extend(processing_diagnostics);
                }
                continue;
            }

            let processing_diagnostics = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.processing_diagnostics.clone());
            if !processing_diagnostics.is_empty() {
                self.processing_diagnostics.extend(processing_diagnostics);
            }

            let file = self
                .files_parser
                .arena
                .with_task(task_index, |task| task.file.take());
            let Some(file) = file else {
                self.missing_files.insert(path.clone());
                continue;
            };

            let lib_file = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.lib_file.clone());
            if let Some(lib_file) = lib_file {
                self.files_by_path
                    .insert(path.clone(), FileSlot::Lib(self.lib_files.len()));
                self.lib_files.push(file);
                self.lib_files_map.insert(path.clone(), lib_file);
            } else {
                self.files_by_path
                    .insert(path.clone(), FileSlot::File(self.files.len()));
                self.files.push(file);
            }
            let resolutions_in_file = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.resolutions_in_file.clone());
            let type_resolutions = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| {
                    task.type_resolutions_in_file.clone()
                });
            let metadata = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| task.metadata.clone());
            let jsx_runtime = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| {
                    task.jsx_runtime_import_specifier.clone()
                });
            let import_helpers = self
                .files_parser
                .arena
                .with_task_read(task_index, |task| {
                    task.import_helpers_import_specifier.clone()
                });
            self.resolved_modules.insert(path.clone(), resolutions_in_file);
            self.type_resolutions_in_file
                .insert(path.clone(), type_resolutions);
            self.source_file_meta_datas.insert(path.clone(), metadata);
            if let Some(jsx_runtime) = jsx_runtime {
                self.jsx_runtime_import_specifiers
                    .insert(path.clone(), jsx_runtime);
            }
            if let Some(import_helpers) = import_helpers {
                self.import_helpers_import_specifiers
                    .insert(path.clone(), import_helpers);
            }
            if group_lowest_depth > 0 {
                self.source_files_found_searching_node_modules
                    .insert(path.clone());
            }
        }
    }

    /// Go: `func (i *fileIncludeData) addProcessingDiagnosticsForFileCasing(...)`
    /// (includeprocessor.go).
    fn add_processing_diagnostics_for_file_casing(
        &mut self,
        file: PathKey,
        existing_casing: &str,
        current_casing: &str,
        reason: Option<Arc<FileIncludeReason>>,
    ) {
        let has_referenced = self
            .file_include_reasons
            .get(&file)
            .is_some_and(|reasons| reasons.iter().any(|reason| reason.is_referenced_file()));
        if reason.as_ref().is_none_or(|r| !r.is_referenced_file()) && has_referenced {
            self.processing_diagnostics.push(ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                reason: None,
                explanation: Some(IncludeExplainingDiagnostic {
                    file,
                    diagnostic_reason: reason,
                    message:
                        &tsc_diagnostics::ALREADY_INCLUDED_FILE_NAME_0_DIFFERS_FROM_FILE_NAME_1_ONLY_IN_CASING,
                    args: vec![existing_casing.to_string(), current_casing.to_string()],
                }),
            });
        } else {
            self.processing_diagnostics.push(ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                reason: None,
                explanation: Some(IncludeExplainingDiagnostic {
                    file,
                    diagnostic_reason: reason,
                    message:
                        &tsc_diagnostics::FILE_NAME_0_DIFFERS_FROM_ALREADY_INCLUDED_FILE_NAME_1_ONLY_IN_CASING,
                    args: vec![current_casing.to_string(), existing_casing.to_string()],
                }),
            });
        }
    }
}

const _: () = {};
