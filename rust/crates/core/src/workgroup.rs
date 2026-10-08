// Ported from tsc/internal/core/workgroup.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT (§5.5): Go's goroutines are replaced with `std::thread::scope` inside
// `run_and_wait`/`wait`; the API surface names (WorkGroup, queue,
// run_and_wait, ThrottleGroup, go, wait) are preserved. Go's `errgroup.Group`
// first-error semantics are kept with `Mutex<Option<...>>` capture, and —
// mirroring `errgroup.WithContext` — the first error cancels the group's
// CancelToken.

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::scope;

use crate::context::CancelToken;
use crate::semaphore::Semaphore;

/// A queued unit of work; Go's bare `func()`.
pub type WorkFn = Box<dyn FnOnce() + Send>;

/// WorkGroup mirrors Go's `WorkGroup` interface.
pub trait WorkGroup: Send + Sync {
    /// Queue queues a function to run. It may be invoked immediately, or deferred until RunAndWait.
    /// It is not safe to call Queue after RunAndWait has returned.
    fn queue(&self, f: WorkFn);

    /// RunAndWait runs all queued functions, blocking until they have all completed.
    fn run_and_wait(&mut self);
}

/// NewWorkGroup mirrors `NewWorkGroup`.
pub fn new_work_group(single_threaded: bool) -> Box<dyn WorkGroup> {
    if single_threaded {
        Box::new(SingleThreadedWorkGroup::default())
    } else {
        Box::new(ParallelWorkGroup::default())
    }
}

struct ParallelState {
    fns: Vec<WorkFn>,
    active: usize,
}

struct ParallelWorkGroup {
    done: AtomicBool,
    state: Mutex<ParallelState>,
    cv: Condvar,
}

impl Default for ParallelWorkGroup {
    fn default() -> Self {
        Self {
            done: AtomicBool::new(false),
            state: Mutex::new(ParallelState {
                fns: Vec::new(),
                active: 0,
            }),
            cv: Condvar::new(),
        }
    }
}

impl WorkGroup for ParallelWorkGroup {
    fn queue(&self, f: WorkFn) {
        if self.done.load(Ordering::SeqCst) {
            panic!("Queue called after RunAndWait returned");
        }
        let mut state = self.state.lock().unwrap();
        state.fns.push(f);
        drop(state);
        self.cv.notify_one();
    }

    // PORT: Go spawns a goroutine per Queue and joins them in RunAndWait
    // (`wg.Wait`); Rust can only spawn borrows inside a scoped scope, so
    // queued functions are drained and spawned here. Queueing *during*
    // run_and_wait still works (Go allows it) because the spawn loop
    // re-checks the queue until it is empty and no thread is active.
    fn run_and_wait(&mut self) {
        // defer w.done.Store(true)
        let this: &ParallelWorkGroup = self;
        scope(|scope| loop {
            let next = {
                let mut state = this.state.lock().unwrap();
                match state.fns.pop() {
                    Some(f) => {
                        state.active += 1;
                        Some(f)
                    }
                    None => {
                        if state.active == 0 {
                            break;
                        }
                        None
                    }
                }
            };
            match next {
                Some(f) => {
                    scope.spawn(move || {
                        f();
                        let mut state = this.state.lock().unwrap();
                        state.active -= 1;
                        drop(state);
                        this.cv.notify_one();
                    });
                }
                None => {
                    // Queue drained; wait for activity to settle or more work.
                    let state = this.state.lock().unwrap();
                    let _guard = this.cv.wait(state).unwrap();
                }
            }
        });
        self.done.store(true, Ordering::SeqCst);
    }
}

struct SingleThreadedWorkGroup {
    done: AtomicBool,
    fns: Mutex<Vec<WorkFn>>,
}

impl Default for SingleThreadedWorkGroup {
    fn default() -> Self {
        Self {
            done: AtomicBool::new(false),
            fns: Mutex::new(Vec::new()),
        }
    }
}

impl WorkGroup for SingleThreadedWorkGroup {
    fn queue(&self, f: WorkFn) {
        if self.done.load(Ordering::SeqCst) {
            panic!("Queue called after RunAndWait returned");
        }
        self.fns.lock().unwrap().push(f);
    }

    fn run_and_wait(&mut self) {
        // defer w.done.Store(true)
        loop {
            let f = self.pop();
            match f {
                None => break,
                Some(f) => f(),
            }
        }
        self.done.store(true, Ordering::SeqCst);
    }
}

impl SingleThreadedWorkGroup {
    fn pop(&self) -> Option<WorkFn> {
        let mut fns = self.fns.lock().unwrap();
        // Go pops from the end (LIFO) and zeroes the slot for the GC.
        fns.pop()
    }
}

/// ThrottleFn is Go's `func() error` parameter of ThrottleGroup.Go.
pub type ThrottleFn = Box<dyn FnOnce() -> Result<(), ThrottleError> + Send>;

/// ThrottleError is Go's `error` returned from ThrottleGroup.Wait.
pub type ThrottleError = Box<dyn Error + Send>;

/// ThrottleGroup is like errgroup.Group but with global concurrency limiting via a semaphore.
pub struct ThrottleGroup {
    semaphore: Box<dyn Semaphore>,
    // PORT: Go's errgroup.WithContext cancels the group context when any
    // function returns an error; the port forwards that to the CancelToken.
    ctx: CancelToken,
    state: Mutex<ThrottleState>,
    cv: Condvar,
}

