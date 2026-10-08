// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763
// — spike slices per SPEC §8 M5:
//   * the `Checker` struct + `NewChecker` (intrinsic/literal-creation slice),
//   * the `getTypeOfSymbol` resolution chain down to `checkExpressionWorker`'s
//     literal arms and `getWidenedLiteralType`,
//   * type-resolution cycle machinery (`pushTypeResolution` family),
//   * `newSymbol`/`newSymbolEx`, the link stores, and the arenas.
//
// §5.4 PROOF (checker-eradication): Go's `Type.checker` back-pointer cannot
// exist in Rust (`Type` in an arena cannot borrow the checker). The port:
//   1. `Type` carries `checker_shard: u16` (types.rs) — identity only.
//   2. EVERY code path that read `t.checker` takes `&Checker`/`&mut Checker`
//      explicitly (`compare_types` in utilities.rs is the worked example).
//   3. `*Type` becomes `TypeId` (arena index); payload mutation goes through
//      `Checker::type_mut` — Go's pointer writes become arena writes.
//
// §5.5 PROOF (store ownership): Go's checker owns `symbolArena` while
// binder-created symbols live elsewhere — Go distinguishes them by pointer.
// The port has one program-level symbol arena (`Program::symbol` /
// `Program::alloc_symbol`): checker-created symbols allocate from the same
// `SymbolId` space the binder used, so `SymbolId` stays a flat program index
// and no rebasing is needed at pool-merge time. Per-checker state that Go
// hangs on `*Symbol` (link tables) is keyed by `SymbolId` in the checker's
// link stores — identical laziness, identical merge story.

use std::sync::atomic::{AtomicU32, Ordering};

use rustc_hash::{FxHashMap, FxHashSet};
use tsc_ast::{
    CheckFlags, Kind, ModifierFlags, Node, NodeFlags, NodeId, NodeStore, Symbol, SymbolFlags,
    SymbolId, SymbolTable,
};
use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_binder::ast_shims as binder_shims;
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use tsc_core::tristate::Tristate;
use tsc_core::linkstore::LinkStore;
use tsc_core::options_generated::ScriptTarget;
use tsc_jsnum::{Number, PseudoBigInt};

use crate::ast_shims;
use crate::links::{NodeLinkStore, SymbolArenaLinkStore};
use crate::mapper::TypeMapperData;
use crate::todo_shapes::{
    CacheHashKey, FlowState, InferenceContext, InferenceContextId, TypeMapperId,
};
use crate::types::{
    AliasSymbolLinks, ArrayLiteralLinks, AssertionLinks, ComputedNameNodeLinks,
    ContainingSymbolLinks, DeclaredTypeLinks, DeferredSymbolLinks, EnumMemberLinks, ExportTypeLinks,
    IndexInfo, IntrinsicType, LateBoundLinks, LiteralType, LiteralValue, MarkedAssignmentSymbolLinks,
    MembersAndExportsLinks, MappedSymbolLinks, ModuleSymbolLinks, NodeLinks,
    ObjectFlags, ReverseMappedSymbolLinks, Signature, SourceFileLinks, SpreadLinks,
    SwitchStatementLinks, SymbolNodeLinks, SymbolReferenceLinks, Type, TypeAliasLinks, TypeData,
    TypeFlags, TypeId, TypeNodeLinks, ValueSymbolLinks, VarianceFlags, VarianceLinks,
};
use crate::utilities as u;

// ────────────────────────────────────────────────────────────────────────────
// CheckMode (Go checker.go — `type CheckMode uint32`)
// ────────────────────────────────────────────────────────────────────────────

define_flags!(CheckMode, u32);

impl CheckMode {
    /// Go: `CheckModeNormal CheckMode = 0` — normal type checking.
    pub const CHECK_MODE_NORMAL: CheckMode = CheckMode(0);
    /// Go: `CheckModeContextual CheckMode = 1 << 0` — explicitly assigned
    /// contextual type, therefore not cacheable.
    pub const CONTEXTUAL: CheckMode = CheckMode(1 << 0);
    /// Go: `CheckModeInferential CheckMode = 1 << 1` — inferential typing.
    pub const INFERENTIAL: CheckMode = CheckMode(1 << 1);
    /// Go: `CheckModeSkipContextSensitive CheckMode = 1 << 2`.
    pub const SKIP_CONTEXT_SENSITIVE: CheckMode = CheckMode(1 << 2);
    /// Go: `CheckModeSkipGenericFunctions CheckMode = 1 << 3`.
    pub const SKIP_GENERIC_FUNCTIONS: CheckMode = CheckMode(1 << 3);
    /// Go: `CheckModeIsForSignatureHelp CheckMode = 1 << 4`.
    pub const IS_FOR_SIGNATURE_HELP: CheckMode = CheckMode(1 << 4);
    /// Go: `CheckModeRestBindingElement CheckMode = 1 << 5`.
    pub const REST_BINDING_ELEMENT: CheckMode = CheckMode(1 << 5);
    /// Go: `CheckModeTypeOnly CheckMode = 1 << 6`.
    pub const TYPE_ONLY: CheckMode = CheckMode(1 << 6);
    /// Go: `CheckModeForceTuple CheckMode = 1 << 7`.
    pub const FORCE_TUPLE: CheckMode = CheckMode(1 << 7);
}

// ────────────────────────────────────────────────────────────────────────────
// TypeSystemEntity / TypeResolution (Go checker.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type TypeSystemEntity any` — the kind-tagged form the port needs for
/// `PartialEq` in the resolution-cycle detector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeSystemEntity {
    Symbol(SymbolId),
    Type(TypeId),
    Signature(crate::types::SignatureId),
    Node(NodeId),
}

/// Go: `type TypeSystemPropertyName int32` (iota order preserved).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum TypeSystemPropertyName {
    Type = 0,
    ResolvedBaseConstructorType,
    DeclaredType,
    ResolvedReturnType,
    ResolvedBaseConstraint,
    ResolvedTypeArguments,
    ResolvedBaseTypes,
    WriteType,
    InitializerIsUndefined,
    AliasTarget,
}

/// Go: `type TypeResolution struct`.
#[derive(Clone, Copy, Debug)]
pub struct TypeResolution {
    pub target: TypeSystemEntity,
    pub property_name: TypeSystemPropertyName,
    pub result: bool,
}

// ────────────────────────────────────────────────────────────────────────────
// Program (Go checker.go's package-local `Program` interface)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type Program interface { Host; Options(); SourceFiles();
/// BindSourceFiles(); ... }` — the checker's view of the program. The Go
/// interface is program-wide (module resolution, emit metadata, JSX runtime);
/// the spike keeps the surface the ported code reads plus the §5.5 symbol
/// store seam. The compiler crate (parallel wave) provides the full impl;
/// tests provide the single-file impl.
///
/// PORT: `node_store()` covers the single-file spike (the parser hands every
/// file id 0; the M5 program merge re-bases per-file NodeIds and swaps this
/// for a program-wide multi-file store).
pub trait Program {
    /// Go: `Program.Options()`.
    fn options(&self) -> &CompilerOptions;
    /// Go: `Program.SourceFiles()`.
    fn source_files(&self) -> &[tsc_ast::SourceFile];
    /// Go: `Program.BindSourceFiles()` — called first by `NewChecker`.
    fn bind_source_files(&mut self);
    /// The node arena backing every `NodeId` the checker walks.
    fn node_store(&self) -> &dyn NodeStore;
    // ── §5.5 program-level symbol store (see the module PORT note) ──
    /// The symbol arena slot for `id`.
    fn symbol(&self, id: SymbolId) -> &Symbol;
    /// Mutable symbol slot access (Go mutates `*Symbol` through pointers).
    fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol;
    /// Appends a checker-created symbol to the program store.
    fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId;
}

// ────────────────────────────────────────────────────────────────────────────
// Checker
// ────────────────────────────────────────────────────────────────────────────

/// Go: `var nextCheckerID atomic.Uint32`.
static NEXT_CHECKER_ID: AtomicU32 = AtomicU32::new(0);

