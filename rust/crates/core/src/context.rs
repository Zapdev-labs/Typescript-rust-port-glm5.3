// Ported from tsc/internal/core/context.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go's context.Context carries request-scoped values *and* a
// cancellation signal. Per SPEC §5.5, cancellation becomes an explicit
// `CancelToken` (AtomicBool, cheaply cloneable so it can be threaded
// everywhere a context.Context used to flow); the value-carrying surface
// keeps the same method names (`with_request_id`/`get_request_id`,
// `with_checker_lifetime`/`get_checker_lifetime`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// CancelToken is the port of `context.Context`'s cancellation channel
/// (`ctx.Done()`) — SPEC §5.5.
#[derive(Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// Cancel mirrors the `cancel` func from `context.WithCancel`.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// IsCancelled is the port of `ctx.Err() != nil`.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// Context is the value-carrying part of Go's `context.Context`
/// (`context.WithValue` chains), flattened into a struct. Derived contexts
/// (`with_request_id`, `with_checker_lifetime`) clone and override, matching
/// Go's copy-on-derive semantics.
#[derive(Clone, Default, Debug)]
pub struct Context {
    request_id: Option<String>,
    checker_lifetime: CheckerLifetime,
    token: CancelToken,
}

impl Context {
    /// New returns the equivalent of `context.Background()`.
    pub fn new() -> Context {
        Context::default()
    }

    /// WithRequestID mirrors `WithRequestID(ctx, id)`.
    pub fn with_request_id(&self, id: String) -> Context {
        let mut derived = self.clone();
        derived.request_id = Some(id);
        derived
    }

    /// GetRequestID mirrors `GetRequestID(ctx)` — "" when absent.
    pub fn get_request_id(&self) -> &str {
        self.request_id.as_deref().unwrap_or("")
    }

    /// WithCheckerLifetime mirrors `WithCheckerLifetime(ctx, lifetime)`.
    pub fn with_checker_lifetime(&self, lifetime: CheckerLifetime) -> Context {
        let mut derived = self.clone();
        derived.checker_lifetime = lifetime;
        derived
    }

    /// GetCheckerLifetime mirrors `GetCheckerLifetime(ctx)` — defaults to
    /// CheckerLifetimeTemporary.
    pub fn get_checker_lifetime(&self) -> CheckerLifetime {
        self.checker_lifetime
    }

    /// Token returns the cancellation token carried by this context chain.
    pub fn token(&self) -> &CancelToken {
        &self.token
    }
}

/// CheckerLifetime mirrors Go's `type CheckerLifetime int`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum CheckerLifetime {
    #[default]
    Temporary = 0,
    Diagnostics = 1,
    API = 2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_values_derive_and_default() {
        let ctx = Context::new();
        assert_eq!(ctx.get_request_id(), "");
        assert_eq!(ctx.get_checker_lifetime(), CheckerLifetime::Temporary);

        let ctx = ctx.with_request_id("req-1".to_owned());
        assert_eq!(ctx.get_request_id(), "req-1");

        let ctx = ctx.with_checker_lifetime(CheckerLifetime::API);
        assert_eq!(ctx.get_checker_lifetime(), CheckerLifetime::API);

        // Derivation does not clobber siblings (like Go context chains).
        let ctx2 = ctx.with_request_id("req-2".to_owned());
        assert_eq!(ctx2.get_checker_lifetime(), CheckerLifetime::API);
        assert_eq!(ctx.get_request_id(), "req-1");
    }

    #[test]
    fn cancel_token_flows_through_derived_contexts() {
        let ctx = Context::new();
        let derived = ctx.with_request_id("x".to_owned());
        assert!(!derived.token().is_cancelled());
        // The token is shared through the clone chain, like a cancelled parent context.
        ctx.token().cancel();
        assert!(derived.token().is_cancelled());
    }
}