struct ThrottleState {
    fns: Vec<ThrottleFn>,
    active: usize,
    err: Option<ThrottleError>,
}

/// NewThrottleGroup creates a new ThrottleGroup with the given context and semaphore for concurrency limiting.
pub fn new_throttle_group(ctx: CancelToken, semaphore: Box<dyn Semaphore>) -> ThrottleGroup {
    ThrottleGroup {
        semaphore,
        ctx,
        state: Mutex::new(ThrottleState {
            fns: Vec::new(),
            active: 0,
            err: None,
        }),
        cv: Condvar::new(),
    }
}

impl ThrottleGroup {
    /// Go runs the given function in a new goroutine, but first acquires a slot from the semaphore.
    /// The semaphore slot is released when the function completes.
    pub fn go(&self, f: ThrottleFn) {
        let mut state = self.state.lock().unwrap();
        state.fns.push(f);
        drop(state);
        self.cv.notify_one();
    }

    /// Wait waits for all goroutines to complete and returns the first error encountered, if any.
    pub fn wait(&self) -> Option<ThrottleError> {
        let this: &ThrottleGroup = self;
        scope(|scope| loop {
            let next = {
                let mut state = this.state.lock().unwrap();
                match state.fns.pop() {
                    Some(f) => {
                        state.active += 1;
                        Some(f)
                    }
                    None => {
                        if state.active == 0 {
                            break;
                        }
                        None
                    }
                }
            };
            match next {
                Some(f) => {
                    scope.spawn(move || {
                        // Acquire semaphore slot - this will block until a slot is available
                        let release = this.semaphore.acquire();
                        let result = f();
                        // Release semaphore slot when done
                        release();
                        let mut state = this.state.lock().unwrap();
                        if state.err.is_none() {
                            if let Err(e) = result {
                                state.err = Some(e);
                                // PORT: errgroup.WithContext cancels the
                                // group context on the first error.
                                this.ctx.cancel();
                            }
                        }
                        state.active -= 1;
                        drop(state);
                        this.cv.notify_one();
                    });
                }
                None => {
                    let state = this.state.lock().unwrap();
                    let _guard = this.cv.wait(state).unwrap();
                }
            }
        });
        self.state.lock().unwrap().err.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semaphore::{new_limited_semaphore, UnlimitedSemaphore};
    use std::sync::Arc;

    #[test]
    fn parallel_work_group_runs_all() {
        let counter = Arc::new(Mutex::new(0));
        let mut wg = new_work_group(false);
        let mut expected = 0;
        for i in 0..10 {
            let counter = Arc::clone(&counter);
            wg.queue(Box::new(move || {
                *counter.lock().unwrap() += i;
            }));
            expected += i;
        }
        wg.run_and_wait();
        assert_eq!(*counter.lock().unwrap(), expected);
    }

    #[test]
    fn single_threaded_work_group_runs_all_lifo() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut wg = new_work_group(true);
        for i in 0..5 {
            let order = Arc::clone(&order);
            wg.queue(Box::new(move || {
                order.lock().unwrap().push(i);
            }));
        }
        wg.run_and_wait();
        assert_eq!(*order.lock().unwrap(), vec![4, 3, 2, 1, 0]);
    }

    #[test]
    fn queue_after_run_and_wait_panics() {
        let mut wg = new_work_group(true);
        wg.run_and_wait();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wg.queue(Box::new(|| {}));
        }));
        assert!(result.is_err());
    }

    #[test]
    fn throttle_group_limits_concurrency() {
        let ctx = CancelToken::new();
        let semaphore = Box::new(new_limited_semaphore(2));
        let tg = new_throttle_group(ctx, semaphore);
        let active = Arc::new(Mutex::new(0usize));
        let max = Arc::new(Mutex::new(0usize));
        for _ in 0..8 {
            let active = Arc::clone(&active);
            let max = Arc::clone(&max);
            tg.go(Box::new(move || {
                let mut a = active.lock().unwrap();
                *a += 1;
                {
                    let mut m = max.lock().unwrap();
                    *m = (*m).max(*a);
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
                *a -= 1;
                Ok(())
            }));
        }
        assert!(tg.wait().is_none());
        assert!(
            *max.lock().unwrap() <= 2,
            "concurrency limit respected"
        );
    }

    #[test]
    fn throttle_group_captures_error() {
        // PORT: Go's "first error wins" is inherently racy between two
        // concurrently failing goroutines; the port pins a single failing
        // task (plus a succeeding one) to keep the assertion deterministic.
        let ctx = CancelToken::new();
        let tg = new_throttle_group(ctx, Box::new(UnlimitedSemaphore));
        tg.go(Box::new(|| Ok(())));
        tg.go(Box::new(|| Err(Box::new(std::io::Error::other("first")) as ThrottleError)));
        let err = tg.wait();
        assert_eq!(err.unwrap().to_string(), "first");
    }

    #[test]
    fn throttle_group_cancels_context_on_first_error() {
        let ctx = CancelToken::new();
        let tg = new_throttle_group(ctx.clone(), Box::new(UnlimitedSemaphore));
        tg.go(Box::new(|| Err(Box::new(std::io::Error::other("boom")) as ThrottleError)));
        tg.wait();
        assert!(ctx.is_cancelled());
    }
}