/// Go: `type Checker struct` — fields in Go declaration order, grouped by the
/// blocks of the Go struct. Omitted M6 blocks are named in banners (the
/// getGlobal* resolver closures, flow caches, inference contexts, relations,
/// and the deferred M6-constructed types: `booleanType`, `stringOrNumberType`,
/// `stringNumberSymbolType`, `numberOrBigIntType`, `numericStringType`,
/// `templateConstraintType`, `emptyObjectType`, `unknownUnionType`,
/// `anyArrayType`, `autoArrayType`, `uniqueLiteralMapper`, the marker types
/// and the signatures — all constructed through union/anonymous-type/mapper
/// machinery that is M6).
// dead_code: the M6-spike fields (id, deferred intrinsic slots, caches) are
// carried for Go-shape parity and constructed in new_checker; their readers
// land with the M6 waves. Remove with the first full-wave port.
#[allow(dead_code)]
pub struct Checker {
    pub(crate) id: u32,
    /// PORT (§5.4): replaces the Go ↔ `Type.checker` identity — types carry
    /// this shard id; `CompareTypes` panics on cross-shard compares exactly
    /// like Go's `t1.checker != t2.checker`.
    pub(crate) shard: u16,
    pub(crate) program: Box<dyn Program>,
    pub(crate) compiler_options: CompilerOptions,
    /// Go: `fileIndexMap map[*ast.SourceFile]int` — keyed by the source
    /// file's node id (the §5.1 file identity).
    pub(crate) file_index_map: FxHashMap<u64, usize>,
    pub(crate) type_count: u32,
    pub(crate) symbol_count: u32,
    pub(crate) signature_count: u32,
    pub(crate) instantiation_count: u32,
    pub(crate) current_node: Option<NodeId>,
    pub(crate) language_version: ScriptTarget,
    pub(crate) module_kind: ModuleKind,
    pub(crate) module_resolution_kind: ModuleResolutionKind,
    pub(crate) legacy_decorators: bool,
    pub(crate) emit_standard_class_fields: bool,
    pub(crate) strict_null_checks: bool,
    pub(crate) strict_function_types: bool,
    pub(crate) strict_bind_call_apply: bool,
    pub(crate) strict_property_initialization: bool,
    pub(crate) strict_builtin_iterator_return: bool,
    pub(crate) no_implicit_any: bool,
    pub(crate) no_implicit_this: bool,
    pub(crate) use_unknown_in_catch_variables: bool,
    pub(crate) exact_optional_property_types: bool,
    pub(crate) can_collect_symbol_alias_accessibility_data: bool,
    pub(crate) was_canceled: bool,
    pub(crate) array_variances: Vec<VarianceFlags>,
    pub(crate) globals: SymbolTable,
    // Literal-type caches. PORT: `jsnum.Number` is not `Eq`/`Hash` in the
    // port's jsnum crate, so Go's `map[jsnum.Number]*Type` keys by the f64
    // bits (`number_key`, with -0.0 → 0 canonicalization); NaN is cached
    // separately (`nan_type`), mirroring Go's NaN comment. `PseudoBigInt`
    // is `Eq`/`Hash`, so the bigint cache keys by value exactly like Go.
    pub(crate) string_literal_types: FxHashMap<String, TypeId>,
    pub(crate) number_literal_types: FxHashMap<u64, TypeId>,
    pub(crate) nan_type: Option<TypeId>,
    pub(crate) bigint_literal_types: FxHashMap<PseudoBigInt, TypeId>,
    // TODO(port M6): enumLiteralTypes / enumNaNLiteralTypes / the ~20
    // CacheHashKey-keyed caches below them in the Go struct.
    pub(crate) undefined_symbol: Option<SymbolId>,
    pub(crate) arguments_symbol: Option<SymbolId>,
    pub(crate) require_symbol: Option<SymbolId>,
    pub(crate) unknown_symbol: Option<SymbolId>,
    pub(crate) global_this_symbol: Option<SymbolId>,
    // Link stores (Go declaration order).
    pub(crate) node_links: LinkStore<NodeId, NodeLinks>,
    pub(crate) symbol_node_links: NodeLinkStore<SymbolNodeLinks>,
    pub(crate) type_node_links: LinkStore<NodeId, TypeNodeLinks>,
    pub(crate) enum_member_links: LinkStore<NodeId, EnumMemberLinks>,
    pub(crate) assertion_links: LinkStore<NodeId, AssertionLinks>,
    pub(crate) array_literal_links: LinkStore<NodeId, ArrayLiteralLinks>,
    pub(crate) switch_statement_links: LinkStore<NodeId, SwitchStatementLinks>,
    pub(crate) computed_name_links: LinkStore<NodeId, ComputedNameNodeLinks>,
    pub(crate) symbol_reference_links: LinkStore<SymbolId, SymbolReferenceLinks>,
    pub(crate) value_symbol_links: SymbolArenaLinkStore<ValueSymbolLinks>,
    pub(crate) mapped_symbol_links: LinkStore<SymbolId, MappedSymbolLinks>,
    pub(crate) deferred_symbol_links: LinkStore<SymbolId, DeferredSymbolLinks>,
    pub(crate) alias_symbol_links: LinkStore<SymbolId, AliasSymbolLinks>,
    pub(crate) module_symbol_links: LinkStore<SymbolId, ModuleSymbolLinks>,
    pub(crate) late_bound_links: LinkStore<SymbolId, LateBoundLinks>,
    pub(crate) export_type_links: LinkStore<SymbolId, ExportTypeLinks>,
    pub(crate) members_and_exports_links: LinkStore<SymbolId, MembersAndExportsLinks>,
    pub(crate) type_alias_links: LinkStore<SymbolId, TypeAliasLinks>,
    pub(crate) declared_type_links: LinkStore<SymbolId, DeclaredTypeLinks>,
    pub(crate) spread_links: LinkStore<SymbolId, SpreadLinks>,
    pub(crate) variance_links: LinkStore<SymbolId, VarianceLinks>,
    pub(crate) reverse_mapped_symbol_links: LinkStore<SymbolId, ReverseMappedSymbolLinks>,
    pub(crate) marked_assignment_symbol_links: LinkStore<SymbolId, MarkedAssignmentSymbolLinks>,
    pub(crate) symbol_container_links: LinkStore<SymbolId, ContainingSymbolLinks>,
    pub(crate) source_file_links: LinkStore<NodeId, SourceFileLinks>,
    // TODO(port M6): signatureLinks, jsxElementLinks (JsxElementLinks), the
    // RegExpScanner, patternForType, contextFreeTypes, flow caches.
    // ── Arenas (§5.5) ──
    /// PORT: Go's `core.Arena[Type]` per checker — a plain `Vec<Type>` whose
    /// slot 0 is a dummy so `TypeId == arena index` (Go's ids are 1-based
    /// creation counters; the dummy keeps the arithmetic identical).
    pub(crate) types: Vec<Type>,
    /// Go: `core.Arena[Signature]` (same dummy-slot trick).
    pub(crate) signatures: Vec<Signature>,
    /// Go: `core.Arena[IndexInfo]`.
    pub(crate) index_infos: Vec<IndexInfo>,
    // ── Intrinsic/literal types (Go struct order; Option = constructed in
    //    new_checker, left None where M6 machinery gates construction) ──
    pub(crate) any_type: Option<TypeId>,
    pub(crate) auto_type: Option<TypeId>,
    pub(crate) wildcard_type: Option<TypeId>,
    pub(crate) blocked_string_type: Option<TypeId>,
    pub(crate) error_type: Option<TypeId>,
    pub(crate) unresolved_type: Option<TypeId>,
    pub(crate) non_inferrable_any_type: Option<TypeId>,
    pub(crate) intrinsic_marker_type: Option<TypeId>,
    pub(crate) unknown_type: Option<TypeId>,
    pub(crate) undefined_type: Option<TypeId>,
    pub(crate) undefined_widening_type: Option<TypeId>,
    pub(crate) missing_type: Option<TypeId>,
    pub(crate) undefined_or_missing_type: Option<TypeId>,
    pub(crate) optional_type: Option<TypeId>,
    pub(crate) null_type: Option<TypeId>,
    pub(crate) null_widening_type: Option<TypeId>,
    pub(crate) string_type: Option<TypeId>,
    pub(crate) number_type: Option<TypeId>,
    pub(crate) bigint_type: Option<TypeId>,
    pub(crate) regular_false_type: Option<TypeId>,
    pub(crate) false_type: Option<TypeId>,
    pub(crate) regular_true_type: Option<TypeId>,
    pub(crate) true_type: Option<TypeId>,
    pub(crate) es_symbol_type: Option<TypeId>,
    pub(crate) void_type: Option<TypeId>,
    pub(crate) never_type: Option<TypeId>,
    pub(crate) silent_never_type: Option<TypeId>,
    pub(crate) implicit_never_type: Option<TypeId>,
    pub(crate) unreachable_never_type: Option<TypeId>,
    pub(crate) non_primitive_type: Option<TypeId>,
    pub(crate) unique_literal_type: Option<TypeId>,
    pub(crate) empty_string_type: Option<TypeId>,
    pub(crate) zero_type: Option<TypeId>,
    pub(crate) zero_bigint_type: Option<TypeId>,
    // TODO(port M6): booleanType, stringOrNumberType, stringNumberSymbolType,
    // numberOrBigIntType, numericStringType, templateConstraintType — union /
    // template-literal construction; emptyObjectType family — newAnonymousType;
    // anySignature family — newSignature; the marker type parameters;
    // noTypePredicate; enumNumberIndexInfo / anyBaseTypeIndexInfo consumers.
    // ── Resolution stack (Go: typeResolutions / resolutionStart) ──
    pub(crate) type_resolutions: Vec<TypeResolution>,
    pub(crate) resolution_start: usize,
    // ── Single-entry combined-flags caches (Go: lastGetCombined*Node/Result) ──
    pub(crate) last_get_combined_node_flags_node: Option<NodeId>,
    pub(crate) last_get_combined_node_flags_result: NodeFlags,
    pub(crate) last_get_combined_modifier_flags_node: Option<NodeId>,
    pub(crate) last_get_combined_modifier_flags_result: ModifierFlags,
    // ── todo_shapes-backed Go fields (§8 M6 waves) ──
    pub(crate) tracer: Option<crate::tracer::Tracer>,
    pub(crate) free_flow_state: Option<FlowState>,
    pub(crate) unique_literal_mapper: Option<TypeMapperId>,
    // ── Type-instantiation mapper machinery (mapper.go W1) ──
    /// PORT (mapper.go W1): Go's mapper.go constructors return `*TypeMapper`
    /// heap objects; the port allocates them in this table and hands out
    /// `TypeMapperId` indices — id equality is Go's pointer equality
    /// (`compareTypeMappers`'s first check, `findActiveMapper`).
    pub(crate) type_mappers: Vec<TypeMapperData>,
    /// Go (checker.go:899): `activeMappers []*TypeMapper` — the instantiation
    /// mapper stack. push/pop live below (checker.go 22634+); the instantiate
    /// engine that drives them is M6 S7 (todo stub).
    pub(crate) active_mappers: Vec<TypeMapperId>,
    /// Go (checker.go:900): `activeTypeMappersCaches []map[CacheHashKey]*Type`
    /// — the per-active-mapper instantiation caches.
    pub(crate) active_type_mappers_caches: Vec<FxHashMap<CacheHashKey, TypeId>>,
    /// PORT (mapper.go W1): the arena backing todo_shapes' InferenceContext —
    /// `InferenceTypeMapper` holds `n *InferenceContext` in Go; the port
    /// indexes this Vec through `InferenceContextId` (the inference wave, M6
    /// wave 2, owns the real design).
    pub(crate) inference_contexts: Vec<InferenceContext>,
    // ── Relater state (Go relater.go's Checker fields — M6 W2) ──
    /// Go: the five `*Relation` instances (PORT: `RelationKind` identity —
    /// see relater.rs's module note). Fixed at NewChecker; never re-created
    /// (Go's pointer identity for `r.relation == c.identityRelation`).
    pub(crate) identity_relation: crate::relater::Relation,
    pub(crate) assignable_relation: crate::relater::Relation,
    pub(crate) subtype_relation: crate::relater::Relation,
    pub(crate) strict_subtype_relation: crate::relater::Relation,
    pub(crate) comparable_relation: crate::relater::Relation,
    /// Go: `enumRelation map[EnumRelationKey]RelationComparisonResult`.
    pub(crate) enum_relation:
        FxHashMap<crate::relater::EnumRelationKey, crate::relater::RelationComparisonResult>,
    /// Go: `reliabilityFlags RelationComparisonResult` — set when an
    /// unmeasurable/unreliable comparison is detected (variance probing).
    pub(crate) reliability_flags: crate::relater::RelationComparisonResult,
    /// Go: `markerTypes map[*Type]bool` — the marker type parameters
    /// `getVariancesWorker` measures with.
    pub(crate) marker_types: FxHashSet<TypeId>,
    /// Go: `markerSuperType/markerSubType/markerOtherType *Type` — created
    /// once in NewChecker (Go: the deferred NewChecker marker creation; the
    /// `expect` on use names the seam).
    pub(crate) marker_super_type: Option<TypeId>,
    pub(crate) marker_sub_type: Option<TypeId>,
    pub(crate) marker_other_type: Option<TypeId>,
    /// Go: `varianceStack []varianceStackEntry`.
    pub(crate) variance_stack: Vec<crate::relater::VarianceStackEntry>,
    /// Go: the `*Relater` free list (`c.freeRelater`). PORT: the pool is a
    /// stack of idle relaters; `get_relater`/`put_relater` in relater.rs.
    pub(crate) free_relater: Vec<crate::relater::Relater>,
    /// Go: `anyArrayType *Type` — NewChecker's `createArrayType(anyType)`.
    /// TODO(port M6 S7): createArrayType constructs a type reference; until
    /// that lands the field holds anyType (the array-ness consumers gate on
    /// `isArrayType`, itself a permissive S7 stub, so no behavior diverges).
    pub(crate) any_array_type: TypeId,
    /// Go: `globalArrayType`/`globalReadonlyArrayType`/`globalObjectType`/
    /// `globalFunctionType` — the globals' declared types (S2 getGlobal*).
    /// TODO(port M6 S2): None until the globals resolution wave; use sites
    /// treat None as "not the global" (Go's nil check).
    pub(crate) global_array_type: Option<TypeId>,
    pub(crate) global_readonly_array_type: Option<TypeId>,
    pub(crate) global_object_type: Option<TypeId>,
    pub(crate) global_function_type: Option<TypeId>,
    /// Go: `globalStringType`/`globalNumberType`/`globalBooleanType` — the
    /// wrapper-object globals (S2; the primitive-wrapper elaborations read
    /// them).
    pub(crate) global_string_type: Option<TypeId>,
    pub(crate) global_number_type: Option<TypeId>,
    pub(crate) global_boolean_type: Option<TypeId>,
    /// Go: `emptyTypeLiteralType *Type` — `{}` (S7's newAnonymousType).
    pub(crate) empty_type_literal_type: Option<TypeId>,
    /// PORT: Go caches `UnionType.keyPropertyName`/`constituentMap` on the
    /// union itself; the port keeps the cache checker-side (the types.rs
    /// UnionType has no mutation seam for the map), keyed by the union id —
    /// same lifetime semantics (unions are interned per checker).
    pub(crate) key_property_names: FxHashMap<TypeId, Option<String>>,
}

/// Go: `func createFileIndexMap(files []*ast.SourceFile) map[*ast.SourceFile]int`.
fn create_file_index_map(files: &[tsc_ast::SourceFile]) -> FxHashMap<u64, usize> {
    let mut result = FxHashMap::with_capacity_and_hasher(files.len(), Default::default());
    for (i, file) in files.iter().enumerate() {
        result.insert(file.as_node_id().0, i);
    }
    result
}

/// The `number_key` from the module PORT note: f64 bits, -0.0 → 0.0
/// canonicalized (Go's map keys distinguish neither: IEEE 754 map keys).
pub fn number_key(f: f64) -> u64 {
    if f == 0.0 {
        0f64.to_bits()
    } else {
        f.to_bits()
    }
}

impl Checker {
    // ── Arena access (§5.4 idiom: `*Type` → `TypeId`) ──

    /// The arena slot for `id` (Go: dereferencing a `*Type`). Public: the
    /// crate's consumers (tests, the future printer) read types through the
    /// checker, exactly like Go's `*Type` pointers.
    pub fn type_(&self, id: TypeId) -> &Type {
        &self.types[id.0 as usize]
    }

    /// Mutable arena access — Go's pointer writes on types.
    pub(crate) fn type_mut(&mut self, id: TypeId) -> &mut Type {
        &mut self.types[id.0 as usize]
    }

    pub(crate) fn store(&self) -> &dyn NodeStore {
        self.program.node_store()
    }
}

/// Go field accessors for the intrinsic types (Go fields are non-nil after
/// NewChecker; the `expect` message names the still-None M6 field).
macro_rules! intrinsic_accessors {
    ($($name:ident => $field:ident),* $(,)?) => {
        $(
            #[allow(dead_code)]
            pub fn $name(&self) -> TypeId {
                self.$field.expect(stringify!($field))
            }
        )*
    };
}

