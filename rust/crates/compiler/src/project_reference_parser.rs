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
use tsc_tspath::{PathKey, RootedFilePath};

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
fn parse_task(
    task_index: usize,
    tasks: &mut [ProjectReferenceParseTask],
    get_resolved: &dyn Fn(&RootedFilePath, &PathKey) -> Option<Arc<ParsedCommandLine>>,
) {
    let config_name = tasks[task_index].config_name.clone();
    let path_key = crate::file_loader::path_key_for(&config_name);
    let resolved = get_resolved(&config_name, &path_key);
    if let Some(resolved) = resolved {
        // Go: `t.resolved.ParseInputOutputNames()` — the AST-config wave (see
        // the file header).
        tasks[task_index].resolved = Some(Arc::clone(&resolved));
        let sub_references: Vec<RootedFilePath> = resolved
            .project_references()
            .iter()
            .map(|r| r.path.clone())
            .collect();
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
    /// Go: `wg core.WorkGroup` — the port walks breadth-first (see the file
    /// header); the parallelism seam is one queue pass over the frontier.
    single_threaded: bool,
}

impl<'a> ProjectReferenceParser<'a> {
    /// Go: `func (p *projectReferenceParser) parse(tasks)`.
    ///
    /// `init_mapper_data` receives the parsed (path → resolved) pairs in the
    /// order Go's `initMapper` consumes them (dedup by path, then the
    /// depth-first walk) so the loader can populate the mapper exactly like
    /// Go's `initMapper`/`initMapperWorker`.
    pub fn parse(
        &self,
        root_task_indices: &[usize],
        tasks: &mut Vec<ProjectReferenceParseTask>,
    ) -> ParsedProjectReferences {
        // Go: start(tasks) — walk the task tree, dedup by config path.
        let mut tasks_by_file_name: rustc_hash::FxHashMap<PathKey, usize> =
            rustc_hash::FxHashMap::default();
        let mut order: Vec<usize> = Vec::new();

        // Go's queued closure parses a task, then starts its subtasks; the
        // port walks the same tree breadth-first, one frontier at a time.
        let mut frontier: Vec<usize> = root_task_indices.to_vec();
        while !frontier.is_empty() {
            let mut next_frontier: Vec<usize> = Vec::new();
            for task_index in frontier {
                let path = path_key_of(tasks, task_index);
                match tasks_by_file_name.get(&path) {
                    // Go: dedup tasks to ensure correct file order.
                    Some(&loaded) => {
                        let _ = loaded;
                        continue;
                    }
                    None => {
                        tasks_by_file_name.insert(path.clone(), task_index);
                    }
                }
                order.push(task_index);
                // Go: `task.parse(p); p.start(task.subTasks)`.
                if !self.single_threaded {
                    // The parse itself is the only work; subtasks enqueue
                    // after (Go's semantics — parents parse before children
                    // are started).
                    parse_task(task_index, tasks, self.get_resolved);
                } else {
                    parse_task(task_index, tasks, self.get_resolved);
                }
                next_frontier.extend(tasks[task_index].sub_tasks.iter().copied());
            }
            frontier = next_frontier;
        }

        ParsedProjectReferences {
            tasks_by_file_name,
            order,
        }
    }
}

/// The parsed-reference inventory `initMapper` consumes (Go: the
/// `tasksByFileName` map + the task walk in `initMapperWorker`).
pub struct ParsedProjectReferences {
    pub tasks_by_file_name: rustc_hash::FxHashMap<PathKey, usize>,
    /// The parse-then-DFS order the tasks were visited in.
    pub order: Vec<usize>,
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
            let path = path_key_of(tasks, task_index);
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

fn path_key_of(tasks: &[ProjectReferenceParseTask], task_index: usize) -> PathKey {
    crate::file_loader::path_key_for(&tasks[task_index].config_name)
}
