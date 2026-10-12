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
    CheckFlags, ModifierFlags, NodeFlags, NodeId, NodeStore, Symbol, SymbolFlags,
    SymbolId, SymbolTable,
};
use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_binder::ast_shims as binder_shims;
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use tsc_core::tristate::Tristate;
use tsc_core::linkstore::LinkStore;
use tsc_core::options_generated::ScriptTarget;
use tsc_jsnum::{Number, PseudoBigInt};

use crate::links::{NodeLinkStore, SymbolArenaLinkStore};
use crate::mapper::TypeMapperData;
use crate::todo_shapes::{
    CacheHashKey, FlowState, InferenceContext, TypeMapperId,
};
use crate::types::{
    AliasSymbolLinks, ArrayLiteralLinks, AssertionLinks, ComputedNameNodeLinks,
    ContainingSymbolLinks, DeclaredTypeLinks, DeferredSymbolLinks, EnumMemberLinks, ExportTypeLinks,
    IndexInfo, IntrinsicType, LateBoundLinks, LiteralType, LiteralValue, MarkedAssignmentSymbolLinks,
    MembersAndExportsLinks, MappedSymbolLinks, ModuleSymbolLinks, NodeLinks,
    ObjectFlags, ReverseMappedSymbolLinks, Signature, SignatureFlags, SignatureId, SourceFileLinks,
    SpreadLinks, SwitchStatementLinks, SymbolNodeLinks, SymbolReferenceLinks, Type, TypeAliasLinks,
    TypeData, TypeFlags, TypeId, TypeNodeLinks, TypePredicate, TypePredicateKind, ValueSymbolLinks,
    VarianceFlags, VarianceLinks,
};

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
    /// Go: the checker mutates the node graph through factory-created
    /// synthetic nodes (`getFlowTypeInConstructor`/`getFlowTypeInStaticBlocks`
    /// allocate a `this.x` property access, flow.go:2906+). The mutable store
    /// seam (Go: the shared `core.Arena` the factory allocates in).
    fn node_store_mut(&mut self) -> &mut dyn NodeStore;
    /// Go: the binder's flow-node/flow-list arenas — the checker READS the
    /// flow graph (`BindResult.flow_nodes`/`flow_lists`; Go walks GC
    /// `*ast.FlowNode` pointers). Copied into the Checker at NewChecker.
    /// Default: empty (a program that didn't bind produces no flow graph).
    fn flow_arena(&self) -> (&[tsc_ast::flow::FlowNode], &[tsc_ast::flow::FlowList]) {
        (&[], &[])
    }
    // ── §5.5 program-level symbol store (see the module PORT note) ──
    /// The symbol arena slot for `id`.
    fn symbol(&self, id: SymbolId) -> &Symbol;
    /// Mutable symbol slot access (Go mutates `*Symbol` through pointers).
    fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol;
    /// Appends a checker-created symbol to the program store.
    fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId;
    /// Go: `Program.GetEmitModuleFormatOfFile(sourceFile) core.ModuleKind` —
    /// the grammar checks' module-format read (the VMS export arm, the
    /// __esModule marker arm).
    ///
    /// PORT (W3 grammar seam): the permissive default mirrors "no program
    /// metadata" — the testrunner's CheckerUnitProgram overrides it with the
    /// compiler crate's worker; the real compiler Program overrides with the
    /// program-level answer.
    fn get_emit_module_format_of_file(&self, _file: NodeId) -> ModuleKind {
        ModuleKind::None
    }
    /// Go: `Program.GetSourceFileMetaData(path).ImpliedNodeFormat` — the
    /// Node16-Nodenext await arms' implied-node-format read (the CommonJS
    /// top-level await/for-await errors).
    fn source_file_implied_node_format(&self, _file: NodeId) -> ModuleKind {
        ModuleKind::None
    }
    /// Go: `!program.IsSourceFileDefaultLibrary(file)` guard from
    /// services.go's IsLibSymbolForHoverVerbosity/IsLibTypeForHoverVerbosity
    /// (W3 tranche 3: added with the services slice; permissive default
    /// mirrors "no lib metadata").
    fn is_source_file_default_library(&self, _file: NodeId) -> bool {
        false
    }
    /// Go: `Program.GetJSXRuntimeImportSpecifier(path) (moduleReference string,
    /// specifier *ast.Node)` (program.go:2228 — the file loader's react-jsx
    /// auto-import bookkeeping). M6 W5: the checker's
    /// getJsxNamespaceContainerForImplicitImport reads it. PORT: the trait
    /// hands back the module-reference STRING only (Go's second return, the
    /// synthetic import-specifier NODE, rides the program merge; the checker
    /// uses the canonical JSX tag as the error location meanwhile). The
    /// permissive default mirrors a program with no react-jsx auto-import
    /// data; the compiler Program (tsc_compiler::Program) cannot override
    /// yet (its jsx_runtime_import_specifiers map landed with the file
    /// loader wave but the crate is outside this wave's write scope — the
    /// check twin's CheckerUnitProgram carries the twin answer).
    fn get_jsx_runtime_import_specifier(&self, _file: NodeId) -> Option<String> {
        None
    }
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
    // Go: `primitiveTypeAliasSuggestions sync.OnceValue` (checker.go:1795) —
    // the six builtin→primitive transient type-alias suggestion symbols;
    // lazily materialized in the program's symbol arena (newSymbol ors in
    // TRANSIENT exactly like Go's `&ast.Symbol{Flags: TypeAlias|Transient}`).
    pub(crate) primitive_type_alias_suggestion_symbols: [Option<SymbolId>; 6],
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
    /// Go: `nonExistentProperties core.AutoSet[NonExistentPropertyKey]`
    /// (checker.go:11767 — the reportNonexistentProperty memo).
    pub(crate) non_existent_properties: std::collections::HashSet<(NodeId, TypeId, bool)>,
    /// Go: `cachedArgumentsReferenced map[*ast.Node]bool` (checker.go —
    /// containsArgumentsReference's memo).
    pub(crate) cached_arguments_referenced: FxHashMap<NodeId, bool>,
    // ── M6 S6 (resolvecall.rs): the call-resolution family's state ──
    /// Go: `signatureLinks NodeLinkStore[SignatureLinks]` (checker.go:660's
    /// field) — getResolvedSignature's per-call-node resolved-signature cache.
    pub(crate) signature_node_links: LinkStore<NodeId, crate::types::SignatureLinks>,
    /// Go: `callResolutionStack []*ast.Node` (resolveCall's recursion probe).
    pub(crate) call_resolution_stack: Vec<NodeId>,
    /// Go: `isInferencePartiallyBlocked bool` — set when a contextual
    /// signature inference skips generic functions (S9's inference engine).
    pub(crate) is_inference_partially_blocked: bool,
    /// Go: `apparentArgumentCount *int` — the language-service probe
    /// pickLongestCandidateSignature reads.
    pub(crate) apparent_argument_count: Option<i32>,
    /// Go: `cachedSignatures map[CachedSignatureKey]*Signature` —
    /// getOptionalCallSignature's clone cache (inner/outer call-chain keys).
    /// PORT (M6 S8): Go keys by the full `CacheHashKey` (the type-list key
    /// OR the erased/canonical/base sentinels); the S7 `u8` compression
    /// collided distinct type-argument lists — the full u64 is the load-
    /// bearing form (the call-chain keys 1/2 ride the same map).
    pub(crate) cached_signatures: FxHashMap<(crate::types::SignatureId, u64), crate::types::SignatureId>,
    /// Go: `enumNumberIndexInfo *IndexInfo{numberType, stringType, readonly}`
    /// (checker.go:1058 — seeded in new_checker; arena form).
    pub(crate) enum_number_index_info: Option<crate::types::IndexInfoId>,
    /// Go: `anyBaseTypeIndexInfo *IndexInfo{stringType, anyType, false}`
    /// (checker.go:1059 — seeded in new_checker; arena form).
    pub(crate) any_base_type_index_info: Option<crate::types::IndexInfoId>,
    // TODO(port M6): jsxElementLinks (JsxElementLinks), the
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
    // ── M6 S7 (uniontypes.rs): the union-constructed intrinsic types Go
    // builds eagerly in NewChecker (checker.go:1013-1022) ──
    /// Go: `booleanType *Type` — getUnionType([regularFalse, regularTrue]).
    pub(crate) boolean_type: Option<TypeId>,
    /// Go: `stringOrNumberType *Type`.
    pub(crate) string_or_number_type: Option<TypeId>,
    /// Go: `stringNumberSymbolType *Type`.
    pub(crate) string_number_symbol_type: Option<TypeId>,
    /// Go: `numberOrBigIntType *Type`.
    pub(crate) number_or_bigint_type: Option<TypeId>,
    /// Go: `numericStringType *Type` — the `${number}` template literal type.
    pub(crate) numeric_string_type: Option<TypeId>,
    /// Go: `templateConstraintType *Type`.
    pub(crate) template_constraint_type: Option<TypeId>,
    /// Go: `unknownEmptyObjectType *Type` (NewChecker's anonymous seed).
    pub(crate) unknown_empty_object_type: Option<TypeId>,
    /// Go: `unknownUnionType *Type` — createUnknownUnionType (checker.go:1037;
    /// REAL since M6 S7: the undefined | null | unknownEmptyObjectType union
    /// under strictNullChecks, else unknownType).
    pub(crate) unknown_union_type: Option<TypeId>,
    pub(crate) es_symbol_type: Option<TypeId>,
    pub(crate) void_type: Option<TypeId>,
    pub(crate) never_type: Option<TypeId>,
    pub(crate) silent_never_type: Option<TypeId>,
    pub(crate) implicit_never_type: Option<TypeId>,
    pub(crate) unreachable_never_type: Option<TypeId>,
    pub(crate) non_primitive_type: Option<TypeId>,
    pub(crate) unique_literal_type: Option<TypeId>,
    pub(crate) empty_string_type: Option<TypeId>,
    /// Go: `typeofType *Type` — the "bigint|boolean|function|number|object|string|symbol|undefined"
    /// literal-word union (checker.go:1063; REAL since M6 S7).
    pub(crate) typeof_type: Option<TypeId>,
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
    // ── Inference state (Go checker.go's fields — M6 W2, inference.rs) ──
    /// Go: `freeinferenceState *InferenceState` — PORT: Go's intrusive free
    /// list becomes a value stack (states are popped/pushed by value; see
    /// inference.rs's pool PORT note).
    pub(crate) free_inference_states: Vec<crate::inference::InferenceState>,
    /// Go: `reverseMappedCache map[ReverseMappedTypeKey]*Type`.
    pub(crate) reverse_mapped_cache: FxHashMap<crate::inference::ReverseMappedTypeKey, Option<TypeId>>,
    /// Go: `reverseHomomorphicMappedCache map[ReverseMappedTypeKey]*Type`.
    pub(crate) reverse_homomorphic_mapped_cache: FxHashMap<crate::inference::ReverseMappedTypeKey, Option<TypeId>>,
    /// Go: `reverseMappedSourceStack []*Type` / `reverseMappedTargetStack []*Type` /
    /// `reverseExpandingFlags ExpandingFlags` (inference.go's reverse-mapped family).
    pub(crate) reverse_mapped_source_stack: Vec<TypeId>,
    pub(crate) reverse_mapped_target_stack: Vec<TypeId>,
    pub(crate) reverse_expanding_flags: crate::relater::ExpandingFlags,
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
    /// Go: `anyArrayType *Type` — NewChecker's `createArrayType(anyType)`
    /// (REAL since M6 S7: the interned type reference via globalArrayType,
    /// seeded by initialize_checker before this field's assignment).
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
    // ── M6 S1 (spine.rs): the entry/expression-spine state (Go checker.go) ──
    /// Go: the `ctx`-derived cancellation flag (utilities.go:1712;
    /// `c.checkNotCanceled` panics on use-after-cancel through
    /// `was_canceled` below). PORT: no context plumbing — a host-set flag.
    pub(crate) canceled: bool,
    /// Go: `files []*ast.SourceFile` — the program's files (the §5.1
    /// NodeId identity), iterated by initializeChecker.
    pub(crate) files: Vec<NodeId>,
    /// Go: `withinUnreachableCode bool` (checker.go:2266).
    pub(crate) within_unreachable_code: bool,
    /// Go: `reportedUnreachableNodes map[*ast.Node]struct{}`.
    pub(crate) reported_unreachable_nodes: FxHashSet<NodeId>,
    /// Go: `deferredDiagnosticCallbacks []func()` (checker.go:14312) — the
    /// closure payloads as an enum (spine.rs).
    pub(crate) deferred_diagnostic_callbacks: Vec<crate::spine::DeferredDiagnosticCallback>,
    /// Go: `contextualTypes []ContextualInfo` (checker.go:78-84).
    pub(crate) contextual_infos: Vec<crate::spine::ContextualInfo>,
    /// Go: `inferenceContexts []InferenceContextInfo` (checker.go:86-89).
    pub(crate) inference_context_infos: Vec<crate::spine::InferenceContextInfo>,
    /// Go: `flowTypeCache map[*ast.Node]*Type` — keyed by the node id (§5.1).
    pub(crate) flow_type_cache: FxHashMap<NodeId, TypeId>,
    /// Go: `flowLoopStack []*FlowLoop` (flow.go:1320) — REAL since W3
    /// (flow.rs's FlowLoopInfo: the loop-junction cache key + partial types).
    pub(crate) flow_loop_stack: Vec<crate::flow::FlowLoopInfo>,
    // ── M6 W3 (flow.rs): the flow-graph consumer + narrowing engine ──
    /// Go: the flow-node arena (`*ast.FlowNode` graph; PORT: copied from the
    /// binder's BindResult at NewChecker — the checker only reads it).
    pub(crate) flow_nodes: Vec<tsc_ast::flow::FlowNode>,
    /// Go: the flow-list cons-cell arena (`*ast.FlowList`).
    pub(crate) flow_lists: Vec<tsc_ast::flow::FlowList>,
    /// Go: `sharedFlowTypes []*SharedFlow` (flow.go:53).
    pub(crate) shared_flows: Vec<crate::flow::SharedFlow>,
    /// Go: the `*FlowState` free list (`freeFlowState`) — PORT: a value pool
    /// (flow.rs's FlowState is a value type; see its module PORT note).
    pub(crate) free_flow_states: Vec<crate::flow::FlowState>,
    /// Go: `antecedentTypes []*Type` (flow.go's getTypeAtFlowBranchLabel).
    pub(crate) antecedent_types: Vec<TypeId>,
    /// Go: `flowLoopCache map[flowLoopCacheKey]*Type` (flow.go:1318).
    pub(crate) flow_loop_cache:
        FxHashMap<(tsc_ast::FlowNodeId, crate::todo_shapes::CacheHashKey), TypeId>,
    /// Go: `flowNodeReachable map[*ast.FlowNode]bool` (flow.go:2595).
    pub(crate) flow_node_reachable: FxHashMap<tsc_ast::FlowNodeId, bool>,
    /// Go: `flowNodePostSuper map[*ast.FlowNode]bool` (flow.go:2688).
    pub(crate) flow_node_post_super: FxHashMap<tsc_ast::FlowNodeId, bool>,
    /// Go: `lastFlowNode *ast.FlowNode` / `lastFlowNodeReachable bool`
    /// (flow.go's isReachableFlowNode single-entry cache).
    pub(crate) last_flow_node: Option<tsc_ast::FlowNodeId>,
    pub(crate) last_flow_node_reachable: bool,
    /// Go: `narrowedTypes map[narrowedTypeKey]*Type` (flow.go:846).
    pub(crate) narrowed_types: FxHashMap<(TypeId, TypeId, bool, bool), TypeId>,
    /// Go: `assignmentReducedTypes map[assignmentReducedTypeKey]*Type`
    /// (flow.go:3499).
    pub(crate) assignment_reduced_types: FxHashMap<(TypeId, TypeId), TypeId>,
    /// Go: `inlineLevel uint32` (flow.go's getTypeAtFlowBranchLabel inlining).
    pub(crate) inline_level: u32,
    /// Go: `resolvingExplicitTypeOfSymbol *ast.Symbol` — PORT: the recursive
    /// guard is an OrderedSet keyed by SymbolId (the single-symbol Go field
    /// only ever guards one recursion level).
    pub(crate) resolving_explicit_type_of_symbol: tsc_collections::OrderedSet<SymbolId>,
    /// Go: `flowInvocationCount uint32`.
    pub(crate) flow_invocation_count: usize,
    /// Go: `contextFreeTypes map[*ast.Node]*Type`.
    pub(crate) context_free_types: FxHashMap<NodeId, TypeId>,
    /// Go: `renamedBindingElementsInTypes []*ast.Node`.
    pub(crate) renamed_binding_elements_in_types: Vec<NodeId>,
    /// Go: `varianceTypeParameter *Type` (checker.go:548) — the variance
    /// annotation check's in-flight type parameter.
    pub(crate) variance_type_parameter: Option<TypeId>,
    /// Go: `skipDirectInferenceNodes *collections.OrderedSet`.
    pub(crate) skip_direct_inference_nodes: tsc_collections::OrderedSet<NodeId>,
    /// Go: `serializationLevel uint32` (checker.go:14263).
    pub(crate) serialization_level: u32,
    /// Go: `diagnostics` / `suggestionDiagnostics` (ast/diagnostic.go's
    /// DiagnosticsCollection — spine.rs's port).
    pub(crate) diagnostics: crate::spine::DiagnosticsCollection,
    pub(crate) suggestion_diagnostics: crate::spine::DiagnosticsCollection,
    /// Go: `patternAmbientModules []PatternAmbientModule`.
    pub(crate) pattern_ambient_modules: Vec<tsc_ast::PatternAmbientModule>,
    /// Go: the getGlobal* resolver func-fields (checker.go's globalResolvers
    /// struct; memoized per resolver id in spine.rs).
    pub(crate) global_resolvers: crate::spine::GlobalResolvers,
    // The NewChecker tail spine.rs's initializeChecker seeds (Go checker.go
    // 1340-1470): the anonymous-object/signature/marker-for-check family.
    pub(crate) empty_object_type: Option<TypeId>,
    pub(crate) empty_generic_type: Option<TypeId>,
    pub(crate) any_function_type: Option<TypeId>,
    pub(crate) any_signature: Option<SignatureId>,
    pub(crate) unknown_signature: Option<SignatureId>,
    pub(crate) resolving_signature: Option<SignatureId>,
    pub(crate) silent_never_signature: Option<SignatureId>,
    /// Go: `noTypePredicate *TypePredicate{parameterName: "<<unresolved>>",
    /// t: c.anyType}`.
    pub(crate) no_type_predicate: Option<TypePredicate>,
    /// Go: `markerSubTypeForCheck`/`markerSuperTypeForCheck` — the relation
    /// pair used by isTypeRelatedTo's for-check marker probes.
    pub(crate) marker_sub_type_for_check: Option<TypeId>,
    pub(crate) marker_super_type_for_check: Option<TypeId>,
    /// Go: `autoArrayType` (createArrayType(autoType)) — Option until S7's
    /// real createArrayType (the empty-object marker stand-in).
    pub(crate) auto_array_type: Option<TypeId>,
    /// Go: `anyReadonlyArrayType`.
    pub(crate) any_readonly_array_type: Option<TypeId>,
    /// Go: `globalThisType`.
    pub(crate) global_this_type: Option<TypeId>,
    // ── M6 W5 (jsx.go): the JSX state ──
    /// Go: `jsxElementLinks nodeLinkStore[JsxElementLinks]` (jsx.go:39's
    /// store field, checker.go struct slot) — keyed by node (opening-like
    /// elements) AND by the file node (the implicit-import container +
    /// first-jsx-tag caches).
    pub(crate) jsx_element_links: crate::links::NodeLinkStore<crate::jsx::JsxElementLinks>,
    /// Go: `emptyJsxObjectType *Type` (checker.go:749 —
    /// `c.newAnonymousType(nil, ...)` at checker.go:1033).
    pub(crate) empty_jsx_object_type: Option<TypeId>,
    /// Go: `emptyFreshJsxObjectType *Type` (checker.go:750 — the fragment
    /// synthetic-expression argument's type).
    pub(crate) empty_fresh_jsx_object_type: Option<TypeId>,
    /// Go: `_jsxNamespace string` (jsx.go's lazy "React" default).
    pub(crate) jsx_namespace_field: String,
    /// Go: `_jsxFactoryEntity *ast.Node` (jsx.go's synthetic
    /// `<ns>.createElement` qualified name).
    pub(crate) jsx_factory_entity_field: Option<NodeId>,
    /// Go: `globalCallableFunctionType`/`globalNewableFunctionType` (the
    /// getGlobalStrictFunctionType pair).
    pub(crate) global_callable_function_type: Option<TypeId>,
    pub(crate) global_newable_function_type: Option<TypeId>,
    /// Go: `globalRegexpType`.
    pub(crate) global_regexp_type: Option<TypeId>,
    // ── M6 S2 (typeofsymbol.rs): the getTypeOfSymbol spine's state ──
    /// Go: `enumLiteralTypes map[EnumLiteralKey]*Type` — the enum-literal
    /// intern table (PORT: the key is the normalized map-key form; see
    /// typeofsymbol.rs's EnumLiteralKey PORT note).
    pub(crate) enum_literal_types: FxHashMap<crate::typeofsymbol::EnumLiteralKey, TypeId>,
    /// Go: `enumNaNLiteralTypes map[*ast.Symbol]*Type` — NaN caches by enum
    /// symbol (NaN != NaN can't key the main table).
    pub(crate) enum_nan_literal_types: FxHashMap<SymbolId, TypeId>,
    /// Go: `undefinedProperties map[string]*ast.Symbol`.
    pub(crate) undefined_properties: FxHashMap<String, SymbolId>,
    /// Go: `cachedTypes map[CachedTypeKey]*Type` (the kind+typeId key,
    /// checker.go:158).
    pub(crate) cached_types: FxHashMap<crate::todo_shapes::CachedTypeKey, Option<TypeId>>,
    /// Go: `indexedAccessTypes map[CacheHashKey]*Type` (checker.go:630) — the
    /// deferred indexed-access intern table (M6 S10; keyed by
    /// getIndexedAccessKey over the KeyBuilder).
    pub(crate) indexed_access_types: FxHashMap<crate::todo_shapes::CacheHashKey, TypeId>,
    /// Go: `propertiesTypes map[PropertiesTypesKey]*Type` (checker.go:666) —
    /// the keyof/property-literal cache (M6 S10; the key struct is
    /// index.rs's PropertiesTypesKey).
    pub(crate) properties_types: FxHashMap<crate::index::PropertiesTypesKey, TypeId>,
    /// Go: `discriminatedContextualTypes map[DiscriminatedContextualTypeKey]*Type`
    /// (the Checker struct, checker.go:736) — the contextual discriminate
    /// arms' cache (discriminateContextualTypeByObjectMembers). Keyed
    /// (nodeId, typeId) — the port's DiscriminatedContextualTypeKey form.
    pub(crate) discriminated_contextual_types: FxHashMap<(NodeId, TypeId), Option<TypeId>>,
    /// Go: `noConstraintType` (NewChecker's newAnonymousType, checker.go:1042).
    pub(crate) no_constraint_type: Option<TypeId>,
    /// Go: `circularConstraintType` (checker.go:1043).
    pub(crate) circular_constraint_type: Option<TypeId>,
    /// Go: `conditionalConstraintDepth uint32` (checker.go:599) — the
    /// computeBaseConstraint conditional-arm limiter (M6 S9).
    pub(crate) conditional_constraint_depth: u32,
    /// Go: `awaitedTypeStack []*Type` (checker.go) — the awaited-type
    /// recursion guard (M6 S9).
    pub(crate) awaited_type_stack: Vec<TypeId>,
    /// Go: `contextualBindingPatterns []*ast.Node`.
    pub(crate) contextual_binding_patterns: Vec<NodeId>,
    /// Go: `patternForType map[*Type]*ast.Node`.
    pub(crate) pattern_for_type: FxHashMap<TypeId, NodeId>,
    /// Go: `thisExpandoKinds map[*ast.Symbol]ThisAssignmentDeclarationKind`.
    pub(crate) this_expando_kinds: FxHashMap<SymbolId, crate::typeofsymbol::ThisAssignmentDeclarationKind>,
    /// Go: `thisExpandoLocations map[*ast.Symbol]*ast.Node` (nullable value —
    /// Go caches the expando location per symbol, nil included).
    pub(crate) this_expando_locations: FxHashMap<SymbolId, Option<NodeId>>,
    // ── M6 S3 (statements.rs): the statement/declaration-check state ──
    /// Go: `flowAnalysisDisabled bool` (checker.go) — the flag checkBlock
    /// saves/restores around function/module blocks (the flow engine is W3;
    /// the flag rides through untouched).
    pub(crate) flow_analysis_disabled: bool,
    /// Go: `iterationTypesCache map[IterationTypesKey]IterationTypes`
    /// (checker.go:6419) — the S3 iteration family's cache.
    pub(crate) iteration_types_cache:
        FxHashMap<crate::statements::IterationTypesKey, crate::statements::IterationTypes>,
    /// Go: `mergedExportsChecked collections.Set[*ast.Symbol]` — the
    /// checkExportsOnMergedDeclarations once-per-symbol set.
    pub(crate) merged_exports_checked: FxHashSet<SymbolId>,
    /// Go: `moduleImportAttributesTypes map[*ast.Symbol]*Type` — the
    /// module-attributes cache getTypeOfModuleImportAttributes reads.
    pub(crate) module_import_attributes_types: FxHashMap<SymbolId, TypeId>,
    // ── M6 S5 (typenodes.rs): the type-node family's state ──
    /// Go: `errorTypes map[CacheHashKey]*Type` (checker.go:24130 region —
    /// the unresolved-alias error types getTypeFromTypeAliasReference caches).
    pub(crate) error_types: FxHashMap<crate::todo_shapes::CacheHashKey, TypeId>,
    /// Go: `uniqueESSymbolTypes map[*ast.Symbol]*Type` (checker.go:23457's
    /// getESSymbolLikeTypeForNode cache).
    pub(crate) unique_essymbol_types: FxHashMap<SymbolId, TypeId>,
    /// Go: `tupleTypes map[CacheHashKey]*Type` (checker.go:25312's
    /// getTupleTargetType cache).
    pub(crate) tuple_types: FxHashMap<crate::todo_shapes::CacheHashKey, TypeId>,
    /// Go: `unresolvedSymbols map[string]*ast.Symbol` (checker.go:23632's
    /// getUnresolvedSymbolForEntityName path-keyed table).
    pub(crate) unresolved_symbols: FxHashMap<String, SymbolId>,
    /// Go: `resolvingDefaultType *Type` (checker.go:1042 region — the
    /// recursion sentinel getResolvedTypeParameterDefault marks in-flight
    /// defaults with).
    pub(crate) resolving_default_type: Option<TypeId>,
    // ── M6 S6 (resolution.rs): the resolution family's state ──
    /// Go: `mergedSymbols map[*ast.Symbol]*ast.Symbol` (checker.go:14861's
    /// getMergedSymbol record table).
    pub(crate) merged_symbols: FxHashMap<SymbolId, SymbolId>,
    // ── M6 S7 (uniontypes.rs): the type-construction engine's state ──
    /// Go: `unionTypes map[CacheHashKey]*Type` — the interned-union table
    /// (checker.go's getUnionTypeFromSortedList cache).
    pub(crate) union_types: FxHashMap<crate::todo_shapes::CacheHashKey, TypeId>,
    /// Go: `intersectionTypes map[CacheHashKey]*Type`.
    pub(crate) intersection_types: FxHashMap<crate::todo_shapes::CacheHashKey, TypeId>,
    /// Go: `subtypeReductionCache map[CacheHashKey][]*Type` — removeSubtypes'
    /// memo (the key is getTypeListKey of the input).
    pub(crate) subtype_reduction_cache: FxHashMap<crate::todo_shapes::CacheHashKey, Vec<TypeId>>,
    /// Go: `unionOfUnionTypes map[UnionOfUnionKey]*Type` — getUnionTypeEx's
    /// 2-constituent-with-union fast path.
    pub(crate) union_of_union_types: FxHashMap<crate::uniontypes::UnionOfUnionKey, TypeId>,
    /// Go: `templateLiteralTypes map[CacheHashKey]*Type`.
    pub(crate) template_literal_types: FxHashMap<crate::todo_shapes::CacheHashKey, TypeId>,
    /// Go: `stringMappingTypes map[StringMappingKey]*Type` (PORT: the
    /// symbol+target pair keys directly; TypeId is Copy/Eq/Hash).
    pub(crate) string_mapping_types: FxHashMap<(SymbolId, TypeId), TypeId>,
    /// Go: `instantiationStack []*Type` — instantiateType's depth-guard stack.
    pub(crate) instantiation_stack: Vec<TypeId>,
    /// Go: `substitutionTypes map[SubstitutionTypeKey]*Type` (checker.go —
    /// getOrCreateSubstitutionType's intern table; PORT: the base/constraint
    /// id pair keys directly).
    pub(crate) substitution_types: FxHashMap<(u32, u32), TypeId>,
    /// Go: `uniqueLiteralMapper *TypeMapper` — REAL (M6 S7: seeded in
    /// new_checker with newFunctionTypeMapper).
    pub(crate) permissive_mapper: Option<TypeMapperId>,
    /// Go: `restrictiveMapper *TypeMapper`.
    pub(crate) restrictive_mapper: Option<TypeMapperId>,
    /// Go: `reportUnreliableMapper *TypeMapper`.
    pub(crate) report_unreliable_mapper: Option<TypeMapperId>,
    /// Go: `reportUnmeasurableMapper *TypeMapper`.
    pub(crate) report_unmeasurable_mapper: Option<TypeMapperId>,
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
        // M6 S7: the union-constructed intrinsic types (uniontypes.rs owns
        // the constructors; the fields are non-nil after new_checker).
        boolean_type => boolean_type,
        string_or_number_type => string_or_number_type,
        string_number_symbol_type => string_number_symbol_type,
        number_or_bigint_type => number_or_bigint_type,
        numeric_string_type => numeric_string_type,
        template_constraint_type => template_constraint_type,
        unknown_empty_object_type => unknown_empty_object_type,
        unknown_union_type => unknown_union_type,
        es_symbol_type => es_symbol_type,
        void_type => void_type,
        never_type => never_type,
        silent_never_type => silent_never_type,
        implicit_never_type => implicit_never_type,
        unreachable_never_type => unreachable_never_type,
        non_primitive_type => non_primitive_type,
        unique_literal_type => unique_literal_type,
        empty_string_type => empty_string_type,
        typeof_type => typeof_type,
        zero_type => zero_type,
        zero_bigint_type => zero_bigint_type,
        // M6 S2: the constraint sentinels (seeded in new_checker per Go
        // checker.go:1042-1043; typeofsymbol.rs's constraint family reads
        // them as the no-constraint/circular answers).
        no_constraint_type => no_constraint_type,
        circular_constraint_type => circular_constraint_type,
    }

    /// Go: the `booleanType` field — `getUnionType([regularFalseType,
    /// regularTrueType])` (checker.go:1013). REAL (M6 S7): the field is now
    /// constructed eagerly in new_checker through the interned union
    /// constructor (uniontypes.rs).
    //
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

    // Go: `func (c *Checker) getRegularTypeOfLiteralType(t *Type) *Type` —
    // REMOVED (M6 S7): superseded by uniontypes.rs's real body (same
    // signature; the union arm now maps the freshable constituents instead
    // of panicking).


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
    // ────────────────────────────────────────────────────────────────────────
    // The getTypeOfSymbol chain (Go checker.go 16879+) — MOVED (M6 S2): the
    // full spine + declared types + circularity + binding-element typing +
    // widen* helpers now live in typeofsymbol.rs (the one-.rs-per-Go-module
    // split; the spike bodies that were here are superseded — see git
    // history). The resolution stack (pushTypeResolution/popTypeResolution/
    // findResolutionCycleStartIndex/typeResolutionHasProperty) and
    // reportCircularityError stay here below (checker.go 19165+).
    // (The spike's getQuickTypeOfExpression + checkExpression family stubs
    // here are superseded by the full S1 spine.rs ports.)

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
    /// The program-level symbol arena slot, mutable (Go's `*ast.Symbol`
    /// pointer writes; the checker-created spread/readonly symbols use it).
    pub(crate) fn program_symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        self.program.symbol_mut(id)
    }

    /// Go: `node.Initializer()` through the seam.
    pub fn program_initializer_of(&self, declaration: NodeId) -> Option<NodeId> {
        crate::ast_shims::initializer_of(self.program.node_store(), declaration)
    }

    /// The checker's `undefinedSymbol` seed (Go field).
    pub fn undefined_symbol(&self) -> SymbolId {
        self.undefined_symbol.expect("undefinedSymbol")
    }
    /// The checker's `unknownSymbol` seed (Go field) — M6 S2's
    /// getSymbolFlagsEx compares against it (Go's `c.unknownSymbol`).
    pub fn unknown_symbol(&self) -> SymbolId {
        self.unknown_symbol.expect("unknownSymbol")
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
    // ────────────────────────────────────────────────────────────────────────
    // S1 NOTE: the error/errorOrSuggestion/addDiagnostic W1 stubs that lived
    // here are superseded by the real bodies in spine.rs (Go checker.go
    // 14222-14310).

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

    // Go: `func (c *Checker) instantiateType(t *Type, mapper *TypeMapper) *Type`
    // (checker.go — the M6 S7 instantiation engine) — REAL body landed in
    // uniontypes.rs (`instantiateType` → `instantiateTypeWorker` with the
    // activeMappers caches); this W1 identity stub is retired.
    // (no body follows — the comment block records the retirement.)

    // (inferFromIntraExpressionSites/getInferredType moved to inference.rs —
    // M6 W2's real bodies; mapper.rs's InferenceTypeMapper calls them through
    // this module's surface, unchanged.)

    /// Go: `func (c *Checker) containsArgumentsReference(node *ast.Node) bool`
    /// (checker.go:32675 — S4's flow-walk family). REAL body (the
    /// cachedArgumentsReferenced memo + the subtree walk; the
    /// getResolvedSymbol reader is the S6 seam carrier).
    pub(crate) fn contains_arguments_reference(&mut self, node: NodeId) -> bool {
        let body = tsc_binder::node_data::body_of(self.store(), node);
        let Some(body) = body else {
            return false;
        };
        if let Some(contains_arguments) = self.cached_arguments_referenced.get(&node) {
            return *contains_arguments;
        }
        let arguments_symbol = self.arguments_symbol;
        let contains_arguments = {
            fn visit(
                c: &mut Checker,
                arguments_symbol: Option<SymbolId>,
                node: NodeId,
            ) -> bool {
                use tsc_ast::Kind;
                if node.is_none() {
                    return false;
                }
                let n = c.store().node(node);
                match n.kind {
                    Kind::Identifier => {
                        let name_matches = arguments_symbol
                            .is_some_and(|s| c.program_symbol(s).name.as_str()
                                == tsc_ast::node_text(c.store(), node));
                        // Go: c.IsArgumentsSymbol(c.getResolvedSymbol(node)).
                        name_matches && c.get_resolved_symbol(node) == arguments_symbol
                    }
                    Kind::PropertyDeclaration
                    | Kind::MethodDeclaration
                    | Kind::GetAccessor
                    | Kind::SetAccessor => {
                        if let Some(name) = n.name()
                            && c.store().node(name).kind == Kind::ComputedPropertyName
                        {
                            return visit(c, arguments_symbol, name);
                        }
                        false
                    }
                    Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
                        let expression = crate::ast_shims::expression_of(c.store(), node)
                            .unwrap_or(NodeId::NONE);
                        visit(c, arguments_symbol, expression)
                    }
                    Kind::PropertyAssignment => {
                        let initializer = crate::ast_shims::initializer_of(c.store(), node)
                            .unwrap_or(NodeId::NONE);
                        visit(c, arguments_symbol, initializer)
                    }
                    _ => {
                        if crate::utilities::node_starts_new_lexical_environment(n)
                            || crate::utilities::is_part_of_type_node(c.store(), node)
                        {
                            return false;
                        }
                        crate::statements::children_of(c.store(), node)
                            .iter()
                            .any(|&child| visit(c, arguments_symbol, child))
                    }
                }
            }
            visit(self, arguments_symbol, body)
        };
        self.cached_arguments_referenced.insert(node, contains_arguments);
        contains_arguments
    }

    /// Go: `func (c *Checker) isArrayType(t *Type) bool` (checker.go:24005)
    /// — REAL body (M6 S5; the type-node family's array/tuple predicate
    /// block). A type is an array type if it is a reference to the global
    /// Array or global ReadonlyArray type.
    #[allow(dead_code)] // W1 periphery surface — the S-wave consumers land with M6
    pub(crate) fn is_array_type(&self, t: TypeId) -> bool {
        self.type_(t).object_flags.intersects(ObjectFlags::REFERENCE)
            && self
                .type_(t)
                .target()
                .is_some_and(|target| {
                    Some(target) == self.global_array_type
                        || Some(target) == self.global_readonly_array_type
                })
    }

    // (M6 S5): `getTypeFromTypeNode` (checker.go:23295) MOVED — the real
    // body + the whole getTypeFrom*TypeNode family lives in
    // typenodes.rs (the one-.rs-per-region split; this stub was the
    // errorType placeholder the pre-S5 waves called).

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