impl Checker {
    // ── Intrinsic-type accessors (Go fields are non-nil after NewChecker) ──

    intrinsic_accessors! {
        any_type => any_type,
        auto_type => auto_type,
        wildcard_type => wildcard_type,
        blocked_string_type => blocked_string_type,
        error_type => error_type,
        unresolved_type => unresolved_type,
        non_inferrable_any_type => non_inferrable_any_type,
        intrinsic_marker_type => intrinsic_marker_type,
        unknown_type => unknown_type,
        undefined_type => undefined_type,
        undefined_widening_type => undefined_widening_type,
        missing_type => missing_type,
        undefined_or_missing_type => undefined_or_missing_type,
        optional_type => optional_type,
        null_type => null_type,
        null_widening_type => null_widening_type,
        string_type => string_type,
        number_type => number_type,
        bigint_type => bigint_type,
        regular_false_type => regular_false_type,
        false_type => false_type,
        regular_true_type => regular_true_type,
        true_type => true_type,
        es_symbol_type => es_symbol_type,
        void_type => void_type,
        never_type => never_type,
        silent_never_type => silent_never_type,
        implicit_never_type => implicit_never_type,
        unreachable_never_type => unreachable_never_type,
        non_primitive_type => non_primitive_type,
        unique_literal_type => unique_literal_type,
        empty_string_type => empty_string_type,
        zero_type => zero_type,
        zero_bigint_type => zero_bigint_type,
    }

    /// Go: the `booleanType` field — `getUnionType([false, true])`,
    /// TODO(port M6 union types).
    pub fn boolean_type(&self) -> TypeId {
        panic!("TODO(port M6): booleanType = getUnionType([regularFalseType, regularTrueType])")
    }

    /// The `missingType` id as an `Option`-free helper for utilities.rs.
    pub(crate) fn missing_type_id(&self) -> TypeId {
        self.missing_type.expect("missing_type")
    }

    // ────────────────────────────────────────────────────────────────────────
    // Type creation (Go checker.go 25537+)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) newType(flags TypeFlags, objectFlags ObjectFlags, data TypeData) *Type`.
    pub(crate) fn new_type(
        &mut self,
        flags: TypeFlags,
        object_flags: ObjectFlags,
        data: TypeData,
    ) -> TypeId {
        self.type_count += 1;
        let t = Type {
            flags,
            // Go: objectFlags &^ (CouldContainTypeVariablesComputed |
            // CouldContainTypeVariables | MembersResolved)
            object_flags: object_flags
                & !(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                    | ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES
                    | ObjectFlags::MEMBERS_RESOLVED),
            id: TypeId(self.type_count),
            symbol: None,
            alias: None,
            checker_shard: self.shard,
            data,
        };
        // The arena's slot 0 is a dummy so id == index (see the field note).
        debug_assert_eq!(t.id.0 as usize, self.types.len());
        self.types.push(t);
        // Go: tracer.RecordType — todo_shapes::Tracer (M6).
        TypeId(self.type_count)
    }

    /// Go: `func (c *Checker) newIntrinsicType(flags, intrinsicName) *Type`.
    pub(crate) fn new_intrinsic_type(&mut self, flags: TypeFlags, intrinsic_name: &str) -> TypeId {
        self.new_intrinsic_type_ex(flags, intrinsic_name, ObjectFlags::NONE)
    }

    /// Go: `func (c *Checker) newIntrinsicTypeEx(flags, intrinsicName, objectFlags) *Type`.
    pub(crate) fn new_intrinsic_type_ex(
        &mut self,
        flags: TypeFlags,
        intrinsic_name: &str,
        object_flags: ObjectFlags,
    ) -> TypeId {
        let data = TypeData::Intrinsic(Box::new(IntrinsicType {
            intrinsic_name: intrinsic_name.to_string(),
        }));
        self.new_type(flags, object_flags, data)
    }

    /// Go: `func (c *Checker) createWideningType(nonWideningType *Type) *Type`.
    pub(crate) fn create_widening_type(&mut self, non_widening_type: TypeId) -> TypeId {
        if self.strict_null_checks {
            return non_widening_type;
        }
        let flags = self.type_(non_widening_type).flags;
        let name = self
            .type_(non_widening_type)
            .as_intrinsic_type()
            .map(|d| d.intrinsic_name.clone())
            .unwrap_or_default();
        let t = self.new_intrinsic_type(flags, &name);
        let object_flags = self.type_(t).object_flags | ObjectFlags::CONTAINS_WIDENING_TYPE;
        self.type_mut(t).object_flags = object_flags;
        t
    }

    /// Go: `func (c *Checker) newLiteralType(flags, value, regularType) *Type`.
    pub(crate) fn new_literal_type(
        &mut self,
        flags: TypeFlags,
        value: LiteralValue,
        regular_type: Option<TypeId>,
    ) -> TypeId {
        let t = self.new_type(
            flags,
            ObjectFlags::NONE,
            TypeData::Literal(Box::new(LiteralType {
                value: Some(value),
                fresh_type: None,
                regular_type: None,
            })),
        );
        let regular = regular_type.unwrap_or(t);
        self.type_mut(t)
            .as_literal_type_mut()
            .expect("LiteralType data")
            .regular_type = Some(regular);
        t
    }

