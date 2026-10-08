// Ported from tsc/internal/core/bfs.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT (§5.5): Go's `go func(...)` per job + `sync.WaitGroup` becomes
// `std::thread::scope` per level, spawning one scoped thread per job. The
// shared closure variable `fallback` is read by goroutines in Go (a benign
// race); the port snapshots `fallback.is_none()` before the level is
// processed, which is the value those goroutines could observe. `Map` from
// core.go is inlined as `iter().map().collect()` until core.go is ported.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::scope;

use tsc_collections::{MapEntry, OrderedMap, SyncSet};

pub struct BreadthFirstSearchResult<N> {
    pub stopped: bool,
    // PORT: Go's nil slice vs empty slice is not distinguished by any
    // consumer; `path` is empty when no stop/fallback was found.
    pub path: Vec<N>,
}

// PORT: Go chains `*breadthFirstSearchJob` via `parent`; the parent chain is
// immutable once built and shared after dedup, so `Arc` is the owner.
struct BreadthFirstSearchJob<N> {
    node: N,
    parent: Option<Arc<BreadthFirstSearchJob<N>>>,
}

type JobRef<N> = Arc<BreadthFirstSearchJob<N>>;

pub struct BreadthFirstSearchLevel<'a, K, N> {
    jobs: &'a mut OrderedMap<K, JobRef<N>>,
}

impl<K: std::hash::Hash + Eq, N> BreadthFirstSearchLevel<'_, K, N> {
    pub fn has(&self, key: &K) -> bool {
        self.jobs.has(key)
    }

    pub fn delete(&mut self, key: &K) {
        self.jobs.delete(key);
    }

    pub fn range(&self, mut f: impl FnMut(&N) -> bool) {
        for job in self.jobs.values() {
            if !f(&job.node) {
                return;
            }
        }
    }
}

pub struct BreadthFirstSearchOptions<'a, K, N> {
    /// Visited is a set of nodes that have already been visited.
    /// If nil, a new set will be created.
    pub visited: Option<&'a SyncSet<K>>,
    /// PreprocessLevel is a function that, if provided, will be called
    /// before each level, giving the caller an opportunity to remove nodes.
    pub preprocess_level: Option<Box<dyn for<'x> Fn(&mut BreadthFirstSearchLevel<'x, K, N>) + Send>>,
}

impl<K, N> Default for BreadthFirstSearchOptions<'_, K, N> {
    fn default() -> Self {
        BreadthFirstSearchOptions {
            visited: None,
            preprocess_level: None,
        }
    }
}

// BreadthFirstSearchParallel performs a breadth-first search on a graph
// starting from the given node. It processes nodes in parallel and returns the path
// from the first node that satisfies the `visit` function back to the start node.
pub fn breadth_first_search_parallel<N>(
    start: N,
    neighbors: impl Fn(&N) -> Vec<N> + Send + Sync,
    visit: impl Fn(&N) -> (bool, bool) + Send + Sync,
) -> BreadthFirstSearchResult<N>
where
    N: Clone + Eq + std::hash::Hash + Send + Sync,
{
    // PORT: `Identity` lives in core.go (not yet ported); the key function
    // is inlined here. K == N because Go requires N comparable for this entry point.
    breadth_first_search_parallel_ex(
        start,
        neighbors,
        visit,
        BreadthFirstSearchOptions::default(),
        |n: &N| n.clone(),
    )
}

