// Ported from tsc/internal/compiler/projectreferenceparser.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Tasks are arena-allocated (`Vec<ProjectReferenceParseTask>` + indices)
//     instead of Go's shared `*projectReferenceParseTask` graph.
//   - `task.resolved.ParseInputOutputNames()` / `SourceToProjectReference()` /
//     `OutputDtsToProjectReference()` are the tsoptions AST-config surface
//     (not ported): the resolved configs are stored as-is, and the mapper's
//     source/output maps stay empty until that wave lands (PORTING-NOTES).
//   - Go's `wg.Queue(func() { task.parse(p); p.start(task.subTasks) })` — the
//     recursive start-from-within-a-task is a queue-after-run_and_wait
//     hazard in the port's WorkGroup (RunAndWait is the only barrier), so the
//     port walks the task tree breadth-first with one queue pass: Go's
//     dataflow (dedup by path, parent before child) is identical.

use std::sync::Arc;

use tsc_tsoptions::ParsedCommandLine;
use tsc_tspath::{CaseSensitivity, PathKey, RootedFilePath, RootedPath};

use crate::project_reference_file_mapper::{
    ProjectReferenceFileMapper, ResolvedProjectReference,
};

/// Go: `type projectReferenceParseTask struct`.
#[derive(Default)]
pub struct ProjectReferenceParseTask {
    pub config_name: RootedFilePath,
    pub resolved: Option<Arc<ParsedCommandLine>>,
    /// Go: `subTasks []*projectReferenceParseTask` — indices into the
    /// parser's task arena.
    pub sub_tasks: Vec<usize>,
}

/// Go: `func (t *projectReferenceParseTask) parse(projectReferenceParser)`.
///
/// `path_for` is Go's `p.loader.toPath(task.configName.AsPath())` — the
/// loader's case sensitivity, threaded as a function because the port's
/// parser does not hold the loader.
fn parse_task(
    task_index: usize,
    tasks: &mut Vec<ProjectReferenceParseTask>,
    get_resolved: &dyn Fn(&RootedFilePath, &PathKey) -> Option<Arc<ParsedCommandLine>>,
    case_sensitivity: CaseSensitivity,
) {
    let config_name = tasks[task_index].config_name.clone();
    let path_key =
        case_sensitivity.path_key(&RootedPath::from(config_name.as_string().to_string()));
    let resolved = get_resolved(&config_name, &path_key);
    if let Some(resolved) = resolved {
        // Go: `t.resolved.ParseInputOutputNames()` — the AST-config wave (see
        // the file header).
        tasks[task_index].resolved = Some(Arc::clone(&resolved));
        // Go: `if subReferences := t.resolved.ResolvedProjectReferencePaths();
        // len(subReferences) > 0 { t.subTasks = ... }` — the sub-reference
        // CONFIG FILE paths.
        let sub_references: Vec<RootedFilePath> = resolved.resolved_project_reference_paths();
        if !sub_references.is_empty() {
            tasks[task_index].sub_tasks = create_project_reference_parse_tasks(
                tasks,
                sub_references,
            );
        }
    }
}

/// Go: `func createProjectReferenceParseTasks(projectReferences)`.
fn create_project_reference_parse_tasks(
    tasks: &mut Vec<ProjectReferenceParseTask>,
    project_references: Vec<RootedFilePath>,
) -> Vec<usize> {
    project_references
        .into_iter()
        .map(|config_name| {
            tasks.push(ProjectReferenceParseTask {
                config_name,
                ..Default::default()
            });
            tasks.len() - 1
        })
        .collect()
}

/// Go: `type projectReferenceParser struct`.
pub struct ProjectReferenceParser<'a> {
    get_resolved: &'a dyn Fn(&RootedFilePath, &PathKey) -> Option<Arc<ParsedCommandLine>>,
    /// Go: `loader *fileLoader` — only its case sensitivity reaches the parse
    /// walk; the port threads that (the loader handle would borrow the
    /// loader the parser is being constructed by).
    case_sensitivity: CaseSensitivity,
}

impl<'a> ProjectReferenceParser<'a> {
    pub fn new(
        get_resolved: &'a dyn Fn(&RootedFilePath, &PathKey) -> Option<Arc<ParsedCommandLine>>,
        case_sensitivity: CaseSensitivity,
    ) -> Self {
        ProjectReferenceParser {
            get_resolved,
            case_sensitivity,
        }
    }