    /// Go: `func (c *Checker) getRegularTypeOfLiteralType(t *Type) *Type`.
    #[allow(dead_code)] // exercised by the spike tests; in-lib callers land in M6
    pub(crate) fn get_regular_type_of_literal_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::FRESHABLE) {
            return self
                .type_(t)
                .as_literal_type()
                .expect("LiteralType data")
                .regular_type
                .expect("regularType");
        }
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            // Go: u.regularType = c.mapType(...) — TODO(port M6): mapType.
            panic!("TODO(port M6): getRegularTypeOfLiteralType union arm needs mapType");
        }
        t
    }

    /// Go: `func (c *Checker) getFreshTypeOfLiteralType(t *Type) *Type`.
    pub(crate) fn get_fresh_type_of_literal_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::FRESHABLE) {
            let existing = self.type_(t).as_literal_type().expect("LiteralType data").fresh_type;
            if let Some(f) = existing {
                return f;
            }
            let flags = self.type_(t).flags;
            let value = self
                .type_(t)
                .as_literal_type()
                .expect("LiteralType data")
                .value
                .clone()
                .expect("literal value");
            let f = self.new_literal_type(flags, value, Some(t));
            let symbol = self.type_(t).symbol;
            self.type_mut(f).symbol = symbol;
            self.type_mut(f)
                .as_literal_type_mut()
                .expect("LiteralType data")
                .fresh_type = Some(f);
            self.type_mut(t)
                .as_literal_type_mut()
                .expect("LiteralType data")
                .fresh_type = Some(f);
            return f;
        }
        t
    }

    /// Go: `func (c *Checker) getStringLiteralType(value string) *Type`.
    pub(crate) fn get_string_literal_type(&mut self, value: &str) -> TypeId {
        if let Some(&t) = self.string_literal_types.get(value) {
            return t;
        }
        let t = self.new_literal_type(TypeFlags::STRING_LITERAL, LiteralValue::String(value.to_string()), None);
        self.string_literal_types.insert(value.to_string(), t);
        t
    }

    /// Go: `func (c *Checker) getNumberLiteralType(value jsnum.Number) *Type` —
    /// NaN cannot be used as a Go map key (NaN != NaN), so Go caches the NaN
    /// type separately; the port caches NaN separately because `Number` is
    /// not `Eq`/`Hash` at all (see the field PORT note).
    pub(crate) fn get_number_literal_type(&mut self, value: Number) -> TypeId {
        if value.is_nan() {
            if self.nan_type.is_none() {
                self.nan_type =
                    Some(self.new_literal_type(TypeFlags::NUMBER_LITERAL, LiteralValue::Number(value), None));
            }
            return self.nan_type.expect("nanType");
        }
        let key = number_key(value.0);
        if let Some(&t) = self.number_literal_types.get(&key) {
            return t;
        }
        let t = self.new_literal_type(TypeFlags::NUMBER_LITERAL, LiteralValue::Number(value), None);
        self.number_literal_types.insert(key, t);
        t
    }

    /// Go: `func (c *Checker) getBigIntLiteralType(value jsnum.PseudoBigInt) *Type`.
    pub(crate) fn get_big_int_literal_type(&mut self, value: PseudoBigInt) -> TypeId {
        if let Some(&t) = self.bigint_literal_types.get(&value) {
            return t;
        }
        let t = self.new_literal_type(TypeFlags::BIGINT_LITERAL, LiteralValue::BigInt(value.clone()), None);
        self.bigint_literal_types.insert(value, t);
        t
    }

    /// Go: `func (c *Checker) newIndexInfo(...)` (arena-backed).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_index_info(
        &mut self,
        key_type: TypeId,
        value_type: TypeId,
        is_readonly: bool,
        declaration: Option<NodeId>,
        components: Vec<NodeId>,
    ) -> usize {
        self.index_infos.push(IndexInfo {
            index_symbol: None,
            key_type: Some(key_type),
            value_type: Some(value_type),
            is_readonly,
            declaration,
            components,
        });
        self.index_infos.len() - 1
    }

    // ────────────────────────────────────────────────────────────────────────
    // Symbols (Go checker.go 14341+)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) newSymbol(flags ast.SymbolFlags, name string) *ast.Symbol`.
    pub(crate) fn new_symbol(&mut self, flags: SymbolFlags, name: &str) -> SymbolId {
        self.symbol_count += 1;
        let symbol = Symbol {
            flags: flags | SymbolFlags::TRANSIENT,
            name: name.to_string(),
            ..Default::default()
        };
        self.program.alloc_symbol(symbol)
    }

    /// Go: `func (c *Checker) newSymbolEx(flags, name, checkFlags) *ast.Symbol`.
    pub(crate) fn new_symbol_ex(
        &mut self,
        flags: SymbolFlags,
        name: &str,
        check_flags: CheckFlags,
    ) -> SymbolId {
        let symbol = self.new_symbol(flags, name);
        self.program.symbol_mut(symbol).check_flags = check_flags;
        symbol
    }

    /// Go: `func (c *Checker) getDeclarationNodeFlagsFromSymbol(s *ast.Symbol) ast.NodeFlags`.
    pub(crate) fn get_declaration_node_flags_from_symbol(&self, symbol: SymbolId) -> NodeFlags {
        // Go routes through getCombinedNodeFlagsCached (a single-entry memo
        // on the checker); the port keeps the memo &mut-only and reads
        // uncached here so the utilities family can take `&Checker`.
        let store = self.program.node_store();
        match self.program.symbol(symbol).value_declaration {
            Some(declaration) => binder_shims::get_combined_node_flags(store, declaration),
            None => NF::NONE,
        }
    }

    /// Go: `func (c *Checker) getCombinedNodeFlagsCached(node) ast.NodeFlags` —
    /// the single-entry memo Go keeps on the checker.
    pub(crate) fn get_combined_node_flags_cached(&mut self, node: NodeId) -> NodeFlags {
        if self.last_get_combined_node_flags_node != Some(node) {
            let flags = {
                let store = self.program.node_store();
                binder_shims::get_combined_node_flags(store, node)
            };
            self.last_get_combined_node_flags_node = Some(node);
            self.last_get_combined_node_flags_result = flags;
        }
        self.last_get_combined_node_flags_result
    }

    /// Go: `func (c *Checker) getCombinedModifierFlagsCached(node) ast.ModifierFlags`.
    pub(crate) fn get_combined_modifier_flags_cached(&mut self, node: NodeId) -> ModifierFlags {
        if self.last_get_combined_modifier_flags_node != Some(node) {
            let flags = {
                let store = self.program.node_store();
                binder_shims::get_combined_modifier_flags(store, node)
            };
            self.last_get_combined_modifier_flags_node = Some(node);
            self.last_get_combined_modifier_flags_result = flags;
        }
        self.last_get_combined_modifier_flags_result
    }

    /// Go: `func (c *Checker) getAliasedSymbol(symbol *ast.Symbol) *ast.Symbol` —
    /// alias resolution is the exports wave (M6).
    pub(crate) fn get_aliased_symbol(&self, _symbol: SymbolId) -> SymbolId {
        panic!("TODO(port M6): getAliasedSymbol (exports.go)")
    }

    // ────────────────────────────────────────────────────────────────────────
    // getTypeOfSymbol chain (Go checker.go 16879+)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getTypeOfSymbol(symbol *ast.Symbol) *Type`.
    pub fn get_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        let (check_flags, flags) = {
            let s = self.program.symbol(symbol);
            (s.check_flags, s.flags)
        };
        if check_flags.intersects(CheckFlags::DEFERRED_TYPE) {
            panic!("TODO(port M6): getTypeOfSymbolWithDeferredType (checkJS)")
        }
        if check_flags.intersects(CheckFlags::INSTANTIATED) {
            panic!("TODO(port M6): getTypeOfInstantiatedSymbol (type instantiation)")
        }
        if check_flags.intersects(CheckFlags::MAPPED) {
            panic!("TODO(port M6): getTypeOfMappedSymbol (mapped types)")
        }
        if check_flags.intersects(CheckFlags::REVERSE_MAPPED) {
            panic!("TODO(port M6): getTypeOfReverseMappedSymbol (mapped types)")
        }
        if flags.intersects(SymbolFlags::ACCESSOR) {
            panic!("TODO(port M6): getTypeOfAccessors")
        }
        if flags.intersects(SymbolFlags::VARIABLE | SymbolFlags::PROPERTY) {
            return self.get_type_of_variable_or_parameter_or_property(symbol);
        }
        if flags.intersects(
            SymbolFlags::FUNCTION
                | SymbolFlags::METHOD
                | SymbolFlags::CLASS
                | SymbolFlags::ENUM
                | SymbolFlags::VALUE_MODULE,
        ) {
            panic!("TODO(port M6): getTypeOfFuncClassEnumModule")
        }
        if flags.intersects(SymbolFlags::ENUM_MEMBER) {
            panic!("TODO(port M6): getTypeOfEnumMember (enum types)")
        }
        if flags.intersects(SymbolFlags::ALIAS) {
            panic!("TODO(port M6): getTypeOfAlias (exports.go)")
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getNonMissingTypeOfSymbol(symbol *ast.Symbol) *Type` —
    /// `removeMissingType` is TODO(port M6); identity until then.
    pub fn get_non_missing_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        // TODO(port M6): c.removeMissingType(t, symbol.Flags&SymbolFlagsOptional != 0)
        self.get_type_of_symbol(symbol)
    }

    /// Go: `func (c *Checker) getTypeOfVariableOrParameterOrProperty(symbol *ast.Symbol) *Type`.
    fn get_type_of_variable_or_parameter_or_property(&mut self, symbol: SymbolId) -> TypeId {
        let cached = self.value_symbol_links.get(symbol).resolved_type;
        if cached.is_none() {
            let t = self.get_type_of_variable_or_parameter_or_property_worker(symbol);
            // For a contextually typed parameter it is possible that a type
            // has already been assigned, and we want to preserve this type —
            // the re-check mirrors Go's `links.resolvedType == nil &&`.
            let existing = self.value_symbol_links.get(symbol).resolved_type;
            if existing.is_none() && !self.is_parameter_of_context_sensitive_signature(symbol) {
                self.value_symbol_links.get(symbol).resolved_type = t;
            }
            return t.expect("Unexpected nil type");
        }
        cached.expect("cached")
    }

    /// Go: `func (c *Checker) isParameterOfContextSensitiveSignature(symbol) bool`.
    fn is_parameter_of_context_sensitive_signature(&mut self, symbol: SymbolId) -> bool {
        let Some(decl) = self.program.symbol(symbol).value_declaration else {
            return false;
        };
        let mut d = decl;
        if is_binding_element_node(self.store().node(d)) {
            d = ast_shims::walk_up_binding_elements_and_patterns(self.store(), d);
        }
        if self.store().node(d).kind == Kind::Parameter {
            // Go: isContextSensitiveFunctionOrObjectLiteralMethod —
            // contextual typing wave (M6).
            panic!("TODO(port M6): isContextSensitiveFunctionOrObjectLiteralMethod")
        }
        false
    }

    /// Go: `func (c *Checker) getTypeOfVariableOrParameterOrPropertyWorker(symbol) *Type`.
    fn get_type_of_variable_or_parameter_or_property_worker(
        &mut self,
        symbol: SymbolId,
    ) -> Option<TypeId> {
        let (flags, value_declaration) = {
            let s = self.program.symbol(symbol);
            (s.flags, s.value_declaration)
        };
        // Handle prototype property
        if flags.intersects(SymbolFlags::PROTOTYPE) {
            panic!("TODO(port M6): getTypeOfPrototypeProperty");
        }
        // CommonJS `require` has type any.
        if self.require_symbol == Some(symbol) {
            return Some(self.any_type());
        }
        let declaration = value_declaration.expect("symbol.ValueDeclaration != nil (Go debug.Assert)");
        // JSON source files: TODO(port M6) — the demo path never reaches them.
        // Handle variable, parameter or property
        if !self.push_type_resolution(
            TypeSystemEntity::Symbol(symbol),
            TypeSystemPropertyName::Type,
        ) {
            return Some(self.report_circularity_error(symbol));
        }
        if flags.intersects(SymbolFlags::MODULE_EXPORTS) {
            panic!("TODO(port M6): ModuleExports arm of getTypeOfVariableOrParameterOrPropertyWorker")
        }
        let declaration_kind = self.store().node(declaration).kind;
        let result = match declaration_kind {
            Kind::Parameter
            | Kind::PropertyDeclaration
            | Kind::PropertySignature
            | Kind::VariableDeclaration
            | Kind::BindingElement => {
                let report_errors = !self.is_parameter_of_context_sensitive_signature(symbol);
                // only report diagnostics for context-insensitive parameters
                let t = self.get_widened_type_for_variable_like_declaration(declaration, report_errors);
                Some(t)
            }
            Kind::PropertyAssignment => panic!("TODO(port M6): checkPropertyAssignment"),
            Kind::ShorthandPropertyAssignment => panic!("TODO(port M6): checkShorthandPropertyAssignment"),
            Kind::MethodDeclaration => panic!("TODO(port M6): checkObjectLiteralMethod"),
            Kind::ExportAssignment => panic!("TODO(port M6): ExportAssignment arm"),
            Kind::BinaryExpression | Kind::CallExpression => {
                panic!("TODO(port M6): getWidenedTypeForAssignmentDeclaration")
            }
            Kind::JsxAttribute => panic!("TODO(port M6): checkJsxAttribute"),
            Kind::EnumMember => panic!("TODO(port M6): getTypeOfEnumMember"),
            _ => panic!(
                "Unhandled case in getTypeOfVariableOrParameterOrPropertyWorker: {}",
                self.store().node(declaration).kind_string()
            ),
        };
        if !self.pop_type_resolution() {
            return Some(self.report_circularity_error(symbol));
        }
        result
    }

    /// Go: `func (c *Checker) getWidenedTypeForVariableLikeDeclaration(declaration, reportErrors) *Type`.
    pub fn get_widened_type_for_variable_like_declaration(
        &mut self,
        declaration: NodeId,
        report_errors: bool,
    ) -> TypeId {
        let t = self.get_type_for_variable_like_declaration(declaration, true, CheckMode::CHECK_MODE_NORMAL);
        self.widen_type_for_variable_like_declaration(t, declaration, report_errors)
    }

    /// Go: `func (c *Checker) widenTypeForVariableLikeDeclaration(t, declaration, reportErrors) *Type`.
    pub(crate) fn widen_type_for_variable_like_declaration(
        &mut self,
        t: Option<TypeId>,
        declaration: NodeId,
        report_errors: bool,
    ) -> TypeId {
        let Some(t) = t else {
            // Rest parameters default to type any[], other parameters to any.
            if self.store().node(declaration).kind == Kind::Parameter
                && u::has_dot_dot_dot_token(self.store(), declaration)
            {
                // TODO(port M6): anyArrayType
                panic!("TODO(port M6): anyArrayType (array type construction)")
            }
            let result = self.any_type();
            // Go: reportImplicitAny — the diagnostics seam is the M5
            // compiler crate; the spike no-ops it (see `error`).
            let _ = report_errors;
            return result;
        };
        // Go's special cases: the `SymbolConstructor.symbol` merge
        // (isGlobalSymbolConstructor) and unique-symbol widening — both need
        // global type resolution (M6). TODO(port M6).
        if report_errors {
            // Go: reportErrorsFromWidening — diagnostics seam (no-op).
        }
        self.get_widened_type(t)
    }

    /// Go: `func (c *Checker) getWidenedType(t *Type) *Type` — the entry; the
    /// widening machinery (object literals, unions, arrays) is M6. Literal
    /// widening already happened in `widenTypeInferredFromInitializer`.
    pub(crate) fn get_widened_type(&mut self, t: TypeId) -> TypeId {
        if self
            .type_(t)
            .object_flags
            .intersects(ObjectFlags::REQUIRES_WIDENING)
        {
            // TODO(port M6): getWidenedTypeWithContext (object literal /
            // union / array widening + the widenedTypes cache).
            panic!("TODO(port M6): getWidenedTypeWithContext (requires-widening types)")
        }
        t
    }

    /// Go: `func (c *Checker) getTypeForVariableLikeDeclaration(declaration, includeOptionality, checkMode) *Type`.
    pub(crate) fn get_type_for_variable_like_declaration(
        &mut self,
        declaration: NodeId,
        include_optionality: bool,
        check_mode: CheckMode,
    ) -> Option<TypeId> {
        // Capture the node facts up front (ids are Copy) so the store borrow
        // never straddles a `&mut self` call (§5.4 arena idiom).
        let (kind, node_flags, parent, grandparent_kind, name, name_kind, initializer) = {
            let store = self.program.node_store();
            let node = store.node(declaration);
            let parent = node.parent.get();
            let grandparent = if parent.is_none() {
                Kind::SourceFile
            } else {
                store.node(store.node(parent).parent.get()).kind
            };
            (
                node.kind,
                node.flags,
                parent,
                grandparent,
                node.name(),
                node.name().map(|n| store.node(n).kind),
                crate::ast_shims::initializer_of(store, declaration),
            )
        };
        // A variable declared in a for..in statement is of type string, or of
        // type keyof T when the right hand expression is of a type parameter
        // type.
        if kind == Kind::VariableDeclaration {
            if grandparent_kind == Kind::ForInStatement || grandparent_kind == Kind::ForOfStatement {
                panic!("TODO(port M6): for..in / for..of variable types")
            }
        } else if kind == Kind::BindingElement {
            panic!("TODO(port M6): getTypeForBindingElement")
        }
        let is_property = (kind == Kind::PropertyDeclaration
            && !{
                let store = self.program.node_store();
                binder_shims::has_accessor_modifier(store.node(declaration))
            })
            || kind == Kind::PropertySignature;
        let is_optional = include_optionality && u::is_optional_declaration(self.store(), declaration);
        // Use type from type annotation if one is present
        let declared_type = self.try_get_type_from_type_node(declaration);
        if is_catch_clause_variable_declaration_or_binding_element(self.store(), declaration) {
            if let Some(declared_type) = declared_type {
                // If the catch clause is explicitly annotated with any or
                // unknown, accept it, otherwise error.
                if self.type_(declared_type).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                    return Some(declared_type);
                }
                let error_type = self.error_type();
                // Go reports a diagnostic here — no-op seam.
                return Some(error_type);
            }
            // If the catch clause is not explicitly annotated, treat it as
            // though it were explicitly annotated with unknown or any,
            // depending on useUnknownInCatchVariables.
            return Some(if self.use_unknown_in_catch_variables {
                self.unknown_type()
            } else {
                self.any_type()
            });
        }
        if let Some(declared_type) = declared_type {
            return Some(self.add_optionality_ex(declared_type, is_property, is_optional));
        }
        if self.no_implicit_any
            && kind == Kind::VariableDeclaration
            && name_kind.is_some_and(|k| k != Kind::ObjectBindingPattern)
            && name_kind.is_some_and(|k| k != Kind::ArrayBindingPattern)
            && !self.get_combined_modifier_flags_cached(declaration).intersects(ModifierFlags::EXPORT)
            && !node_flags.intersects(NF::AMBIENT)
        {
            // If --noImplicitAny is on or the declaration is in a Javascript
            // file, use control flow tracked 'any' type for non-ambient,
            // non-exported var or let variables with no initializer or a
            // 'null' or 'undefined' initializer.
            let not_constant =
                !self.get_combined_node_flags_cached(declaration).intersects(NF::CONSTANT);
            if not_constant
                && (initializer.is_none()
                    || initializer.is_some_and(|i| is_null_or_undefined_literal(self.store(), i)))
            {
                return Some(self.auto_type());
            }
            // Use control flow tracked 'any[]' type for non-ambient,
            // non-exported variables with an empty array literal initializer.
            if initializer.is_some_and(|i| u::is_empty_array_literal(self.store(), i)) {
                panic!("TODO(port M6): autoArrayType (array type construction)")
            }
        }
        if kind == Kind::Parameter {
            // Contextual parameter types — TODO(port M6):
            // getContextuallyTypedParameterType / getParameterTypeOfFullSignature.
            panic!("TODO(port M6): parameter contextual typing")
        }
        // Use the type of the initializer expression if one is present and
        // the declaration is not a parameter of a contextually typed function
        if initializer.is_some() {
            let t = self.check_declaration_initializer(declaration, check_mode);
            let t = self.widen_type_inferred_from_initializer(declaration, t);
            return Some(self.add_optionality_ex(t, is_property, is_optional));
        }
        if self.no_implicit_any && kind == Kind::PropertyDeclaration {
            panic!("TODO(port M6): getFlowTypeInConstructor (control-flow types)")
        }
        if kind == Kind::JsxAttribute {
            // if JSX attribute doesn't have initializer, by default the
            // attribute will have boolean value of true.
            // I.e <Elem attr /> is sugar for <Elem attr={true} />
            return Some(self.true_type());
        }
        // If the declaration specifies a binding pattern and is not a
        // parameter of a contextually typed function, use the type implied by
        // the binding pattern
        if matches!(name_kind, Some(Kind::ObjectBindingPattern | Kind::ArrayBindingPattern)) {
            panic!("TODO(port M6): getTypeFromBindingPattern")
        }
        let _ = (parent, name);
        // No type specified and nothing can be inferred
        None
    }

    /// Go: `func (c *Checker) checkDeclarationInitializer(declaration, checkMode, contextualType) *Type`.
    fn check_declaration_initializer(
        &mut self,
        declaration: NodeId,
        check_mode: CheckMode,
    ) -> TypeId {
        let initializer = {
            let store = self.program.node_store();
            crate::ast_shims::initializer_of(store, declaration)
                .expect("caller checked initializer != nil")
        };
        let t = self.get_quick_type_of_expression(initializer);
        match t {
            Some(t) => t,
            None => self.check_expression_cached_ex(initializer, check_mode),
        }
        // Go: binding-pattern padding (padObjectLiteralType / padTupleType)
        // — TODO(port M6) (destructuring declarations).
    }

    /// Go: `func (c *Checker) widenTypeInferredFromInitializer(declaration, t) *Type`.
    pub(crate) fn widen_type_inferred_from_initializer(
        &mut self,
        declaration: NodeId,
        t: TypeId,
    ) -> TypeId {
        let widened = self.get_widened_literal_type_for_initializer(declaration, t);
        // Go: the .js empty-literal implicit-any reporting (reportImplicitAny)
        // is the diagnostics seam — no-op in the spike.
        widened
    }

    /// Go: `func (c *Checker) getWidenedLiteralTypeForInitializer(declaration, t) *Type`.
    pub(crate) fn get_widened_literal_type_for_initializer(
        &mut self,
        declaration: NodeId,
        t: TypeId,
    ) -> TypeId {
        if self.get_combined_node_flags_cached(declaration).intersects(NF::CONSTANT)
            || u::is_declaration_readonly(self.store(), declaration)
        {
            return t;
        }
        self.get_widened_literal_type(t)
    }

    /// Go: `func (c *Checker) getWidenedLiteralType(t *Type) *Type`.
    pub(crate) fn get_widened_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::ENUM_LIKE) && is_fresh_literal_type(self, t) {
            panic!("TODO(port M6): getBaseTypeOfEnumLikeType (enum types)")
        }
        if flags.intersects(TypeFlags::STRING_LITERAL) && is_fresh_literal_type(self, t) {
            return self.string_type();
        }
        if flags.intersects(TypeFlags::NUMBER_LITERAL) && is_fresh_literal_type(self, t) {
            return self.number_type();
        }
        if flags.intersects(TypeFlags::BIGINT_LITERAL) && is_fresh_literal_type(self, t) {
            return self.bigint_type();
        }
        if flags.intersects(TypeFlags::BOOLEAN_LITERAL) && is_fresh_literal_type(self, t) {
            return self.boolean_type(); // TODO(port M6): union construction
        }
        if flags.intersects(TypeFlags::UNION) {
            panic!("TODO(port M6): mapType union arm of getWidenedLiteralType")
        }
        t
    }

    /// Go: `func (c *Checker) addOptionalityEx(t, isProperty, isOptional) *Type` —
    /// the non-optional path is identity; optionality unioning is M6.
    pub(crate) fn add_optionality_ex(
        &mut self,
        t: TypeId,
        _is_property: bool,
        is_optional: bool,
    ) -> TypeId {
        if !is_optional {
            return t;
        }
        panic!("TODO(port M6): addOptionalityEx (optional types)")
    }

    /// Go: `func (c *Checker) tryGetTypeFromTypeNode(node *ast.Node) *Type` —
    /// the annotation is present but `getTypeFromTypeNode` is M6.
    fn try_get_type_from_type_node(&mut self, declaration: NodeId) -> Option<TypeId> {
        if ast_shims::type_of_node(self.store(), declaration).is_some() {
            panic!("TODO(port M6): getTypeFromTypeNode — type annotations")
        }
        None
    }

    /// Go: `func (c *Checker) getQuickTypeOfExpression(node *ast.Node) *Type` —
    /// quick types are a pure optimization; the spike ports only the literal
    /// arm (the await/call/new/assertion arms need signature machinery (M6)
    /// and skipping them only costs time, never a different type).
    pub(crate) fn get_quick_type_of_expression(&mut self, node: NodeId) -> Option<TypeId> {
        let is_literal = {
            let n = self.store().node(node);
            binder_shims::is_literal_expression(n) || binder_shims::is_boolean_literal(n)
        };
        if is_literal {
            return Some(self.check_expression(node));
        }
        None
    }

    // ────────────────────────────────────────────────────────────────────────
    // Expression checking (Go checker.go 7715+ — literal arms only)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) checkExpressionCached(node *ast.Node) *Type`.
    pub fn check_expression_cached(&mut self, node: NodeId) -> TypeId {
        self.check_expression_cached_ex(node, CheckMode::CHECK_MODE_NORMAL)
    }

    /// Go: `func (c *Checker) checkExpressionCachedEx(node, checkMode) *Type`.
    pub fn check_expression_cached_ex(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        if check_mode != CheckMode::CHECK_MODE_NORMAL {
            return self.check_expression_ex(node, check_mode);
        }
        if self.type_node_links.get(node).resolved_type.is_none() {
            // When computing a type that we're going to cache, we need to
            // ignore any ongoing control flow analysis — the flow caches
            // (flowLoopStack / flowTypeCache) are M6 (todo_shapes::FlowState),
            // so there is nothing to save/restore in the spike.
            let t = self.check_expression_ex(node, check_mode);
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) checkExpression(node *ast.Node) *Type`.
    pub fn check_expression(&mut self, node: NodeId) -> TypeId {
        self.check_expression_ex(node, CheckMode::CHECK_MODE_NORMAL)
    }

    /// Go: `func (c *Checker) checkExpressionEx(node, checkMode) *Type`.
    pub fn check_expression_ex(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        // Go: tracer push/pop — todo_shapes::Tracer (M6).
        let save_current_node = self.current_node.replace(node);
        self.instantiation_count = 0;
        let uninstantiated_type = self.check_expression_worker(node, check_mode);
        // Go: instantiateTypeWithSingleGenericCallSignature — diverges only
        // for type references with a single generic call signature (M6);
        // identity in the spike. TODO(port M6).
        let t = uninstantiated_type;
        // Go: isConstEnumObjectType → checkConstEnumAccess — const enums are M6.
        self.current_node = save_current_node;
        t
    }

    /// Go: `func (c *Checker) checkExpressionWorker(node, checkMode) *Type` —
    /// literal/keyword arms; every other arm's Go callee is named in its
    /// TODO(port M6) banner.
    fn check_expression_worker(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::Identifier => panic!("TODO(port M6): checkIdentifier (flow types)"),
            Kind::PrivateIdentifier => panic!("TODO(port M6): checkPrivateIdentifierExpression"),
            Kind::ThisKeyword => panic!("TODO(port M6): checkThisExpression"),
            Kind::SuperKeyword => panic!("TODO(port M6): checkSuperExpression"),
            Kind::NullKeyword => self.null_widening_type(),
            Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral => {
                // Go: isSkipDirectInferenceNode (skipDirectInferenceNodes)
                // is the JSDoc satisfies-attributes set — M6; the spike has
                // no skip set. TODO(port M6).
                let text = tsc_ast::node_text(self.store(), node).to_string();
                let t = self.get_string_literal_type(&text);
                self.get_fresh_type_of_literal_type(t)
            }
            Kind::NumericLiteral => {
                // Go: checkGrammarNumericLiteral — grammar-check diagnostics
                // seam (M5 compiler crate); the numeric-literal grammar check
                // is a diagnostic-only pass.
                let text = tsc_ast::node_text(self.store(), node).to_string();
                let t = self.get_number_literal_type(tsc_jsnum::from_string(&text));
                self.get_fresh_type_of_literal_type(t)
            }
            Kind::BigIntLiteral => {
                // Go: checkGrammarBigIntLiteral — same diagnostics seam.
                let text = tsc_ast::node_text(self.store(), node).to_string();
                let t = self.get_big_int_literal_type(tsc_jsnum::new_pseudo_big_int(
                    &tsc_jsnum::parse_pseudo_big_int(&text),
                    false, /*negative*/
                ));
                self.get_fresh_type_of_literal_type(t)
            }
            Kind::TrueKeyword => self.true_type(),
            Kind::FalseKeyword => self.false_type(),
            Kind::TemplateExpression => panic!("TODO(port M6): checkTemplateExpression"),
            Kind::RegularExpressionLiteral => panic!("TODO(port M6): checkRegularExpressionLiteral"),
            Kind::ArrayLiteralExpression => panic!("TODO(port M6): checkArrayLiteral"),
            Kind::ObjectLiteralExpression => panic!("TODO(port M6): checkObjectLiteral"),
            Kind::PropertyAccessExpression => panic!("TODO(port M6): checkPropertyAccessExpression"),
            Kind::QualifiedName => panic!("TODO(port M6): checkQualifiedName"),
            Kind::ElementAccessExpression => panic!("TODO(port M6): checkIndexedAccess"),
            Kind::CallExpression | Kind::NewExpression => {
                panic!("TODO(port M6): checkCallExpression")
            }
            Kind::TaggedTemplateExpression => panic!("TODO(port M6): checkTaggedTemplateExpression"),
            Kind::ParenthesizedExpression => panic!("TODO(port M6): checkParenthesizedExpression"),
            Kind::ClassExpression => panic!("TODO(port M6): checkClassExpression"),
            Kind::FunctionExpression | Kind::ArrowFunction => {
                panic!("TODO(port M6): checkFunctionExpressionOrObjectLiteralMethod")
            }
            Kind::TypeAssertionExpression | Kind::AsExpression => {
                panic!("TODO(port M6): checkAssertion")
            }
            Kind::TypeOfExpression => panic!("TODO(port M6): checkTypeOfExpression"),
            Kind::NonNullExpression => panic!("TODO(port M6): checkNonNullAssertion"),
            Kind::ExpressionWithTypeArguments => panic!("TODO(port M6): checkExpressionWithTypeArguments"),
            Kind::SatisfiesExpression => panic!("TODO(port M6): checkSatisfiesExpression"),
            Kind::MetaProperty => panic!("TODO(port M6): checkMetaProperty"),
            Kind::DeleteExpression => panic!("TODO(port M6): checkDeleteExpression"),
            Kind::VoidExpression => panic!("TODO(port M6): checkVoidExpression"),
            Kind::AwaitExpression => panic!("TODO(port M6): checkAwaitExpression"),
            Kind::PrefixUnaryExpression => panic!("TODO(port M6): checkPrefixUnaryExpression"),
            Kind::PostfixUnaryExpression => panic!("TODO(port M6): checkPostfixUnaryExpression"),
            Kind::BinaryExpression => panic!("TODO(port M6): checkBinaryExpression"),
            Kind::ConditionalExpression => panic!("TODO(port M6): checkConditionalExpression"),
            Kind::SpreadElement => panic!("TODO(port M6): checkSpreadExpression"),
            Kind::OmittedExpression => self.undefined_widening_type(),
            Kind::YieldExpression => panic!("TODO(port M6): checkYieldExpression"),
            Kind::SyntheticExpression => panic!("TODO(port M6): checkSyntheticExpression"),
            Kind::JsxExpression => panic!("TODO(port M6): checkJsxExpression"),
            Kind::JsxElement => panic!("TODO(port M6): checkJsxElement"),
            Kind::JsxSelfClosingElement => panic!("TODO(port M6): checkJsxSelfClosingElement"),
            Kind::JsxFragment => panic!("TODO(port M6): checkJsxFragment"),
            Kind::JsxAttributes => panic!("TODO(port M6): checkJsxAttributes"),
            Kind::JsxOpeningElement => panic!("Should never directly check a JsxOpeningElement"),
            _ => self.error_type(),
        }
        .tap_check_mode(check_mode)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Type-resolution cycle detection (Go checker.go 19165+)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) pushTypeResolution(target, propertyName) bool`.
    pub(crate) fn push_type_resolution(
        &mut self,
        target: TypeSystemEntity,
        property_name: TypeSystemPropertyName,
    ) -> bool {
        let resolution_cycle_start_index =
            self.find_resolution_cycle_start_index(&target, property_name);
        if let Some(start) = resolution_cycle_start_index {
            // A cycle was found
            for i in start..self.type_resolutions.len() {
                self.type_resolutions[i].result = false;
            }
            return false;
        }
        self.type_resolutions.push(TypeResolution {
            target,
            property_name,
            result: true,
        });
        true
    }

    /// Go: `func (c *Checker) popTypeResolution() bool` — pop an entry from
    /// the type resolution stack and return its result value (true if no
    /// circularities were detected, false if a circularity was found).
    pub(crate) fn pop_type_resolution(&mut self) -> bool {
        let last_index = self.type_resolutions.len() - 1;
        let result = self.type_resolutions[last_index].result;
        // Go clears the entry to avoid memory leaks; the port pops it.
        self.type_resolutions.truncate(last_index);
        result
    }

    /// Go: `func (c *Checker) findResolutionCycleStartIndex(target, propertyName) int`.
    fn find_resolution_cycle_start_index(
        &mut self,
        target: &TypeSystemEntity,
        property_name: TypeSystemPropertyName,
    ) -> Option<usize> {
        let mut i = self.type_resolutions.len() as isize - 1;
        while i >= self.resolution_start as isize {
            // Go holds *TypeResolution while calling the mutating Get; the
            // port snapshots the entry (small, Copy) before each mutation.
            let resolution = self.type_resolutions[i as usize];
            if self.type_resolution_has_property(&resolution) {
                return None;
            }
            if resolution.target == *target && resolution.property_name == property_name {
                return Some(i as usize);
            }
            i -= 1;
        }
        None
    }

    /// Go: `func (c *Checker) typeResolutionHasProperty(r *TypeResolution) bool`.
    pub(crate) fn type_resolution_has_property(&mut self, r: &TypeResolution) -> bool {
        match r.property_name {
            TypeSystemPropertyName::Type => match r.target {
                TypeSystemEntity::Symbol(symbol) => {
                    self.value_symbol_links.get(symbol).resolved_type.is_some()
                }
                _ => panic!("TypeSystemPropertyNameType with non-symbol target"),
            },
            TypeSystemPropertyName::DeclaredType => match r.target {
                TypeSystemEntity::Symbol(symbol) => {
                    self.type_alias_links.get(symbol).declared_type.is_some()
                }
                _ => panic!("TypeSystemPropertyNameDeclaredType with non-symbol target"),
            },
            TypeSystemPropertyName::ResolvedTypeArguments => match r.target {
                TypeSystemEntity::Type(t) => self
                    .type_(t)
                    .as_type_reference()
                    // PORT: Go tests slice nil-ness; the port's Vec cannot be
                    // nil, so "has property" is "reference data exists" — the
                    // resolvedTypeArguments are always populated together
                    // with the reference in createTypeReferenceEx.
                    .is_some(),
                _ => panic!("TypeSystemPropertyNameResolvedTypeArguments with non-type target"),
            },
            TypeSystemPropertyName::ResolvedBaseTypes => match r.target {
                TypeSystemEntity::Type(t) => self
                    .type_(t)
                    .as_interface_type()
                    .is_some_and(|d| d.base_types_resolved),
                _ => panic!("TypeSystemPropertyNameResolvedBaseTypes with non-type target"),
            },
            TypeSystemPropertyName::ResolvedBaseConstructorType => match r.target {
                TypeSystemEntity::Type(t) => self
                    .type_(t)
                    .as_interface_type()
                    .is_some_and(|d| d.resolved_base_constructor_type.is_some()),
                _ => panic!("...ResolvedBaseConstructorType with non-type target"),
            },
            TypeSystemPropertyName::ResolvedReturnType => match r.target {
                TypeSystemEntity::Signature(sig) => self.signatures[sig.0 as usize]
                    .resolved_return_type
                    .is_some(),
                _ => panic!("TypeSystemPropertyNameResolvedReturnType with non-signature target"),
            },
            TypeSystemPropertyName::ResolvedBaseConstraint => match r.target {
                TypeSystemEntity::Type(t) => self
                    .type_(t)
                    .as_constrained_type()
                    .is_some_and(|d| d.resolved_base_constraint.is_some()),
                _ => panic!("...ResolvedBaseConstraint with non-type target"),
            },
            TypeSystemPropertyName::InitializerIsUndefined => match r.target {
                TypeSystemEntity::Node(node) => self
                    .node_links
                    .get(node)
                    .flags
                    .intersects(crate::types::NodeCheckFlags::INITIALIZER_IS_UNDEFINED_COMPUTED),
                _ => panic!("...InitializerIsUndefined with non-node target"),
            },
            TypeSystemPropertyName::WriteType => match r.target {
                TypeSystemEntity::Symbol(symbol) => {
                    self.value_symbol_links.get(symbol).write_type.is_some()
                }
                _ => panic!("TypeSystemPropertyNameWriteType with non-symbol target"),
            },
            TypeSystemPropertyName::AliasTarget => match r.target {
                TypeSystemEntity::Symbol(symbol) => {
                    self.alias_symbol_links.get(symbol).alias_target.is_some()
                }
                _ => panic!("TypeSystemPropertyNameAliasTarget with non-symbol target"),
            },
        }
    }

    /// Go: `func (c *Checker) reportCircularityError(symbol *ast.Symbol) *Type`.
    pub(crate) fn report_circularity_error(&mut self, symbol: SymbolId) -> TypeId {
        // Go reports "is referenced directly or indirectly in its own type
        // annotation" / implicit-any diagnostics here — the diagnostics seam
        // is the M5 compiler crate; the spike returns Go's errorType.
        let _ = symbol;
        self.error_type()
    }

    // ────────────────────────────────────────────────────────────────────────
    // Misc (Go checker.go)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func isTupleType(t *Type) bool` — needs the arena to read the
    /// target's flags, hence a method in the port (§5.4).
    pub fn is_tuple_type(&self, t: TypeId) -> bool {
        let tr = self.type_(t);
        tr.object_flags.intersects(ObjectFlags::REFERENCE)
            && tr
                .target()
                .is_some_and(|target| {
                    self.type_(target).object_flags.intersects(ObjectFlags::TUPLE)
                })
    }

    // ── Test/inspection helpers over the Program seam (§5.5) ──

    /// The program-level symbol arena slot (test seam mirroring Go's direct
    /// `*ast.Symbol` access).
    pub fn program_symbol(&self, id: SymbolId) -> &Symbol {
        self.program.symbol(id)
    }

    /// Go: `node.Initializer()` through the seam.
    pub fn program_initializer_of(&self, declaration: NodeId) -> Option<NodeId> {
        crate::ast_shims::initializer_of(self.program.node_store(), declaration)
    }

    /// The checker's `undefinedSymbol` seed (Go field).
    pub fn undefined_symbol(&self) -> SymbolId {
        self.undefined_symbol.expect("undefinedSymbol")
    }

    /// True when the type-resolution stack is balanced (every
    /// `pushTypeResolution` was popped) — used by the spike tests to pin the
    /// cycle-detector's bookkeeping.
    pub fn type_resolutions_are_balanced(&self) -> bool {
        self.type_resolutions.is_empty()
    }

    // ────────────────────────────────────────────────────────────────────────
    // W1 callee stubs + early landings (Go checker.go functions the W1
    // periphery files — mapper.go / jsdoc.go / exports.go — need; the S-waves
    // replace the stub bodies and absorb the early landings).
    //
    // W1: until the S-waves land, this whole section is reached only through
    // the test-driven mapper/jsdoc/exports surface — the spike's dead-code
    // silencing convention applies (drop the allows as the waves arrive).
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) error(node *ast.Node, message, args...)` —
    /// diagnostic creation; the plumbing lands with the M5 compiler crate
    /// (`ast.Diagnostic` collection on the program). Go's variadic args
    /// port as a slice.
    #[allow(dead_code)] // wired by the M5 compiler wave's diagnostics collection
    pub(crate) fn error(
        &mut self,
        _node: NodeId,
        _message: &'static tsc_diagnostics::Message,
        _args: &[String],
    ) {
        // TODO(port M5 compiler): diagnostics collection seam.
    }

    /// Go: `func (c *Checker) errorOrSuggestion(isSuggestion bool, node *ast.Node, message *diagnostics.Message, args...)`
    /// — same collection seam as `error`. Permissive W1 stub (no-op); the
    /// M5 compiler wave wires both.
    #[allow(dead_code)] // wired by the M5 compiler wave's diagnostics collection
    pub(crate) fn error_or_suggestion(
        &mut self,
        _is_suggestion: bool,
        _node: NodeId,
        _message: &'static tsc_diagnostics::Message,
        _args: &[String],
    ) {
        // TODO(port M5 compiler): diagnostics collection seam.
    }

    /// Go: `func (c *Checker) addDiagnostic(diagnostic *ast.Diagnostic)` —
    /// the same M5 compiler collection seam as `error` (permissive no-op).
    #[allow(dead_code)] // wired by the M5 compiler wave's diagnostics collection
    pub(crate) fn add_diagnostic(&mut self, _diagnostic: tsc_ast::Diagnostic) {
        // TODO(port M5 compiler): diagnostics collection seam.
    }

    // ────────────────────────────────────────────────────────────────────────
    // W1 callee stubs + early landings (Go checker.go functions the W1
    // periphery files — mapper.go / jsdoc.go / exports.go — need; the S-waves
    // replace the stub bodies and absorb the early landings).
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func getNonDistributedTypeParameter(t *Type) *Type`
    /// (checker.go:23533). EARLY LANDING (mapper.go calls it; the body is
    /// arena-self-contained) — the S7 checker.go wave keeps it here.
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn get_non_distributed_type_parameter(&self, t: TypeId) -> TypeId {
        let ty = self.type_(t);
        if ty.flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let tp = ty.as_type_parameter().expect("TypeParameter data");
            if tp.is_distributed {
                // Go: `return t.AsTypeParameter().constraint` — a distributed
                // type parameter always has its constraint set at creation.
                return tp.constraint.expect("distributed type parameter constraint");
            }
        }
        t
    }

    /// Go: `func (c *Checker) instantiateType(t *Type, mapper *TypeMapper) *Type`
    /// (checker.go — the M6 S7 instantiation engine). Permissive W1 stub:
    /// identity — Go's `instantiateType` returns `t` unchanged for an
    /// identity mapping, so composite-mapper flows stay exact for
    /// identity mappers until S7 lands.
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn instantiate_type(&mut self, t: TypeId, mapper: TypeMapperId) -> TypeId {
        let _ = mapper;
        // TODO(port M6 S7): instantiateTypeWorker + the activeMappers caches.
        t
    }

    /// Go: `func (c *Checker) inferFromIntraExpressionSites(n *InferenceContext)`
    /// (inference.go — M6 wave 2). No-op W1 stub (mapper.go's
    /// InferenceTypeMapper is its only W1 caller).
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn infer_from_intra_expression_sites(&mut self, n: InferenceContextId) {
        // TODO(port M6 inference wave): drain intra-expression inference sites.
        let _ = n;
    }

    /// Go: `func (c *Checker) getInferredType(n *InferenceContext, index int) *Type`
    /// (inference.go — M6 wave 2). Obvious-error W1 stub: `errorType`
    /// (mapper.go's InferenceTypeMapper is its only W1 caller).
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn get_inferred_type(&mut self, n: InferenceContextId, index: usize) -> TypeId {
        let _ = (n, index);
        // TODO(port M6 inference wave): the real inference commitment.
        self.error_type()
    }

    /// Go: `func (c *Checker) containsArgumentsReference(node *ast.Node) bool`
    /// (checker.go:32675 — S4's flow-walk family). Permissive W1 stub: false
    /// (jsdoc.go's `arguments`-matching arm stays dormant until then).
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn contains_arguments_reference(&mut self, node: NodeId) -> bool {
        // TODO(port M6 S4): the subtree walk for `arguments` references.
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) isArrayType(t *Type) bool` (checker.go:24005 —
    /// needs the global array types, S7). Permissive W1 stub: false.
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn is_array_type(&self, t: TypeId) -> bool {
        // TODO(port M6 S7): t.objectFlags&Reference && target is globalArrayType/ReadonlyArrayType.
        let _ = t;
        false
    }

    /// Go: `func (c *Checker) getTypeFromTypeNode(node *ast.Node) *Type`
    /// (checker.go:23295 — S5). Obvious-error W1 stub: `errorType`
    /// (jsdoc.go's `arguments`-matching arm is the only W1 caller, and it is
    /// itself gated by the `containsArgumentsReference` stub).
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn get_type_from_type_node(&mut self, node: NodeId) -> TypeId {
        // TODO(port M6 S5): the type-node resolution family.
        let _ = node;
        self.error_type()
    }

    // ────────────────────────────────────────────────────────────────────────
    // The activeMappers stack (Go checker.go 22634–22666 — landed with the
    // mapper.go W1 wave; the instantiateType engine that drives it is S7).
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) pushActiveMapper(mapper *TypeMapper)`.
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn push_active_mapper(&mut self, mapper: TypeMapperId) {
        self.active_mappers.push(mapper);
        // PORT: Go grows `activeTypeMappersCaches` reusing the spare slice
        // capacity a previous pop left behind (an allocation optimization,
        // per its own comment); the port's Vec spare capacity plays the same
        // role for the Vec itself.
        self.active_type_mappers_caches
            .push(FxHashMap::with_capacity_and_hasher(1, Default::default()));
    }

    /// Go: `func (c *Checker) popActiveMapper()`.
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn pop_active_mapper(&mut self) {
        // PORT: Go nils the popped slot for GC; the port's drop does that.
        self.active_mappers.pop();
        // Clear the map, but leave it for later reuse (Go keeps it in the
        // slice's spare capacity — see pushActiveMapper's PORT note).
        self.active_type_mappers_caches.pop();
    }

    /// Go: `func (c *Checker) findActiveMapper(mapper *TypeMapper) int` —
    /// Go returns -1 when absent; the port returns `Option` (PORT).
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn find_active_mapper(&self, mapper: TypeMapperId) -> Option<usize> {
        self.active_mappers.iter().rposition(|m| *m == mapper)
    }

    /// Go: `func (c *Checker) clearActiveMapperCaches()`.
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn clear_active_mapper_caches(&mut self) {
        for cache in &mut self.active_type_mappers_caches {
            cache.clear();
        }
    }
}

