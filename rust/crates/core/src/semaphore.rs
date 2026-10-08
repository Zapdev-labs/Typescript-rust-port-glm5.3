// Ported from tsc/internal/core/semaphore.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go's `Acquire`/`TryAcquire` return a release *function*; the port
// returns the same shape as `Box<dyn FnOnce() + Send>` (a `Release`) so
// call sites can drop it exactly where Go calls `release()`. The semaphore
// channel (`chan struct{}`) becomes a Mutex<usize> slot counter + Condvar.

use std::sync::{Arc, Condvar, Mutex};

use crate::context::CancelToken;

/// Release is the port of Go's `release func()` return value.
pub type Release = Box<dyn FnOnce() + Send>;

/// Semaphore mirrors Go's `Semaphore` interface.
pub trait Semaphore: Send + Sync {
    /// Acquire blocks until a slot is available and returns a release function.
    fn acquire(&self) -> Release;

    /// TryAcquire acquires a slot, or returns `acquired == false` when the
    /// context is cancelled first.
    fn try_acquire(&self, ctx: &CancelToken) -> (Release, bool);
}

/// UnlimitedSemaphore mirrors Go's `UnlimitedSemaphore`.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnlimitedSemaphore;

impl Semaphore for UnlimitedSemaphore {
    fn acquire(&self) -> Release {
        Box::new(|| {})
    }

    fn try_acquire(&self, _ctx: &CancelToken) -> (Release, bool) {
        (Box::new(|| {}), true)
    }
}

struct LimitedSemaphoreState {
    // Number of free slots; the channel buffer size is maxConcurrency.
    free: usize,
}

/// LimitedSemaphore mirrors Go's `LimitedSemaphore`.
#[derive(Clone)]
pub struct LimitedSemaphore {
    state: Arc<Mutex<LimitedSemaphoreState>>,
    cv: Arc<Condvar>,
}

/// NewLimitedSemaphore mirrors `NewLimitedSemaphore`.
pub fn new_limited_semaphore(max_concurrency: i32) -> LimitedSemaphore {
    if max_concurrency <= 0 {
        panic!("maxConcurrency must be positive");
    }
    let s = LimitedSemaphore {
        state: Arc::new(Mutex::new(LimitedSemaphoreState {
            free: max_concurrency as usize,
        })),
        cv: Arc::new(Condvar::new()),
    };
    // PORT: Go stores `s.release = func() { <-s.ch }` once on the struct; the
    // port builds the release closure per Acquire, capturing shared handles.
    s
}

impl LimitedSemaphore {
    fn release_fn(self: &LimitedSemaphore) -> Release {
        let state = Arc::clone(&self.state);
        let cv = Arc::clone(&self.cv);
        Box::new(move || {
            let mut guard = state.lock().unwrap();
            guard.free += 1;
            cv.notify_one();
        })
    }
}

impl Semaphore for LimitedSemaphore {
    fn acquire(&self) -> Release {
        // PORT: Go's blocking channel send becomes a Condvar wait on the
        // slot counter.
        let mut guard = self.state.lock().unwrap();
        while guard.free == 0 {
            guard = self.cv.wait(guard).unwrap();
        }
        guard.free -= 1;
        drop(guard);
        self.release_fn()
    }

    fn try_acquire(&self, ctx: &CancelToken) -> (Release, bool) {
        // PORT: Go selects between "slot free" and "<-ctx.Done()"; the
        // CancelToken has no notification channel, so cancellation is
        // polled between waits. Cancellation is checked *before* acquiring.
        let mut guard = self.state.lock().unwrap();
        loop {
            if ctx.is_cancelled() {
                return (Box::new(|| {}), false);
            }
            if guard.free > 0 {
                guard.free -= 1;
                drop(guard);
                return (self.release_fn(), true);
            }
            // PORT: short poll interval — see note above.
            (guard, _) = self
                .cv
                .wait_timeout(guard, std::time::Duration::from_millis(1))
                .unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::CancelToken;

    #[test]
    fn unlimited_always_acquires() {
        let s = UnlimitedSemaphore;
        let release = s.acquire();
        release();
        let (release, acquired) = s.try_acquire(&CancelToken::new());
        assert!(acquired);
        release();
    }

    #[test]
    fn limited_panics_on_non_positive() {
        let result = std::panic::catch_unwind(|| new_limited_semaphore(0));
        assert!(result.is_err());
    }

    #[test]
    fn limited_acquire_release_roundtrip() {
        let s = new_limited_semaphore(2);
        let r1 = s.acquire();
        let r2 = s.acquire();
        let ctx = CancelToken::new();
        let (r3, acquired) = s.try_acquire(&ctx);
        assert!(!acquired, "no slot should be free with max=2 held");
        r3();
        r1();
        let (r4, acquired) = s.try_acquire(&ctx);
        assert!(acquired);
        r4();
        r2();
        // All released; two more acquires must succeed.
        let r5 = s.acquire();
        let r6 = s.acquire();
        r5();
        r6();
    }

    #[test]
    fn limited_try_acquire_respects_cancellation() {
        let s = new_limited_semaphore(1);
        let r1 = s.acquire();
        let ctx = CancelToken::new();
        let ctx2 = ctx.clone();
        ctx2.cancel();
        let (r3, acquired) = s.try_acquire(&ctx);
        assert!(!acquired);
        r3();
        r1();
    }
}
