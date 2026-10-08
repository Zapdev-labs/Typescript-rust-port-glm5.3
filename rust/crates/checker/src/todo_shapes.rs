//! TODO-shapes for the checker skeleton (SPEC §8 M5 spike scope: types.go +
//! utilities.go + intrinsic/literal types ONLY).
//!
//! These are placeholder declarations for Go types whose home files are M6/M7
//! waves. They exist so the `Checker` struct and the types.go link structs can
//! mirror Go's full field list (grep-parity, SPEC §4.1) without porting the
//! machinery behind them. Each one is replaced wholesale when its owning wave
//! lands; every use site carries a `TODO(port)` banner naming the wave.
//!
//! Never construct these; nothing in the spike reads or writes them.

/// Go: `*TypeMapper` (checker/mapper.go — M6 wave 1). Mapper internals are
/// compared by `compareTypeMappers` (utilities.go) and threaded through the
/// object/type-parameter link structs. The M6 mapper port defines the table
/// these ids index.
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

/// Go: `*Tracer` (checker/tracer.go — M6 wave 2, "ported early" per SPEC §8
/// M6 order). Records type/check events for `--generateTrace`.
#[derive(Debug)]
pub struct Tracer;

/// Go: `*ast.DiagnosticsCollection` — the ast crate's diagnostic.go port
/// currently carries only the `Diagnostic` struct (its header notes the
/// collection factories are a separate task). This alias marks the checker's
/// `diagnostics`/`suggestionDiagnostics` fields until that lands.
#[derive(Debug, Default)]
pub struct DiagnosticsCollection;

/// Go: `*Relation` (checker/relater.go — M6 wave 2). The four relation
/// instances live on the checker; empty until the relater wave.
#[derive(Debug, Default)]
pub struct Relation;

/// Go: `*Relater` (checker/relater.go).
#[derive(Debug, Default)]
pub struct Relater;

/// Go: `*InferenceState` / `*InferenceContext` / `ContextualInfo` /
/// `InferenceContextInfo` (checker/inference.go — M6 wave 2).
#[derive(Debug, Default)]
pub struct InferenceState;
#[derive(Debug, Default)]
pub struct InferenceContext;
#[derive(Debug, Default)]
pub struct ContextualInfo;
#[derive(Debug, Default)]
pub struct InferenceContextInfo;

/// Go: `*FlowState` and the flow caches (checker/flow.go — M6 wave 3).
#[derive(Debug, Default)]
pub struct FlowState;

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

/// Go: `ExternalEmitHelpers`-adjacent `patternForType`/`contextFreeTypes` map
/// key types from checker.go that the spike's Checker skeleton fields need.
/// `contextFreeTypes` is `map[*ast.Node]*Type` — keyed by NodeId (a plain
/// FxHashMap<NodeId, TypeId> needs no shape).
pub type PatternForTypeKey = ();