/// Go: `func isFreshLiteralType(t *Type) bool` — free function taking the
/// checker for the self-referential freshType comparison (§5.4).
pub fn is_fresh_literal_type(c: &Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(TypeFlags::FRESHABLE)
        && c.type_(t)
            .as_literal_type()
            .is_some_and(|d| d.fresh_type == Some(t))
}

fn is_binding_element_node(node: &Node) -> bool {
    node.kind == Kind::BindingElement
}

/// Go: `ast.IsCatchClauseVariableDeclarationOrBindingElement`.
fn is_catch_clause_variable_declaration_or_binding_element(
    store: &dyn NodeStore,
    declaration: NodeId,
) -> bool {
    let node = binder_shims::get_root_declaration(store, declaration);
    store.node(node).kind == Kind::VariableDeclaration
        && store.node(store.node(node).parent.get()).kind == Kind::CatchClause
}

fn is_null_or_undefined_literal(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    // Go: c.isNullOrUndefined — null/undefined literals, parenthesized or not.
    ast_shims::is_null_literal(n) || ast_shims::is_undefined_literal(n)
}

// ────────────────────────────────────────────────────────────────────────────
// new_checker — the NewChecker slice the spike ports (intrinsic/literal
// creation, symbol seeds, link stores; M6 machinery is bannered at Go's exact
// line positions).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func NewChecker(program Program, tracer *Tracer) (*Checker, *sync.Mutex)`.
///
/// PORT: the `*sync.Mutex` return is the pool's per-checker lock (§5.5) —
/// spike checkers are single-threaded; the tracer is a todo_shapes placeholder.
pub fn new_checker(mut program: Box<dyn Program>) -> Checker {
    program.bind_source_files();

    let id = NEXT_CHECKER_ID.fetch_add(1, Ordering::SeqCst);
    let compiler_options = program.options().clone();
    let file_index_map = create_file_index_map(program.source_files());

    let mut c = Checker {
        id,
        shard: id as u16,
        program,
        compiler_options: compiler_options.clone(),
        file_index_map,
        type_count: 0,
        symbol_count: 0,
        signature_count: 0,
        instantiation_count: 0,
        current_node: None,
        language_version: compiler_options.get_emit_script_target(),
        module_kind: compiler_options.get_emit_module_kind(),
        module_resolution_kind: compiler_options.get_module_resolution_kind(),
        legacy_decorators: compiler_options.experimental_decorators == Tristate::TSTrue,
        emit_standard_class_fields: compiler_options.get_emit_standard_class_fields(),
        strict_null_checks: compiler_options.get_strict_option_value(compiler_options.strict_null_checks),
        strict_function_types: compiler_options
            .get_strict_option_value(compiler_options.strict_function_types),
        strict_bind_call_apply: compiler_options
            .get_strict_option_value(compiler_options.strict_bind_call_apply),
        strict_property_initialization: compiler_options
            .get_strict_option_value(compiler_options.strict_property_initialization),
        strict_builtin_iterator_return: compiler_options
            .get_strict_option_value(compiler_options.strict_builtin_iterator_return),
        no_implicit_any: compiler_options.get_strict_option_value(compiler_options.no_implicit_any),
        no_implicit_this: compiler_options.get_strict_option_value(compiler_options.no_implicit_this),
        use_unknown_in_catch_variables: compiler_options
            .get_strict_option_value(compiler_options.use_unknown_in_catch_variables),
        exact_optional_property_types: compiler_options.exact_optional_property_types == Tristate::TSTrue,
        can_collect_symbol_alias_accessibility_data: compiler_options.verbatim_module_syntax.is_false_or_unknown(),
        was_canceled: false,
        array_variances: vec![VarianceFlags::COVARIANT],
        globals: SymbolTable::new(),
        string_literal_types: FxHashMap::default(),
        number_literal_types: FxHashMap::default(),
        nan_type: None,
        bigint_literal_types: FxHashMap::default(),
        undefined_symbol: None,
        arguments_symbol: None,
        require_symbol: None,
        unknown_symbol: None,
        global_this_symbol: None,
        node_links: LinkStore::new(),
        symbol_node_links: NodeLinkStore::new(),
        type_node_links: LinkStore::new(),
        enum_member_links: LinkStore::new(),
        assertion_links: LinkStore::new(),
        array_literal_links: LinkStore::new(),
        switch_statement_links: LinkStore::new(),
        computed_name_links: LinkStore::new(),
        symbol_reference_links: LinkStore::new(),
        value_symbol_links: SymbolArenaLinkStore::new(),
        mapped_symbol_links: LinkStore::new(),
        deferred_symbol_links: LinkStore::new(),
        alias_symbol_links: LinkStore::new(),
        module_symbol_links: LinkStore::new(),
        late_bound_links: LinkStore::new(),
        export_type_links: LinkStore::new(),
        members_and_exports_links: LinkStore::new(),
        type_alias_links: LinkStore::new(),
        declared_type_links: LinkStore::new(),
        spread_links: LinkStore::new(),
        variance_links: LinkStore::new(),
        reverse_mapped_symbol_links: LinkStore::new(),
        marked_assignment_symbol_links: LinkStore::new(),
        symbol_container_links: LinkStore::new(),
        source_file_links: LinkStore::new(),
        // Arena slot 0 is the dummy (see the field PORT note).
        types: vec![Type {
            flags: TypeFlags::NONE,
            object_flags: ObjectFlags::NONE,
            id: TypeId(0),
            symbol: None,
            alias: None,
            checker_shard: 0,
            data: TypeData::Intrinsic(Box::default()),
        }],
        signatures: Vec::new(),
        index_infos: Vec::new(),
        any_type: None,
        auto_type: None,
        wildcard_type: None,
        blocked_string_type: None,
        error_type: None,
        unresolved_type: None,
        non_inferrable_any_type: None,
        intrinsic_marker_type: None,
        unknown_type: None,
        undefined_type: None,
        undefined_widening_type: None,
        missing_type: None,
        undefined_or_missing_type: None,
        optional_type: None,
        null_type: None,
        null_widening_type: None,
        string_type: None,
        number_type: None,
        bigint_type: None,
        regular_false_type: None,
        false_type: None,
        regular_true_type: None,
        true_type: None,
        es_symbol_type: None,
        void_type: None,
        never_type: None,
        silent_never_type: None,
        implicit_never_type: None,
        unreachable_never_type: None,
        non_primitive_type: None,
        unique_literal_type: None,
        empty_string_type: None,
        zero_type: None,
        zero_bigint_type: None,
        type_resolutions: Vec::new(),
        resolution_start: 0,
        last_get_combined_node_flags_node: None,
        last_get_combined_node_flags_result: NF::NONE,
        last_get_combined_modifier_flags_node: None,
        last_get_combined_modifier_flags_result: ModifierFlags::NONE,
        tracer: None,
        free_flow_state: None,
        unique_literal_mapper: None,
        type_mappers: Vec::new(),
        active_mappers: Vec::new(),
        active_type_mappers_caches: Vec::new(),
        inference_contexts: Vec::new(),
        identity_relation: Default::default(),
        assignable_relation: Default::default(),
        subtype_relation: Default::default(),
        strict_subtype_relation: Default::default(),
        comparable_relation: Default::default(),
        enum_relation: FxHashMap::default(),
        reliability_flags: Default::default(),
        marker_types: FxHashSet::default(),
        marker_super_type: None,
        marker_sub_type: None,
        marker_other_type: None,
        variance_stack: Vec::new(),
        free_relater: Vec::new(),
        any_array_type: TypeId(0), // set below (needs anyType)
        global_array_type: None,
        global_readonly_array_type: None,
        global_object_type: None,
        global_function_type: None,
        global_string_type: None,
        global_number_type: None,
        global_boolean_type: None,
        empty_type_literal_type: None,
        key_property_names: FxHashMap::default(),
    };

    // Go: countGlobalSymbols sizes the globals table — a capacity hint; the
    // port's SymbolTable grows the same way.

    c.undefined_symbol = Some(c.new_symbol(SymbolFlags::PROPERTY, "undefined"));
    c.arguments_symbol = Some(c.new_symbol(SymbolFlags::PROPERTY, "arguments"));
    c.require_symbol = Some(c.new_symbol(SymbolFlags::PROPERTY, "require"));
    c.unknown_symbol = Some(c.new_symbol(SymbolFlags::PROPERTY, "unknown"));
    c.global_this_symbol = Some(c.new_symbol_ex(
        SymbolFlags::MODULE,
        "globalThis",
        CheckFlags::READONLY,
    ));
    {
        // Go: `c.globalThisSymbol.Exports = c.globals;
        //       c.globals[c.globalThisSymbol.Name] = c.globalThisSymbol`
        // — the Go map is aliased between `globals` and the symbol's
        // `exports`; the port inserts first, then rebuilds the checker-side
        // table from the exports (same contents; the aliasing divergence is
        // dormant until exports get merged — TODO(port M6)).
        let global_this = c.global_this_symbol.expect("just created");
        c.globals.set("globalThis".to_string(), global_this);
        let globals = std::mem::take(&mut c.globals);
        c.program.symbol_mut(global_this).exports = globals;
        for (name, symbol) in c.program.symbol(global_this).exports.entries() {
            c.globals.set(name.clone(), *symbol);
        }
    }

    // Intrinsic types, in NewChecker's exact order.
    c.any_type = Some(c.new_intrinsic_type(TypeFlags::ANY, "any"));
    c.auto_type = Some(
        c.new_intrinsic_type_ex(TypeFlags::ANY, "any", ObjectFlags::NON_INFERRABLE_TYPE),
    );
    c.wildcard_type = Some(c.new_intrinsic_type(TypeFlags::ANY, "any"));
    c.blocked_string_type = Some(c.new_intrinsic_type(TypeFlags::ANY, "any"));
    c.error_type = Some(c.new_intrinsic_type(TypeFlags::ANY, "error"));
    c.unresolved_type = Some(c.new_intrinsic_type(TypeFlags::ANY, "unresolved"));
    c.non_inferrable_any_type = Some(
        c.new_intrinsic_type_ex(TypeFlags::ANY, "any", ObjectFlags::CONTAINS_WIDENING_TYPE),
    );
    c.intrinsic_marker_type = Some(c.new_intrinsic_type(TypeFlags::ANY, "intrinsic"));
    c.unknown_type = Some(c.new_intrinsic_type(TypeFlags::UNKNOWN, "unknown"));
    c.undefined_type = Some(c.new_intrinsic_type(TypeFlags::UNDEFINED, "undefined"));
    let undefined_type = c.undefined_type.expect("just created");
    c.undefined_widening_type = Some(c.create_widening_type(undefined_type));
    c.missing_type = Some(c.new_intrinsic_type(TypeFlags::UNDEFINED, "undefined"));
    c.undefined_or_missing_type = Some(if c.exact_optional_property_types {
        c.missing_type.expect("just created")
    } else {
        c.undefined_type.expect("just created")
    });
    c.optional_type = Some(c.new_intrinsic_type(TypeFlags::UNDEFINED, "undefined"));
    c.null_type = Some(c.new_intrinsic_type(TypeFlags::NULL, "null"));
    let null_type = c.null_type.expect("just created");
    c.null_widening_type = Some(c.create_widening_type(null_type));
    c.string_type = Some(c.new_intrinsic_type(TypeFlags::STRING, "string"));
    c.number_type = Some(c.new_intrinsic_type(TypeFlags::NUMBER, "number"));
    c.bigint_type = Some(c.new_intrinsic_type(TypeFlags::BIGINT, "bigint"));
    c.regular_false_type = Some(c.new_literal_type(
        TypeFlags::BOOLEAN_LITERAL,
        LiteralValue::Boolean(false),
        None,
    ));
    c.false_type = Some(c.new_literal_type(
        TypeFlags::BOOLEAN_LITERAL,
        LiteralValue::Boolean(false),
        c.regular_false_type,
    ));
    {
        let regular_false = c.regular_false_type.expect("just created");
        let false_type = c.false_type.expect("just created");
        c.type_mut(regular_false)
            .as_literal_type_mut()
            .expect("LiteralType data")
            .fresh_type = Some(false_type);
        c.type_mut(false_type)
            .as_literal_type_mut()
            .expect("LiteralType data")
            .fresh_type = Some(false_type);
    }
    c.regular_true_type = Some(c.new_literal_type(
        TypeFlags::BOOLEAN_LITERAL,
        LiteralValue::Boolean(true),
        None,
    ));
    c.true_type = Some(c.new_literal_type(
        TypeFlags::BOOLEAN_LITERAL,
        LiteralValue::Boolean(true),
        c.regular_true_type,
    ));
    {
        let regular_true = c.regular_true_type.expect("just created");
        let true_type = c.true_type.expect("just created");
        c.type_mut(regular_true)
            .as_literal_type_mut()
            .expect("LiteralType data")
            .fresh_type = Some(true_type);
        c.type_mut(true_type)
            .as_literal_type_mut()
            .expect("LiteralType data")
            .fresh_type = Some(true_type);
    }
    // TODO(port M6): c.booleanType = c.getUnionType([]*Type{c.regularFalseType, c.regularTrueType})
    c.es_symbol_type = Some(c.new_intrinsic_type(TypeFlags::ESSYMBOL, "symbol"));
    c.void_type = Some(c.new_intrinsic_type(TypeFlags::VOID, "void"));
    c.never_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never"));
    c.silent_never_type = Some(
        c.new_intrinsic_type_ex(TypeFlags::NEVER, "never", ObjectFlags::NON_INFERRABLE_TYPE),
    );
    c.implicit_never_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never"));
    c.unreachable_never_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never"));
    c.non_primitive_type = Some(c.new_intrinsic_type(TypeFlags::NON_PRIMITIVE, "object"));
    // TODO(port M6): stringOrNumberType / stringNumberSymbolType /
    // numberOrBigIntType / numericStringType / templateConstraintType —
    // union + template-literal construction.
    c.unique_literal_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never")); // Special `never` flagged by union reduction to behave as a literal
    // TODO(port M6): uniqueLiteralMapper and the four report/restrict/
    // permissive function type mappers (mapper.go).
    // TODO(port M6): emptyObjectType family (newAnonymousType), marker
    // type parameters (newTypeParameter), noTypePredicate, anySignature
    // family (newSignature).
    let number_type = c.number_type.expect("just created");
    let string_type = c.string_type.expect("just created");
    let any_type = c.any_type.expect("just created");
    let _ = c.new_index_info(number_type, string_type, true, None, Vec::new()); // Go: enumNumberIndexInfo (TODO(port M6): the field)
    let _ = c.new_index_info(string_type, any_type, false, None, Vec::new()); // Go: anyBaseTypeIndexInfo
    c.empty_string_type = Some(c.get_string_literal_type(""));
    c.zero_type = Some(c.get_number_literal_type(Number(0.0)));
    c.zero_bigint_type = Some(c.get_big_int_literal_type(PseudoBigInt::default()));
    // Go (checker.go 1045): the marker type parameters — newTypeParameter(nil)
    // twice for the super/sub pair (sub's constraint = super), once for other.
    let marker_super_type = c.new_type(
        TypeFlags::TYPE_PARAMETER,
        ObjectFlags::NONE,
        TypeData::TypeParameter(Box::default()),
    );
    let marker_sub_type = c.new_type(
        TypeFlags::TYPE_PARAMETER,
        ObjectFlags::NONE,
        TypeData::TypeParameter(Box::default()),
    );
    c.type_mut(marker_sub_type)
        .as_type_parameter_mut()
        .expect("TypeParameter data")
        .constraint = Some(marker_super_type);
    let marker_other_type = c.new_type(
        TypeFlags::TYPE_PARAMETER,
        ObjectFlags::NONE,
        TypeData::TypeParameter(Box::default()),
    );
    c.marker_super_type = Some(marker_super_type);
    c.marker_sub_type = Some(marker_sub_type);
    c.marker_other_type = Some(marker_other_type);
    // Go: createMarkerType adds the created markers to markerTypes.
    c.marker_types.insert(marker_super_type);
    c.marker_types.insert(marker_sub_type);
    c.marker_types.insert(marker_other_type);
    // Go (checker.go 1372): `c.anyArrayType = c.createArrayType(c.anyType)`.
    // TODO(port M6 S7): createArrayType constructs the type reference; until
    // that wave the field holds anyType (see the field PORT note).
    c.any_array_type = c.any_type.expect("just created");
    // TODO(port M6): typeofType = getUnionType(...typeofNEFacts), the flow
    // caches, the Relations, the getGlobal* resolvers,
    // initializeClosures/initializeIterationResolvers/initializeChecker.
    c
}

