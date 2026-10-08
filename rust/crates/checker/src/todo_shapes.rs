//! TODO-shapes for the checker skeleton (SPEC §8 M5 spike scope: types.go +
//! utilities.go + intrinsic/literal types ONLY).
//!
//! These are placeholder declarations for Go types whose home files are M6/M7
//! waves. They exist so the `Checker` struct and the types.go link structs can
//! mirror Go's full field list (grep-parity, SPEC §4.1) without porting the
//! machinery behind them. Each one is replaced wholesale when its owning wave
//! lands; every use site carries a `TODO(port)` banner naming the wave.
//!
//! The W1 periphery wave (mapper/jsdoc/exports/symboltracker) activated the
//! shapes its files need (marked "W1" below); the rest are still
//! never-constructed placeholders no spike code reads or writes.

use tsc_ast::{NodeId, SymbolFlags};

/// Go: `*TypeMapper` (checker/mapper.go — M6 wave 1, LANDED in mapper.rs).
/// Mapper internals are compared by `compareTypeMappers` (utilities.go) and
/// threaded through the object/type-parameter link structs. The table these
/// ids index is `Checker::type_mappers` (mapper.rs) — id equality is Go's
/// `*TypeMapper` pointer equality.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TypeMapperId(pub u32);

/// Go: `evaluator.Result` (tsc/internal/evaluator — not yet ported). The field
/// type of `EnumMemberLinks.value` (types.go). The `tsc-evaluator` crate port
/// replaces this alias.
pub type EvaluatorResult = ();

/// Go: `evaluator.Evaluator` (checker field `evaluate`) — a function table the
/// checker installs for constant-expression evaluation. M6/evaluator wave.
#[derive(Debug)]
pub struct Evaluator;

/// Go: `*ast.DiagnosticsCollection` — the ast crate's diagnostic.go port
/// currently carries only the `Diagnostic` struct (its header notes the
/// collection factories are a separate task). This alias marks the checker's
/// `diagnostics`/`suggestionDiagnostics` fields until that lands.
#[derive(Debug, Default)]
pub struct DiagnosticsCollection;

/// Go: `*InferenceState` / `*InferenceContext` / `ContextualInfo` /
/// `InferenceContextInfo` (checker/inference.go — M6 wave 2).
///
/// PORT (M6 W2): the W1 stub shapes below are replaced wholesale by
/// inference.rs's real `InferenceContext`/`InferenceInfo` (checker.go's
/// structs — the arena design the mapper reaches through
/// [`InferenceContextId`] is unchanged; the three field names the mapper
/// reads/writes — `type_parameter`/`is_fixed`/`inferred_type` — are kept).
pub use crate::inference::{InferenceContext, InferenceInfo};

/// PORT (mapper.go W1): Go's `InferenceTypeMapper` holds `n *InferenceContext`
/// — a heap pointer the inference engine creates. The Rust port stores
/// contexts in the checker-owned `inference_contexts` arena and the mapper
/// holds this index; the inference wave owns the final design and replaces it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InferenceContextId(pub u32);

/// Go: `*FlowState` and the flow caches (checker/flow.go — M6 wave 3).
#[derive(Debug, Default)]
pub struct FlowState;

/// Go: `type FlowLoopInfo struct` (checker/flow.go — M6 wave 13). The shape
/// exists so the S1 spine can carry Go's `flowLoopStack` field (the
/// save/restore around `checkExpressionCachedEx`, checker.go:7774) with the
/// real type landing with the flow wave.
#[derive(Debug, Default)]
pub struct FlowLoopInfo;

/// Go: `JsxElementLinks` (checker/jsx.go — M6 wave 5).
#[derive(Debug, Default)]
pub struct JsxElementLinks;

/// Go: `*NodeBuilder` (checker/nodebuilder*.go — M6 wave 4).
#[derive(Debug, Default)]
pub struct NodeBuilder;

/// Go: `EmitResolverLinks` (checker/emitresolver.go — M6 wave 5).
#[derive(Debug, Default)]
pub struct EmitResolverLinks;

/// Go: `*IterationTypesResolver` + `IterationTypes` (checker — iteration of
/// iterables; M6).
#[derive(Debug, Default)]
pub struct IterationTypesResolver;
#[derive(Debug, Default)]
pub struct IterationTypes;

/// Go: `cache hash keys` — `CacheHashKey` is the xxh3-derived key computed by
/// `createTypeListKey`/`getTypeListKey` etc. (checker.go — M6). Declared here so
/// the types.go instantiations tables (`ObjectType.instantiations`,
/// `TypeAliasLinks.instantiations`, ...) mirror Go's map types.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CacheHashKey(pub u64);

/// Go: `type UnionReduction uint32` + the iota consts (checker.go:26139 —
/// the S7 union-construction wave owns the values' use; exports.go's
/// GetUnionTypeEx carries the type in its signature, so W1 declares it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum UnionReduction {
    None = 0,
    Literal = 1,
    Subtype = 2,
}

/// Go: `ExternalEmitHelpers`-adjacent `patternForType`/`contextFreeTypes` map
/// key types from checker.go that the spike's Checker skeleton fields need.
/// `contextFreeTypes` is `map[*ast.Node]*Type` — keyed by NodeId (a plain
/// FxHashMap<NodeId, TypeId> needs no shape).
pub type PatternForTypeKey = ();