// BreadthFirstSearchParallelEx is an extension of BreadthFirstSearchParallel that allows
// the caller to pass a pre-seeded set of already-visited nodes and a preprocessing function
// that can be used to remove nodes from each level before parallel processing.
pub fn breadth_first_search_parallel_ex<'a, K, N>(
    start: N,
    neighbors: impl Fn(&N) -> Vec<N> + Send + Sync,
    visit: impl Fn(&N) -> (bool, bool) + Send + Sync,
    options: BreadthFirstSearchOptions<'a, K, N>,
    get_key: impl Fn(&N) -> K + Send + Sync,
) -> BreadthFirstSearchResult<N>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync,
    N: Clone + Send + Sync,
{
    let local_visited;
    let visited: &SyncSet<K> = match options.visited {
        Some(visited) => visited,
        None => {
            local_visited = SyncSet::new();
            &local_visited
        }
    };

    struct LevelResult<K, N> {
        stop: bool,
        job: Option<JobRef<N>>,
        next: Option<OrderedMap<K, JobRef<N>>>,
    }

    // Borrow the callbacks as shared refs so the process_level closure (and
    // the scoped threads it spawns) can copy them per level.
    let neighbors = &neighbors;
    let visit = &visit;
    let get_key = &get_key;

    let mut fallback: Option<JobRef<N>> = None;
    // processLevel processes each node at the current level in parallel.
    // It produces either a list of jobs to be processed in the next level,
    // or a result if the visit function returns true for any node.
    let process_level = |index: usize,
                          jobs: &mut OrderedMap<K, JobRef<N>>,
                          fallback: &mut Option<JobRef<N>>|
     -> LevelResult<K, N> {
        let _ = index;
        let lowest_goal = AtomicI64::new(i64::MAX);
        let lowest_fallback = AtomicI64::new(i64::MAX);
        let next_job_count = AtomicI64::new(0);
        // PORT: Go's goroutines read the shared `fallback` variable (benign
        // race); snapshot its emptiness before processing the level.
        let fallback_is_none = fallback.is_none();
        if let Some(preprocess_level) = &options.preprocess_level {
            preprocess_level(&mut BreadthFirstSearchLevel { jobs });
        }
        let jobs: &OrderedMap<K, JobRef<N>> = jobs;
        // One slot per job so the deterministic dedup pass below can walk
        // jobs in insertion order.
        let next: Vec<Mutex<Vec<JobRef<N>>>> =
            (0..jobs.size()).map(|_| Mutex::new(Vec::new())).collect();
        scope(|scope| {
            // Shared per-level state: spawn closures borrow these (&-refs are Copy).
            let lowest_goal = &lowest_goal;
            let lowest_fallback = &lowest_fallback;
            let next_job_count = &next_job_count;
            let next = &next;
            for (i, job) in jobs.values().enumerate() {
                scope.spawn(move || {
                    if i as i64 >= lowest_goal.load(Ordering::SeqCst) {
                        return; // Stop processing if we already found a lower result
                    }

                    // If we have already visited this node, skip it.
                    if !visited.add_if_absent(get_key(&job.node)) {
                        // Note that if we are here, we already visited this node at a
                        // previous *level*, which means `visit` must have returned false,
                        // so we don't need to update our result indices. This holds true
                        // because we deduplicated jobs before queuing the level.
                        return;
                    }

                    let (is_result, stop) = visit(&job.node);
                    if is_result {
                        // We found a result, so we will stop at this level, but an
                        // earlier job may still find a true result at a lower index.
                        if stop {
                            update_min(lowest_goal, i as i64);
                            return;
                        }
                        if fallback_is_none {
                            update_min(lowest_fallback, i as i64);
                        }
                    }

                    if i as i64 >= lowest_goal.load(Ordering::SeqCst) {
                        // If `visit` is expensive, it's likely that by the time we get here,
                        // a different job has already found a lower index result, so we
                        // don't even need to collect the next jobs.
                        return;
                    }
                    // Add the next level jobs
                    let neighbor_nodes = neighbors(&job.node);
                    if !neighbor_nodes.is_empty() {
                        next_job_count.fetch_add(neighbor_nodes.len() as i64, Ordering::SeqCst);
                        // PORT: `Map(neighborNodes, ...)` (core.go) is inlined.
                        next[i].lock().unwrap().extend(
                            neighbor_nodes
                                .iter()
                                .map(|child| {
                                    Arc::new(BreadthFirstSearchJob {
                                        node: child.clone(),
                                        parent: Some(Arc::clone(job)),
                                    })
                                })
                                .collect::<Vec<_>>(),
                        );
                    }
                });
            }
        });
        let goal = lowest_goal.load(Ordering::SeqCst);
        if goal != i64::MAX {
            // If we found a result, return it immediately.
            let (_, job) = jobs.entry_at(goal as usize).unwrap();
            return LevelResult {
                stop: true,
                job: Some(Arc::clone(job)),
                next: None,
            };
        }
        if fallback.is_none() {
            let fb = lowest_fallback.load(Ordering::SeqCst);
            if fb != i64::MAX {
                let (_, job) = jobs.entry_at(fb as usize).unwrap();
                *fallback = Some(Arc::clone(job));
            }
        }
        let mut next_jobs = OrderedMap::with_size_hint(next_job_count.load(Ordering::SeqCst) as usize);
        for slot in &next {
            for job in slot.lock().unwrap().iter() {
                if !next_jobs.has(&get_key(&job.node)) {
                    // Deduplicate synchronously to avoid messy locks and spawning
                    // unnecessary goroutines.
                    next_jobs.set(get_key(&job.node), Arc::clone(job));
                }
            }
        }
        LevelResult {
            stop: false,
            job: None,
            next: Some(next_jobs),
        }
    };

    let create_path = |job: Option<JobRef<N>>| -> Vec<N> {
        let mut path = Vec::new();
        let mut job = job;
        while let Some(current) = job {
            path.push(current.node.clone());
            job = current.parent.clone();
        }
        path
    };

    let start_key = get_key(&start);
    let mut level = OrderedMap::from_list([MapEntry {
        key: start_key,
        value: Arc::new(BreadthFirstSearchJob {
            node: start,
            parent: None,
        }),
    }]);
    let mut level_index: usize = 0;
    while !level.is_empty() {
        let result = process_level(level_index, &mut level, &mut fallback);
        if result.stop {
            return BreadthFirstSearchResult {
                stopped: true,
                path: create_path(result.job),
            };
        } else if let Some(job) = result.job {
            if fallback.is_none() {
                fallback = Some(job);
            }
        }
        level = result.next.unwrap_or_else(OrderedMap::new);
        level_index += 1;
    }
    BreadthFirstSearchResult {
        stopped: false,
        path: create_path(fallback),
    }
}

// updateMin updates the atomic integer `a` to the candidate value if it is less than the current value.
fn update_min(a: &AtomicI64, candidate: i64) -> bool {
    loop {
        let current = a.load(Ordering::SeqCst);
        if current < candidate {
            return false;
        }
        if a
            .compare_exchange(current, candidate, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return true;
        }
    }
}
