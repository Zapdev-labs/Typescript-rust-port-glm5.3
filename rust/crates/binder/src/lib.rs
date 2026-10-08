// Rust port of tsc/internal/binder @ ec47d33c23e464a17cdf2475632cba629bee8763
// (M4 wave A): binder.go + nameresolver.go.
//
// referenceresolver.go is deferred to the emit wave (per the M4 milestone
// note in rust/SPEC.md); everything else in the Go package is ported here:
//
//   binder.go        → binder.rs (the Binder, BindSourceFile, flow graph)
//   nameresolver.go  → nameresolver.rs (NameResolver; checker-owned hooks)
//
// Go helpers the binder pulls from *other* packages (ast/utilities.go,
// scanner GetErrorRangeForNode) that the earlier waves did not yet port
// live in ast_shims.rs / scanner_shims.rs with their Go symbols documented
// (dedup candidates — fold on the wholesale utilities.go / scanner wave).
//
// Go name mapping (the crate-wide contract, SPEC §5.1/§5.2):
//   *Node / *Symbol / *FlowNode / *FlowList pointers → NodeId / SymbolId /
//   FlowNodeId / FlowListId arena handles; the arenas are the Binder's
//   Vec<Symbol> / Vec<FlowNode> / Vec<FlowList>, exposed to the caller via
//   BindResult (see binder.rs's arena PORT note).

pub mod ast_shims;
pub mod binder;
pub mod nameresolver;
pub mod node_data;
pub mod scanner_shims;

pub use binder::{bind_source_file, Binder, BindResult};
pub use nameresolver::{get_local_symbol_for_export_default, NameResolver};