// ────────────────────────────────────────────────────────────────────────────
// nodebuilder-side shapes (symboltracker.go W1)
//
// The tsc/internal/nodebuilder crate is not yet ported; these are the shapes
// checker/symboltracker.go needs from it. The nodebuilder wave replaces them
// (re-exporting from the new crate, Go: the checker implements
// nodebuilder.SymbolTracker).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `nodebuilder.SymbolTracker` (nodebuilder/types.go) — the tracker
/// interface the printer/emit-resolver consumes.
///
/// PORT (Go → Rust interface port): Go's optional-but-now-required interface
/// methods become required trait methods on `&mut self` (Go's pointer
/// receivers); `track_symbol` takes the symbol payload by reference (Go's
/// `*ast.Symbol`) because the tracker has no checker access (§5.4 — the
/// nodebuilder wave, which constructs trackers with the checker in scope,
/// owns the final signature). The `'inner` lifetime parameter is the Rust
/// shape for Go's interface indirection through `*SymbolTrackerImpl` layers:
/// `take_inner_if_impl` backs the `tracker.(*SymbolTrackerImpl)` type
/// assertion in NewSymbolTrackerImpl's unwrap loop (the checker impl returns
/// its inner tracker; every other implementation returns None).
pub trait SymbolTracker<'inner> {
    /// Go: `TrackSymbol(symbol *ast.Symbol, enclosingDeclaration *ast.Node, meaning ast.SymbolFlags) bool`.
    ///
    /// PORT: Go's `*ast.Symbol` splits into the payload reference (the flags
    /// reads the impl performs) and the `SymbolId` (the §5.5 identity the
    /// tracked args record) — the nodebuilder call sites carry both (they
    /// walk the checker's symbols).
    #[allow(clippy::too_many_arguments)]
    fn track_symbol(
        &mut self,
        symbol: &tsc_ast::Symbol,
        symbol_id: tsc_ast::SymbolId,
        enclosing_declaration: Option<NodeId>,
        meaning: SymbolFlags,
    ) -> bool;
    /// Go: `ReportInaccessibleThisError()`.
    fn report_inaccessible_this_error(&mut self);
    /// Go: `ReportPrivateInBaseOfClassExpression(propertyName string)`.
    fn report_private_in_base_of_class_expression(&mut self, property_name: &str);
    /// Go: `ReportInaccessibleUniqueSymbolError()`.
    fn report_inaccessible_unique_symbol_error(&mut self);
    /// Go: `ReportCyclicStructureError()`.
    fn report_cyclic_structure_error(&mut self);
    /// Go: `ReportLikelyUnsafeImportRequiredError(specifier string, symbolName string)`.
    fn report_likely_unsafe_import_required_error(&mut self, specifier: &str, symbol_name: &str);
    /// Go: `ReportTruncationError()`.
    fn report_truncation_error(&mut self);
    /// Go: `ReportNonlocalAugmentation(containingFile *ast.SourceFile, parentSymbol *ast.Symbol, augmentingSymbol *ast.Symbol)`.
    fn report_nonlocal_augmentation(
        &mut self,
        containing_file: NodeId,
        parent_symbol: &tsc_ast::Symbol,
        augmenting_symbol: &tsc_ast::Symbol,
    );
    /// Go: `ReportNonSerializableProperty(propertyName string)`.
    fn report_non_serializable_property(&mut self, property_name: &str);
    /// Go: `ReportInferenceFallback(node *ast.Node)`.
    fn report_inference_fallback(&mut self, node: NodeId);
    /// Go: `PushErrorFallbackNode(node *ast.Node)`.
    fn push_error_fallback_node(&mut self, node: NodeId);
    /// Go: `PopErrorFallbackNode()`.
    fn pop_error_fallback_node(&mut self);
    /// PORT (not Go surface): the `tracker.(*SymbolTrackerImpl)` assertion in
    /// NewSymbolTrackerImpl's unwrap loop — see the trait PORT note.
    fn take_inner_if_impl(
        &mut self,
    ) -> Option<Box<dyn SymbolTracker<'inner> + 'inner>> {
        None
    }
}

/// Go: `type TrackedSymbolArgs struct` (checker/nodebuilderimpl.go:30 — M6
/// wave 4's file; only the three fields symboltracker.go appends are carried).
///
/// PORT: Go's `symbol *ast.Symbol` → `SymbolId` (the §5.5 identity; the
/// late-painted-statements consumer resolves the payload through the
/// program arena).
#[derive(Clone, Copy, Debug)]
pub struct TrackedSymbolArgs {
    pub symbol: tsc_ast::SymbolId,
    pub enclosing_declaration: Option<NodeId>,
    pub meaning: SymbolFlags,
}

/// Go: `type NodeBuilderContext struct` (checker/nodebuilderimpl.go:64 — M6
/// wave 4's file). The two fields symboltracker.go touches are carried; the
/// full struct (~25 fields, incl. the Host) lands with the nodebuilder-impl
/// wave.
#[derive(Debug, Default)]
pub struct NodeBuilderContext {
    /// Go: `trackedSymbols []*TrackedSymbolArgs`.
    pub tracked_symbols: Vec<TrackedSymbolArgs>,
    /// Go: `reportedDiagnostic bool`.
    pub reported_diagnostic: bool,
}