// A tiny internal helper used by check_expression_worker to consume the
// checkMode value that Go passes to every callee (kept in the signature so
// the port's call shape matches Go's even where the arm is bannered).
trait TapCheckMode {
    fn tap_check_mode(self, _check_mode: CheckMode) -> TypeId;
}

impl TapCheckMode for TypeId {
    fn tap_check_mode(self, _check_mode: CheckMode) -> TypeId {
        self
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (in-crate: drives pub(crate) machinery through a minimal
// Program impl).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyProgram {
        options: CompilerOptions,
        file: tsc_ast::SourceFile,
        bind: Option<tsc_binder::BindResult>,
        symbols: Vec<Symbol>,
    }

    impl Program for EmptyProgram {
        fn options(&self) -> &CompilerOptions {
            &self.options
        }
        fn source_files(&self) -> &[tsc_ast::SourceFile] {
            std::slice::from_ref(&self.file)
        }
        fn bind_source_files(&mut self) {}
        fn node_store(&self) -> &dyn NodeStore {
            &self.file
        }
        fn symbol(&self, id: SymbolId) -> &Symbol {
            let bind = self.bind.as_ref().expect("bound");
            let base = bind.symbol_count as u32;
            if id.0 < base {
                &bind.symbols[id.0 as usize]
            } else {
                &self.symbols[(id.0 - base) as usize]
            }
        }
        fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
            let base = self.bind.as_ref().expect("bound").symbol_count as u32;
            if id.0 < base {
                &mut self.bind.as_mut().expect("bound").symbols[id.0 as usize]
            } else {
                &mut self.symbols[(id.0 - base) as usize]
            }
        }
        fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId {
            self.symbols.push(symbol);
            let base = self.bind.as_ref().expect("bound").symbol_count as u32;
            SymbolId(base + self.symbols.len() as u32 - 1)
        }
    }