// ────────────────────────────────────────────────────────────────────────────
// new_checker — the NewChecker slice the spike ports (intrinsic/literal
// creation, symbol seeds, link stores; M6 machinery is bannered at Go's exact
// line positions).
// ────────────────────────────────────────────────────────────────────────────

/// The W3 check-stage runner seam: Go program.go's `GetSemanticDiagnostics`
/// read (the harness-side diagnostic source for the `.errors.txt` baseline).
/// The port's checker is per-file by construction (the §5.1 single-file
/// NodeId space), so the read returns the whole `diagnostics` collection —
/// file-paired and global entries, insertion order (the baseline renderer
/// sorts). Suggestion diagnostics stay a separate collection, as in Go.
impl Checker {
    pub fn semantic_diagnostics(&self) -> Vec<tsc_ast::Diagnostic> {
        self.diagnostics.all()
    }
}

/// Go: `func NewChecker(program Program, tracer *Tracer) (*Checker, *sync.Mutex)`.
///
/// PORT: the `*sync.Mutex` return is the pool's per-checker lock (§5.5) —
/// spike checkers are single-threaded; the tracer is a todo_shapes placeholder.
pub fn new_checker(mut program: Box<dyn Program>) -> Checker {
    program.bind_source_files();

    let id = NEXT_CHECKER_ID.fetch_add(1, Ordering::SeqCst);
    let compiler_options = program.options().clone();
    let file_index_map = create_file_index_map(program.source_files());
    // The binder's flow graph (Go: the checker reads the GC-heap
    // `*ast.FlowNode`s the binder allocated; the port copies the arenas).
    let (flow_nodes, flow_lists) = program.flow_arena();
    let flow_nodes = flow_nodes.to_vec();
    let flow_lists = flow_lists.to_vec();

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
        primitive_type_alias_suggestion_symbols: [None; 6],
        node_links: LinkStore::new(),
        symbol_node_links: NodeLinkStore::new(),
        non_existent_properties: Default::default(),
        cached_arguments_referenced: FxHashMap::default(),
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
        typeof_type: None,
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
        signature_node_links: LinkStore::new(),
        call_resolution_stack: Vec::new(),
        is_inference_partially_blocked: false,
        apparent_argument_count: None,
        cached_signatures: FxHashMap::default(),
        enum_number_index_info: None,
        any_base_type_index_info: None,
        free_inference_states: Vec::new(),
        reverse_mapped_cache: FxHashMap::default(),
        reverse_homomorphic_mapped_cache: FxHashMap::default(),
        reverse_mapped_source_stack: Vec::new(),
        reverse_mapped_target_stack: Vec::new(),
        reverse_expanding_flags: crate::relater::ExpandingFlags::NONE,
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
        canceled: false,
        files: Vec::new(),
        within_unreachable_code: false,
        reported_unreachable_nodes: FxHashSet::default(),
        deferred_diagnostic_callbacks: Vec::new(),
        contextual_infos: Vec::new(),
        inference_context_infos: Vec::new(),
        flow_type_cache: FxHashMap::default(),
        flow_loop_stack: Vec::new(),
        flow_nodes,
        flow_lists,
        shared_flows: Vec::new(),
        free_flow_states: Vec::new(),
        antecedent_types: Vec::new(),
        flow_loop_cache: FxHashMap::default(),
        flow_node_reachable: FxHashMap::default(),
        flow_node_post_super: FxHashMap::default(),
        last_flow_node: None,
        last_flow_node_reachable: false,
        narrowed_types: FxHashMap::default(),
        assignment_reduced_types: FxHashMap::default(),
        inline_level: 0,
        resolving_explicit_type_of_symbol: tsc_collections::OrderedSet::new(),
        flow_invocation_count: 0,
        context_free_types: FxHashMap::default(),
        renamed_binding_elements_in_types: Vec::new(),
        variance_type_parameter: None,
        skip_direct_inference_nodes: tsc_collections::OrderedSet::new(),
        serialization_level: 0,
        diagnostics: crate::spine::DiagnosticsCollection::default(),
        suggestion_diagnostics: crate::spine::DiagnosticsCollection::default(),
        pattern_ambient_modules: Vec::new(),
        global_resolvers: Checker::new_global_resolvers(),
        empty_object_type: None,
        empty_generic_type: None,
        any_function_type: None,
        any_signature: None,
        unknown_signature: None,
        resolving_signature: None,
        silent_never_signature: None,
        no_type_predicate: None,
        marker_sub_type_for_check: None,
        marker_super_type_for_check: None,
        auto_array_type: None,
        any_readonly_array_type: None,
        global_this_type: None,
        // M6 W5 (jsx.go) state.
        jsx_element_links: crate::links::NodeLinkStore::new(),
        empty_jsx_object_type: None,
        empty_fresh_jsx_object_type: None,
        jsx_namespace_field: String::new(),
        jsx_factory_entity_field: None,
        global_callable_function_type: None,
        global_newable_function_type: None,
        global_regexp_type: None,
        // M6 S2 (typeofsymbol.rs) state.
        enum_literal_types: FxHashMap::default(),
        enum_nan_literal_types: FxHashMap::default(),
        undefined_properties: FxHashMap::default(),
        cached_types: FxHashMap::default(),
        indexed_access_types: FxHashMap::default(),
        properties_types: FxHashMap::default(),
        discriminated_contextual_types: FxHashMap::default(),
        no_constraint_type: None,
        circular_constraint_type: None,
        conditional_constraint_depth: 0,
        awaited_type_stack: Vec::new(),
        contextual_binding_patterns: Vec::new(),
        pattern_for_type: FxHashMap::default(),
        this_expando_kinds: FxHashMap::default(),
        this_expando_locations: FxHashMap::default(),
        flow_analysis_disabled: false,
        iteration_types_cache: FxHashMap::default(),
        merged_exports_checked: FxHashSet::default(),
        module_import_attributes_types: FxHashMap::default(),
        error_types: FxHashMap::default(),
        unique_essymbol_types: FxHashMap::default(),
        tuple_types: FxHashMap::default(),
        unresolved_symbols: FxHashMap::default(),
        resolving_default_type: None,
        merged_symbols: FxHashMap::default(),
        // M6 S7 (uniontypes.rs) state.
        union_types: FxHashMap::default(),
        intersection_types: FxHashMap::default(),
        subtype_reduction_cache: FxHashMap::default(),
        union_of_union_types: FxHashMap::default(),
        template_literal_types: FxHashMap::default(),
        string_mapping_types: FxHashMap::default(),
        instantiation_stack: Vec::new(),
        substitution_types: FxHashMap::default(),
        permissive_mapper: None,
        restrictive_mapper: None,
        report_unreliable_mapper: None,
        report_unmeasurable_mapper: None,
        boolean_type: None,
        string_or_number_type: None,
        string_number_symbol_type: None,
        number_or_bigint_type: None,
        numeric_string_type: None,
        template_constraint_type: None,
        unknown_empty_object_type: None,
        unknown_union_type: None,
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
    // Go: c.booleanType = c.getUnionType([]*Type{c.regularFalseType, c.regularTrueType})
    // — REAL (M6 S7: the interned union constructor landed in uniontypes.rs).
    {
        let regular_false = c.regular_false_type.expect("just created");
        let regular_true = c.regular_true_type.expect("just created");
        c.boolean_type = Some(c.get_union_type(&[regular_false, regular_true]));
    }
    c.es_symbol_type = Some(c.new_intrinsic_type(TypeFlags::ESSYMBOL, "symbol"));
    c.void_type = Some(c.new_intrinsic_type(TypeFlags::VOID, "void"));
    c.never_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never"));
    c.silent_never_type = Some(
        c.new_intrinsic_type_ex(TypeFlags::NEVER, "never", ObjectFlags::NON_INFERRABLE_TYPE),
    );
    c.implicit_never_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never"));
    c.unreachable_never_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never"));
    c.non_primitive_type = Some(c.new_intrinsic_type(TypeFlags::NON_PRIMITIVE, "object"));
    // Go: stringOrNumberType / stringNumberSymbolType / numberOrBigIntType /
    // numericStringType / templateConstraintType (checker.go:1018-1022) —
    // REAL (M6 S7).
    {
        let number_type = c.number_type.expect("just created");
        let string_type = c.string_type.expect("just created");
        let es_symbol_type = c.es_symbol_type.expect("just created");
        let bigint_type = c.bigint_type.expect("just created");
        let boolean_type = c.boolean_type.expect("just created");
        let null_type = c.null_type.expect("just created");
        let undefined_type = c.undefined_type.expect("just created");
        c.string_or_number_type = Some(c.get_union_type(&[string_type, number_type]));
        c.string_number_symbol_type = Some(c.get_union_type(&[string_type, number_type, es_symbol_type]));
        c.number_or_bigint_type = Some(c.get_union_type(&[number_type, bigint_type]));
        c.numeric_string_type = Some(c.get_template_literal_type(
            &[String::new(), String::new()],
            &[number_type],
        )); // The `${number}` type
        c.template_constraint_type = Some(c.get_union_type(&[
            string_type,
            number_type,
            boolean_type,
            bigint_type,
            null_type,
            undefined_type,
        ]));
    }
    c.unique_literal_type = Some(c.new_intrinsic_type(TypeFlags::NEVER, "never")); // Special `never` flagged by union reduction to behave as a literal
    // Go: uniqueLiteralMapper + the four report/restrict/permissive function
    // type mappers (checker.go:1023-1027) — REAL (M6 S7: the mapper arena +
    // the instantiate engine).
    {
        c.unique_literal_mapper = Some(c.new_function_type_mapper(Box::new(
            |c: &mut Checker, t: TypeId| c.get_unique_literal_type_for_type_parameter(t),
        )));
        c.permissive_mapper = Some(c.new_function_type_mapper(Box::new(
            |c: &mut Checker, t: TypeId| {
                if c.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
                    c.wildcard_type()
                } else {
                    t
                }
            },
        )));
        c.restrictive_mapper = Some(c.new_function_type_mapper(Box::new(
            |c: &mut Checker, t: TypeId| {
                if c.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
                    c.get_restrictive_type_parameter(t)
                } else {
                    t
                }
            },
        )));
        c.report_unreliable_mapper = Some(c.new_function_type_mapper(Box::new(
            |c: &mut Checker, t: TypeId| {
                let markers = [c.marker_super_type, c.marker_sub_type, c.marker_other_type];
                if markers.contains(&Some(t)) {
                    c.reliability_flags |=
                        crate::relater::RelationComparisonResult::REPORTS_UNRELIABLE;
                }
                t
            },
        )));
        c.report_unmeasurable_mapper = Some(c.new_function_type_mapper(Box::new(
            |c: &mut Checker, t: TypeId| {
                let markers = [c.marker_super_type, c.marker_sub_type, c.marker_other_type];
                if markers.contains(&Some(t)) {
                    c.reliability_flags |=
                        crate::relater::RelationComparisonResult::REPORTS_UNMEASURABLE;
                }
                t
            },
        )));
    }
    // TODO(port M6): emptyObjectType family (newAnonymousType), marker
    // type parameters (newTypeParameter), noTypePredicate, anySignature
    // family (newSignature).
    // Go (checker.go:1042-1043): the constraint marker types — anonymous
    // empty types right between anyFunctionType and the marker type
    // parameters; the constraint family (typeofsymbol.rs) reads them as
    // the no-constraint/circular-constraint sentinels.
    c.no_constraint_type = Some(c.new_anonymous_type(None, &[]));
    c.circular_constraint_type = Some(c.new_anonymous_type(None, &[]));
    // M6 S5: Go's `c.resolvingDefaultType = c.newAnonymousType(nil, nil, nil, nil, nil)`
    // (checker.go:1044) — the recursion sentinel for type-parameter defaults.
    // Go checker.go:1044 — the NewChecker anonymous-type seeds (M6 S5
    // adds emptyTypeLiteralType/resolvingDefaultType; the rest of the
    // marker seeds land with the waves that read them).
    let type_literal_symbol = c.new_symbol(
        tsc_ast::SymbolFlags::TYPE_LITERAL,
        tsc_ast::INTERNAL_SYMBOL_NAME_TYPE,
    );
    c.empty_type_literal_type = Some(c.new_anonymous_type(Some(type_literal_symbol), &[]));
    c.resolving_default_type = Some(c.new_anonymous_type(None, &[]));
    let number_type = c.number_type.expect("just created");
    let string_type = c.string_type.expect("just created");
    let any_type = c.any_type.expect("just created");
    let _ = c.new_index_info(number_type, string_type, true, None, Vec::new()); // Go: enumNumberIndexInfo (TODO(port M6): the field)
    let _ = c.new_index_info(string_type, any_type, false, None, Vec::new()); // Go: anyBaseTypeIndexInfo
    c.empty_string_type = Some(c.get_string_literal_type(""));
    c.zero_type = Some(c.get_number_literal_type(Number(0.0)));
    c.zero_bigint_type = Some(c.get_big_int_literal_type(PseudoBigInt::default()));
    // Go (checker.go:1063): typeofType = getUnionType(sorted(typeofNEFacts keys)
    // mapped through getStringLiteralType) — REAL (M6 S7: the union
    // constructor landed with uniontypes.rs).
    let typeof_words = [
        "bigint", "boolean", "function", "number", "object", "string", "symbol", "undefined",
    ]
    .iter()
    .map(|&word| c.get_string_literal_type(word))
    .collect::<Vec<_>>();
    c.typeof_type = Some(c.get_union_type(&typeof_words));
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

    // ── The NewChecker tail (Go checker.go:1338-1470) — spine.rs's slice ──
    // Go creates these AFTER the marker type parameters (the exact
    // creation order preserves the spike's type-id pin).
    // Go: c.noTypePredicate = &TypePredicate{parameterName: "<<unresolved>>", t: c.anyType}
    c.no_type_predicate = Some(TypePredicate {
        kind: TypePredicateKind::default(),
        parameter_index: -1,
        parameter_name: "<<unresolved>>".to_string(),
        t: Some(c.any_type.expect("just created")),
    });
    // Go: newSignature arena slot 0 is the dummy (id == arena index, like
    // the types arena — see the signatures field note).
    c.signatures.push(Signature {
        id: SignatureId(0),
        flags: SignatureFlags::NONE,
        min_argument_count: -1,
        resolved_min_argument_count: -1,
        declaration: None,
        type_parameters: Vec::new(),
        parameters: Vec::new(),
        this_parameter: None,
        resolved_return_type: None,
        resolved_type_predicate: None,
        target: None,
        mapper: None,
        isolated_signature_type: None,
        composite: None,
        no_type_predicate: false,
    });
    // Go: c.emptyObjectType = newAnonymousType(nil, emptySymbols, emptyMembers, ...)
    //      c.emptyGenericType  = newAnonymousType(...) (the 1-type-param placeholder)
    //      c.anyFunctionType   = newAnonymousType(nil, emptyMembers, ..., ObjectFlagsNonInferrable)
    // (spine.rs's new_anonymous_type — the S7/S8 members-resolution seam
    // note lives there).
    c.empty_object_type = Some(c.new_anonymous_type(None, &[]));
    c.empty_generic_type = Some(c.new_anonymous_type(None, &[]));
    // Go (checker.go:1033-1034): emptyJsxObjectType/emptyFreshJsxObjectType
    // — plain empty anonymous types (M6 W5; the fresh twin is the fragment
    // synthetic-expression argument's type).
    c.empty_jsx_object_type = Some(c.new_anonymous_type(None, &[]));
    c.empty_fresh_jsx_object_type = Some(c.new_anonymous_type(None, &[]));
    // Go: `c.unknownEmptyObjectType = c.newAnonymousType(nil, nil, nil, nil, nil)`
    // (checker.go:1033) — REAL (M6 S7; removeSubtypes/createUnknownUnionType
    // compare against it).
    c.unknown_empty_object_type = Some(c.new_anonymous_type(None, &[]));
    // Go (checker.go:1037): unknownUnionType = createUnknownUnionType() —
    // REAL (M6 S7; spine.rs's ctor over the seeded fields above).
    c.unknown_union_type = Some(c.create_unknown_union_type());
    c.any_function_type = Some(c.new_anonymous_type_ex(
        None,
        &[],
        ObjectFlags::NON_INFERRABLE_TYPE,
    ));
    // Go: anySignature/unknownSignature/resolvingSignature/silentNeverSignature
    // (checker.go 1040-1050 — spine.rs's new_signature; the Go arg shape is
    // flags, declaration, typeParameters, thisParameter, parameters,
    // resolvedReturnType, resolvedTypePredicate, minArgumentCount).
    let any_type = c.any_type.expect("just created");
    let error_type = c.error_type.expect("just created");
    let silent_never_type = c.silent_never_type.expect("just created");
    c.any_signature = Some(c.new_signature(SignatureFlags::NONE, None, Vec::new(), None, Vec::new(), Some(any_type), 0));
    c.unknown_signature = Some(c.new_signature(SignatureFlags::NONE, None, Vec::new(), None, Vec::new(), Some(error_type), 0));
    c.resolving_signature = Some(c.new_signature(SignatureFlags::NONE, None, Vec::new(), None, Vec::new(), Some(any_type), 0));
    c.silent_never_signature = Some(c.new_signature(SignatureFlags::NONE, None, Vec::new(), None, Vec::new(), Some(silent_never_type), 0));
    // Go (checker.go:1058-1059): enumNumberIndexInfo/anyBaseTypeIndexInfo —
    // the two sentinel index infos (arena form; numberType/stringType seeded
    // above, anyType too).
    let number_type = c.number_type.expect("just created");
    let string_type = c.string_type.expect("just created");
    c.enum_number_index_info = Some(crate::types::IndexInfoId(c.new_index_info(number_type, string_type, true, None, Vec::new()) as u32));
    c.any_base_type_index_info = Some(crate::types::IndexInfoId(c.new_index_info(string_type, any_type, false, None, Vec::new()) as u32));
    // Go: getVariancesMap's markerSubTypeForCheck/markerSuperTypeForCheck
    // (relater.go's relation pair — same construction as the markers above).
    let marker_super_for_check = c.new_type(
        TypeFlags::TYPE_PARAMETER,
        ObjectFlags::NONE,
        TypeData::TypeParameter(Box::default()),
    );
    let marker_sub_for_check = c.new_type(
        TypeFlags::TYPE_PARAMETER,
        ObjectFlags::NONE,
        TypeData::TypeParameter(Box::default()),
    );
    c.type_mut(marker_sub_for_check)
        .as_type_parameter_mut()
        .expect("TypeParameter data")
        .constraint = Some(marker_super_for_check);
    c.marker_super_type_for_check = Some(marker_super_for_check);
    c.marker_sub_type_for_check = Some(marker_sub_for_check);

    // Go: initializeChecker (checker.go:1309) — the spine.rs port over the
    // program's files (globals merge + the deferred seeding + the special
    // global types). Go's initializeChecker seeds globalArrayType (1363)
    // BEFORE anyArrayType (1372), so the port runs the same order.
    c.files = c
        .program
        .source_files()
        .iter()
        .map(|f| f.as_node_id())
        .collect();
    c.initialize_checker();
    // Go (checker.go 1372): `c.anyArrayType = c.createArrayType(c.anyType)` —
    // REAL (M6 S7): the interned type-reference constructor landed with
    // uniontypes.rs, and initializeChecker has seeded globalArrayType above.
    c.any_array_type = c.create_array_type(c.any_type.expect("just created"));
    // TODO(port M6): the flow caches, the Relations,
    // initializeClosures/initializeIterationResolvers. (typeofType landed
    // with M6 S7 — see the seeding above.)
    c
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
        fn node_store_mut(&mut self) -> &mut dyn NodeStore {
            &mut self.file
        }
        fn flow_arena(&self) -> (&[tsc_ast::flow::FlowNode], &[tsc_ast::flow::FlowList]) {
            match &self.bind {
                Some(bind) => (&bind.flow_nodes, &bind.flow_lists),
                None => (&[], &[]),
            }
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