    /// Go: `func (p *projectReferenceParser) parse(tasks)`.
    ///
    /// PORT: Go runs the parse closures on a WorkGroup (parents parse before
    /// children start; dedup by path, order preserved by tasksByFileName).
    /// The port walks the same tree breadth-first, sequentially — the arena
    /// borrows do not admit the parallel queue pass, and the dataflow
    /// (dedup by path, parent-before-child) is identical.
    pub fn parse(
        &self,
        root_task_indices: &[usize],
        tasks: &mut Vec<ProjectReferenceParseTask>,
    ) -> ParsedProjectReferences {
        let mut tasks_by_file_name: rustc_hash::FxHashMap<PathKey, usize> =
            rustc_hash::FxHashMap::default();
        let mut order: Vec<usize> = Vec::new();

        // Go's queued closure parses a task, then starts its subtasks; the
        // port walks the same tree breadth-first, one frontier at a time.
        let mut frontier: Vec<usize> = root_task_indices.to_vec();
        while !frontier.is_empty() {
            let mut next_frontier: Vec<usize> = Vec::new();
            for task_index in frontier {
                let path =
                    path_key_of(tasks, task_index, self.case_sensitivity);
                if tasks_by_file_name.contains_key(&path) {
                    // Go: dedup tasks to ensure correct file order,
                    // regardless of which task would be started first.
                    continue;
                }
                tasks_by_file_name.insert(path.clone(), task_index);
                order.push(task_index);
                // Go: `task.parse(p); p.start(task.subTasks)`.
                parse_task(task_index, tasks, self.get_resolved, self.case_sensitivity);
                next_frontier.extend(tasks[task_index].sub_tasks.iter().copied());
            }
            frontier = next_frontier;
        }

        ParsedProjectReferences {
            tasks_by_file_name,
            order,
            case_sensitivity: self.case_sensitivity,
        }
    }
}

/// The parsed-reference inventory `initMapper` consumes (Go: the
/// `tasksByFileName` map + the task walk in `initMapperWorker`).
pub struct ParsedProjectReferences {
    pub tasks_by_file_name: rustc_hash::FxHashMap<PathKey, usize>,
    /// The parse-then-DFS order the tasks were visited in.
    pub order: Vec<usize>,
    /// Go: the parser's `loader.toPath` case sensitivity (see
    /// `ProjectReferenceParser`).
    pub case_sensitivity: CaseSensitivity,
}

impl ParsedProjectReferences {
    /// Go: `func (p *projectReferenceParser) initMapper(tasks)` — the
    /// loader-side population of the mapper maps (the final resolution-host
    /// wrap is the identity in this port — see
    /// project_reference_dts_faking_host.rs).
    pub fn init_mapper(
        &self,
        tasks: &[ProjectReferenceParseTask],
        root_config_path: &PathKey,
        mapper: &mut ProjectReferenceFileMapper,
    ) {
        let total_references = self.tasks_by_file_name.len() + 1;
        mapper
            .config_to_project_reference
            .reserve(total_references);
        mapper
            .references_in_config_file
            .reserve(total_references);

        let mut seen: rustc_hash::FxHashSet<usize> = rustc_hash::FxHashSet::default();
        let results = self.init_mapper_worker(self.order.clone(), tasks, mapper, &mut seen);
        mapper
            .references_in_config_file
            .insert(root_config_path.clone(), results);
    }

    /// Go: `func (p *projectReferenceParser) initMapperWorker(tasks, seen) []tspath.PathKey`.
    fn init_mapper_worker(
        &self,
        task_indices: Vec<usize>,
        tasks: &[ProjectReferenceParseTask],
        mapper: &mut ProjectReferenceFileMapper,
        seen: &mut rustc_hash::FxHashSet<usize>,
    ) -> Vec<PathKey> {
        if task_indices.is_empty() {
            return Vec::new();
        }
        let mut results: Vec<PathKey> = Vec::with_capacity(task_indices.len());
        for task_index in task_indices {
            let task = &tasks[task_index];
            let path = path_key_of(tasks, task_index, self.case_sensitivity);
            results.push(path.clone());
            // ensure we only walk each task once
            if !seen.insert(task_index) {
                continue;
            }
            let resolved = &task.resolved;
            mapper.config_to_project_reference.insert(
                path.clone(),
                resolved
                    .as_ref()
                    .map(|config| Arc::new(ResolvedProjectReference { config: Arc::clone(config) })),
            );
            // Go: `task.resolved.ParseInputOutputNames()` + the
            // SourceToProjectReference/OutputDtsToProjectReference
            // `maps.Copy` — the AST-config wave (see the file header): the
            // mapper maps stay empty.
            let references_in_config = self.init_mapper_worker(
                task.sub_tasks.clone(),
                tasks,
                mapper,
                seen,
            );
            mapper
                .references_in_config_file
                .insert(path.clone(), references_in_config);
        }
        results
    }
}

fn path_key_of(
    tasks: &[ProjectReferenceParseTask],
    task_index: usize,
    case_sensitivity: CaseSensitivity,
) -> PathKey {
    case_sensitivity.path_key_str(tasks[task_index].config_name.as_string())
}