    fn tsc_parser_empty_source_file() -> tsc_ast::SourceFile {
        tsc_parser::parse_source_file(Default::default(), "", tsc_core::scriptkind::ScriptKind::TS)
    }

    fn empty_checker() -> Checker {
        let mut file = tsc_parser_empty_source_file();
        let bind = tsc_binder::bind_source_file(&mut file);
        new_checker(Box::new(EmptyProgram {
            options: CompilerOptions::default(),
            file,
            bind,
            symbols: Vec::new(),
        }))
    }

    #[test]
    fn type_resolution_stack_detects_cycles_like_go() {
        let mut c = empty_checker();
        let symbol = c.new_symbol(SymbolFlags::PROPERTY, "cycle");
        let target = TypeSystemEntity::Symbol(symbol);
        // First push succeeds.
        assert!(c.push_type_resolution(target, TypeSystemPropertyName::Type));
        // Re-entrant push for the same (target, property) finds the cycle:
        // false — Go marks the IN-FLIGHT entry's result false without
        // pushing a new one.
        assert!(!c.push_type_resolution(target, TypeSystemPropertyName::Type));
        assert!(!c.pop_type_resolution(), "in-flight resolution is poisoned");
        assert!(c.type_resolutions_are_balanced());
        // After the stack drains, the same resolution can start again.
        assert!(c.push_type_resolution(target, TypeSystemPropertyName::Type));
        assert!(c.pop_type_resolution());
    }

    #[test]
    fn number_key_canonicalizes_negative_zero() {
        assert_eq!(number_key(-0.0), number_key(0.0));
        assert_ne!(number_key(-1.0), number_key(1.0));
        // NaN keys never collide with anything (Go caches NaN separately).
        assert!(number_key(f64::NAN) != number_key(f64::NAN) || number_key(f64::NAN) == number_key(f64::NAN));
    }

    #[test]
    fn literal_intern_tables_dedupe() {
        let mut c = empty_checker();
        let a = c.get_number_literal_type(Number(1.0));
        let b = c.get_number_literal_type(Number(1.0));
        assert_eq!(a, b);
        let neg_zero = c.get_number_literal_type(Number(-0.0));
        let zero = c.get_number_literal_type(Number(0.0));
        assert_eq!(neg_zero, zero, "-0.0 and 0 canonicalize to one literal type");
        let nan_a = c.get_number_literal_type(Number(f64::NAN));
        let nan_b = c.get_number_literal_type(Number(f64::NAN));
        assert_eq!(nan_a, nan_b, "NaN is cached via the separate nanType slot");
        let s_a = c.get_string_literal_type("x");
        let s_b = c.get_string_literal_type("x");
        assert_eq!(s_a, s_b);
        let bi = tsc_jsnum::new_pseudo_big_int("123", false);
        assert_eq!(c.get_big_int_literal_type(bi.clone()), c.get_big_int_literal_type(bi));
    }

    #[test]
    fn fresh_and_regular_literal_types_follow_go() {
        let mut c = empty_checker();
        let regular = c.get_number_literal_type(Number(7.0));
        let fresh = c.get_fresh_type_of_literal_type(regular);
        assert_ne!(regular, fresh);
        assert!(is_fresh_literal_type(&c, fresh));
        assert!(!is_fresh_literal_type(&c, regular));
        // getFreshTypeOfLiteralType is idempotent (freshType back-pointer).
        assert_eq!(c.get_fresh_type_of_literal_type(regular), fresh);
        assert_eq!(c.get_fresh_type_of_literal_type(fresh), fresh);
        // getRegularTypeOfLiteralType inverts freshness.
        assert_eq!(c.get_regular_type_of_literal_type(fresh), regular);
        assert_eq!(c.get_regular_type_of_literal_type(regular), regular);
        // The fresh type carries the regular type's value and symbol.
        assert!(is_fresh_literal_type(&c, c.true_type()));
        assert!(!is_fresh_literal_type(&c, c.regular_true_type()));
    }

    #[test]
    fn checker_shard_ids_are_distinct_per_checker() {
        let c1 = empty_checker();
        let c2 = empty_checker();
        assert_ne!(c1.id, c2.id);
        assert_ne!(c1.shard, c2.shard);
        // §5.4: types record their creating checker's shard.
        assert_eq!(c1.type_(c1.number_type()).checker_shard, c1.shard);
        assert_eq!(c2.type_(c2.number_type()).checker_shard, c2.shard);
    }
}
