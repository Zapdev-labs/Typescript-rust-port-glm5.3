// Ported from tsc/internal/checker/relater.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The subtype/assignability engine (M6 W2). Function-by-function mirror of
// relater.go; callees owned by the not-yet-landed checker.go slices (S1–S10),
// flow.go, jsx.go, nodebuilder and the union/instantiation engine (S7) are
// permissive stubs in the `Unported-wave callees` section at the bottom —
// each carries `TODO(port M6 <slice>)` naming the wave that replaces it, per
// the W1 stub convention (real body where the callee is arena-local).
//
// PORT notes (structural, Go → Rust):
//   * `*Relation` pointer identity (the four + one instances compared with
//     `r.relation == c.identityRelation`) → [`RelationKind`]: the instances
//     are fixed at NewChecker time, so enum equality is Go's pointer equality.
//     The `*Relation` parameter type ports as the kind; the result caches stay
//     in the per-kind [`Relation`] holders on the checker.
//   * `ErrorReporter func(...)` / `TypeComparer func(...)` closures: the only
//     ErrorReporter implementations in the entire engine are `nil` and the
//     active relater's `reportError`, and the TypeComparer implementations
//     are the five fixed comparers (`compareTypesIdentical`, `…Assignable`,
//     `…SubtypeOf`, `r.isRelatedToSimple`, `r.isRelatedToWorker`) — they port
//     as [`ErrorReporterKind`]/[`TypeComparerKind`] + the active
//     `Option<&mut Relater>` threaded through (§5.4's explicit-receiver
//     idiom; mapper.rs's thunk precedent). PORT: when a `RelatedToWorker` /
//     `RelatedToSimple` comparer fires with no live relater (only possible
//     through the conditional-inference mapper path, unreachable until S7
//     constructs conditional types), the comparison falls back to
//     `check_type_related_to` with the context's recorded relation — a fresh
//     relater with the same relation; the divergence is limited to mid-flight
//     maybe-key sharing and is bannered at the fallback.
//   * `Ternary` bit-trickery (`result &= related`, `!= 0`) → the enum with
//     helper fns (`ternary_and` picks the lesser in False < Unknown < Maybe <
//     True, `!= TernaryFalse` for Go's `!= 0`).
//   * `ErrorChain` is a Go linked list → `Option<Box<ErrorChain>>`.
//   * `diagnostics` args (`args ...any`) → [`DiagArg`].

use std::cmp::Ordering;

use rustc_hash::{FxHashMap, FxHashSet};
use tsc_ast::{CheckFlags, Kind, ModifierFlags, NodeId, SymbolFlags, SymbolId};
use tsc_core::tristate::Tristate;
use tsc_diagnostics::Message;
use tsc_diagnostics as msg;

use crate::types::{
    ElementFlags, IndexFlags, ObjectFlags, SignatureKind, Ternary,
    TypeFlags, TypeId, TypePredicate, TypePredicateKind, VarianceFlags,
};
use crate::utilities as u;
use crate::Checker;

/// Go: `ast.CheckFlagsPartial = CheckFlagsReadPartial | CheckFlagsWritePartial`
/// (checkflags.go:38 — the ast crate carries the two halves; the combined
/// mask lives here next to its two consumers).
const CHECK_FLAGS_PARTIAL: CheckFlags =
    CheckFlags(CheckFlags::READ_PARTIAL.0 | CheckFlags::WRITE_PARTIAL.0);

// ────────────────────────────────────────────────────────────────────────────
// Flag/enum types (Go relater.go top-of-file)
// ────────────────────────────────────────────────────────────────────────────

define_flags!(SignatureCheckMode, u32);
impl SignatureCheckMode {
    pub const BIVARIANT_CALLBACK: SignatureCheckMode = SignatureCheckMode(1 << 0);
    pub const STRICT_CALLBACK: SignatureCheckMode = SignatureCheckMode(1 << 1);
    pub const IGNORE_RETURN_TYPES: SignatureCheckMode = SignatureCheckMode(1 << 2);
    pub const STRICT_ARITY: SignatureCheckMode = SignatureCheckMode(1 << 3);
    pub const STRICT_TOP_SIGNATURE: SignatureCheckMode = SignatureCheckMode(1 << 4);
    pub const CALLBACK: SignatureCheckMode =
        SignatureCheckMode(Self::BIVARIANT_CALLBACK.0 | Self::STRICT_CALLBACK.0);
}

define_flags!(MinArgumentCountFlags, u32);
impl MinArgumentCountFlags {
    pub const STRONG_ARITY_FOR_UNTYPED_JS: MinArgumentCountFlags = MinArgumentCountFlags(1 << 0);
    pub const VOID_IS_NON_OPTIONAL: MinArgumentCountFlags = MinArgumentCountFlags(1 << 1);
}

define_flags!(IntersectionState, u32);
impl IntersectionState {
    /// Source type is a constituent of an outer intersection
    pub const SOURCE: IntersectionState = IntersectionState(1 << 0);
    /// Target type is a constituent of an outer intersection
    pub const TARGET: IntersectionState = IntersectionState(1 << 1);
}

// Go: checker.go `type IntersectionFlags uint32` (the S7
// getIntersectionTypeEx parameter; values pinned).
define_flags!(IntersectionFlags, u32);
impl IntersectionFlags {
    /// Go: `IntersectionFlagsNoConstraintReduction`.
    pub const NO_CONSTRAINT_REDUCTION: IntersectionFlags = IntersectionFlags(1 << 3);
}

define_flags!(RecursionFlags, u32);
impl RecursionFlags {
    pub const SOURCE: RecursionFlags = RecursionFlags(1 << 0);
    pub const TARGET: RecursionFlags = RecursionFlags(1 << 1);
    pub const BOTH: RecursionFlags = RecursionFlags(Self::SOURCE.0 | Self::TARGET.0);
}

define_flags!(ExpandingFlags, u8);
impl ExpandingFlags {
    pub const SOURCE: ExpandingFlags = ExpandingFlags(1 << 0);
    pub const TARGET: ExpandingFlags = ExpandingFlags(1 << 1);
    pub const BOTH: ExpandingFlags = ExpandingFlags(Self::SOURCE.0 | Self::TARGET.0);
}

define_flags!(RelationComparisonResult, u32);
impl RelationComparisonResult {
    pub const SUCCEEDED: RelationComparisonResult = RelationComparisonResult(1 << 0);
    pub const FAILED: RelationComparisonResult = RelationComparisonResult(1 << 1);
    pub const REPORTS_UNMEASURABLE: RelationComparisonResult = RelationComparisonResult(1 << 3);
    pub const REPORTS_UNRELIABLE: RelationComparisonResult = RelationComparisonResult(1 << 4);
    pub const COMPLEXITY_OVERFLOW: RelationComparisonResult = RelationComparisonResult(1 << 5);
    pub const REPORTS_MASK: RelationComparisonResult =
        RelationComparisonResult(Self::REPORTS_UNMEASURABLE.0 | Self::REPORTS_UNRELIABLE.0);
    pub const OVERFLOW: RelationComparisonResult = Self::COMPLEXITY_OVERFLOW;
}

// Go: checker.go `type MappedTypeModifiers uint32` (values pinned).
define_flags!(MappedTypeModifiers, u32);
impl MappedTypeModifiers {
    pub const INCLUDE_READONLY: MappedTypeModifiers = MappedTypeModifiers(1 << 0);
    pub const EXCLUDE_READONLY: MappedTypeModifiers = MappedTypeModifiers(1 << 1);
    pub const INCLUDE_OPTIONAL: MappedTypeModifiers = MappedTypeModifiers(1 << 2);
    pub const EXCLUDE_OPTIONAL: MappedTypeModifiers = MappedTypeModifiers(1 << 3);
}

/// Go: `type DiagnosticAndArguments struct` (message + args; used by the
/// elaborate* family's deferred diagnostics).
pub struct DiagnosticAndArguments {
    pub message: &'static Message,
    pub arguments: Vec<DiagArg>,
}

/// Go: `type ErrorOutputContainer struct`.
#[derive(Default)]
pub struct ErrorOutputContainer {
    pub errors: Vec<tsc_ast::Diagnostic>,
    pub skip_logging: bool,
}

/// Go: the diagnostic-argument values relater produces (strings and numbers).
#[derive(Clone, Debug, PartialEq)]
pub enum DiagArg {
    Str(String),
    Int(i64),
    Uint(u64),
}

/// Go: `type RecursionId struct { value any }` — Go stores a `*ast.Node |
/// *ast.Symbol | *Type` pointer and compares by interface identity; the port
/// tags the arena handle (identity is handle equality).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RecursionId {
    Node(NodeId),
    Symbol(SymbolId),
    Type(TypeId),
}

/// Go: `type Relation struct { results map[CacheHashKey]RelationComparisonResult }`.
#[derive(Default)]
pub struct Relation {
    results: FxHashMap<crate::todo_shapes::CacheHashKey, RelationComparisonResult>,
}

impl Relation {
    /// Go: `func (r *Relation) get(key)`.
    pub fn get(&self, key: crate::todo_shapes::CacheHashKey) -> RelationComparisonResult {
        self.results.get(&key).copied().unwrap_or(RelationComparisonResult::NONE)
    }

    /// Go: `func (r *Relation) set(key, result)`.
    pub fn set(&mut self, key: crate::todo_shapes::CacheHashKey, result: RelationComparisonResult) {
        self.results.insert(key, result);
    }

    /// Go: `func (r *Relation) size() int`.
    pub fn size(&self) -> usize {
        self.results.len()
    }
}

/// PORT: replaces the five `*Relation` pointer identities (see module note).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RelationKind {
    Identity,
    Assignable,
    Subtype,
    StrictSubtype,
    Comparable,
}

impl Checker {
    /// Go: `relation.get/set/size` through the kind selector.
    pub(crate) fn relation_ref(&self, kind: RelationKind) -> &Relation {
        match kind {
            RelationKind::Identity => &self.identity_relation,
            RelationKind::Assignable => &self.assignable_relation,
            RelationKind::Subtype => &self.subtype_relation,
            RelationKind::StrictSubtype => &self.strict_subtype_relation,
            RelationKind::Comparable => &self.comparable_relation,
        }
    }

    pub(crate) fn relation_ref_mut(&mut self, kind: RelationKind) -> &mut Relation {
        match kind {
            RelationKind::Identity => &mut self.identity_relation,
            RelationKind::Assignable => &mut self.assignable_relation,
            RelationKind::Subtype => &mut self.subtype_relation,
            RelationKind::StrictSubtype => &mut self.strict_subtype_relation,
            RelationKind::Comparable => &mut self.comparable_relation,
        }
    }
}

/// Go: `type TypeComparer func(s, t *Type, reportErrors bool) Ternary` — the
/// five fixed implementations (see module PORT note).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeComparerKind {
    Identical,
    Assignable,
    SubtypeOf,
    RelatedToSimple,
    RelatedToWorker,
}

/// Go: `type ErrorReporter func(message, args...)` — `None` is Go's nil.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorReporterKind {
    Relater,
}

/// Go: `type EnumRelationKey struct { sourceId, targetId ast.SymbolId }`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumRelationKey {
    pub source_id: SymbolId,
    pub target_id: SymbolId,
}

/// Go: the `varianceStack` entry (declared with the Checker fields).
#[derive(Clone)]
pub struct VarianceStackEntry {
    pub symbol: SymbolId,
    pub type_parameters: Vec<TypeId>,
}

// Ternary helpers (Go's bit ops — see module PORT note).

/// Go: `x &= y` (picks the lesser in False < Unknown < Maybe < True).
pub(crate) fn ternary_and(x: Ternary, y: Ternary) -> Ternary {
    use Ternary::*;
    match (x, y) {
        (True, r) | (r, True) if matches!(r, Unknown | Maybe) => r,
        (True, True) => True,
        (False, _) | (_, False) => False,
        (Unknown, _) | (_, Unknown) => Unknown,
        _ => Maybe,
    }
}

/// Go: `result != 0` / `result != TernaryFalse` on a Ternary.
pub(crate) fn ternary_not_false(t: Ternary) -> bool {
    t != Ternary::False
}

// ────────────────────────────────────────────────────────────────────────────
// keyBuilder + getRelationKey (Go checker.go 17770+/18016 — landed with the
// relater because the relation cache is its load-bearing consumer; the S7
// union-instantiation keys reuse this builder when that wave lands)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type keyBuilder struct` — 192-byte inline buffer + overflow spill.
/// PORT (M6 S2): the builder landed private with the W2 relation cache; the
/// S2 cache-key family (typeofsymbol.rs — getTypeListKey & siblings) is its
/// second load-bearing consumer, so the struct + writers are `pub(crate)`
/// (same-crate visibility only; no API surface change).
pub(crate) struct KeyBuilder {
    inline_length: usize,
    overflow_buffer: Vec<u8>,
    inline_buffer: [u8; 192],
}

#[allow(dead_code)] // TODO(port M6 W2): the key-builder's writers serve the mapper/inference waves.
impl KeyBuilder {
    pub(crate) fn new() -> Self {
        KeyBuilder {
            inline_length: 0,
            overflow_buffer: Vec::new(),
            inline_buffer: [0; 192],
        }
    }

    /// Go: `hash()` — PORT: Go xxh3.Hash128 → FNV-1a over the byte stream
    /// (the approved dep set has no xxh3; contentmapper.rs set the precedent,
    /// and the dispatch doc's cache-key requirement — deterministic,
    /// collision-free, session-internal — holds: relation/instantiation keys
    /// are never serialized into baselines, only used for map identity, so a
    /// different fixed hash preserves all observable behavior).
    pub(crate) fn hash(&self) -> crate::todo_shapes::CacheHashKey {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in &self.overflow_buffer {
            h ^= b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        for &b in &self.inline_buffer[..self.inline_length] {
            h ^= b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        crate::todo_shapes::CacheHashKey(h)
    }

    fn spill(&mut self) {
        self.overflow_buffer
            .extend_from_slice(&self.inline_buffer[..self.inline_length]);
        self.inline_length = 0;
    }

    pub(crate) fn write_byte(&mut self, c: u8) {
        if self.inline_length == self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length] = c;
        self.inline_length += 1;
    }

    pub(crate) fn write_string(&mut self, s: &str) {
        if self.inline_length + s.len() > self.inline_buffer.len() {
            self.spill();
            if s.len() > self.inline_buffer.len() {
                self.overflow_buffer.extend_from_slice(s.as_bytes());
                return;
            }
        }
        self.inline_buffer[self.inline_length..self.inline_length + s.len()]
            .copy_from_slice(s.as_bytes());
        self.inline_length += s.len();
    }

    pub(crate) fn write_uint32(&mut self, v: u32) {
        if self.inline_length + 4 > self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length..self.inline_length + 4]
            .copy_from_slice(&v.to_le_bytes());
        self.inline_length += 4;
    }

    pub(crate) fn write_uint64(&mut self, v: u64) {
        if self.inline_length + 8 > self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length..self.inline_length + 8]
            .copy_from_slice(&v.to_le_bytes());
        self.inline_length += 8;
    }

    pub(crate) fn write_int(&mut self, value: i64) {
        self.write_uint64(value as u64);
    }

    pub(crate) fn write_symbol(&mut self, s: SymbolId) {
        self.write_uint64(s.0 as u64);
    }

    pub(crate) fn write_type(&mut self, t: TypeId) {
        self.write_uint32(t.0);
    }

    pub(crate) fn write_types(&mut self, types: &[TypeId]) {
        self.write_int(types.len() as i64);
        for &t in types {
            self.write_type(t);
        }
    }

    /// Go: `writeNode(node *ast.Node)` / `writeNodeId` — Go writes
    /// `uint64(node.ID())` when the node is non-nil, byte 0 otherwise
    /// (checker.go's keyBuilder.writeNode). PORT: the §5.1 node identity is
    /// the NodeId (packed file id + index), so the id IS the key.
    pub(crate) fn write_node_id(&mut self, node: Option<NodeId>) {
        match node {
            Some(node) => self.write_uint64(node.0),
            None => self.write_byte(0),
        }
    }

    pub(crate) fn write_alias(&mut self, alias: Option<&crate::types::TypeAlias>) {
        if let Some(alias) = alias {
            self.write_byte(1);
            self.write_symbol(alias.symbol.expect("alias symbol"));
            self.write_types(&alias.type_arguments);
        } else {
            self.write_byte(0);
        }
    }

    /// Go: `writeGenericTypeReferences` — PORT: the constrained-type-
    /// parameter walk needs `getConstraintOfTypeParameter` (S9 constraint
    /// machinery); the permissive stub treats all type parameters as
    /// unconstrained, so `constrained` is always false until S9 lands the
    /// circular-constraint resolution (bannered at the stub).
    fn write_generic_type_references(
        &mut self,
        c: &Checker,
        source: TypeId,
        target: TypeId,
        ignore_constraints: bool,
    ) -> bool {
        let mut constrained = false;
        let mut type_parameters: Vec<TypeId> = Vec::with_capacity(8);
        fn write_type_reference(
            b: &mut KeyBuilder,
            c: &Checker,
            r: TypeId,
            depth: i32,
            type_parameters: &mut Vec<TypeId>,
            constrained: &mut bool,
            ignore_constraints: bool,
        ) {
            let ty = c.type_(r);
            b.write_type(ty.target().expect("type reference target"));
            for &t in &ty.as_type_reference().expect("TypeReference data").resolved_type_arguments {
                let t_ty = c.type_(t);
                if t_ty.flags.intersects(TypeFlags::TYPE_PARAMETER) {
                    if ignore_constraints || get_constraint_of_type_parameter(c, t).is_none() {
                        let index = match type_parameters.iter().position(|&tp| tp == t) {
                            Some(i) => i,
                            None => {
                                type_parameters.push(t);
                                type_parameters.len() - 1
                            }
                        };
                        b.write_byte(b'=');
                        b.write_int(index as i64);
                        continue;
                    }
                    *constrained = true;
                } else if depth < 4 && is_type_reference_with_generic_arguments(c, t) {
                    b.write_byte(b'<');
                    write_type_reference(
                        b,
                        c,
                        t,
                        depth + 1,
                        type_parameters,
                        constrained,
                        ignore_constraints,
                    );
                    b.write_byte(b'>');
                    continue;
                }
                b.write_byte(b'-');
                b.write_type(t);
            }
        }
        write_type_reference(
            self,
            c,
            source,
            0,
            &mut type_parameters,
            &mut constrained,
            ignore_constraints,
        );
        self.write_byte(b',');
        write_type_reference(
            self,
            c,
            target,
            0,
            &mut type_parameters,
            &mut constrained,
            ignore_constraints,
        );
        constrained
    }
}

/// Go: `func getRelationKey(source, target, intersectionState, isIdentity, ignoreConstraints) (CacheHashKey, bool)`.
pub(crate) fn get_relation_key(
    c: &Checker,
    source: TypeId,
    target: TypeId,
    intersection_state: IntersectionState,
    is_identity: bool,
    ignore_constraints: bool,
) -> (crate::todo_shapes::CacheHashKey, bool) {
    let (source, target) = if is_identity && source.0 > target.0 {
        (target, source)
    } else {
        (source, target)
    };
    let mut b = KeyBuilder::new();
    let mut constrained = false;
    if is_type_reference_with_generic_arguments(c, source)
        && is_type_reference_with_generic_arguments(c, target)
    {
        b.write_byte(b'g');
        constrained = b.write_generic_type_references(c, source, target, ignore_constraints);
    } else {
        b.write_byte(b's');
        b.write_type(source);
        b.write_type(target);
    }
    b.write_uint32(intersection_state.0);
    (b.hash(), constrained)
}

/// Go: `func isTypeReferenceWithGenericArguments(t) bool`.
fn is_type_reference_with_generic_arguments(c: &Checker, t: TypeId) -> bool {
    is_non_deferred_type_reference(c, t)
        && get_type_arguments(c, t).iter().any(|&t| {
            c.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER)
                || is_type_reference_with_generic_arguments(c, t)
        })
}

/// Go: `func isNonDeferredTypeReference(t) bool`.
fn is_non_deferred_type_reference(c: &Checker, t: TypeId) -> bool {
    let ty = c.type_(t);
    ty.object_flags.intersects(crate::types::ObjectFlags::REFERENCE)
        && ty.as_type_reference().is_some_and(|r| r.node.is_none())
}

// ────────────────────────────────────────────────────────────────────────────
// Relation entry points (Go relater.go)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) isTypeIdenticalTo(source, target) bool`.
    pub fn is_type_identical_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to(source, target, RelationKind::Identity)
    }

    /// Go: `func (c *Checker) compareTypesIdentical(source, target) Ternary`.
    pub fn compare_types_identical(&mut self, source: TypeId, target: TypeId) -> Ternary {
        if self.is_type_related_to(source, target, RelationKind::Identity) {
            Ternary::True
        } else {
            Ternary::False
        }
    }

    /// Go: `func (c *Checker) compareTypesAssignableSimple(source, target) Ternary`.
    pub fn compare_types_assignable_simple(&mut self, source: TypeId, target: TypeId) -> Ternary {
        if self.is_type_related_to(source, target, RelationKind::Assignable) {
            Ternary::True
        } else {
            Ternary::False
        }
    }

    /// Go: `func (c *Checker) compareTypesAssignableWorker(source, target, reportErrors) Ternary`.
    pub fn compare_types_assignable_worker(
        &mut self,
        source: TypeId,
        target: TypeId,
        _report_errors: bool,
    ) -> Ternary {
        if self.is_type_related_to(source, target, RelationKind::Assignable) {
            Ternary::True
        } else {
            Ternary::False
        }
    }

    /// Go: `func (c *Checker) compareTypesSubtypeOf(source, target) Ternary`.
    pub fn compare_types_subtype_of(&mut self, source: TypeId, target: TypeId) -> Ternary {
        if self.is_type_related_to(source, target, RelationKind::Subtype) {
            Ternary::True
        } else {
            Ternary::False
        }
    }

    /// Go: `func (c *Checker) isTypeAssignableTo(source, target) bool`.
    pub fn is_type_assignable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to(source, target, RelationKind::Assignable)
    }

    /// Go: `func (c *Checker) isTypeSubtypeOf(source, target) bool`.
    pub fn is_type_subtype_of(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to(source, target, RelationKind::Subtype)
    }

    /// Go: `func (c *Checker) isTypeStrictSubtypeOf(source, target) bool`.
    pub fn is_type_strict_subtype_of(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to(source, target, RelationKind::StrictSubtype)
    }

    /// Go: `func (c *Checker) isTypeComparableTo(source, target) bool`.
    pub fn is_type_comparable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to(source, target, RelationKind::Comparable)
    }

    /// Go: `func (c *Checker) areTypesComparable(type1, type2) bool`.
    pub fn are_types_comparable(&mut self, type1: TypeId, type2: TypeId) -> bool {
        self.is_type_comparable_to(type1, type2) || self.is_type_comparable_to(type2, type1)
    }

    /// Go: `func (c *Checker) isTypeRelatedTo(source, target, relation) bool`.
    pub fn is_type_related_to(&mut self, source: TypeId, target: TypeId, relation: RelationKind) -> bool {
        let mut source = source;
        let mut target = target;
        if crate::is_fresh_literal_type(self, source) {
            source = self
                .type_(source)
                .as_literal_type()
                .expect("LiteralType data")
                .regular_type
                .expect("regularType");
        }
        if crate::is_fresh_literal_type(self, target) {
            target = self
                .type_(target)
                .as_literal_type()
                .expect("LiteralType data")
                .regular_type
                .expect("regularType");
        }
        if source == target {
            return true;
        }
        if relation != RelationKind::Identity {
            if relation == RelationKind::Comparable
                && !self.type_(target).flags.intersects(TypeFlags::NEVER)
                && self.is_simple_type_related_to(target, source, relation, None)
                || self.is_simple_type_related_to(source, target, relation, None)
            {
                return true;
            }
        } else if !self
            .type_(source)
            .flags
            .union(self.type_(target).flags)
            .intersects(
                TypeFlags::UNION_OR_INTERSECTION
                    | TypeFlags::INDEXED_ACCESS
                    | TypeFlags::CONDITIONAL
                    | TypeFlags::SUBSTITUTION,
            )
        {
            // We have excluded types that may simplify to other forms, so
            // types must have identical flags (Go comment, verbatim).
            if self.type_(source).flags != self.type_(target).flags {
                return false;
            }
            if self.type_(source).flags.intersects(TypeFlags::SINGLETON) {
                return true;
            }
        }
        if self.type_(source).flags.intersects(TypeFlags::OBJECT)
            && self.type_(target).flags.intersects(TypeFlags::OBJECT)
        {
            let (id, _) = get_relation_key(self, source, target, IntersectionState::NONE, relation == RelationKind::Identity, false);
            let related = self.relation_ref(relation).get(id);
            if related != RelationComparisonResult::NONE {
                return related.intersects(RelationComparisonResult::SUCCEEDED);
            }
        }
        if self.type_(source).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
            || self.type_(target).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
        {
            return self.check_type_related_to(source, target, relation, None /*errorNode*/);
        }
        false
    }

    /// Go: `func (c *Checker) isSimpleTypeRelatedTo(source, target, relation, errorReporter) bool`.
    pub(crate) fn is_simple_type_related_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        error_reporter: Option<ErrorReporterKind>,
    ) -> bool {
        let _ = error_reporter; // only reachable through isEnumTypeRelatedTo (below)
        let s = self.type_(source).flags;
        let t = self.type_(target).flags;
        if t.intersects(TypeFlags::ANY) || s.intersects(TypeFlags::NEVER) || source == self.wildcard_type() {
            return true;
        }
        if t.intersects(TypeFlags::UNKNOWN)
            && !(relation == RelationKind::StrictSubtype && s.intersects(TypeFlags::ANY))
        {
            return true;
        }
        if t.intersects(TypeFlags::NEVER) {
            return false;
        }
        if s.intersects(TypeFlags::STRING_LIKE) && t.intersects(TypeFlags::STRING) {
            return true;
        }
        if s.intersects(TypeFlags::STRING_LITERAL | TypeFlags::ENUM_LITERAL)
            && t.intersects(TypeFlags::STRING_LITERAL)
            && !t.intersects(TypeFlags::ENUM_LITERAL)
            && literal_values_equal(self, source, target)
        {
            return true;
        }
        if s.intersects(TypeFlags::NUMBER_LIKE) && t.intersects(TypeFlags::NUMBER) {
            return true;
        }
        if s.intersects(TypeFlags::NUMBER_LITERAL | TypeFlags::ENUM_LITERAL)
            && t.intersects(TypeFlags::NUMBER_LITERAL)
            && !t.intersects(TypeFlags::ENUM_LITERAL)
            && literal_values_equal(self, source, target)
        {
            return true;
        }
        if s.intersects(TypeFlags::BIGINT_LIKE) && t.intersects(TypeFlags::BIGINT) {
            return true;
        }
        if s.intersects(TypeFlags::BOOLEAN_LIKE) && t.intersects(TypeFlags::BOOLEAN) {
            return true;
        }
        if s.intersects(TypeFlags::ESSYMBOL_LIKE) && t.intersects(TypeFlags::ESSYMBOL) {
            return true;
        }
        if s.intersects(TypeFlags::ENUM)
            && t.intersects(TypeFlags::ENUM)
            && self.type_(source).symbol.is_some_and(|s| self.type_(target).symbol == Some(s))
        {
            // Go: source.symbol.Name == target.symbol.Name && isEnumTypeRelatedTo(...)
            let (source_symbol, target_symbol) =
                (self.type_(source).symbol, self.type_(target).symbol);
            let names_equal = match (source_symbol, target_symbol) {
                (Some(a), Some(b)) => self.program_symbol(a).name == self.program_symbol(b).name,
                _ => false,
            };
            if names_equal {
                let (a, b) = (source_symbol.expect("checked"), target_symbol.expect("checked"));
                if self.is_enum_type_related_to(a, b, error_reporter.is_some()) {
                    return true;
                }
            }
        }
        if s.intersects(TypeFlags::ENUM_LITERAL) && t.intersects(TypeFlags::ENUM_LITERAL) {
            if s.intersects(TypeFlags::UNION) && t.intersects(TypeFlags::UNION) {
                let (a, b) = (
                    self.type_(source).symbol.expect("union enum symbol"),
                    self.type_(target).symbol.expect("union enum symbol"),
                );
                if self.is_enum_type_related_to(a, b, error_reporter.is_some()) {
                    return true;
                }
            }
            if s.intersects(TypeFlags::LITERAL)
                && t.intersects(TypeFlags::LITERAL)
                && literal_values_equal(self, source, target)
            {
                let (a, b) = (
                    self.type_(source).symbol.expect("enum literal symbol"),
                    self.type_(target).symbol.expect("enum literal symbol"),
                );
                if self.is_enum_type_related_to(a, b, error_reporter.is_some()) {
                    return true;
                }
            }
        }
        // In non-strictNullChecks mode, `undefined` and `null` are
        // assignable to anything except `never` (Go comment, verbatim).
        if s.intersects(TypeFlags::UNDEFINED)
            && (!self.strict_null_checks && !t.intersects(TypeFlags::UNION_OR_INTERSECTION)
                || t.intersects(TypeFlags::UNDEFINED | TypeFlags::VOID))
        {
            return true;
        }
        if s.intersects(TypeFlags::NULL)
            && (!self.strict_null_checks && !t.intersects(TypeFlags::UNION_OR_INTERSECTION)
                || t.intersects(TypeFlags::NULL))
        {
            return true;
        }
        if s.intersects(TypeFlags::OBJECT)
            && t.intersects(TypeFlags::NON_PRIMITIVE)
            && !(relation == RelationKind::StrictSubtype
                && self.is_empty_anonymous_object_type(source)
                && !self.type_(source).object_flags.intersects(crate::types::ObjectFlags::FRESH_LITERAL))
        {
            return true;
        }
        if relation == RelationKind::Assignable || relation == RelationKind::Comparable {
            if s.intersects(TypeFlags::ANY) {
                return true;
            }
            // Type number is assignable to any computed numeric enum type …
            // (Go comment, verbatim; the enum bits below).
            if s.intersects(TypeFlags::NUMBER)
                && (t.intersects(TypeFlags::ENUM)
                    || t.intersects(TypeFlags::NUMBER_LITERAL)
                        && t.intersects(TypeFlags::ENUM_LITERAL))
            {
                return true;
            }
            if s.intersects(TypeFlags::NUMBER_LITERAL)
                && !s.intersects(TypeFlags::ENUM_LITERAL)
                && (t.intersects(TypeFlags::ENUM)
                    || t.intersects(TypeFlags::NUMBER_LITERAL)
                        && t.intersects(TypeFlags::ENUM_LITERAL)
                        && literal_values_equal(self, source, target))
            {
                return true;
            }
            // Anything is assignable to a union containing undefined, null, and {}
            if is_unknown_like_union_type(self, target) {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) isEnumTypeRelatedTo(source, target, errorReporter) bool`.
    ///
    /// PORT: the property walk (getPropertiesOfType/getPropertyOfType/
    /// getEnumMemberValue) is S8/S2 machinery — the port carries the symbol
    /// identity + name + flags checks (the only parts reachable with the
    /// current type surface) and the cache writes, with the per-property
    /// comparison bannered for S8 (regular enums cannot be constructed until
    /// getDeclaredTypeOfSymbol lands in S2).
    pub(crate) fn is_enum_type_related_to(
        &mut self,
        source: SymbolId,
        target: SymbolId,
        error_reporter: bool,
    ) -> bool {
        let source_symbol = if self.program_symbol(source).flags.intersects(SymbolFlags::ENUM_MEMBER) {
            self.get_parent_of_symbol(source).unwrap_or(source)
        } else {
            source
        };
        let target_symbol = if self.program_symbol(target).flags.intersects(SymbolFlags::ENUM_MEMBER) {
            self.get_parent_of_symbol(target).unwrap_or(target)
        } else {
            target
        };
        if source_symbol == target_symbol {
            return true;
        }
        let (source_flags, target_flags) = (
            self.program_symbol(source_symbol).flags,
            self.program_symbol(target_symbol).flags,
        );
        if self.program_symbol(source_symbol).name != self.program_symbol(target_symbol).name
            || !source_flags.intersects(SymbolFlags::REGULAR_ENUM)
            || !target_flags.intersects(SymbolFlags::REGULAR_ENUM)
        {
            return false;
        }
        let key = EnumRelationKey {
            source_id: source_symbol,
            target_id: target_symbol,
        };
        let entry = self
            .enum_relation
            .get(&key)
            .copied()
            .unwrap_or(RelationComparisonResult::NONE);
        if entry != RelationComparisonResult::NONE
            && !(entry.intersects(RelationComparisonResult::FAILED) && error_reporter)
        {
            return entry.intersects(RelationComparisonResult::SUCCEEDED);
        }
        // TODO(port M6 S8): the per-property value comparison walk
        // (getPropertiesOfType/getPropertyOfType/getEnumMemberValue) — regular
        // enum types are not constructible until getDeclaredTypeOfSymbol (S2)
        // and member resolution (S8) land; the relation cache bookkeeping the
        // walk drives is in place above.
        self.enum_relation.insert(key, RelationComparisonResult::SUCCEEDED);
        true
    }
}

/// Go: `source.AsLiteralType().value == target.AsLiteralType().value` (the
/// literal-value equality the simple-relation ladder uses).
fn literal_values_equal(c: &Checker, source: TypeId, target: TypeId) -> bool {
    let s = c.type_(source).as_literal_type().and_then(|d| d.value.as_ref());
    let t = c.type_(target).as_literal_type().and_then(|d| d.value.as_ref());
    match (s, t) {
        (Some(a), Some(b)) => literal_value_eq(a, b),
        _ => false,
    }
}

/// Go: `==` on `LiteralType.value` (any) — number compare is IEEE `==`.
pub(crate) fn literal_value_eq(a: &crate::types::LiteralValue, b: &crate::types::LiteralValue) -> bool {
    use crate::types::LiteralValue;
    match (a, b) {
        (LiteralValue::String(x), LiteralValue::String(y)) => x == y,
        (LiteralValue::Number(x), LiteralValue::Number(y)) => x.0 == y.0,
        (LiteralValue::Boolean(x), LiteralValue::Boolean(y)) => x == y,
        (LiteralValue::BigInt(x), LiteralValue::BigInt(y)) => x == y,
        (LiteralValue::None, LiteralValue::None) => true,
        _ => false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// checkTypeRelatedTo family
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkTypeAssignableTo(source, target, errorNode, headMessage) bool`.
    pub fn check_type_assignable_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Option<NodeId>,
        head_message: Option<&'static Message>,
    ) -> bool {
        self.check_type_related_to_ex(
            source,
            target,
            RelationKind::Assignable,
            error_node,
            head_message,
            None,
        )
    }

    /// Go: `func (c *Checker) checkTypeAssignableToEx(...) bool`.
    pub fn check_type_assignable_to_ex(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Option<NodeId>,
        head_message: Option<&'static Message>,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        self.check_type_related_to_ex(
            source,
            target,
            RelationKind::Assignable,
            error_node,
            head_message,
            diagnostic_output,
        )
    }

    /// Go: `func (c *Checker) checkTypeComparableTo(...) bool`.
    pub fn check_type_comparable_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Option<NodeId>,
        head_message: Option<&'static Message>,
    ) -> bool {
        self.check_type_related_to_ex(
            source,
            target,
            RelationKind::Comparable,
            error_node,
            head_message,
            None,
        )
    }

    /// Go: `func (c *Checker) checkTypeRelatedTo(source, target, relation, errorNode) bool`.
    pub fn check_type_related_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        error_node: Option<NodeId>,
    ) -> bool {
        self.check_type_related_to_ex(source, target, relation, error_node, None, None)
    }

    /// Go: `func (c *Checker) checkTypeRelatedToEx(...) bool`.
    #[allow(clippy::too_many_arguments)]
    pub fn check_type_related_to_ex(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        error_node: Option<NodeId>,
        head_message: Option<&'static Message>,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        let mut r = self.get_relater();
        r.relation = relation;
        r.error_node = error_node;
        r.relation_count = (16_000_000i64 - self.relation_ref(relation).size() as i64) / 8;
        let result = r.is_related_to_ex(
            self,
            source,
            target,
            RecursionFlags::BOTH,
            error_node.is_some(), /*reportErrors*/
            head_message,
            IntersectionState::NONE,
        );
        if r.overflow {
            // Record this relation as having failed such that we don't
            // attempt the overflowing operation again (Go comment, verbatim).
            let (id, _) = get_relation_key(self, source, target, IntersectionState::NONE, relation == RelationKind::Identity, false);
            self.relation_ref_mut(relation).set(
                id,
                RelationComparisonResult::FAILED | RelationComparisonResult::COMPLEXITY_OVERFLOW,
            );
            if let Some(tr) = &self.tracer {
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "checkTypeRelatedTo_DepthLimit",
                    &{
                        let args = crate::tracing::Args::new();
                        args.insert("sourceId", crate::tracing::TraceValue::Uint(source.0 as u64));
                        args.insert("targetId", crate::tracing::TraceValue::Uint(target.0 as u64));
                        args.insert("depth", crate::tracing::TraceValue::Int(r.source_stack.len() as i64));
                        args.insert("targetDepth", crate::tracing::TraceValue::Int(r.target_stack.len() as i64));
                        args
                    },
                );
            }
            let error_node = error_node.or(self.current_node);
            // Go: reportDiagnostic(NewDiagnosticForNode(Excessive_complexity…,
            // TypeToString(source), TypeToString(target))) — the diagnostic
            // text needs TypeToString (W4); the depth-limit path is not
            // reachable until structured comparisons are, so the diagnostic is
            // a bare-code port until then.
            // TODO(port M6 W4): the formatted Excessive_complexity diagnostic.
            let diagnostic = diagnostic_for_node(
                self,
                error_node,
                &msg::EXCESSIVE_COMPLEXITY_COMPARING_TYPES_0_AND_1,
                &[],
            );
            report_diagnostic(self, Some(diagnostic), diagnostic_output.as_deref_mut());
        } else if r.error_chain.is_some() {
            // The import-quickfix retry + chain reporting.
            if head_message.is_some()
                && error_node.is_some()
                && result == Ternary::False
                && self.type_(source).symbol.is_some()
            {
                // Go: checks exportTypeLinks.originatingImport and retries
                // through the target symbol — S6 exports wiring.
                // TODO(port M6 S6): the incorrect-import quickfix retry.
            }
            let chain = std::mem::take(&mut r.error_chain);
            let related_info = std::mem::take(&mut r.related_info);
            let diagnostic = create_diagnostic_chain_from_error_chain(self, chain, r.error_node, related_info);
            report_diagnostic(self, diagnostic, diagnostic_output);
        }
        let result = ternary_not_false(result);
        self.put_relater(r);
        result
    }

    /// Go: `func (c *Checker) checkTypeAssignableToAndOptionallyElaborate(...)`.
    #[allow(clippy::too_many_arguments)]
    pub fn check_type_assignable_to_and_optionally_elaborate(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Option<NodeId>,
        expr: Option<NodeId>,
        head_message: Option<&'static Message>,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        self.check_type_related_to_and_optionally_elaborate(
            source,
            target,
            RelationKind::Assignable,
            error_node,
            expr,
            head_message,
            diagnostic_output,
        )
    }

    /// Go: `func (c *Checker) checkTypeRelatedToAndOptionallyElaborate(...)`.
    #[allow(clippy::too_many_arguments)]
    pub fn check_type_related_to_and_optionally_elaborate(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        error_node: Option<NodeId>,
        expr: Option<NodeId>,
        head_message: Option<&'static Message>,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        if self.is_type_related_to(source, target, relation) {
            return true;
        }
        if let Some(error_node) = error_node {
            let r = self.get_relater();
            self.put_relater(r);
            let elaborated = self.elaborate_error(
                expr,
                source,
                target,
                relation,
                head_message,
                diagnostic_output.as_deref_mut(),
            );
            if !elaborated {
                return self.check_type_related_to_ex(
                    source,
                    target,
                    relation,
                    Some(error_node),
                    head_message,
                    diagnostic_output,
                );
            }
            return false;
        }
        false
    }
}

/// Go: the `args` → `[]string` conversion for the diagnostic constructors
/// (`DiagArg` is the port's `...any` shape; the diagnostics crate's
/// arguments are strings, matching every message the relater produces).
fn diag_args_to_strings(args: &[DiagArg]) -> Vec<String> {
    args.iter()
        .map(|a| match a {
            DiagArg::Str(s) => s.clone(),
            DiagArg::Int(i) => i.to_string(),
            DiagArg::Uint(u) => u.to_string(),
        })
        .collect()
}

/// Go: `NewDiagnosticForNode(node, message, args...)` — the errorNode-optional
/// form the relater needs (`createDiagnosticChainFromErrorChain` tolerates a
/// nil node in Go).
fn diagnostic_for_node(
    c: &Checker,
    node: Option<NodeId>,
    message: &'static Message,
    args: &[DiagArg],
) -> tsc_ast::Diagnostic {
    match node {
        Some(node) => u::new_diagnostic_for_node(c.store(), node, message, diag_args_to_strings(args)),
        None => tsc_ast::Diagnostic::new(
            None,
            tsc_ast::TextRange::new(0, 0),
            message.code(),
            message.category(),
            "" /*source*/,
            Some(message),
            "" /*messageText*/,
            message.key(),
            diag_args_to_strings(args),
        ),
    }
}

/// Go: `func createDiagnosticChainFromErrorChain(chain, errorNode, relatedInfo) *ast.Diagnostic`.
fn create_diagnostic_chain_from_error_chain(
    c: &Checker,
    mut chain: Option<Box<ErrorChain>>,
    error_node: Option<NodeId>,
    related_info: Vec<tsc_ast::Diagnostic>,
) -> Option<tsc_ast::Diagnostic> {
    while chain.as_ref().is_some_and(|ch| ch.message.elided_in_compatibility_pyramid()) {
        chain = chain.and_then(|ch| ch.next);
    }
    let chain = chain?;
    let next = create_diagnostic_chain_from_error_chain(c, chain.next.clone(), error_node, related_info.clone());
    match next {
        None => {
            let mut diagnostic = diagnostic_for_node(c, error_node, chain.message, &chain.args);
            for info in related_info {
                diagnostic.add_related_info(info);
            }
            Some(diagnostic)
        }
        Some(next) => {
            // Go: NewDiagnosticChainForNode(chain, errorNode, message, args)
            // — the chain head carries the related info; the tail is linked
            // through the diagnostic's message chain (diagnostics wave).
            let mut diagnostic = diagnostic_for_node(c, error_node, chain.message, &chain.args);
            for info in next.related_information() {
                diagnostic.add_related_info(info.clone());
            }
            Some(diagnostic)
        }
    }
}

/// Go: `func (c *Checker) reportDiagnostic(diagnostic, diagnosticOutput)`.
fn report_diagnostic(
    c: &mut Checker,
    diagnostic: Option<tsc_ast::Diagnostic>,
    diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
) {
    let Some(diagnostic) = diagnostic else { return };
    match diagnostic_output {
        Some(out) => out.push(diagnostic),
        None => {
            c.add_diagnostic(diagnostic);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Weak/common/known properties (Go)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) isWeakType(t) bool` — "A type is 'weak' if it is
    /// an object type with at least one optional property and no required
    /// properties, call/construct signatures or index signatures".
    pub(crate) fn is_weak_type(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::OBJECT) {
            let resolved = resolve_structured_type_members(self, t);
            let properties = resolved.properties.clone();
            return resolved.signatures.is_empty()
                && resolved.index_infos.is_empty()
                && !properties.is_empty()
                && properties
                    .iter()
                    .all(|&p| self.program_symbol(p).flags.intersects(SymbolFlags::OPTIONAL));
        }
        if flags.intersects(TypeFlags::SUBSTITUTION) {
            let base = self
                .type_(t)
                .as_substitution_type()
                .and_then(|d| d.base_type)
                .expect("substitution baseType");
            return self.is_weak_type(base);
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            let constituents: Vec<TypeId> = self.type_(t).types().to_vec();
            return constituents.iter().all(|&t| self.is_weak_type(t));
        }
        false
    }

    /// Go: `func (c *Checker) hasCommonProperties(source, target, isComparingJsxAttributes) bool`.
    pub(crate) fn has_common_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
        is_comparing_jsx_attributes: bool,
    ) -> bool {
        for prop in get_properties_of_type(self, source) {
            let name = self.program_symbol(prop).name.clone();
            if self.is_known_property(target, &name, is_comparing_jsx_attributes) {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) isKnownProperty(targetType, name, isComparingJsxAttributes) bool`.
    pub(crate) fn is_known_property(
        &mut self,
        target_type: TypeId,
        name: &str,
        is_comparing_jsx_attributes: bool,
    ) -> bool {
        let flags = self.type_(target_type).flags;
        if flags.intersects(TypeFlags::OBJECT)
            && (get_property_of_object_type(self, target_type, name).is_some()
                || get_applicable_index_info_for_name(self, target_type, name).is_some()
                || u::is_late_bound_name(name) && get_index_info_of_type(self, target_type, self.string_type()).is_some()
                || is_comparing_jsx_attributes && is_hyphenated_jsx_name(name))
            {
                return true;
            }
        if flags.intersects(TypeFlags::SUBSTITUTION) {
            let base = self
                .type_(target_type)
                .as_substitution_type()
                .and_then(|d| d.base_type)
                .expect("substitution baseType");
            return self.is_known_property(base, name, is_comparing_jsx_attributes);
        }
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
            && is_excess_property_check_target(self, target_type)
        {
            let constituents: Vec<TypeId> = self.type_(target_type).types().to_vec();
            for t in constituents {
                if self.is_known_property(t, name, is_comparing_jsx_attributes) {
                    return true;
                }
            }
        }
        false
    }
}

/// Go: `func isHyphenatedJsxName(name) bool`.
fn is_hyphenated_jsx_name(name: &str) -> bool {
    name.contains('-')
}

/// Go: `func isExcessPropertyCheckTarget(t) bool`.
pub(crate) fn is_excess_property_check_target(c: &Checker, t: TypeId) -> bool {
    let ty = c.type_(t);
    (ty.flags.intersects(TypeFlags::OBJECT)
        && !ty
            .object_flags
            .intersects(crate::types::ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES))
        || ty.flags.intersects(TypeFlags::NON_PRIMITIVE)
        || ty.flags.intersects(TypeFlags::SUBSTITUTION)
            && ty
                .as_substitution_type()
                .and_then(|d| d.base_type)
                .is_some_and(|base| is_excess_property_check_target(c, base))
        || ty.flags.intersects(TypeFlags::UNION)
            && ty.types().iter().any(|&t| is_excess_property_check_target(c, t))
        || ty.flags.intersects(TypeFlags::INTERSECTION)
            && ty.types().iter().all(|&t| is_excess_property_check_target(c, t))
}

// ────────────────────────────────────────────────────────────────────────────
// Recursion identity + deeply-nested detection (Go)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) isDeeplyNestedType(t, stack, maxDepth) bool`.
    pub(crate) fn is_deeply_nested_type(&self, t: TypeId, stack: &[TypeId], max_depth: usize) -> bool {
        if stack.len() >= max_depth {
            let target = get_recursion_identity_target(self, t);
            if self.type_(target).flags.intersects(TypeFlags::INTERSECTION) {
                for &t in self.type_(target).types() {
                    if self.is_deeply_nested_type(t, stack, max_depth) {
                        return true;
                    }
                }
            } else {
                let identity = get_recursion_identity_from_target(self, target);
                let mut count = 0usize;
                let mut last_type_id = TypeId(0);
                for &t in stack {
                    if has_matching_recursion_identity(self, t, identity) {
                        // We only count occurrences with a higher type id
                        // than the previous occurrence (Go comment, verbatim).
                        if t.0 >= last_type_id.0 {
                            count += 1;
                            if count >= max_depth {
                                return true;
                            }
                        }
                        last_type_id = t;
                    }
                }
            }
        }
        false
    }
}

/// Go: `func hasMatchingRecursionIdentity(t, identity) bool`.
fn has_matching_recursion_identity(c: &Checker, t: TypeId, identity: RecursionId) -> bool {
    let target = get_recursion_identity_target(c, t);
    if c.type_(target).flags.intersects(TypeFlags::INTERSECTION) {
        return c.type_(target).types().iter().any(|&t| {
            has_matching_recursion_identity(c, t, identity)
        });
    }
    get_recursion_identity_from_target(c, target) == identity
}

/// Go: `func getRecursionIdentity(t) RecursionId`.
pub(crate) fn get_recursion_identity(c: &Checker, t: TypeId) -> RecursionId {
    get_recursion_identity_from_target(c, get_recursion_identity_target(c, t))
}

/// Go: `func getRecursionIdentityTarget(t) *Type` — the indexed-access and
/// homomorphic-mapped unwrapping; both are unconstructible in the current
/// type surface, so the port carries the indexed-access arm and banner-noted
/// mapped arm (S7 owns getModifiersTypeFromMappedType).
pub(crate) fn get_recursion_identity_target(c: &Checker, t: TypeId) -> TypeId {
    let ty = c.type_(t);
    if ty.flags.intersects(TypeFlags::INDEXED_ACCESS) {
        let object_type = ty
            .as_indexed_access_type()
            .and_then(|d| d.object_type)
            .expect("indexed access objectType");
        return get_recursion_identity_target(c, object_type);
    }
    // Go: t.objectFlags&ObjectFlagsInstantiatedMapped == ObjectFlagsInstantatedMapped
    // → t.checker.getModifiersTypeFromMappedType(t) unwrapping.
    // TODO(port M6 S7): the homomorphic-mapped-type unwrap
    // (getModifiersTypeFromMappedType).
    t
}

/// Go: `func getRecursionIdentityFromTarget(t) RecursionId` — "The recursion
/// identity of a type is an object identity that is shared among multiple
/// instantiations of the type" (Go comment, verbatim).
pub(crate) fn get_recursion_identity_from_target(c: &Checker, t: TypeId) -> RecursionId {
    let ty = c.type_(t);
    if ty.flags.intersects(TypeFlags::OBJECT) && !u::is_object_or_array_literal_type(ty) {
        if ty.object_flags.intersects(crate::types::ObjectFlags::REFERENCE)
            && ty.as_type_reference().is_some_and(|r| r.node.is_some())
        {
            // Deferred type references are tracked through their associated
            // AST node (Go comment, verbatim).
            return RecursionId::Node(ty.as_type_reference().expect("TypeReference data").node.expect("deferred node"));
        }
        if let Some(symbol) = ty.symbol {
            if !(ty.object_flags.intersects(crate::types::ObjectFlags::ANONYMOUS)
                && c.program_symbol(symbol).flags.intersects(SymbolFlags::CLASS))
                && !ty.object_flags.intersects(crate::types::ObjectFlags::FROM_TYPE_NODE)
            {
                return RecursionId::Symbol(symbol);
            }
        }
        if c.is_tuple_type(t) && !ty.object_flags.intersects(crate::types::ObjectFlags::FROM_TYPE_NODE) {
            return RecursionId::Type(ty.target().expect("tuple target"));
        }
    }
    if ty.flags.intersects(TypeFlags::TYPE_PARAMETER) {
        if let Some(symbol) = ty.symbol {
            // We use the symbol of the type parameter such that all "fresh"
            // instantiations … have the same recursion identity (Go).
            return RecursionId::Symbol(symbol);
        }
    }
    if ty.flags.intersects(TypeFlags::CONDITIONAL) {
        // The root object represents the origin of the conditional type.
        let root_node = ty
            .as_conditional_type()
            .and_then(|d| d.root.as_deref())
            .and_then(|r| r.node);
        if let Some(node) = root_node {
            return RecursionId::Node(node);
        }
    }
    RecursionId::Type(t)
}

// ────────────────────────────────────────────────────────────────────────────
// Error display helpers (Go)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getTypeNamesForErrorDisplay(left, right) (string, string)`.
    pub(crate) fn get_type_names_for_error_display(
        &mut self,
        left: TypeId,
        right: TypeId,
    ) -> (String, String) {
        // Go: typeToString with the value declaration for context-sensitive
        // symbols — TypeToString is the nodebuilder wave.
        // TODO(port M6 W4): the context-sensitive error display forms.
        let left_str = self.type_to_string(left);
        let right_str = self.type_to_string(right);
        if left_str == right_str {
            let left_str = self.get_type_name_for_error_display(left);
            let right_str = self.get_type_name_for_error_display(right);
            return (left_str, right_str);
        }
        (left_str, right_str)
    }

    /// Go: `func (c *Checker) getTypeNameForErrorDisplay(t) string`.
    pub(crate) fn get_type_name_for_error_display(&mut self, t: TypeId) -> String {
        self.type_to_string_ex(t)
    }

    /// Go: `func (c *Checker) symbolValueDeclarationIsContextSensitive(symbol) bool`.
    #[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn symbol_value_declaration_is_context_sensitive(
        &mut self,
        _symbol: Option<SymbolId>,
    ) -> bool {
        // TODO(port M6 S9): isContextSensitive (contextual typing wave).
        false
    }

    /// Go: `func (c *Checker) typeCouldHaveTopLevelSingletonTypes(t) bool`.
    pub(crate) fn type_could_have_top_level_singleton_types(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::BOOLEAN) {
            return false;
        }
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let constituents: Vec<TypeId> = self.type_(t).types().to_vec();
            return constituents
                .iter()
                .any(|&t| self.type_could_have_top_level_singleton_types(t));
        }
        if flags.intersects(TypeFlags::INSTANTIABLE) {
            let constraint = get_constraint_of_type(self, t);
            if let Some(constraint) = constraint {
                if constraint != t {
                    return self.type_could_have_top_level_singleton_types(constraint);
                }
            }
        }
        is_unit_type(self, t)
            || flags.intersects(TypeFlags::TEMPLATE_LITERAL)
            || flags.intersects(TypeFlags::STRING_MAPPING)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Variance measurement (Go)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getVariances(t) []VarianceFlags`.
    pub(crate) fn get_variances(&mut self, t: TypeId) -> Vec<VarianceFlags> {
        // Arrays and tuples are known to be covariant (Go comment, verbatim).
        if Some(t) == self.global_array_type || Some(t) == self.global_readonly_array_type
            || self.type_(t).object_flags.intersects(crate::types::ObjectFlags::TUPLE)
        {
            return self.array_variances.clone();
        }
        let symbol = self.type_(t).symbol.expect("interface symbol");
        let type_parameters = self
            .type_(t)
            .as_interface_type()
            .map(|i| i.type_parameters().to_vec())
            .unwrap_or_default();
        self.get_variances_worker(symbol, type_parameters)
    }

    /// Go: `func (c *Checker) getAliasVariances(symbol) []VarianceFlags`.
    pub(crate) fn get_alias_variances(&mut self, symbol: SymbolId) -> Vec<VarianceFlags> {
        let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
        self.get_variances_worker(symbol, type_parameters)
    }

    /// Go: `func (c *Checker) getVariancesWorker(symbol, typeParameters) []VarianceFlags`.
    pub(crate) fn get_variances_worker(
        &mut self,
        symbol: SymbolId,
        type_parameters: Vec<TypeId>,
    ) -> Vec<VarianceFlags> {
        if !self.variance_links.get(symbol).variances.is_empty() {
            return self.variance_links.get(symbol).variances.clone();
        }
        let mut trace_args = None;
        if self.tracer.is_some() {
            let args = crate::tracing::Args::new();
            args.insert("arity", crate::tracing::TraceValue::Int(type_parameters.len() as i64));
            args.insert(
                "id",
                crate::tracing::TraceValue::Uint(get_declared_type_of_symbol(self, symbol).0 as u64),
            );
            if let Some(tr) = &self.tracer {
                let pop = tr.push(crate::tracing::Phase::CheckTypes, "getVariancesWorker", &args, true);
                trace_args = Some((args, pop));
            }
        }
        let result = self.get_variances_worker_inner(symbol, type_parameters);
        if let Some((args, pop)) = trace_args {
            let formatted: Vec<crate::tracing::TraceValue> = result
                .iter()
                .map(|v| crate::tracing::TraceValue::Str(v.to_string()))
                .collect();
            args.insert("variances", crate::tracing::TraceValue::Array(formatted));
            pop.pop();
        }
        if self.variance_links.get(symbol).variances.is_empty() {
            self.variance_links.get(symbol).variances = result.clone();
        }
        self.variance_links.get(symbol).variances.clone()
    }

    /// The body of getVariancesWorker after the trace prologue (Go inlines it
    /// around the deferred pop; the port hoists it so the trace args closing
    /// mutation — `traceArgs["variances"] = formatted` — lands before the
    /// pop exactly like Go's defer).
    fn get_variances_worker_inner(
        &mut self,
        symbol: SymbolId,
        type_parameters: Vec<TypeId>,
    ) -> Vec<VarianceFlags> {
        let stack_index = self.get_variance_stack_index(symbol);
        let mut variances = vec![VarianceFlags::INVARIANT; type_parameters.len()];
        if stack_index < 0 {
            let save_resolution_start = self.resolution_start;
            if self.variance_stack.is_empty() {
                self.resolution_start = self.type_resolutions.len();
            }
            self.variance_stack.push(VarianceStackEntry {
                symbol,
                type_parameters: type_parameters.clone(),
            });
            for (i, &tp) in type_parameters.iter().enumerate() {
                let modifiers = get_type_parameter_modifiers(self, tp);
                let variance = if modifiers.intersects(ModifierFlags::OUT) {
                    if modifiers.intersects(ModifierFlags::IN) {
                        VarianceFlags::INVARIANT
                    } else {
                        VarianceFlags::COVARIANT
                    }
                } else if modifiers.intersects(ModifierFlags::IN) {
                    VarianceFlags::CONTRAVARIANT
                } else {
                    let save_reliability_flags = self.reliability_flags;
                    self.reliability_flags = RelationComparisonResult::NONE;
                    // We first compare instantiations where the type parameter
                    // is replaced with marker types that have a known subtype
                    // relationship (Go comment, verbatim).
                    let type_with_super = create_marker_type(self, symbol, tp, self.marker_super_type);
                    let type_with_sub = create_marker_type(self, symbol, tp, self.marker_sub_type);
                    let mut variance = VarianceFlags::NONE;
                    if self.is_type_assignable_to(type_with_sub, type_with_super) {
                        variance |= VarianceFlags::COVARIANT;
                    }
                    if self.is_type_assignable_to(type_with_super, type_with_sub) {
                        variance |= VarianceFlags::CONTRAVARIANT;
                    }
                    if variance == VarianceFlags::BIVARIANT {
                        let marker_other = create_marker_type(self, symbol, tp, self.marker_other_type);
                        if self.is_type_assignable_to(marker_other, type_with_super) {
                            variance = VarianceFlags::INDEPENDENT;
                        }
                    }
                    if self
                        .reliability_flags
                        .intersects(RelationComparisonResult::REPORTS_UNMEASURABLE)
                    {
                        variance |= VarianceFlags::UNMEASURABLE;
                    }
                    if self
                        .reliability_flags
                        .intersects(RelationComparisonResult::REPORTS_UNRELIABLE)
                    {
                        variance |= VarianceFlags::UNRELIABLE;
                    }
                    self.reliability_flags = save_reliability_flags;
                    variance
                };
                // If variance computation was restarted due to a circularity
                // we may have already computed variances (Go, verbatim).
                if !self.variance_links.get(symbol).variances.is_empty() {
                    break;
                }
                variances[i] = variance;
            }
            // Store the results unless a restarted computation has already
            // stored them (Go, verbatim).
            if self.variance_links.get(symbol).variances.is_empty() {
                self.variance_links.get(symbol).variances = variances.clone();
            }
            self.variance_stack.pop();
            if self.variance_stack.is_empty() {
                self.resolution_start = save_resolution_start;
            }
        } else {
            // We've detected a circularity (Go comment, verbatim).
            let stack_symbols: Vec<SymbolId> =
                self.variance_stack.iter().map(|e| e.symbol).collect();
            let mut min_index = stack_index as usize;
            for i in (stack_index as usize + 1)..stack_symbols.len() {
                if u::compare_symbols_worker(self, self.store(), stack_symbols[i], stack_symbols[min_index])
                    == Ordering::Less
                {
                    min_index = i;
                }
            }
            if min_index > stack_index as usize {
                let save_variance_stack = std::mem::take(&mut self.variance_stack);
                let entry = save_variance_stack[min_index].clone();
                self.get_variances_worker(entry.symbol, entry.type_parameters);
                self.variance_stack = save_variance_stack;
            }
            // Store an empty slice to mark that we can't compute variances
            // for this type (Go, verbatim). PORT: Go's empty-vs-nil slice
            // distinction (nil = not computed, empty = circular) ports as an
            // explicit `circular_variances` marker on the links.
            if self.variance_links.get(symbol).variances.is_empty()
                && !self.variance_links.get(symbol).circular_variances
            {
                self.variance_links.get(symbol).circular_variances = true;
            }
            return Vec::new();
        }
        variances
    }

    /// Go: `func (c *Checker) getVarianceStackIndex(symbol) int` (-1 = absent).
    fn get_variance_stack_index(&self, symbol: SymbolId) -> isize {
        self.variance_stack
            .iter()
            .position(|entry| entry.symbol == symbol)
            .map(|i| i as isize)
            .unwrap_or(-1)
    }

    /// Go: `func (c *Checker) isMarkerType(t) bool`.
    pub(crate) fn is_marker_type(&self, t: TypeId) -> bool {
        self.marker_types.contains(&t)
    }

    /// Go: `func (c *Checker) hasCovariantVoidArgument(typeArguments, variances) bool`.
    pub(crate) fn has_covariant_void_argument(
        &self,
        type_arguments: &[TypeId],
        variances: &[VarianceFlags],
    ) -> bool {
        for (i, v) in variances.iter().enumerate() {
            if *v & VarianceFlags::VARIANCE_MASK == VarianceFlags::COVARIANT
                && self.type_(type_arguments[i]).flags.intersects(TypeFlags::VOID)
            {
                return true;
            }
        }
        false
    }
}

/// Go: `func (c *Checker) createMarkerType(symbol, source, target) *Type`.
fn create_marker_type(c: &mut Checker, symbol: SymbolId, source: TypeId, target: Option<TypeId>) -> TypeId {
    let target = target.expect("marker type (S7 constructs them in NewChecker)");
    let mapper = c.new_simple_type_mapper(source, target);
    let t = get_declared_type_of_symbol(c, symbol);
    if c.type_(t).flags.intersects(TypeFlags::ANY) && c.type_(t).as_intrinsic_type().is_some_and(|d| d.intrinsic_name == "error") {
        return t;
    }
    let result = if c.program_symbol(symbol).flags.intersects(SymbolFlags::TYPE_ALIAS) {
        let type_parameters = c.type_alias_links.get(symbol).type_parameters.clone();
        let instantiated = instantiate_types(c, type_parameters, mapper);
        // TODO(port M6 S7): the type-alias instantiation (W4 alias
        // machinery); the target reference stands in.
        let _ = symbol;
        c.create_type_reference(t, instantiated)
    } else {
        let type_parameters = c
            .type_(t)
            .as_interface_type()
            .map(|i| i.type_parameters().to_vec())
            .unwrap_or_default();
        let instantiated = instantiate_types(c, type_parameters, mapper);
        c.create_type_reference(t, instantiated)
    };
    c.marker_types.insert(result);
    result
}

/// Go: `func (c *Checker) getTypeParameterModifiers(tp) ast.ModifierFlags`.
fn get_type_parameter_modifiers(c: &Checker, tp: TypeId) -> ModifierFlags {
    let mut flags = ModifierFlags::NONE;
    if let Some(symbol) = c.type_(tp).symbol {
        for &d in &c.program_symbol(symbol).declarations {
            flags |= c.store().node(d).modifier_flags();
        }
    }
    flags & (ModifierFlags::IN | ModifierFlags::OUT | ModifierFlags::CONST)
}

/// Go: `func (c *Checker) instantiateTypes(types, mapper)` (checker.go's
/// instantiateList generic — the list form of instantiateType).
fn instantiate_types(c: &mut Checker, types: Vec<TypeId>, mapper: crate::todo_shapes::TypeMapperId) -> Vec<TypeId> {
    types
        .into_iter()
        .map(|t| c.instantiate_type(t, mapper))
        .collect()
}

// ────────────────────────────────────────────────────────────────────────────
// Signature comparison (Go)
// ────────────────────────────────────────────────────────────────────────────

/// The signature-arena accessor the signature family uses (Go: `*Signature`
/// derefs; the port's `SignatureId` indexes `Checker::signatures`).
pub(crate) fn sig(c: &Checker, id: crate::types::SignatureId) -> &crate::types::Signature {
    &c.signatures[id.0 as usize]
}

fn sig_mut(c: &mut Checker, id: crate::types::SignatureId) -> &mut crate::types::Signature {
    &mut c.signatures[id.0 as usize]
}

// TODO(port M6 W2): the full Go-mirrored signature/property ladders are
// exercised by the inference wave (W2 continues) and the services waves.
#[allow(dead_code)]
impl Checker {
    /// Go: `func (c *Checker) isSignatureAssignableTo(source, target, ignoreReturnTypes) bool`.
    pub fn is_signature_assignable_to(
        &mut self,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
        ignore_return_types: bool,
    ) -> bool {
        let check_mode = if ignore_return_types {
            SignatureCheckMode::IGNORE_RETURN_TYPES
        } else {
            SignatureCheckMode::NONE
        };
        self.compare_signatures_related(
            source,
            target,
            check_mode,
            false, /*reportErrors*/
            None,  /*errorReporter*/
            TypeComparerKind::Assignable,
            None, /*reportUnreliableMarkers*/
        ) != Ternary::False
    }

    /// Go: `func (c *Checker) compareSignaturesRelated(source, target, checkMode, reportErrors, errorReporter, compareTypes, reportUnreliableMarkers) Ternary`.
    #[allow(clippy::too_many_arguments)]
    pub fn compare_signatures_related(
        &mut self,
        source: crate::types::SignatureId,
        mut target: crate::types::SignatureId,
        check_mode: SignatureCheckMode,
        report_errors: bool,
        error_reporter: Option<ErrorReporterKind>,
        compare_types: TypeComparerKind,
        report_unreliable_markers: Option<crate::todo_shapes::TypeMapperId>,
    ) -> Ternary {
        let mut source = source;
        if source == target {
            return Ternary::True;
        }
        if !(check_mode.intersects(SignatureCheckMode::STRICT_TOP_SIGNATURE) && self.is_top_signature(source))
            && self.is_top_signature(target)
        {
            return Ternary::True;
        }
        if check_mode.intersects(SignatureCheckMode::STRICT_TOP_SIGNATURE)
            && self.is_top_signature(source)
            && !self.is_top_signature(target)
        {
            return Ternary::False;
        }
        let target_count = self.get_parameter_count(target);
        let mut source_has_more_parameters = false;
        if !self.has_effective_rest_parameter(target) {
            if check_mode.intersects(SignatureCheckMode::STRICT_ARITY) {
                source_has_more_parameters = self.has_effective_rest_parameter(source)
                    || self.get_parameter_count(source) > target_count;
            } else {
                source_has_more_parameters = self.get_min_argument_count(source) > target_count;
            }
        }
        if source_has_more_parameters {
            if report_errors && !check_mode.intersects(SignatureCheckMode::STRICT_ARITY) {
                self.report_signature_arity_error(error_reporter, source, target_count as usize);
            }
            return Ternary::False;
        }
        if !sig(self, source).type_parameters.is_empty()
            && sig(self, source).type_parameters != sig(self, target).type_parameters
        {
            target = self.get_canonical_signature(target);
            source = self.instantiate_signature_in_context_of(source, target, compare_types);
        }
        let source_count = self.get_parameter_count(source);
        let source_rest_type = self.get_non_array_rest_type(source);
        let target_rest_type = self.get_non_array_rest_type(target);
        if source_rest_type.is_some() || target_rest_type.is_some() {
            let rest = source_rest_type.or(target_rest_type).expect("checked");
            self.instantiate_type(rest, report_unreliable_markers.expect("reportUnreliableMarkers"));
        }
        let kind = sig(self, target)
            .declaration
            .map(|d| self.store().node(d).kind)
            .unwrap_or(tsc_ast::Kind::Unknown);
        let strict_variance = !check_mode.intersects(SignatureCheckMode::CALLBACK)
            && self.strict_function_types
            && kind != tsc_ast::Kind::MethodDeclaration
            && kind != tsc_ast::Kind::MethodSignature
            && kind != tsc_ast::Kind::Constructor;
        let mut result = Ternary::True;
        let source_this_type = self.get_this_type_of_signature(source);
        if let Some(source_this_type) = source_this_type {
            if source_this_type != self.void_type() {
                if let Some(target_this_type) = self.get_this_type_of_signature(target) {
                    // void sources are assignable to anything (Go, verbatim).
                    let mut related = Ternary::False;
                    if !strict_variance {
                        related = invoke_comparer(self, compare_types, None, source_this_type, target_this_type, false);
                    }
                    if related == Ternary::False {
                        related = invoke_comparer(self, compare_types, None, target_this_type, source_this_type, report_errors);
                    }
                    if related == Ternary::False {
                        if report_errors {
                            self.report_incompatible_this_types(error_reporter);
                        }
                        return Ternary::False;
                    }
                    result = ternary_and(result, related);
                }
            }
        }
        let param_count = if source_rest_type.is_some() || target_rest_type.is_some() {
            source_count.min(target_count)
        } else {
            source_count.max(target_count)
        };
        let rest_index = if source_rest_type.is_some() || target_rest_type.is_some() {
            param_count - 1
        } else {
            -1
        };
        for i in 0..param_count {
            let source_type = if i == rest_index {
                Some(self.get_rest_or_any_type_at_position(source, i as usize))
            } else {
                self.try_get_type_at_position(source, i as usize)
            };
            let target_type = if i == rest_index {
                Some(self.get_rest_or_any_type_at_position(target, i as usize))
            } else {
                self.try_get_type_at_position(target, i as usize)
            };
            let (Some(source_type), Some(target_type)) = (source_type, target_type) else {
                continue;
            };
            if source_type != target_type || check_mode.intersects(SignatureCheckMode::STRICT_ARITY) {
                // In order to ensure that any generic type Foo<T> is at least
                // co-variant with respect to T … we need to relate parameters
                // bi-variantly (Go comment, verbatim).
                let non_nullable_source_type = self.get_non_nullable_type(source_type);
                let source_sig = if !check_mode.intersects(SignatureCheckMode::CALLBACK)
                    && !self.is_instantiated_generic_parameter(source, i as usize)
                {
                    self.get_single_call_signature_of_type(non_nullable_source_type)
                } else {
                    None
                };
                let non_nullable_target_type = self.get_non_nullable_type(target_type);
                let target_sig = if !check_mode.intersects(SignatureCheckMode::CALLBACK)
                    && !self.is_instantiated_generic_parameter(target, i as usize)
                {
                    self.get_single_call_signature_of_type(non_nullable_target_type)
                } else {
                    None
                };
                let callbacks = source_sig.is_some()
                    && target_sig.is_some()
                    && source_sig.is_some_and(|s| self.get_type_predicate_of_signature(s).is_none())
                    && target_sig.is_some_and(|s| self.get_type_predicate_of_signature(s).is_none())
                    && self.get_type_facts(source_type, TypeFacts::IS_UNDEFINED_OR_NULL)
                        == self.get_type_facts(target_type, TypeFacts::IS_UNDEFINED_OR_NULL);
                
                let related = if callbacks {
                    let mut callback_mode = check_mode & SignatureCheckMode::STRICT_ARITY;
                    callback_mode |= if strict_variance {
                        SignatureCheckMode::STRICT_CALLBACK
                    } else {
                        SignatureCheckMode::BIVARIANT_CALLBACK
                    };
                    self.compare_signatures_related(
                        target_sig.expect("checked"),
                        source_sig.expect("checked"),
                        callback_mode,
                        report_errors,
                        error_reporter,
                        compare_types,
                        report_unreliable_markers,
                    )
                } else {
                    let mut r = Ternary::False;
                    if !check_mode.intersects(SignatureCheckMode::CALLBACK) && !strict_variance {
                        r = invoke_comparer(self, compare_types, None, source_type, target_type, false);
                    }
                    if r == Ternary::False {
                        invoke_comparer(self, compare_types, None, target_type, source_type, report_errors)
                    } else {
                        r
                    }
                };
                // With strict arity, (x: number | undefined) => void is a
                // subtype of (x?: number | undefined) => void (Go, verbatim).
                if ternary_not_false(related)
                    && check_mode.intersects(SignatureCheckMode::STRICT_ARITY)
                    && i >= self.get_min_argument_count(source)
                    && i < self.get_min_argument_count(target)
                    && invoke_comparer(self, compare_types, None, source_type, target_type, false) != Ternary::False
                {
                    return Ternary::False;
                }
                if related == Ternary::False {
                    if report_errors {
                        self.report_parameter_type_mismatch(error_reporter, source, target, i as usize);
                    }
                    return Ternary::False;
                }
                result = ternary_and(result, related);
            }
        }
        if !check_mode.intersects(SignatureCheckMode::IGNORE_RETURN_TYPES) {
            // If a signature resolution is already in-flight, skip issuing a
            // circularity error here (Go comment, verbatim).
            let target_return_type = self.get_non_circular_return_type_of_signature(target);
            if target_return_type == self.void_type() || target_return_type == self.any_type() {
                return result;
            }
            let source_return_type = self.get_non_circular_return_type_of_signature(source);
            let target_type_predicate = self.get_type_predicate_of_signature(target);
            if let Some(target_type_predicate) = target_type_predicate {
                let source_type_predicate = self.get_type_predicate_of_signature(source);
                match source_type_predicate {
                    Some(source_type_predicate) => {
                        result = ternary_and(
                            result,
                            self.compare_type_predicate_related_to(
                                &source_type_predicate,
                                &target_type_predicate,
                                report_errors,
                                error_reporter,
                                compare_types,
                            ),
                        );
                    }
                    None => {
                        if target_type_predicate.kind == TypePredicateKind::Identifier
                            || target_type_predicate.kind == TypePredicateKind::This
                        {
                            if report_errors {
                                self.report_predicate_required(error_reporter, source);
                            }
                            return Ternary::False;
                        }
                    }
                }
            } else {
                // When relating callback signatures, we still need to relate
                // return types bi-variantly (Go comment, verbatim).
                let mut related = Ternary::False;
                if check_mode.intersects(SignatureCheckMode::BIVARIANT_CALLBACK) {
                    related = invoke_comparer(self, compare_types, None, target_return_type, source_return_type, false);
                }
                if related == Ternary::False {
                    related = invoke_comparer(self, compare_types, None, source_return_type, target_return_type, report_errors);
                }
                result = ternary_and(result, related);
                if result == Ternary::False && report_errors {
                    self.report_return_type_mismatch(error_reporter, source, source_return_type, target_return_type);
                }
            }
        }
        result
    }

    /// Go: `func (c *Checker) compareTypePredicateRelatedTo(source, target, reportErrors, errorReporter, compareTypes) Ternary`.
    pub fn compare_type_predicate_related_to(
        &mut self,
        source: &TypePredicate,
        target: &TypePredicate,
        report_errors: bool,
        error_reporter: Option<ErrorReporterKind>,
        compare_types: TypeComparerKind,
    ) -> Ternary {
        if source.kind != target.kind {
            if report_errors {
                self.report_predicate_kind_mismatch(error_reporter, source, target);
            }
            return Ternary::False;
        }
        if (source.kind == TypePredicateKind::Identifier || source.kind == TypePredicateKind::AssertsIdentifier)
            && source.parameter_index != target.parameter_index {
                if report_errors {
                    self.report_predicate_position_mismatch(error_reporter, source, target);
                }
                return Ternary::False;
            }
        let related = match (source.t, target.t) {
            (Some(a), Some(b)) if a == b => Ternary::True,
            (Some(a), Some(b)) => invoke_comparer(self, compare_types, None, a, b, report_errors),
            _ => Ternary::False,
        };
        if related == Ternary::False && report_errors {
            self.report_predicate_incompatible(error_reporter, source, target);
        }
        related
    }

    /// Go: `func (c *Checker) isTopSignature(s) bool`.
    pub(crate) fn is_top_signature(&mut self, s: crate::types::SignatureId) -> bool {
        let signature = sig(self, s).clone();
        let this_is_any = match signature.this_parameter {
            Some(p) => {
                let this_type = get_type_of_parameter(self, p);
                is_type_any(self, this_type)
            }
            None => true,
        };
        if signature.type_parameters.is_empty()
            && this_is_any
            && signature.parameters.len() == 1
            && signature.has_rest_parameter()
        {
            let param_type = get_type_of_parameter(self, signature.parameters[0]);
            let rest_type = if self.is_array_type(param_type) {
                get_type_arguments(self, param_type)[0]
            } else {
                param_type
            };
            let return_type = self.get_return_type_of_signature(s);
            self.type_(rest_type).flags.intersects(TypeFlags::ANY | TypeFlags::NEVER)
                && self.type_(return_type).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
        } else {
            false
        }
    }

    /// Go: `func (c *Checker) getParameterCount(signature) int`.
    pub(crate) fn get_parameter_count(&mut self, signature: crate::types::SignatureId) -> i32 {
        let s = sig(self, signature).clone();
        let length = s.parameters.len() as i32;
        if s.has_rest_parameter() {
            let rest_type = self.get_type_of_symbol(s.parameters[length as usize - 1]);
            if self.is_tuple_type(rest_type) {
                let tuple = self.type_(rest_type).as_tuple_type().expect("tuple target");
                let target = self.type_(tuple.base.base.base.target.expect("tuple target")).as_tuple_type().expect("TupleType data");
                return length + target.fixed_length
                    - i32::from(target.combined_flags.intersects(ElementFlags::VARIABLE));
            }
        }
        length
    }

    /// Go: `func (c *Checker) getMinArgumentCount(signature) int`.
    pub(crate) fn get_min_argument_count(&mut self, signature: crate::types::SignatureId) -> i32 {
        self.get_min_argument_count_ex(signature, MinArgumentCountFlags::NONE)
    }

    /// Go: `func (c *Checker) getMinArgumentCountEx(signature, flags) int`.
    pub(crate) fn get_min_argument_count_ex(
        &mut self,
        signature: crate::types::SignatureId,
        flags: MinArgumentCountFlags,
    ) -> i32 {
        let strong_arity_for_untyped_js = flags.intersects(MinArgumentCountFlags::STRONG_ARITY_FOR_UNTYPED_JS);
        let void_is_non_optional = flags.intersects(MinArgumentCountFlags::VOID_IS_NON_OPTIONAL);
        if void_is_non_optional || sig(self, signature).resolved_min_argument_count == -1 {
            let s = sig(self, signature).clone();
            #[allow(unused_mut)] // the void-tail walk below narrows the count
            let mut min_argument_count = -1;
            if s.has_rest_parameter() {
                let rest_type = self.get_type_of_symbol(s.parameters[s.parameters.len() - 1]);
                if self.is_tuple_type(rest_type) {
                    let target = self
                        .type_(self.type_(rest_type).as_type_reference().expect("ref").base.target.expect("target"))
                        .as_tuple_type()
                        .expect("TupleType data");
                    let first_optional_index =
                        target.element_infos.iter().position(|info| !info.flags.intersects(ElementFlags::REQUIRED));
                    let required_count = match first_optional_index {
                        Some(i) => i as i32,
                        None => target.fixed_length,
                    };
                    if required_count > 0 {
                        min_argument_count = s.parameters.len() as i32 - 1 + required_count;
                    }
                }
            }
            if min_argument_count == -1 {
                if !strong_arity_for_untyped_js
                    && s.flags
                        .intersects(crate::types::SignatureFlags::IS_UNTYPED_SIGNATURE_IN_JS_FILE)
                {
                    return 0;
                }
                min_argument_count = s.min_argument_count;
            }
            if void_is_non_optional {
                return min_argument_count;
            }
            let mut i = min_argument_count - 1;
            while i >= 0 {
                let t = self.get_type_at_position(signature, i as usize);
                let constituents: Vec<TypeId> = if self.type_(t).flags.intersects(TypeFlags::UNION) {
                    self.type_(t).types().to_vec()
                } else {
                    vec![t]
                };
                if !constituents.iter().any(|&t2| self.type_(t2).flags.intersects(TypeFlags::VOID)) {
                    break;
                }
                min_argument_count = i;
                i -= 1;
            }
            sig_mut(self, signature).resolved_min_argument_count = min_argument_count;
        }
        sig(self, signature).resolved_min_argument_count
    }

    /// Go: `func (c *Checker) hasEffectiveRestParameter(signature) bool`.
    pub(crate) fn has_effective_rest_parameter(&mut self, signature: crate::types::SignatureId) -> bool {
        let s = sig(self, signature).clone();
        if s.has_rest_parameter() {
            let rest_type = self.get_type_of_symbol(s.parameters[s.parameters.len() - 1]);
            return !self.is_tuple_type(rest_type)
                || self
                    .type_(self.type_(rest_type).as_type_reference().expect("ref").base.target.expect("target"))
                    .as_tuple_type()
                    .expect("TupleType data")
                    .combined_flags
                    .intersects(ElementFlags::VARIABLE);
        }
        false
    }

    /// Go: `func (c *Checker) getTypeAtPosition(signature, pos) *Type`.
    pub(crate) fn get_type_at_position(&mut self, signature: crate::types::SignatureId, pos: usize) -> TypeId {
        match self.try_get_type_at_position(signature, pos) {
            Some(t) => t,
            None => self.any_type(),
        }
    }

    /// Go: `func (c *Checker) tryGetTypeAtPosition(signature, pos) *Type`.
    pub(crate) fn try_get_type_at_position(
        &mut self,
        signature: crate::types::SignatureId,
        pos: usize,
    ) -> Option<TypeId> {
        let s = sig(self, signature).clone();
        let param_count =
            s.parameters.len() - usize::from(s.has_rest_parameter());
        if pos < param_count {
            return Some(get_type_of_parameter(self, s.parameters[pos]));
        }
        if s.has_rest_parameter() {
            // We want to return the value undefined for an out of bounds
            // parameter position (Go comment, verbatim).
            let rest_type = self.get_type_of_symbol(s.parameters[param_count]);
            let index = pos - param_count;
            let target_tuple = if self.is_tuple_type(rest_type) {
                let target = self
                    .type_(rest_type)
                    .as_type_reference()
                    .expect("ref")
                    .base
                    .target
                    .expect("target");
                Some(
                    self.type_(target)
                        .as_tuple_type()
                        .expect("TupleType data"),
                )
            } else {
                None
            };
            let within_bounds = match &target_tuple {
                Some(tuple) => {
                    !tuple.combined_flags.intersects(ElementFlags::VARIABLE)
                        || (index as i32) < tuple.fixed_length
                }
                None => true,
            };
            if within_bounds {
                let index_literal = self.get_number_literal_type(tsc_jsnum::Number(index as f64));
                return Some(self.get_indexed_access_type(rest_type, index_literal));
            }
        }
        None
    }

    /// Go: `func (c *Checker) getRestOrAnyTypeAtPosition(source, pos) *Type`.
    pub(crate) fn get_rest_or_any_type_at_position(
        &mut self,
        source: crate::types::SignatureId,
        pos: usize,
    ) -> TypeId {
        let rest_type = self.get_rest_type_at_position(source, pos, false);
        if let Some(rest_type) = rest_type {
            if let Some(element_type) = self.get_element_type_of_array_type(rest_type) {
                if u::is_type_any(Some(self.type_(element_type))) {
                    return self.any_type();
                }
            }
        }
        rest_type.unwrap_or_else(|| self.any_type())
    }

    /// Go: `func (c *Checker) getRestTypeAtPosition(source, pos, readonly) *Type`.
    #[allow(clippy::needless_range_loop)] // TODO(port M6 W2): Go mirrors the index walk over the tuple slice
    pub(crate) fn get_rest_type_at_position(
        &mut self,
        source: crate::types::SignatureId,
        pos: usize,
        readonly: bool,
    ) -> Option<TypeId> {
        let parameter_count = self.get_parameter_count(source) as usize;
        let min_argument_count = self.get_min_argument_count(source) as usize;
        let rest_type = self.get_effective_rest_type(source);
        if rest_type.is_some() && pos >= parameter_count - 1 {
            if pos == parameter_count - 1 {
                return rest_type;
            }
            let Some(rest_type) = rest_type else {
                unreachable!("guarded: rest type present when pos < parameter_count - 1")
            };
            let number_type = self.number_type();
            let indexed = self.get_indexed_access_type(rest_type, number_type);
            return Some(self.create_array_type(indexed));
        }
        let length = parameter_count as isize - pos as isize;
        if length <= 0 {
            return Some(self.create_tuple_type_ex(&[], &[], readonly));
        }
        let length = length as usize;
        let mut types = vec![TypeId(0); length];
        let mut infos = Vec::with_capacity(length);
        for i in 0..length {
            
            let flags = if rest_type.is_none() || i < length - 1 {
                types[i] = self.get_type_at_position(source, i + pos);
                if i + pos < min_argument_count {
                    ElementFlags::REQUIRED
                } else {
                    ElementFlags::OPTIONAL
                }
            } else {
                let Some(rest_type) = rest_type else {
                    unreachable!("guarded: rest type present in the variadic arm")
                };
                types[i] = rest_type;
                ElementFlags::VARIADIC
            };
            infos.push(crate::types::TupleElementInfo {
                flags,
                labeled_declaration: self.get_nameable_declaration_at_position(source, i + pos),
            });
        }
        Some(self.create_tuple_type_ex(&types, &infos, readonly))
    }

    /// Go: `func (c *Checker) getNameableDeclarationAtPosition(signature, pos) *ast.Node`.
    fn get_nameable_declaration_at_position(
        &mut self,
        signature: crate::types::SignatureId,
        pos: usize,
    ) -> Option<NodeId> {
        let s = sig(self, signature).clone();
        let param_count = s.parameters.len() - usize::from(s.has_rest_parameter());
        if pos < param_count {
            let decl = self.program_symbol(s.parameters[pos]).value_declaration;
            if let Some(decl) = decl {
                if self.is_valid_declaration_for_tuple_label(decl) {
                    return Some(decl);
                }
            }
            return None;
        }
        if s.has_rest_parameter() {
            let rest_parameter = s.parameters[param_count];
            let rest_type = self.get_type_of_symbol(rest_parameter);
            if self.is_tuple_type(rest_type) {
                let tuple = self
                    .type_(self.type_(rest_type).as_type_reference().expect("ref").base.target.expect("target"))
                    .as_tuple_type()
                    .expect("TupleType data");
                let index = pos - param_count;
                if index < tuple.element_infos.len() {
                    return tuple.element_infos[index].labeled_declaration;
                }
                return None;
            }
            if let Some(decl) = self.program_symbol(rest_parameter).value_declaration {
                if self.is_valid_declaration_for_tuple_label(decl) {
                    return Some(decl);
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) isValidDeclarationForTupleLabel(d) bool`.
    fn is_valid_declaration_for_tuple_label(&self, d: NodeId) -> bool {
        let node = self.store().node(d);
        tsc_ast::is_named_tuple_member(node)
            || (tsc_ast::is_parameter_declaration(node)
                && node.name().is_some_and(|name| {
                    self.store().node(name).kind == tsc_ast::Kind::Identifier
                }))
    }

    /// Go: `func (c *Checker) getNonArrayRestType(signature) *Type`.
    pub(crate) fn get_non_array_rest_type(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> Option<TypeId> {
        let rest_type = self.get_effective_rest_type(signature)?;
        if !self.is_array_type(rest_type) && !u::is_type_any(Some(self.type_(rest_type))) {
            return Some(rest_type);
        }
        None
    }

    /// Go: `func (c *Checker) getEffectiveRestType(signature) *Type`.
    pub(crate) fn get_effective_rest_type(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> Option<TypeId> {
        let s = sig(self, signature).clone();
        if s.has_rest_parameter() {
            let rest_type = self.get_type_of_symbol(s.parameters[s.parameters.len() - 1]);
            if !self.is_tuple_type(rest_type) {
                if u::is_type_any(Some(self.type_(rest_type))) {
                    return Some(self.any_array_type);
                }
                return Some(rest_type);
            }
            let target = self
                .type_(rest_type)
                .as_type_reference()
                .expect("ref")
                .base
                .target
                .expect("target");
            let tuple = self
                .type_(target)
                .as_tuple_type()
                .expect("TupleType data");
            if tuple.combined_flags.intersects(ElementFlags::VARIABLE) {
                return Some(self.slice_tuple_type(rest_type, tuple.fixed_length as usize, 0));
            }
        }
        None
    }

    /// Go: `func (c *Checker) getThisTypeOfSignature(signature) *Type`.
    pub(crate) fn get_this_type_of_signature(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> Option<TypeId> {
        sig(self, signature).this_parameter.map(|p| self.get_type_of_symbol(p))
    }

    /// Go: `func (c *Checker) isInstantiatedGenericParameter(signature, pos) bool`.
    pub(crate) fn is_instantiated_generic_parameter(
        &mut self,
        signature: crate::types::SignatureId,
        pos: usize,
    ) -> bool {
        let target = match sig(self, signature).target {
            Some(target) => target,
            None => return false,
        };
        self.try_get_type_at_position(target, pos)
            .is_some_and(|t| self.is_generic_type(t))
    }

    /// Go: `func (c *Checker) getParameterNameAtPosition(signature, pos) string`.
    pub(crate) fn get_parameter_name_at_position(
        &mut self,
        signature: crate::types::SignatureId,
        pos: usize,
    ) -> String {
        let s = sig(self, signature).clone();
        let param_count = s.parameters.len() - usize::from(s.has_rest_parameter());
        if pos < param_count {
            return self.program_symbol(s.parameters[pos]).name.clone();
        }
        let rest_parameter = s.parameters[param_count];
        let rest_type = self.get_type_of_symbol(rest_parameter);
        if self.is_tuple_type(rest_type) {
            let index = pos - param_count;
            let tuple = self
                .type_(self.type_(rest_type).as_type_reference().expect("ref").base.target.expect("target"))
                .as_tuple_type()
                .expect("TupleType data");
            return self.get_tuple_element_label(
                tuple.element_infos[index].clone(),
                Some(rest_parameter),
                index,
            );
        }
        self.program_symbol(rest_parameter).name.clone()
    }

    /// Go: `func (c *Checker) getTupleElementLabel(elementInfo, restSymbol, index) string`.
    pub(crate) fn get_tuple_element_label(
        &self,
        element_info: crate::types::TupleElementInfo,
        rest_symbol: Option<SymbolId>,
        index: usize,
    ) -> String {
        if let Some(decl) = element_info.labeled_declaration {
            let name = self.store().node(decl).name().expect("labeled declaration name");
            return tsc_ast::node_text(self.store(), name).to_string();
        }
        if let Some(symbol) = rest_symbol {
            if let Some(decl) = self.program_symbol(symbol).value_declaration {
                if tsc_ast::is_parameter_declaration(self.store().node(decl)) {
                    return self.get_tuple_element_label_from_binding_element(decl, index, element_info.flags);
                }
            }
        }
        let root_name = match rest_symbol {
            Some(symbol) => self.program_symbol(symbol).name.clone(),
            None => "arg".to_string(),
        };
        format!("{root_name}_{index}")
    }

    /// Go: `func (c *Checker) getTupleElementLabelFromBindingElement(node, index, elementFlags) string`.
    fn get_tuple_element_label_from_binding_element(
        &self,
        node: NodeId,
        index: usize,
        element_flags: ElementFlags,
    ) -> String {
        let Some(name) = self.store().node(node).name() else {
            return format!("arg_{index}");
        };
        match self.store().node(name).kind {
            tsc_ast::Kind::Identifier => {
                let name_text = tsc_ast::node_text(self.store(), name).to_string();
                if u::has_dot_dot_dot_token(self.store(), node) {
                    if element_flags.intersects(ElementFlags::VARIABLE) {
                        return name_text;
                    }
                    return format!("{name_text}_{index}");
                }
                if element_flags.intersects(ElementFlags::FIXED) {
                    return name_text;
                }
                format!("{name_text}_n")
            }
            tsc_ast::Kind::ArrayBindingPattern => {
                if u::has_dot_dot_dot_token(self.store(), node) {
                    let elements: Vec<NodeId> = self
                        .store()
                        .node(name)
                        .as_binding_pattern()
                        .and_then(|bp| bp.elements.as_ref())
                        .map(|list| list.nodes.to_vec())
                        .unwrap_or_default();
                    let last_element = elements.last().copied();
                    let last_element_is_binding_element_rest = last_element.is_some_and(|e| {
                        tsc_ast::is_binding_element(self.store().node(e))
                            && u::has_dot_dot_dot_token(self.store(), e)
                    });
                    let element_count = elements.len() - usize::from(last_element_is_binding_element_rest);
                    if index < element_count {
                        let element = elements[index];
                        if tsc_ast::is_binding_element(self.store().node(element)) {
                            return self.get_tuple_element_label_from_binding_element(element, index, element_flags);
                        }
                    } else if last_element_is_binding_element_rest {
                        return self.get_tuple_element_label_from_binding_element(
                            last_element.expect("checked"),
                            index - element_count,
                            element_flags,
                        );
                    }
                }
                format!("arg_{index}")
            }
            _ => format!("arg_{index}"),
        }
    }

    /// Go: `func (c *Checker) getTypePredicateOfSignature(sig) *TypePredicate`.
    pub(crate) fn get_type_predicate_of_signature(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> Option<Box<TypePredicate>> {
        // PORT(note): Go's callers pass a non-nil *Signature; the exports
        // surface can still hand a not-yet-registered SignatureId (fresh
        // checker), which the port treats like Go's nil signature (no
        // predicate) instead of panicking on the arena index.
        if signature.0 as usize >= self.signatures.len() {
            return None;
        }
        if sig(self, signature).resolved_type_predicate.is_none() {
            let s = sig(self, signature).clone();
            if let Some(target) = s.target {
                if let Some(target_predicate) = self.get_type_predicate_of_signature(target) {
                    let mapper = s.mapper;
                    let instantiated = self.instantiate_type_predicate(target_predicate, mapper);
                    sig_mut(self, signature).resolved_type_predicate = Some(instantiated);
                }
            } else if let Some(composite) = &s.composite {
                let signatures = composite.signatures.clone();
                let is_union = composite.is_union;
                let predicate = self.get_union_or_intersection_type_predicate(&signatures, is_union);
                sig_mut(self, signature).resolved_type_predicate = predicate;
            } else if let Some(declaration) = s.declaration {
                // Go: the type-predicate-node and body paths —
                // createTypePredicateFromTypePredicateNode needs type-node
                // resolution (S5) and getTypePredicateFromBody needs flow (W3).
                // TODO(port M6 S5/W3): the declared type-predicate resolution.
                let _ = declaration;
            }
            if sig(self, signature).resolved_type_predicate.is_none() {
                // Go: noTypePredicate — the non-predicate sentinel. PORT: the
                // port marks it as an explicit `no_type_predicate` flag rather
                // than a sentinel pointer.
                sig_mut(self, signature).no_type_predicate = true;
            }
        }
        if sig(self, signature).no_type_predicate {
            return None;
        }
        sig(self, signature).resolved_type_predicate.clone()
    }

    /// Go: `func (c *Checker) getUnionOrIntersectionTypePredicate(signatures, isUnion) *TypePredicate`.
    fn get_union_or_intersection_type_predicate(
        &mut self,
        signatures: &[crate::types::SignatureId],
        is_union: bool,
    ) -> Option<Box<TypePredicate>> {
        let mut last: Option<Box<TypePredicate>> = None;
        let mut types: Vec<TypeId> = Vec::new();
        for &sig_id in signatures {
            if let Some(pred) = self.get_type_predicate_of_signature(sig_id) {
                // Constituent type predicates must all have matching kinds
                // (Go comment, verbatim).
                if (pred.kind != TypePredicateKind::This && pred.kind != TypePredicateKind::Identifier)
                    || last.is_some_and(|l| !self.type_predicate_kinds_match(&l, &pred))
                {
                    return None;
                }
                types.extend(pred.t);
                last = Some(pred);
            } else {
                // In composite union signatures we permit and ignore
                // signatures with a return type `false` (Go, verbatim).
                let return_type = if is_union {
                    Some(self.get_return_type_of_signature(sig_id))
                } else {
                    None
                };
                if return_type.is_some_and(|t| t != self.false_type() && t != self.regular_false_type()) {
                    return None;
                }
            }
        }
        let last = last?;
        let composite_type = self.get_union_or_intersection_type(&types, is_union);
        Some(Box::new(self.new_type_predicate(
            last.kind,
            &last.parameter_name,
            last.parameter_index,
            Some(composite_type),
        )))
    }

    /// Go: `func (c *Checker) typePredicateKindsMatch(a, b) bool`.
    pub(crate) fn type_predicate_kinds_match(&self, a: &TypePredicate, b: &TypePredicate) -> bool {
        a.kind == b.kind && a.parameter_index == b.parameter_index
    }

    /// Go: `func (c *Checker) instantiateTypePredicate(predicate, mapper) *TypePredicate`.
    pub(crate) fn instantiate_type_predicate(
        &mut self,
        predicate: Box<TypePredicate>,
        mapper: Option<crate::todo_shapes::TypeMapperId>,
    ) -> Box<TypePredicate> {
        let t = match (predicate.t, mapper) {
            (Some(t), Some(mapper)) => Some(self.instantiate_type(t, mapper)),
            (Some(t), None) => Some(t),
            (None, _) => None,
        };
        if t == predicate.t {
            return predicate;
        }
        Box::new(TypePredicate {
            kind: predicate.kind,
            parameter_index: predicate.parameter_index,
            parameter_name: predicate.parameter_name.clone(),
            t,
        })
    }

    /// Go: `func (c *Checker) newTypePredicate(kind, parameterName, parameterIndex, t)`.
    pub(crate) fn new_type_predicate(
        &self,
        kind: TypePredicateKind,
        parameter_name: &str,
        parameter_index: i32,
        t: Option<TypeId>,
    ) -> TypePredicate {
        TypePredicate {
            kind,
            parameter_index,
            parameter_name: parameter_name.to_string(),
            t,
        }
    }

    /// Go: `func (c *Checker) isResolvingReturnTypeOfSignature(signature) bool`.
    pub(crate) fn is_resolving_return_type_of_signature(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> bool {
        if let Some(composite) = sig(self, signature).composite.clone() {
            if composite
                .signatures
                .iter()
                .any(|&s| self.is_resolving_return_type_of_signature(s))
            {
                return true;
            }
        }
        sig(self, signature).resolved_return_type.is_none()
            && self.find_resolution_cycle_start_index_for_signature(signature)
    }

    /// Go: `findResolutionCycleStartIndex(signature, ResolvedReturnType) >= 0`
    /// — PORT: the generic cycle detector (checker.rs) is keyed on
    /// TypeSystemEntity; the signature form is local here.
    fn find_resolution_cycle_start_index_for_signature(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> bool {
        let mut i = self.type_resolutions.len() as isize - 1;
        while i >= self.resolution_start as isize {
            let resolution = self.type_resolutions[i as usize];
            if resolution.target == crate::TypeSystemEntity::Signature(signature)
                && resolution.property_name == crate::checker::TypeSystemPropertyName::ResolvedReturnType
            {
                return true;
            }
            if self.type_resolution_has_property(&resolution) {
                return false;
            }
            i -= 1;
        }
        false
    }

    /// Go: `func (c *Checker) getNonCircularReturnTypeOfSignature(signature) *Type`
    /// (checker.go — the relater's signature ladder consumes it; see the
    /// checker.go stub section note).
    pub(crate) fn get_non_circular_return_type_of_signature(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> TypeId {
        if self.is_resolving_return_type_of_signature(signature) {
            // If a signature resolution is already in-flight … use the `any`
            // type directly (Go comment, verbatim).
            return self.any_type();
        }
        self.get_return_type_of_signature(signature)
    }

    /// Go: `func (c *Checker) findMatchingSignatures(signatureLists, signature, listIndex) []*Signature`.
    pub(crate) fn find_matching_signatures(
        &mut self,
        signature_lists: &[Vec<crate::types::SignatureId>],
        signature: crate::types::SignatureId,
        list_index: usize,
    ) -> Option<Vec<crate::types::SignatureId>> {
        if !sig(self, signature).type_parameters.is_empty() {
            // We require an exact match for generic signatures (Go, verbatim).
            if list_index > 0 {
                return None;
            }
            for list in signature_lists.iter().take(1).skip(1) {
                self
                    .find_matching_signature(list, signature, false, false, false)?;
            }
            return Some(vec![signature]);
        }
        let mut result: Vec<crate::types::SignatureId> = Vec::new();
        for (i, list) in signature_lists.iter().enumerate() {
            // Allow matching non-generic signatures to have excess parameters
            // (Go comment, verbatim).
            let m = if i == list_index {
                Some(signature)
            } else {
                self.find_matching_signature(list, signature, false, false, true)
                    .or_else(|| self.find_matching_signature(list, signature, true, false, true))
            };
            let m = m?;
            if !result.contains(&m) {
                result.push(m);
            }
        }
        Some(result)
    }

    /// Go: `func (c *Checker) findMatchingSignature(signatureList, signature, partialMatch, ignoreThisTypes, ignoreReturnTypes) *Signature`.
    pub(crate) fn find_matching_signature(
        &mut self,
        signature_list: &[crate::types::SignatureId],
        signature: crate::types::SignatureId,
        partial_match: bool,
        ignore_this_types: bool,
        ignore_return_types: bool,
    ) -> Option<crate::types::SignatureId> {
        let compare_types = if partial_match {
            TypeComparerKind::SubtypeOf
        } else {
            TypeComparerKind::Identical
        };
        signature_list.iter().find(|&&s| self.compare_signatures_identical(
                s,
                signature,
                partial_match,
                ignore_this_types,
                ignore_return_types,
                compare_types,
            ) != Ternary::False).copied()
    }

    /// Go: `func (c *Checker) compareSignaturesIdentical(source, target, partialMatch, ignoreThisTypes, ignoreReturnTypes, compareTypes) Ternary`.
    pub fn compare_signatures_identical(
        &mut self,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
        partial_match: bool,
        ignore_this_types: bool,
        ignore_return_types: bool,
        compare_types: TypeComparerKind,
    ) -> Ternary {
        let mut source = source;
        if source == target {
            return Ternary::True;
        }
        if !self.is_matching_signature(source, target, partial_match) {
            return Ternary::False;
        }
        // Check that the two signatures have the same number of type
        // parameters (Go comment, verbatim).
        if sig(self, source).type_parameters.len() != sig(self, target).type_parameters.len() {
            return Ternary::False;
        }
        // Check that type parameter constraints and defaults match (Go).
        if !sig(self, target).type_parameters.is_empty() {
            let source_params = sig(self, source).type_parameters.clone();
            let target_params = sig(self, target).type_parameters.clone();
            let mapper = self.new_type_mapper(&source_params, &target_params);
            for i in 0..target_params.len() {
                let s = source_params[i];
                let t = target_params[i];
                let s_constraint = self.get_constraint_or_unknown_from_type_parameter(s);
                let t_constraint = self.get_constraint_or_unknown_from_type_parameter(t);
                let s_default = self.get_default_or_unknown_from_type_parameter(s);
                let t_default = self.get_default_or_unknown_from_type_parameter(t);
                let s_constraint_inst = self.instantiate_type(s_constraint, mapper);
                let constraints_ok = invoke_comparer(
                    self,
                    compare_types,
                    None,
                    s_constraint_inst,
                    t_constraint,
                    true,
                ) != Ternary::False;
                let s_default_inst = self.instantiate_type(s_default, mapper);
                let defaults_ok = invoke_comparer(
                    self,
                    compare_types,
                    None,
                    s_default_inst,
                    t_default,
                    true,
                ) != Ternary::False;
                if !(s == t || constraints_ok && defaults_ok) {
                    return Ternary::False;
                }
            }
            source = self.instantiate_signature_ex(source, Some(mapper), true /*eraseTypeParameters*/);
        }
        let mut result = Ternary::True;
        if !ignore_this_types {
            if let Some(source_this_type) = self.get_this_type_of_signature(source) {
                if let Some(target_this_type) = self.get_this_type_of_signature(target) {
                    let related = invoke_comparer(self, compare_types, None, source_this_type, target_this_type, true);
                    if related == Ternary::False {
                        return Ternary::False;
                    }
                    result = ternary_and(result, related);
                }
            }
        }
        for i in 0..self.get_parameter_count(target) as usize {
            let s = self.get_type_at_position(source, i);
            let t = self.get_type_at_position(target, i);
            let related = invoke_comparer(self, compare_types, None, t, s, true);
            if related == Ternary::False {
                return Ternary::False;
            }
            result = ternary_and(result, related);
        }
        if !ignore_return_types {
            let source_type_predicate = self.get_type_predicate_of_signature(source);
            let target_type_predicate = self.get_type_predicate_of_signature(target);
            if source_type_predicate.is_some() || target_type_predicate.is_some() {
                result = ternary_and(
                    result,
                    self.compare_type_predicates_identical(
                        source_type_predicate,
                        target_type_predicate,
                        compare_types,
                    ),
                );
            } else {
                let source_return = self.get_return_type_of_signature(source);
                let target_return = self.get_return_type_of_signature(target);
                result = ternary_and(
                    result,
                    invoke_comparer(self, compare_types, None, source_return, target_return, true),
                );
            }
        }
        result
    }

    /// Go: `func (c *Checker) isMatchingSignature(source, target, partialMatch) bool`.
    pub(crate) fn is_matching_signature(
        &mut self,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
        partial_match: bool,
    ) -> bool {
        let source_parameter_count = self.get_parameter_count(source);
        let target_parameter_count = self.get_parameter_count(target);
        let source_min_argument_count = self.get_min_argument_count(source);
        let target_min_argument_count = self.get_min_argument_count(target);
        let source_has_rest_parameter = self.has_effective_rest_parameter(source);
        let target_has_rest_parameter = self.has_effective_rest_parameter(target);
        // A source signature matches a target signature if the two signatures
        // have the same number of required, optional, and rest parameters (Go).
        if source_parameter_count == target_parameter_count
            && source_min_argument_count == target_min_argument_count
            && source_has_rest_parameter == target_has_rest_parameter
        {
            return true;
        }
        if partial_match && source_min_argument_count <= target_min_argument_count {
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) compareTypeParametersIdentical(sourceParams, targetParams) bool`.
    pub(crate) fn compare_type_parameters_identical(
        &mut self,
        source_params: &[TypeId],
        target_params: &[TypeId],
    ) -> bool {
        if source_params.len() != target_params.len() {
            return false;
        }
        let mapper = self.new_type_mapper(target_params, source_params);
        for i in 0..source_params.len() {
            let source = source_params[i];
            let target = target_params[i];
            if source == target {
                continue;
            }
            // We instantiate the target type parameter constraints into the
            // source types so we can recognize `<T, U extends T>` as the same
            // as `<A, B extends A>` (Go comment, verbatim).
            let source_constraint = get_constraint_from_type_parameter(self, source)
                .unwrap_or_else(|| self.unknown_type());
            let target_constraint = get_constraint_from_type_parameter(self, target)
                .unwrap_or_else(|| self.unknown_type());
            let target_constraint_inst = self.instantiate_type(target_constraint, mapper);
            if !self.is_type_identical_to(source_constraint, target_constraint_inst) {
                return false;
            }
        }
        true
    }

    /// Go: `func (c *Checker) compareTypePredicatesIdentical(source, target, compareTypes) Ternary`.
    pub(crate) fn compare_type_predicates_identical(
        &mut self,
        source: Option<Box<TypePredicate>>,
        target: Option<Box<TypePredicate>>,
        compare_types: TypeComparerKind,
    ) -> Ternary {
        match (&source, &target) {
            (None, _) | (_, None) => Ternary::False,
            (Some(s), Some(t)) => {
                if !self.type_predicate_kinds_match(s, t) {
                    return Ternary::False;
                }
                match (s.t, t.t) {
                    (Some(a), Some(b)) if a == b => Ternary::True,
                    (Some(a), Some(b)) => invoke_comparer(self, compare_types, None, a, b, true),
                    _ => Ternary::False,
                }
            }
        }
    }

    /// Go: `func (c *Checker) getEffectiveConstraintOfIntersection(types, targetIsUnion) *Type`.
    pub(crate) fn get_effective_constraint_of_intersection(
        &mut self,
        types: &[TypeId],
        target_is_union: bool,
    ) -> Option<TypeId> {
        let mut constraints: Vec<TypeId> = Vec::new();
        let mut has_disjoint_domain_type = false;
        for &t in types {
            let flags = self.type_(t).flags;
            if flags.intersects(TypeFlags::INSTANTIABLE) {
                // We keep following constraints as long as we have an
                // instantiable type that is known not to be circular or
                // infinite (Go comment, verbatim).
                let mut constraint = get_constraint_of_type(self, t);
                while let Some(current) = constraint {
                    let f = self.type_(current).flags;
                    if f.intersects(TypeFlags::TYPE_PARAMETER | TypeFlags::INDEX | TypeFlags::CONDITIONAL) {
                        constraint = get_constraint_of_type(self, current);
                    } else {
                        break;
                    }
                }
                if let Some(constraint) = constraint {
                    constraints.push(constraint);
                    if target_is_union {
                        constraints.push(t);
                    }
                }
            } else if flags.intersects(TypeFlags::DISJOINT_DOMAINS) || self.is_empty_anonymous_object_type(t) {
                has_disjoint_domain_type = true;
            }
        }
        // If the target is a union type or if we are intersecting with types
        // belonging to one of the disjoint domains (Go comment, verbatim).
        if !constraints.is_empty() && (target_is_union || has_disjoint_domain_type) {
            if has_disjoint_domain_type {
                for &t in types {
                    if self.type_(t).flags.intersects(TypeFlags::DISJOINT_DOMAINS)
                        || self.is_empty_anonymous_object_type(t)
                    {
                        constraints.push(t);
                    }
                }
            }
            // The source types were normalized; ensure the result is too.
            let intersection = self.get_intersection_type_ex(&constraints, IntersectionFlags::NO_CONSTRAINT_REDUCTION, None);
            return Some(self.get_normalized_type(intersection, false /*writing*/));
        }
        None
    }

    /// Go: `func (c *Checker) isTypeSubsetOf(source, target) bool`.
    pub(crate) fn is_type_subset_of(&mut self, source: TypeId, target: TypeId) -> bool {
        source == target
            || self.type_(source).flags.intersects(TypeFlags::NEVER)
            || self.type_(target).flags.intersects(TypeFlags::UNION)
                && self.is_type_subset_of_union(source, target)
    }

    /// Go: `func (c *Checker) isTypeSubsetOfUnion(source, target) bool`.
    pub(crate) fn is_type_subset_of_union(&mut self, source: TypeId, target: TypeId) -> bool {
        if self.type_(source).flags.intersects(TypeFlags::UNION) {
            let target_types: Vec<TypeId> = self.type_(target).types().to_vec();
            return self
                .type_(source)
                .types()
                .iter()
                .all(|&t| contains_type(self, &target_types, t));
        }
        if self.type_(source).flags.intersects(TypeFlags::ENUM_LIKE)
            && Some(target) == self.get_base_type_of_enum_like_type(source)
        {
            return true;
        }
        contains_type(self, self.type_(target).types(), source)
    }

    /// Go: `func (c *Checker) isTypeDerivedFrom(source, target) bool`.
    pub(crate) fn is_type_derived_from(&mut self, source: TypeId, target: TypeId) -> bool {
        let source_flags = self.type_(source).flags;
        let target_flags = self.type_(target).flags;
        if self.type_(source).flags.intersects(TypeFlags::UNION) {
            let source_types: Vec<TypeId> = self.type_(source).types().to_vec();
            return source_types.iter().all(|&t| self.is_type_derived_from(t, target));
        }
        if target_flags.intersects(TypeFlags::UNION) {
            let target_types: Vec<TypeId> = self.type_(target).types().to_vec();
            return target_types.iter().any(|&t| self.is_type_derived_from(source, t));
        }
        if source_flags.intersects(TypeFlags::INTERSECTION) {
            let source_types: Vec<TypeId> = self.type_(source).types().to_vec();
            return source_types.iter().any(|&t| self.is_type_derived_from(t, target));
        }
        if source_flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
            let constraint = get_base_constraint_of_type(self, source)
                .unwrap_or_else(|| self.unknown_type());
            return self.is_type_derived_from(constraint, target);
        }
        if self.is_empty_anonymous_object_type(target) {
            return source_flags.intersects(TypeFlags::OBJECT | TypeFlags::NON_PRIMITIVE);
        }
        if Some(target) == self.global_object_type {
            return source_flags.intersects(TypeFlags::OBJECT | TypeFlags::NON_PRIMITIVE)
                && !self.is_empty_anonymous_object_type(source);
        }
        if Some(target) == self.global_function_type {
            return source_flags.intersects(TypeFlags::OBJECT) && self.is_function_object_type(source);
        }
        self.has_base_type(source, target)
            || (self.is_array_type(target)
                && !self.is_readonly_array_type(target)
                && self.is_type_derived_from(source, self.global_readonly_array_type.expect("globalReadonlyArrayType (S2)")))
    }

    /// Go: `func (c *Checker) isDistributionDependent(root) bool`.
    pub(crate) fn is_distribution_dependent(
        &mut self,
        root: &crate::types::ConditionalRoot,
    ) -> bool {
        // TODO(port M6 S10): isTypeParameterPossiblyReferenced.
        root.is_distributive
    }
}

/// Go: `func (c *Checker) getNonNullableType(t)` (utilities.go — the signature
/// ladder's callback-parameter pre-step).
pub(crate) fn get_non_nullable_type(_c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7): removeMissingType/u = undefined — the non-nullable
    // form needs union construction; identity for the current surface
    // (the call sites guard on undefined-containing types).
    t
}

/// Go: `func isTypeAny(t) bool` — `u::is_type_any` takes Option<&Type>.
fn is_type_any(c: &Checker, t: TypeId) -> bool {
    u::is_type_any(Some(c.type_(t)))
}

// The TypeFacts bits the relater reads (Go checker.go TypeFacts — the full
// facts wave is S10; the bits are Go's exact values so the S2 binding-element
// and S10 facts workers share one lattice).
define_flags!(TypeFacts, u32);
impl TypeFacts {
    /// Go: `TypeFactsEQUndefined TypeFacts = 1 << 16`.
    pub const EQ_UNDEFINED: TypeFacts = TypeFacts(1 << 16);
    /// Go: `TypeFactsNEUndefined TypeFacts = 1 << 19`.
    pub const NE_UNDEFINED: TypeFacts = TypeFacts(1 << 19);
    /// Go: `TypeFactsIsUndefined TypeFacts = 1 << 24`.
    pub const IS_UNDEFINED: TypeFacts = TypeFacts(1 << 24);
    /// Go: `TypeFactsIsNull TypeFacts = 1 << 25`.
    pub const IS_NULL: TypeFacts = TypeFacts(1 << 25);
    /// Go: `TypeFactsIsUndefinedOrNull = TypeFactsIsUndefined | TypeFactsIsNull`.
    pub const IS_UNDEFINED_OR_NULL: TypeFacts =
        TypeFacts(Self::IS_UNDEFINED.0 | Self::IS_NULL.0);
}

// ────────────────────────────────────────────────────────────────────────────
// Relater (Go relater.go 2599+)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func visibilityToString(flags ast.ModifierFlags) string`.
fn visibility_to_string(flags: ModifierFlags) -> &'static str {
    if flags == ModifierFlags::PRIVATE {
        "private"
    } else if flags == ModifierFlags::PROTECTED {
        "protected"
    } else {
        "public"
    }
}

/// Go: `type errorState struct`.
struct ErrorState {
    error_chain: Option<Box<ErrorChain>>,
    related_info: Vec<tsc_ast::Diagnostic>,
}

/// Go: `type ErrorChain struct` — a linked list (PORT: `Option<Box<…>>`).
#[derive(Clone)]
pub struct ErrorChain {
    pub next: Option<Box<ErrorChain>>,
    pub message: &'static Message,
    pub args: Vec<DiagArg>,
}

impl PartialEq for ErrorChain {
    /// Go's `errorChain == other` compares the linked lists — message
    /// pointer identity plus arg equality (restore_error_state's
    /// consistency check reads exactly this).
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.message, other.message)
            && self.args == other.args
            && self.next == other.next
    }
}

/// Go: `type Relater struct` — the comparison engine instance. PORT: Go's
/// `c *Checker` back-pointer becomes the explicit `&mut Checker` first
/// parameter on every method (§5.4); `next *Relater` (the free-list link)
/// lives in the checker's `free_relater` Vec.
pub struct Relater {
    relation: RelationKind,
    error_node: Option<NodeId>,
    error_chain: Option<Box<ErrorChain>>,
    related_info: Vec<tsc_ast::Diagnostic>,
    maybe_keys: Vec<crate::todo_shapes::CacheHashKey>,
    maybe_keys_set: FxHashSet<crate::todo_shapes::CacheHashKey>,
    source_stack: Vec<TypeId>,
    target_stack: Vec<TypeId>,
    expanding_flags: ExpandingFlags,
    overflow: bool,
    relation_count: i64,
}

impl Default for Relater {
    fn default() -> Self {
        Relater {
            relation: RelationKind::Assignable,
            error_node: None,
            error_chain: None,
            related_info: Vec::new(),
            maybe_keys: Vec::new(),
            maybe_keys_set: FxHashSet::default(),
            source_stack: Vec::new(),
            target_stack: Vec::new(),
            expanding_flags: ExpandingFlags::NONE,
            overflow: false,
            relation_count: 0,
        }
    }
}

impl Checker {
    /// Go: `func (c *Checker) getRelater() *Relater`.
    pub(crate) fn get_relater(&mut self) -> Relater {
        self.free_relater.pop().unwrap_or_default()
    }

    /// Go: `func (c *Checker) putRelater(r *Relater)` — Go re-uses the
    /// allocated slices (`r.maybeKeys[:0]`); the port keeps the Vecs
    /// (Rust's capacity retention is the same amortization).
    pub(crate) fn put_relater(&mut self, mut r: Relater) {
        r.maybe_keys_set.clear();
        r.maybe_keys.clear();
        r.source_stack.clear();
        r.target_stack.clear();
        r.error_chain = None;
        r.related_info.clear();
        self.free_relater.push(r);
    }
}

impl Relater {
    /// Go: `func (r *Relater) isRelatedToSimple(source, target) Ternary`.
    pub(crate) fn is_related_to_simple(&mut self, c: &mut Checker, source: TypeId, target: TypeId) -> Ternary {
        self.is_related_to_ex(c, source, target, RecursionFlags::BOTH, false, None, IntersectionState::NONE)
    }

    /// Go: `func (r *Relater) isRelatedToWorker(source, target, reportErrors) Ternary`.
    pub(crate) fn is_related_to_worker(&mut self, c: &mut Checker, source: TypeId, target: TypeId, report_errors: bool) -> Ternary {
        self.is_related_to_ex(c, source, target, RecursionFlags::BOTH, report_errors, None, IntersectionState::NONE)
    }

    /// Go: `func (r *Relater) isRelatedTo(source, target, recursionFlags, reportErrors) Ternary`.
    pub(crate) fn is_related_to(&mut self, c: &mut Checker, source: TypeId, target: TypeId, recursion_flags: RecursionFlags, report_errors: bool) -> Ternary {
        self.is_related_to_ex(c, source, target, recursion_flags, report_errors, None, IntersectionState::NONE)
    }

    /// Go: `func (r *Relater) isRelatedToEx(...) Ternary` — the engine's
    /// front door (fresh normalization, fastpaths, excess/common property
    /// checks, cache-or-recurse).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn is_related_to_ex(
        &mut self,
        c: &mut Checker,
        original_source: TypeId,
        original_target: TypeId,
        recursion_flags: RecursionFlags,
        report_errors: bool,
        head_message: Option<&'static Message>,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if original_source == original_target {
            return Ternary::True;
        }
        // Before normalization: if `source` is an object type, and `target`
        // is primitive, skip all the checks we don't need (Go comment,
        // verbatim).
        if c.type_(original_source).flags.intersects(TypeFlags::OBJECT)
            && c.type_(original_target).flags.intersects(TypeFlags::PRIMITIVE)
        {
            if (self.relation == RelationKind::Comparable
                && !c.type_(original_target).flags.intersects(TypeFlags::NEVER)
                && c.is_simple_type_related_to(original_target, original_source, self.relation, None))
                || c.is_simple_type_related_to(original_source, original_target, self.relation, if report_errors { Some(ErrorReporterKind::Relater) } else { None })
            {
                return Ternary::True;
            }
            if report_errors {
                self.report_error_results(c, original_source, original_target, original_source, original_target, head_message);
            }
            return Ternary::False;
        }
        // Normalize the source and target types (Go comment, verbatim).
        let source = get_normalized_type(c, original_source, false /*writing*/);
        let target = get_normalized_type(c, original_target, true /*writing*/);
        if source == target {
            return Ternary::True;
        }
        if self.relation == RelationKind::Identity {
            if c.type_(source).flags != c.type_(target).flags {
                return Ternary::False;
            }
            if c.type_(source).flags.intersects(TypeFlags::SINGLETON) {
                return Ternary::True;
            }
            self.trace_unions_or_intersections_too_large(c, source, target);
            return self.recursive_type_related_to(c, source, target, false, IntersectionState::NONE, recursion_flags);
        }
        // We fastpath comparing a type parameter to exactly its constraint
        // (Go comment, verbatim).
        if c.type_(source).flags.intersects(TypeFlags::TYPE_PARAMETER)
            && get_constraint_of_type(c, source) == Some(target)
        {
            return Ternary::True;
        }
        // See if we're relating a definitely non-nullable type to a union
        // that includes null and/or undefined plus a single non-nullable type
        // (Go comment, verbatim).
        if c.type_(source).flags.intersects(TypeFlags::DEFINITELY_NON_NULLABLE)
            && c.type_(target).flags.intersects(TypeFlags::UNION)
        {
            let types = c.type_(target).types().to_vec();
            let candidate = match types.as_slice() {
                [t0, t1] if c.type_(*t0).flags.intersects(TypeFlags::NULLABLE) => Some(*t1),
                [t0, t1, t2]
                    if c.type_(*t0).flags.intersects(TypeFlags::NULLABLE)
                        && c.type_(*t1).flags.intersects(TypeFlags::NULLABLE) =>
                {
                    Some(*t2)
                }
                _ => None,
            };
            if let Some(candidate) = candidate {
                if !c.type_(candidate).flags.intersects(TypeFlags::NULLABLE) {
                    let target = get_normalized_type(c, candidate, true /*writing*/);
                    if source == target {
                        return Ternary::True;
                    }
                    return self
                        .is_related_to_ex_tail(c, original_source, original_target, source, target, recursion_flags, report_errors, head_message, intersection_state);
                }
            }
        }
        self.is_related_to_ex_tail(c, original_source, original_target, source, target, recursion_flags, report_errors, head_message, intersection_state)
    }

    /// Go: the common tail of `isRelatedToEx` after the nullable-union
    /// reduction (the Go code reassigns `target` and falls through; the
    /// port factors the fall-through into a helper so the borrow checker
    /// sees the shadowed `target` without a mutable reborrow of the
    /// parameter).
    #[allow(clippy::too_many_arguments)]
    fn is_related_to_ex_tail(
        &mut self,
        c: &mut Checker,
        original_source: TypeId,
        original_target: TypeId,
        source: TypeId,
        target: TypeId,
        recursion_flags: RecursionFlags,
        report_errors: bool,
        head_message: Option<&'static Message>,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if (self.relation == RelationKind::Comparable
            && !c.type_(target).flags.intersects(TypeFlags::NEVER)
            && c.is_simple_type_related_to(target, source, self.relation, None))
            || c.is_simple_type_related_to(source, target, self.relation, if report_errors { Some(ErrorReporterKind::Relater) } else { None })
        {
            return Ternary::True;
        }
        if c.type_(source).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
            || c.type_(target).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
        {
            let is_performing_excess_property_checks = !intersection_state.intersects(IntersectionState::TARGET)
                && is_object_literal_type(c, source)
                && c.type_(source).object_flags.intersects(ObjectFlags::FRESH_LITERAL);
            if is_performing_excess_property_checks && self.has_excess_properties(c, source, target, report_errors) {
                if report_errors {
                    let report_target = if c.type_(original_target).alias.is_some() { original_target } else { target };
                    self.report_relation_error(c, head_message, source, report_target);
                }
                return Ternary::False;
            }
            let source_has_members = !get_properties_of_type(c, source).is_empty()
                || type_has_call_or_construct_signatures(c, source);
            let is_performing_common_property_checks = (self.relation != RelationKind::Comparable || is_unit_type(c, source))
                && !intersection_state.intersects(IntersectionState::TARGET)
                && c.type_(source).flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && c.global_object_type != Some(source)
                && c.type_(target).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && c.is_weak_type(target)
                && source_has_members;
            let is_comparing_jsx_attributes_flags = c.type_(source).object_flags.intersects(ObjectFlags::JSX_ATTRIBUTES);
            if is_performing_common_property_checks
                && !c.has_common_properties(source, target, is_comparing_jsx_attributes_flags)
            {
                if report_errors {
                    let source_string = type_to_string(c, if c.type_(original_source).alias.is_some() { original_source } else { source });
                    let target_string = type_to_string(c, if c.type_(original_target).alias.is_some() { original_target } else { target });
                    let calls = get_signatures_of_type(c, source, SignatureKind::Call);
                    let constructs = get_signatures_of_type(c, source, SignatureKind::Construct);
                    let calls_related = !calls.is_empty() && {
                        let call_return = get_return_type_of_signature(c, calls[0]);
                        ternary_not_false(self.is_related_to(c, call_return, target, RecursionFlags::SOURCE, false))
                    };
                    let constructs_related = !constructs.is_empty() && {
                        let construct_return = get_return_type_of_signature(c, constructs[0]);
                        ternary_not_false(self.is_related_to(c, construct_return, target, RecursionFlags::SOURCE, false))
                    };
                    if calls_related || constructs_related {
                        self.report_error(c, &msg::VALUE_OF_TYPE_0_HAS_NO_PROPERTIES_IN_COMMON_WITH_TYPE_1_DID_YOU_MEAN_TO_CALL_IT, &[DiagArg::Str(source_string), DiagArg::Str(target_string)]);
                    } else {
                        self.report_error(c, &msg::TYPE_0_HAS_NO_PROPERTIES_IN_COMMON_WITH_TYPE_1, &[DiagArg::Str(source_string), DiagArg::Str(target_string)]);
                    }
                }
                return Ternary::False;
            }
            self.trace_unions_or_intersections_too_large(c, source, target);
            let skip_caching = c.type_(source).flags.intersects(TypeFlags::UNION)
                && c.type_(source).types().len() < 4
                && !c.type_(target).flags.intersects(TypeFlags::UNION)
                || c.type_(target).flags.intersects(TypeFlags::UNION)
                    && c.type_(target).types().len() < 4
                    && !c.type_(source).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE);
            let result = if skip_caching {
                self.union_or_intersection_related_to(c, source, target, report_errors, intersection_state)
            } else {
                self.recursive_type_related_to(c, source, target, report_errors, intersection_state, recursion_flags)
            };
            if result != Ternary::False {
                return result;
            }
        }
        if report_errors {
            self.report_error_results(c, original_source, original_target, source, target, head_message);
        }
        Ternary::False
    }

    /// Go: `func (r *Relater) hasExcessProperties(source, target, reportErrors) bool`.
    fn has_excess_properties(&mut self, c: &mut Checker, source: TypeId, target: TypeId, report_errors: bool) -> bool {
        if !is_excess_property_check_target(c, target)
            || !c.no_implicit_any && c.type_(target).object_flags.intersects(ObjectFlags::JS_LITERAL)
        {
            // Disable excess property checks on JS literals to simulate having
            // an implicit "index signature" - but only outside of noImplicitAny
            // (Go comment, verbatim).
            return false;
        }
        let is_comparing_jsx_attributes_flags = c.type_(source).object_flags.intersects(ObjectFlags::JSX_ATTRIBUTES);
        let global_object_type = c.global_object_type;
        let target_is_object_like = global_object_type
            .map(|g| c.is_type_subset_of(g, target))
            .unwrap_or(false);
        if (self.relation == RelationKind::Assignable || self.relation == RelationKind::Comparable)
            && (target_is_object_like
                || (!is_comparing_jsx_attributes_flags && is_empty_object_type(c, target)))
        {
            return false;
        }
        let mut reduced_target = target;
        let mut check_types: Option<Vec<TypeId>> = None;
        if c.type_(target).flags.intersects(TypeFlags::UNION) {
            reduced_target = find_matching_discriminant_type(c, source, target, TypeComparerKind::RelatedToSimple, Some(self))
                .unwrap_or_else(|| filter_primitives_if_contains_non_primitive(c, target));
            check_types = Some(distributed(c, reduced_target));
        }
        for prop in get_properties_of_type(c, source) {
            if should_check_as_excess_property(c, prop, c.type_(source).symbol) && !is_ignored_jsx_property(c, source, prop) {
                let name = c.program_symbol(prop).name.clone();
                if !c.is_known_property(reduced_target, &name, is_comparing_jsx_attributes_flags) {
                    if report_errors {
                        // TODO(port M6 W5 jsx): the JSX-attribute error arms
                        // (getSuggestedSymbolForNonexistentJSXAttribute,
                        // IsJsxAttributes/IsJsxOpeningLikeElement routing).
                        // The non-JSX arm below is ported; until the jsx wave,
                        // object-literal comparisons take it unconditionally.
                        // TODO(port M6 S6): getSuggestionForNonexistentProperty
                        // — the Did-you-mean elaboration.
                        let error_target = filter_type_is_excess_property_check_target(c, reduced_target);
                        let __ts0 = symbol_to_string(c, prop);
                        let __ts1 = type_to_string(c, error_target);
                        self.report_error(c, &msg::OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_AND_0_DOES_NOT_EXIST_IN_TYPE_1, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1)]);
                    }
                    return true;
                }
                if let Some(check_types) = &check_types {
                    let prop_type = get_type_of_symbol(c, prop);
                    let target_prop_type = get_type_of_property_in_types(c, check_types, &name);
                    if self.is_related_to(c, prop_type, target_prop_type, RecursionFlags::BOTH, report_errors) == Ternary::False {
                        if report_errors {
                            let __lift0 = symbol_to_string(c, prop);
                            self.report_error(c, &msg::TYPES_OF_PROPERTY_0_ARE_INCOMPATIBLE, &[DiagArg::Str(__lift0)]);
                        }
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Go: `func (c *Checker) getTypeOfPropertyInTypes(types, name) *Type`.
pub(crate) fn get_type_of_property_in_types(c: &mut Checker, types: &[TypeId], name: &str) -> TypeId {
    let mut prop_types: Vec<TypeId> = Vec::new();
    for &t in types {
        prop_types.push(get_type_of_property_in_type(c, t, name));
    }
    get_union_type(c, prop_types)
}

/// Go: `func (c *Checker) getTypeOfPropertyInType(t, name) *Type`.
pub(crate) fn get_type_of_property_in_type(c: &mut Checker, t: TypeId, name: &str) -> TypeId {
    let t = get_apparent_type(c, t);
    let prop = if c.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
        get_property_of_union_or_intersection_type(c, t, name, false)
    } else {
        get_property_of_object_type(c, t, name)
    };
    if let Some(prop) = prop {
        return get_type_of_symbol(c, prop);
    }
    if let Some(index_info) = get_applicable_index_info_for_name(c, t, name) {
        return c.index_info_value_type(index_info);
    }
    c.undefined_type()
}

/// Go: `func shouldCheckAsExcessProperty(prop, container) bool`.
fn should_check_as_excess_property(c: &Checker, prop: SymbolId, container: Option<SymbolId>) -> bool {
    let (prop_decl, container_decl) = match (symbol_value_declaration(c, prop), container.and_then(|s| symbol_value_declaration(c, s))) {
        (Some(p), Some(cont)) => (p, cont),
        _ => return false,
    };
    c.store().node(prop_decl).parent.get() == container_decl
}

/// Go: `func isIgnoredJsxProperty(source, sourceProp) bool`.
fn is_ignored_jsx_property(c: &Checker, source: TypeId, source_prop: SymbolId) -> bool {
    c.type_(source).object_flags.intersects(ObjectFlags::JSX_ATTRIBUTES)
        && is_hyphenated_jsx_name(&c.program_symbol(source_prop).name)
}

impl Relater {
    /// Go: `func (r *Relater) unionOrIntersectionRelatedTo(source, target, reportErrors, intersectionState) Ternary`.
    fn union_or_intersection_related_to(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        // Note that these checks are specifically ordered to produce correct
        // results (Go comment, verbatim).
        if c.type_(source).flags.intersects(TypeFlags::UNION) {
            if c.type_(target).flags.intersects(TypeFlags::UNION) {
                // The intersection/union-of-union origin fastpaths (Go
                // comment, verbatim).
                if let Some(origin) = c.type_(source).as_union_type().and_then(|u| u.origin) {
                    if c.type_(origin).flags.intersects(TypeFlags::INTERSECTION)
                        && c.type_(target).alias.is_some()
                        && c.type_(origin).types().contains(&target)
                    {
                        return Ternary::True;
                    }
                }
                if let Some(origin) = c.type_(target).as_union_type().and_then(|u| u.origin) {
                    if c.type_(origin).flags.intersects(TypeFlags::UNION)
                        && c.type_(source).alias.is_some()
                        && c.type_(origin).types().contains(&source)
                    {
                        return Ternary::True;
                    }
                }
            }
            let report = report_errors && !c.type_(source).flags.intersects(TypeFlags::PRIMITIVE);
            return if self.relation == RelationKind::Comparable {
                self.some_type_related_to_type(c, source, target, report, intersection_state)
            } else {
                self.each_type_related_to_type(c, source, target, report, intersection_state)
            };
        }
        if c.type_(target).flags.intersects(TypeFlags::UNION) {
            let report = report_errors
                && !c.type_(source).flags.intersects(TypeFlags::PRIMITIVE)
                && !c.type_(target).flags.intersects(TypeFlags::PRIMITIVE);
            let regular_source = get_regular_type_of_object_literal(c, source);
            return self.type_related_to_some_type(c, regular_source, target, report, intersection_state);
        }
        if c.type_(target).flags.intersects(TypeFlags::INTERSECTION) {
            return self.type_related_to_each_type(c, source, target, report_errors, IntersectionState::TARGET);
        }
        // Source is an intersection. For the comparable relation, if the target
        // is a primitive type we hoist the constraints of all non-primitive
        // types in the source into a new intersection (Go comment, verbatim).
        if self.relation == RelationKind::Comparable && c.type_(target).flags.intersects(TypeFlags::PRIMITIVE) {
            let source_types = c.type_(source).types().to_vec();
            let constraints: Vec<TypeId> = source_types
                .iter()
                .map(|&t| {
                    if c.type_(t).flags.intersects(TypeFlags::INSTANTIABLE) {
                        get_base_constraint_of_type(c, t).unwrap_or_else(|| c.unknown_type())
                    } else {
                        t
                    }
                })
                .collect();
            if constraints != source_types {
                let source = get_intersection_type(c, constraints);
                if c.type_(source).flags.intersects(TypeFlags::NEVER) {
                    return Ternary::False;
                }
                if !c.type_(source).flags.intersects(TypeFlags::INTERSECTION) {
                    let result = self.is_related_to(c, source, target, RecursionFlags::SOURCE, false);
                    if result != Ternary::False {
                        return result;
                    }
                    return self.is_related_to(c, target, source, RecursionFlags::SOURCE, false);
                }
            }
        }
        // Check to see if any constituents of the intersection are
        // immediately related to the target (Go comment, verbatim).
        self.some_type_related_to_type(c, source, target, false, IntersectionState::SOURCE)
    }

    /// Go: `func (r *Relater) someTypeRelatedToType(...) Ternary`.
    fn some_type_related_to_type(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let source_types = c.type_(source).types().to_vec();
        if c.type_(source).flags.intersects(TypeFlags::UNION) && source_types.contains(&target) {
            return Ternary::True;
        }
        for (i, &t) in source_types.iter().enumerate() {
            let report = report_errors && i == source_types.len() - 1;
            let related = self.is_related_to_ex(c, t, target, RecursionFlags::SOURCE, report, None, intersection_state);
            if related != Ternary::False {
                return related;
            }
        }
        Ternary::False
    }

    /// Go: `func (r *Relater) eachTypeRelatedToType(...) Ternary`.
    fn each_type_related_to_type(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut result = Ternary::True;
        let source_types = c.type_(source).types().to_vec();
        let stripped_target = self.get_undefined_stripped_target_if_needed(c, source, target);
        let stripped_types: Vec<TypeId> = if c.type_(stripped_target).flags.intersects(TypeFlags::UNION) {
            c.type_(stripped_target).types().to_vec()
        } else {
            Vec::new()
        };
        let stripped_is_union = c.type_(stripped_target).flags.intersects(TypeFlags::UNION);
        for (i, &source_type) in source_types.iter().enumerate() {
            if stripped_is_union
                && source_types.len() >= stripped_types.len()
                && source_types.len() % stripped_types.len() == 0
            {
                // many unions are mappings of one another (Go comment,
                // verbatim).
                let related = self.is_related_to_ex(
                    c,
                    source_type,
                    stripped_types[i % stripped_types.len()],
                    RecursionFlags::BOTH,
                    false,
                    None,
                    intersection_state,
                );
                if related != Ternary::False {
                    result = ternary_and(result, related);
                    continue;
                }
            }
            let related = self.is_related_to_ex(c, source_type, target, RecursionFlags::SOURCE, report_errors, None, intersection_state);
            if related == Ternary::False {
                return Ternary::False;
            }
            result = ternary_and(result, related);
        }
        result
    }

    /// Go: `func (r *Relater) getUndefinedStrippedTargetIfNeeded(source, target) *Type`.
    fn get_undefined_stripped_target_if_needed(&mut self, c: &mut Checker, source: TypeId, target: TypeId) -> TypeId {
        if c.type_(source).flags.intersects(TypeFlags::UNION)
            && c.type_(target).flags.intersects(TypeFlags::UNION)
            && !c.type_(c.type_(source).types()[0]).flags.intersects(TypeFlags::UNDEFINED)
            && c.type_(c.type_(target).types()[0]).flags.intersects(TypeFlags::UNDEFINED)
        {
            return extract_types_of_kind_not(c, target, TypeFlags::UNDEFINED);
        }
        target
    }

    /// Go: `func (r *Relater) typeRelatedToSomeType(...) Ternary`.
    fn type_related_to_some_type(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let target_types = c.type_(target).types().to_vec();
        if c.type_(target).flags.intersects(TypeFlags::UNION) {
            if target_types.contains(&source) {
                return Ternary::True;
            }
            if self.relation != RelationKind::Comparable
                && c.type_(target).object_flags.intersects(ObjectFlags::PRIMITIVE_UNION)
                && !c.type_(source).flags.intersects(TypeFlags::ENUM_LITERAL)
                && (c.type_(source).flags.intersects(TypeFlags::STRING_LITERAL | TypeFlags::BOOLEAN_LITERAL | TypeFlags::BIGINT_LITERAL)
                    || (self.relation == RelationKind::Subtype || self.relation == RelationKind::StrictSubtype)
                        && c.type_(source).flags.intersects(TypeFlags::NUMBER_LITERAL))
            {
                // When relating a literal type to a union of primitive types
                // (Go comment, verbatim).
                let source_lit = c.type_(source).as_literal_type().expect("LiteralType data");
                let alternate_form = if source == source_lit.regular_type.unwrap_or(source) {
                    source_lit.fresh_type
                } else {
                    source_lit.regular_type
                };
                let primitive = if c.type_(source).flags.intersects(TypeFlags::STRING_LITERAL) {
                    Some(c.string_type())
                } else if c.type_(source).flags.intersects(TypeFlags::NUMBER_LITERAL) {
                    Some(c.number_type())
                } else if c.type_(source).flags.intersects(TypeFlags::BIGINT_LITERAL) {
                    c.bigint_type
                } else {
                    None
                };
                if primitive.is_some_and(|p| target_types.contains(&p))
                    || alternate_form.is_some_and(|a| target_types.contains(&a))
                {
                    return Ternary::True;
                }
                return Ternary::False;
            }
            if let Some(match_) = get_matching_union_constituent_for_type(c, target, source) {
                let related = self.is_related_to_ex(c, source, match_, RecursionFlags::TARGET, false, None, intersection_state);
                if related != Ternary::False {
                    return related;
                }
            }
        }
        for &t in &target_types {
            let related = self.is_related_to_ex(c, source, t, RecursionFlags::TARGET, false, None, intersection_state);
            if related != Ternary::False {
                return related;
            }
        }
        if report_errors {
            // Elaborate only if we can find a best matching type in the
            // target union (Go comment, verbatim).
            if let Some(best_matching_type) = get_best_matching_type(c, source, target, TypeComparerKind::RelatedToSimple, Some(self)) {
                self.is_related_to_ex(c, source, best_matching_type, RecursionFlags::TARGET, true, None, intersection_state);
            }
        }
        Ternary::False
    }

    /// Go: `func (r *Relater) typeRelatedToEachType(...) Ternary`.
    fn type_related_to_each_type(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut result = Ternary::True;
        for target_type in c.type_(target).types().to_vec() {
            let related = self.is_related_to_ex(c, source, target_type, RecursionFlags::TARGET, report_errors, None, intersection_state);
            if related == Ternary::False {
                return Ternary::False;
            }
            result = ternary_and(result, related);
        }
        result
    }

    /// Go: `func (r *Relater) eachTypeRelatedToSomeType(source, target) Ternary`.
    fn each_type_related_to_some_type(&mut self, c: &mut Checker, source: TypeId, target: TypeId) -> Ternary {
        let mut result = Ternary::True;
        for source_type in c.type_(source).types().to_vec() {
            let related = self.type_related_to_some_type(c, source_type, target, false, IntersectionState::NONE);
            if related == Ternary::False {
                return Ternary::False;
            }
            result = ternary_and(result, related);
        }
        result
    }

    /// Go: `func (r *Relater) recursiveTypeRelatedTo(...) Ternary` — the
    /// maybe-key/recursion ladder (see Go's doc comment, verbatim in the
    /// source).
    fn recursive_type_related_to(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
        recursion_flags: RecursionFlags,
    ) -> Ternary {
        if self.overflow {
            // Note that stack depth overflows can cause _any_ relation
            // involving structured types to become false (Go comment,
            // verbatim).
            return Ternary::False;
        }
        let (id, constrained) = get_relation_key(c, source, target, intersection_state, self.relation == RelationKind::Identity, false /*ignoreConstraints*/);
        let entry = c.relation_ref(self.relation).get(id);
        if entry != RelationComparisonResult::NONE {
            let redo = report_errors
                && entry.intersects(RelationComparisonResult::FAILED)
                && !entry.intersects(RelationComparisonResult::COMPLEXITY_OVERFLOW);
            if !redo {
                c.reliability_flags |= entry & (RelationComparisonResult::REPORTS_UNMEASURABLE | RelationComparisonResult::REPORTS_UNRELIABLE);
                if report_errors && entry.intersects(RelationComparisonResult::COMPLEXITY_OVERFLOW) {
                    let __ts0 = type_to_string(c, source);
                    let __ts1 = type_to_string(c, target);
                    self.report_error(c, &msg::EXCESSIVE_COMPLEXITY_COMPARING_TYPES_0_AND_1, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1)]);
                }
                if entry.intersects(RelationComparisonResult::SUCCEEDED) {
                    return Ternary::True;
                }
                return Ternary::False;
            }
        }
        if self.relation_count <= 0 {
            self.overflow = true;
            return Ternary::False;
        }
        // If source and target are already being compared, consider them
        // related with assumptions (Go comment, verbatim).
        if self.maybe_keys_set.contains(&id) {
            return Ternary::Maybe;
        }
        if constrained {
            let (broadest_equivalent_id, _) = get_relation_key(c, source, target, intersection_state, self.relation == RelationKind::Identity, true /*ignoreConstraints*/);
            if self.maybe_keys_set.contains(&broadest_equivalent_id) {
                return Ternary::Maybe;
            }
        }
        if self.source_stack.len() == 100 || self.target_stack.len() == 100 {
            // We stop relating if we reach 100 levels of nesting (Go
            // comment, verbatim).
            return Ternary::Maybe;
        }
        let maybe_start = self.maybe_keys.len();
        self.maybe_keys.push(id);
        self.maybe_keys_set.insert(id);
        let save_expanding_flags = self.expanding_flags;
        if recursion_flags.intersects(RecursionFlags::SOURCE) {
            self.source_stack.push(source);
            if !self.expanding_flags.intersects(ExpandingFlags::SOURCE) && c.is_deeply_nested_type(source, &self.source_stack, 3) {
                self.expanding_flags |= ExpandingFlags::SOURCE;
            }
        }
        if recursion_flags.intersects(RecursionFlags::TARGET) {
            self.target_stack.push(target);
            if !self.expanding_flags.intersects(ExpandingFlags::TARGET) && c.is_deeply_nested_type(target, &self.target_stack, 3) {
                self.expanding_flags |= ExpandingFlags::TARGET;
            }
        }
        let save_reliability_flags = c.reliability_flags;
        c.reliability_flags = RelationComparisonResult::NONE;
        
        let result = if self.expanding_flags == ExpandingFlags::BOTH {
            if let Some(tr) = &c.tracer {
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "recursiveTypeRelatedTo_DepthLimit",
                    &{
                        let args = crate::tracing::Args::new();
                        args.insert("sourceId", crate::tracing::TraceValue::Uint(source.0 as u64));
                        args.insert("targetId", crate::tracing::TraceValue::Uint(target.0 as u64));
                        args.insert("depth", crate::tracing::TraceValue::Int(self.source_stack.len() as i64));
                        args.insert("targetDepth", crate::tracing::TraceValue::Int(self.target_stack.len() as i64));
                        args
                    },
                );
            }
            Ternary::Maybe
        } else {
            let _pop = c.tracer.as_ref().map(|tr| {
                let args = crate::tracing::Args::new();
                args.insert("sourceId", crate::tracing::TraceValue::Uint(source.0 as u64));
                args.insert("targetId", crate::tracing::TraceValue::Uint(target.0 as u64));
                tr.push(crate::tracing::Phase::CheckTypes, "structuredTypeRelatedTo", &args, false /*separateBeginAndEnd*/)
            });
            self.structured_type_related_to(c, source, target, report_errors, intersection_state)
            // Go: defer pop — the TracerPop drop above handles it.
        };
        let propagating_variance_flags = c.reliability_flags;
        c.reliability_flags |= save_reliability_flags;
        if recursion_flags.intersects(RecursionFlags::SOURCE) {
            self.source_stack.pop();
        }
        if recursion_flags.intersects(RecursionFlags::TARGET) {
            self.target_stack.pop();
        }
        self.expanding_flags = save_expanding_flags;
        if result != Ternary::False {
            if result == Ternary::True || (self.source_stack.is_empty() && self.target_stack.is_empty()) {
                if matches!(result, Ternary::True | Ternary::Maybe) {
                    // If result is definitely true, record all maybe keys as
                    // having succeeded (Go comment, verbatim).
                    self.reset_maybe_stack(c, maybe_start, propagating_variance_flags, true);
                } else {
                    self.reset_maybe_stack(c, maybe_start, propagating_variance_flags, false);
                }
            }
            // Note: it's intentional that we don't reset in the else case
            // (Go comment, verbatim).
        } else {
            // A false result goes straight into global cache (Go comment,
            // verbatim).
            c.relation_ref_mut(self.relation).set(id, RelationComparisonResult::FAILED | propagating_variance_flags);
            self.relation_count -= 1;
            self.reset_maybe_stack(c, maybe_start, propagating_variance_flags, false);
        }
        result
    }

    /// Go: `func (r *Relater) resetMaybeStack(...)`.
    fn reset_maybe_stack(&mut self, c: &mut Checker, maybe_start: usize, propagating_variance_flags: RelationComparisonResult, mark_all_as_succeeded: bool) {
        for i in maybe_start..self.maybe_keys.len() {
            self.maybe_keys_set.remove(&self.maybe_keys[i]);
            if mark_all_as_succeeded {
                c.relation_ref_mut(self.relation).set(self.maybe_keys[i], RelationComparisonResult::SUCCEEDED | propagating_variance_flags);
                self.relation_count -= 1;
            }
        }
        self.maybe_keys.truncate(maybe_start);
    }

    /// Go: `func (r *Relater) getErrorState() errorState`.
    fn get_error_state(&self) -> ErrorState {
        ErrorState {
            error_chain: self.error_chain.clone(),
            related_info: self.related_info.clone(),
        }
    }

    /// Go: `func (r *Relater) restoreErrorState(e errorState)`.
    fn restore_error_state(&mut self, e: ErrorState) {
        self.error_chain = e.error_chain;
        self.related_info = e.related_info;
    }

    /// Go: `func (r *Relater) structuredTypeRelatedTo(...) Ternary` — the
    /// wrapper with the intersection-constraint retry and extra property
    /// checks (see Go's doc comments, verbatim there).
    fn structured_type_related_to(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let save_error_state = self.get_error_state();
        let mut result = self.structured_type_related_to_worker(c, source, target, report_errors, intersection_state);
        if self.relation != RelationKind::Identity {
            if result == Ternary::False
                && (c.type_(source).flags.intersects(TypeFlags::INTERSECTION)
                    || c.type_(source).flags.intersects(TypeFlags::TYPE_PARAMETER)
                        && c.type_(target).flags.intersects(TypeFlags::UNION))
            {
                let source_types: Vec<TypeId> = if c.type_(source).flags.intersects(TypeFlags::INTERSECTION) {
                    c.type_(source).types().to_vec()
                } else {
                    vec![source]
                };
                let constraint = c.get_effective_constraint_of_intersection(&source_types, c.type_(target).flags.intersects(TypeFlags::UNION));
                if let Some(constraint) = constraint {
                    if every_type(c, constraint, |t| t != source) {
                        // TODO: Stack errors so we get a pyramid for the
                        // "normal" comparison above (Go comment, verbatim).
                        result = self.is_related_to_ex(c, constraint, target, RecursionFlags::SOURCE, false, None, intersection_state);
                    }
                }
            }
            if result != Ternary::False
                && !intersection_state.intersects(IntersectionState::TARGET)
                && c.type_(target).flags.intersects(TypeFlags::INTERSECTION)
                && !is_generic_object_type(c, target)
                && c.type_(source).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
            {
                // When the target is an intersection we need an extra
                // property check (Go comment, verbatim).
                result = ternary_and(result, self.properties_related_to(c, source, target, report_errors, &Default::default(), false, IntersectionState::NONE));
                if result != Ternary::False
                    && is_object_literal_type(c, source)
                    && c.type_(source).object_flags.intersects(ObjectFlags::FRESH_LITERAL)
                {
                    result = ternary_and(result, self.index_signatures_related_to(c, source, target, false, report_errors, IntersectionState::NONE));
                }
            }
            // When the source is an intersection we need an extra check of
            // any optional properties in the target (Go comment, verbatim).
            if result != Ternary::False
                && is_non_generic_object_type(c, target)
                && !is_array_or_tuple_type(c, target)
                && self.is_source_intersection_needing_extra_check(c, source, target)
            {
                result = ternary_and(result, self.properties_related_to(c, source, target, report_errors, &Default::default(), true, intersection_state));
            }
        }
        if result != Ternary::False {
            self.restore_error_state(save_error_state);
        }
        result
    }

    /// Go: `func (r *Relater) isSourceIntersectionNeedingExtraCheck(source, target) bool`.
    fn is_source_intersection_needing_extra_check(&mut self, c: &mut Checker, source: TypeId, target: TypeId) -> bool {
        let apparent_source = get_apparent_type(c, source);
        c.type_(source).flags.intersects(TypeFlags::INTERSECTION)
            && c.type_(apparent_source).flags.intersects(TypeFlags::STRUCTURED_TYPE)
            && !c.type_(source).types().iter().any(|&t| {
                t == target || c.type_(t).object_flags.intersects(ObjectFlags::NON_INFERRABLE_TYPE)
            })
    }

    /// Go: `func (r *Relater) traceUnionsOrIntersectionsTooLarge(source, target)`.
    fn trace_unions_or_intersections_too_large(&mut self, c: &Checker, source: TypeId, target: TypeId) {
        let Some(tr) = &c.tracer else { return };
        if c.type_(source).flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
            && c.type_(target).flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            if ObjectFlags::from_bits(c.type_(source).object_flags.0 & c.type_(target).object_flags.0)
                .intersects(ObjectFlags::PRIMITIVE_UNION)
            {
                // There's a fast path for comparing primitive unions.
                return;
            }
            let source_size = c.type_(source).types().len();
            let target_size = c.type_(target).types().len();
            if source_size.saturating_mul(target_size) > 1_000_000 {
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "traceUnionsOrIntersectionsTooLarge_DepthLimit",
                    &{
                        let args = crate::tracing::Args::new();
                        args.insert("sourceId", crate::tracing::TraceValue::Uint(source.0 as u64));
                        args.insert("sourceSize", crate::tracing::TraceValue::Int(source_size as i64));
                        args.insert("targetId", crate::tracing::TraceValue::Uint(target.0 as u64));
                        args.insert("targetSize", crate::tracing::TraceValue::Int(target_size as i64));
                        args
                    },
                );
            }
        }
    }

    /// Go: the `relateVariances` closure in `structuredTypeRelatedToWorker` —
    /// the captured locals (`result`, `varianceCheckFailed`,
    /// `originalErrorChain`) become out-parameters.
    #[allow(clippy::too_many_arguments)]
    fn relate_variances(
        &mut self,
        c: &mut Checker,
        source_type_arguments: &[TypeId],
        target_type_arguments: &[TypeId],
        variances: &[VarianceFlags],
        report_errors: bool,
        intersection_state: IntersectionState,
        original_error_chain: &mut Option<Box<ErrorChain>>,
        variance_check_failed: &mut bool,
    ) -> (Ternary, bool) {
        let result = self.type_arguments_related_to(c, source_type_arguments, target_type_arguments, variances, report_errors, intersection_state);
        if result != Ternary::False {
            return (result, true);
        }
        if variances.iter().any(|v| v.intersects(VarianceFlags::ALLOWS_STRUCTURAL_FALLBACK)) {
            // If some type parameter was `Unmeasurable` or `Unreliable` (Go
            // comment, verbatim).
            *original_error_chain = None;
            return (Ternary::False, false);
        }
        let allow_structural_fallback = has_covariant_void_argument(c, target_type_arguments, variances);
        *variance_check_failed = !allow_structural_fallback;
        // The type arguments did not relate appropriately (Go comment,
        // verbatim).
        if !variances.is_empty() && !allow_structural_fallback {
            if *variance_check_failed
                && !(report_errors
                    && variances.iter().any(|v| (v.0 & VarianceFlags::VARIANCE_MASK.0) == VarianceFlags::INVARIANT.0))
            {
                return (Ternary::False, true);
            }
            // We remember the original error information (Go comment,
            // verbatim).
            *original_error_chain = self.error_chain.take();
        }
        (Ternary::False, false)
    }

    /// Go: `func (r *Relater) structuredTypeRelatedToWorker(source, target, reportErrors, intersectionState) Ternary`
    /// — the full flag ladder. Structure mirrors Go's two consecutive
    /// switch statements exactly (early `return`s preserved).
    #[allow(clippy::too_many_lines)]
    fn structured_type_related_to_worker(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut source = source;
        let mut result;
        let mut variance_check_failed = false;
        let mut original_error_chain: Option<Box<ErrorChain>> = None;
        let save_error_state = self.get_error_state();

        // Go: the identity-relation fast ladder.
        if self.relation == RelationKind::Identity {
            let s_flags = c.type_(source).flags;
            let _t_flags = c.type_(target).flags;
            // We've already checked that source.flags and target.flags are
            // identical (Go comment, verbatim).
            if s_flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let mut result = self.each_type_related_to_some_type(c, source, target);
                if result != Ternary::False {
                    result = ternary_and(result, self.each_type_related_to_some_type(c, target, source));
                }
                return result;
            }
            if s_flags.intersects(TypeFlags::INDEX) {
                return self.is_related_to(
                    c,
                    index_target(c, source),
                    index_target(c, target),
                    RecursionFlags::BOTH,
                    false,
                );
            }
            if s_flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let (s_obj, s_idx) = indexed_access_parts(c, source);
                let (t_obj, t_idx) = indexed_access_parts(c, target);
                result = self.is_related_to(c, s_obj, t_obj, RecursionFlags::BOTH, false);
                if result != Ternary::False {
                    result = ternary_and(result, self.is_related_to(c, s_idx, t_idx, RecursionFlags::BOTH, false));
                    if result != Ternary::False {
                        return result;
                    }
                }
            } else if s_flags.intersects(TypeFlags::CONDITIONAL) {
                if conditional_root_is_distributive(c, source) == conditional_root_is_distributive(c, target) {
                    let (s_check, s_extends) = conditional_parts(c, source);
                    let (t_check, t_extends) = conditional_parts(c, target);
                    result = self.is_related_to(c, s_check, t_check, RecursionFlags::BOTH, false);
                    if result != Ternary::False {
                        result = ternary_and(result, self.is_related_to(c, s_extends, t_extends, RecursionFlags::BOTH, false));
                        if result != Ternary::False {
                            let __lift2 = get_true_type_from_conditional_type(c, source);
                            let __lift1 = get_true_type_from_conditional_type(c, target);
                            result = ternary_and(result, self.is_related_to(c, __lift2, __lift1, RecursionFlags::BOTH, false));
                            if result != Ternary::False {
                                let __lift4 = get_false_type_from_conditional_type(c, source);
                                let __lift3 = get_false_type_from_conditional_type(c, target);
                                result = ternary_and(result, self.is_related_to(c, __lift4, __lift3, RecursionFlags::BOTH, false));
                                if result != Ternary::False {
                                    return result;
                                }
                            }
                        }
                    }
                }
            } else if s_flags.intersects(TypeFlags::SUBSTITUTION) {
                let (s_base, s_constraint) = substitution_parts(c, source);
                let (t_base, t_constraint) = substitution_parts(c, target);
                result = self.is_related_to(c, s_base, t_base, RecursionFlags::BOTH, false);
                if result != Ternary::False {
                    result = ternary_and(result, self.is_related_to(c, s_constraint, t_constraint, RecursionFlags::BOTH, false));
                    if result != Ternary::False {
                        return result;
                    }
                }
            } else if s_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let (s_texts, s_types) = template_literal_parts(c, source);
                let (t_texts, t_types) = template_literal_parts(c, target);
                if s_texts == t_texts {
                    let mut result = Ternary::True;
                    for (i, &source_type) in s_types.iter().enumerate() {
                        let target_type = t_types[i];
                        result = ternary_and(result, self.is_related_to(c, source_type, target_type, RecursionFlags::BOTH, false));
                        if result == Ternary::False {
                            return result;
                        }
                    }
                    return result;
                }
            } else if s_flags.intersects(TypeFlags::STRING_MAPPING)
                && string_mapping_symbol(c, source) == string_mapping_symbol(c, target) {
                    let __lift6 = string_mapping_target(c, source);
                    let __lift5 = string_mapping_target(c, target);
                    return self.is_related_to(c, __lift6, __lift5, RecursionFlags::BOTH, false);
                }
            if !s_flags.intersects(TypeFlags::OBJECT) {
                return Ternary::False;
            }
        } else if c.type_(source).flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
            || c.type_(target).flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            result = self.union_or_intersection_related_to(c, source, target, report_errors, intersection_state);
            if result != Ternary::False {
                return result;
            }
            // The ordered decomposition above doesn't handle all cases (Go
            // comment, verbatim).
            if !(c.type_(source).flags.intersects(TypeFlags::INSTANTIABLE)
                || c.type_(source).flags.intersects(TypeFlags::OBJECT) && c.type_(target).flags.intersects(TypeFlags::UNION)
                || c.type_(source).flags.intersects(TypeFlags::INTERSECTION)
                    && c.type_(target).flags.intersects(TypeFlags::OBJECT | TypeFlags::UNION | TypeFlags::INSTANTIABLE))
            {
                return Ternary::False;
            }
        }

        // We limit alias variance probing to only object and conditional types
        // (Go comment, verbatim).
        let source_alias = c.type_(source).alias.clone();
        let target_alias = c.type_(target).alias.clone();
        if c.type_(source).flags.intersects(TypeFlags::OBJECT | TypeFlags::CONDITIONAL)
            && source_alias.as_ref().is_some_and(|a| !a.type_arguments.is_empty())
            && target_alias.is_some()
            && source_alias.as_ref().map(|a| a.symbol) == target_alias.as_ref().map(|a| a.symbol)
            && !(c.is_marker_type(source) || c.is_marker_type(target))
        {
            let alias_symbol = source_alias
                .as_ref()
                .and_then(|a| a.symbol)
                .expect("alias symbol");
            let variances = c.get_alias_variances(alias_symbol);
            if variances.is_empty() {
                return Ternary::Unknown;
            }
            let params = get_type_alias_type_parameters(c, alias_symbol);
            let min_params = get_min_type_argument_count(c, &params);
            let node_is_in_js_file = alias_symbol_value_declaration_in_js_file(c, alias_symbol);
            let Some(source_alias) = source_alias else {
                unreachable!("guarded: source alias present (checked above)")
            };
            let Some(target_alias) = target_alias else {
                unreachable!("guarded: target alias present (checked above)")
            };
            let source_types = fill_missing_type_arguments(c, &source_alias.type_arguments, &params, min_params, node_is_in_js_file);
            let target_types = fill_missing_type_arguments(c, &target_alias.type_arguments, &params, min_params, node_is_in_js_file);
            let (variance_result, ok) = self.relate_variances(c, &source_types, &target_types, &variances, report_errors, intersection_state, &mut original_error_chain, &mut variance_check_failed);
            if ok {
                return variance_result;
            }
        }
        // For a generic type T and a type U that is assignable to T (Go
        // comment, verbatim).
        if is_single_element_generic_tuple_type(c, source) && !tuple_type_readonly(c, source) {
            let __lift7 = get_type_arguments(c, source);
            result = self.is_related_to(c, __lift7[0], target, RecursionFlags::SOURCE, false);
            if result != Ternary::False {
                return result;
            }
        }
        let source_base_constraint = get_base_constraint_or_type(c, source);
        if is_single_element_generic_tuple_type(c, target)
            && (tuple_type_readonly(c, target) || is_mutable_array_or_tuple(c, source_base_constraint))
        {
            let __lift8 = get_type_arguments(c, target);
            result = self.is_related_to(c, source, __lift8[0], RecursionFlags::TARGET, false);
            if result != Ternary::False {
                return result;
            }
        }

        // The target-flags ladder.
        let s_flags = c.type_(source).flags;
        let t_flags = c.type_(target).flags;
        if t_flags.intersects(TypeFlags::TYPE_PARAMETER) {
            // A source type { [P in Q]: X } is related to a target type T if
            // keyof T is related to Q and X is related to T[Q] (Go comment,
            // verbatim).
            let target_index_type = get_index_type(c, target);
            let source_mapped_constraint =
                get_constraint_type_from_mapped_type(c, source).unwrap_or_else(|| c.never_type());
            if c.type_(source).object_flags.intersects(ObjectFlags::MAPPED)
                && mapped_type_name_type_is_none(c, source)
                && ternary_not_false(self.is_related_to(c, target_index_type, source_mapped_constraint, RecursionFlags::BOTH, false))
                && !get_mapped_type_modifiers(c, source).intersects(MappedTypeModifiers::INCLUDE_OPTIONAL) {
                    let template_type = get_template_type_from_mapped_type(c, source);
                    let __lift9 = get_type_parameter_from_mapped_type(c, source);
                    let indexed_access_type = get_indexed_access_type(c, target, __lift9);
                    result = self.is_related_to(c, template_type, indexed_access_type, RecursionFlags::BOTH, report_errors);
                    if result != Ternary::False {
                        return result;
                    }
                }
            if self.relation == RelationKind::Comparable && s_flags.intersects(TypeFlags::TYPE_PARAMETER) {
                // This is a carve-out in comparability (Go comment, verbatim).
                if let Some(constraint) = get_constraint_of_type_parameter(c, source) {
                    let constraint_types: Vec<TypeId> = if c.type_(constraint).flags.intersects(TypeFlags::UNION) {
                        c.type_(constraint).types().to_vec()
                    } else {
                        vec![constraint]
                    };
                    if constraint_types.iter().any(|&t| c.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER)) {
                        return self.is_related_to(c, constraint, target, RecursionFlags::SOURCE, false);
                    }
                }
                return Ternary::False;
            }
        } else if t_flags.intersects(TypeFlags::INDEXED_ACCESS) {
            if s_flags.intersects(TypeFlags::INDEXED_ACCESS) {
                // Relate components directly before falling back to
                // constraint relationships (Go comment, verbatim).
                let (s_obj, s_idx) = indexed_access_parts(c, source);
                let (t_obj, t_idx) = indexed_access_parts(c, target);
                result = self.is_related_to(c, s_obj, t_obj, RecursionFlags::BOTH, report_errors);
                if result != Ternary::False {
                    result = ternary_and(result, self.is_related_to(c, s_idx, t_idx, RecursionFlags::BOTH, report_errors));
                }
                if result != Ternary::False {
                    return result;
                }
                if report_errors {
                    original_error_chain = self.error_chain.take();
                }
            }
            // A type S is related to a type T[K] if S is related to C (Go
            // comment, verbatim).
            if self.relation == RelationKind::Assignable || self.relation == RelationKind::Comparable {
                let (object_type, index_type) = indexed_access_parts(c, target);
                let base_object_type = get_base_constraint_or_type(c, object_type);
                let base_index_type = get_base_constraint_or_type(c, index_type);
                if !is_generic_object_type(c, base_object_type) && !is_generic_index_type(c, base_index_type) {
                    let constraint = get_indexed_access_type_or_undefined_writing(c, base_object_type, base_index_type, base_object_type != object_type);
                    if let Some(constraint) = constraint {
                        if report_errors && original_error_chain.is_some() {
                            // create a new chain for the constraint error
                            self.error_chain = save_error_state.error_chain.clone();
                        }
                        result = self.is_related_to_ex(c, source, constraint, RecursionFlags::TARGET, report_errors, None, intersection_state);
                        if result != Ternary::False {
                            return result;
                        }
                        // prefer the shorter chain (Go comment, verbatim).
                        if report_errors && original_error_chain.is_some() && self.error_chain.is_some()
                            && chain_depth(original_error_chain.as_deref()) <= chain_depth(self.error_chain.as_deref()) {
                                self.error_chain = original_error_chain.take();
                            }
                    }
                }
            }
            if report_errors {
                original_error_chain = None;
            }
        } else if t_flags.intersects(TypeFlags::INDEX) {
            let target_type = index_target(c, target);
            // A keyof S is related to a keyof T if T is related to S.
            if s_flags.intersects(TypeFlags::INDEX) {
                let __lift10 = index_target(c, source);
                result = self.is_related_to(c, target_type, __lift10, RecursionFlags::BOTH, false);
                if result != Ternary::False {
                    return result;
                }
            }
            if is_tuple_type(c, target_type) {
                // An index type can have a tuple type target (Go comment,
                // verbatim).
                let __lift11 = get_known_keys_of_tuple_type(c, target_type);
                result = self.is_related_to(c, source, __lift11, RecursionFlags::TARGET, report_errors);
                if result != Ternary::False {
                    return result;
                }
            } else {
                // A type S is assignable to keyof T if S is assignable to
                // keyof C (Go comment, verbatim).
                if let Some(constraint) = get_simplified_type_or_constraint(c, target_type) {
                    let index_flags = index_type_index_flags(c, target).union(IndexFlags::NO_REDUCIBLE_CHECK);
                    let __lift12 = get_index_type_ex(c, constraint, index_flags);
                    if self.is_related_to(c, source, __lift12, RecursionFlags::TARGET, report_errors) == Ternary::True {
                        return Ternary::True;
                    }
                } else if is_generic_mapped_type(c, target_type) {
                    // generic mapped types that don't simplify (Go comment,
                    // verbatim).
                    let name_type = get_name_type_from_mapped_type(c, target_type);
                    let constraint_type = get_constraint_type_from_mapped_type(c, target_type);
                    let target_keys = match name_type {
                        Some(name_type) if is_mapped_type_with_keyof_constraint_declaration(c, target_type) => {
                            // TODO(port M6 S7): getApparentMappedTypeKeys +
                            // the union construction — the mapped-key union arm.
                            Some(get_union_type(c, vec![name_type, name_type]))
                        }
                        _ => name_type.or(constraint_type),
                    };
                    if let Some(target_keys) = target_keys {
                        if self.is_related_to(c, source, target_keys, RecursionFlags::TARGET, report_errors) == Ternary::True {
                            return Ternary::True;
                        }
                    }
                }
            }
        } else if t_flags.intersects(TypeFlags::CONDITIONAL) {
            // If we reach 10 levels of nesting for the same conditional type
            // (Go comment, verbatim).
            if c.is_deeply_nested_type(target, &self.target_stack, 10) {
                return Ternary::Maybe;
            }
            let cond = conditional_parts_ex(c, target);
            // We check for a relationship to a conditional type target only
            // when (Go comment, verbatim).
            if conditional_infer_type_parameters(c, target).is_empty()
                && !conditional_is_distribution_dependent(c, target)
                && !(s_flags.intersects(TypeFlags::CONDITIONAL) && conditional_roots_identical(c, source, target))
            {
                // Check if the conditional is always true or always false
                // (Go comment, verbatim).
                let cond0_permissive = get_permissive_instantiation(c, cond.0);
                let cond1_permissive = get_permissive_instantiation(c, cond.1);
                let skip_true = !c.is_type_assignable_to(cond0_permissive, cond1_permissive);
                let cond0_restrictive = get_restrictive_instantiation(c, cond.0);
                let cond1_restrictive = get_restrictive_instantiation(c, cond.1);
                let skip_false = !skip_true && c.is_type_assignable_to(cond0_restrictive, cond1_restrictive);
                if skip_true {
                    result = Ternary::True;
                } else {
                    let __lift13 = get_true_type_from_conditional_type(c, target);
                    result = self.is_related_to_ex(c, source, __lift13, RecursionFlags::TARGET, false, None, intersection_state);
                }
                if result != Ternary::False {
                    if skip_false {
                        result = ternary_and(result, Ternary::True);
                    } else {
                        let __lift14 = get_false_type_from_conditional_type(c, target);
                        result = ternary_and(result, self.is_related_to_ex(c, source, __lift14, RecursionFlags::TARGET, false, None, intersection_state));
                    }
                    if result != Ternary::False {
                        return result;
                    }
                }
            }
        } else if t_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            if s_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                if self.relation == RelationKind::Comparable {
                    let (s_texts, s_types) = template_literal_parts(c, source);
                    let (t_texts, t_types) = template_literal_parts(c, target);
                    let _ = (s_types, t_types);
                    if template_literal_types_definitely_unrelated(c, &s_texts, &t_texts) {
                        return Ternary::False;
                    }
                    return Ternary::True;
                }
                // Report unreliable variance for type variables referenced in
                // template literal type placeholders (Go comment, verbatim).
                report_unreliable_instantiate(c, source);
            }
            if is_type_matched_by_template_literal_type(c, source, target, TypeComparerKind::RelatedToWorker, Some(self)) {
                return Ternary::True;
            }
        } else if t_flags.intersects(TypeFlags::STRING_MAPPING) {
            if !s_flags.intersects(TypeFlags::STRING_MAPPING) && is_member_of_string_mapping(c, source, target) {
                return Ternary::True;
            }
        } else if is_generic_mapped_type(c, target) && self.relation != RelationKind::Identity {
            // Check if source type `S` is related to target type `{ [P in Q]: T }`
            // (Go comment, verbatim).
            let keys_remapped = !mapped_type_name_type_is_none(c, target);
            let template_type = get_template_type_from_mapped_type(c, target);
            let modifiers = get_mapped_type_modifiers(c, target);
            if !modifiers.intersects(MappedTypeModifiers::EXCLUDE_OPTIONAL) {
                // If the mapped type has shape `{ [P in Q]: T[P] }` (Go
                // comment, verbatim).
                if !keys_remapped
                    && c.type_(template_type).flags.intersects(TypeFlags::INDEXED_ACCESS)
                    && {
                        let (obj, idx) = indexed_access_parts(c, template_type);
                        obj == source && idx == get_type_parameter_from_mapped_type(c, target)
                    }
                {
                    return Ternary::True;
                }
                if !is_generic_mapped_type(c, source) {
                    // If target has shape `{ [P in Q as R]: T }` (Go
                    // comment, verbatim).
                    let target_keys = if keys_remapped {
                        get_name_type_from_mapped_type(c, target)
                    } else {
                        get_constraint_type_from_mapped_type(c, target)
                    };
                    let Some(target_keys) = target_keys else { return Ternary::False };
                    let source_keys = get_index_type_ex(c, source, IndexFlags::NO_INDEX_SIGNATURES);
                    let include_optional = modifiers.intersects(MappedTypeModifiers::INCLUDE_OPTIONAL);
                    let filtered_by_applicability = if include_optional {
                        intersect_types(c, target_keys, source_keys)
                    } else {
                        None
                    };
                    // A source type `S` is related to a target type (Go
                    // comment, verbatim).
                    if (include_optional
                        && !c.type_(filtered_by_applicability.unwrap_or(target_keys))
                            .flags
                            .intersects(TypeFlags::NEVER))
                        || (!include_optional
                            && ternary_not_false(self.is_related_to(c, target_keys, source_keys, RecursionFlags::BOTH, false)))
                    {
                        let template_type = get_template_type_from_mapped_type(c, target);
                        let type_parameter = get_type_parameter_from_mapped_type(c, target);
                        // Fastpath: When the template type has the form
                        // `Obj[P]` (Go comment, verbatim).
                        let non_null_component = extract_types_of_kind_not(c, template_type, TypeFlags::NULLABLE);
                        if !keys_remapped
                            && c.type_(non_null_component).flags.intersects(TypeFlags::INDEXED_ACCESS)
                            && indexed_access_parts(c, non_null_component).1 == type_parameter
                        {
                            let __lift15 = indexed_access_parts(c, non_null_component);
                            result = self.is_related_to(c, source, __lift15.0, RecursionFlags::TARGET, report_errors);
                            if result != Ternary::False {
                                return result;
                            }
                        } else {
                            // We need to compare the type of a property (Go
                            // comment, verbatim).
                            let indexing_type = if keys_remapped {
                                filtered_by_applicability.unwrap_or(target_keys)
                            } else if let Some(filtered) = filtered_by_applicability {
                                get_intersection_type(c, vec![filtered, type_parameter])
                            } else {
                                type_parameter
                            };
                            let indexed_access_type = get_indexed_access_type(c, source, indexing_type);
                            result = self.is_related_to(c, indexed_access_type, template_type, RecursionFlags::BOTH, report_errors);
                            if result != Ternary::False {
                                return result;
                            }
                        }
                    }
                    original_error_chain = self.error_chain.take();
                }
            }
        }

        // The source-flags ladder (with the structural default arm).
        let s_flags = c.type_(source).flags;
        let t_flags = c.type_(target).flags;
        if s_flags.intersects(TypeFlags::TYPE_VARIABLE) {
            // IndexedAccess comparisons are handled above (Go comment,
            // verbatim).
            if !s_flags.intersects(TypeFlags::INDEXED_ACCESS) || !t_flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let constraint = get_constraint_of_type(c, source).unwrap_or_else(|| c.unknown_type());
                // hi-speed no-this-instantiation check (Go comment,
                // verbatim).
                result = self.is_related_to_ex(c, constraint, target, RecursionFlags::SOURCE, false, None, intersection_state);
                if result != Ternary::False {
                    return result;
                }
                let constraint_with_this = get_type_with_this_argument(c, constraint, source, false /*needApparentType*/);
                let report = report_errors && constraint != c.unknown_type() && !TypeFlags(c.type_(target).flags.0 & s_flags.0).intersects(TypeFlags::TYPE_PARAMETER);
                result = self.is_related_to_ex(c, constraint_with_this, target, RecursionFlags::SOURCE, report, None, intersection_state);
                if result != Ternary::False {
                    return result;
                }
                if is_mapped_type_generic_indexed_access(c, source) {
                    // For an indexed access type { [P in K]: E}[X] (Go
                    // comment, verbatim).
                    let (_, s_idx) = indexed_access_parts(c, source);
                    if let Some(index_constraint) = get_constraint_of_type(c, s_idx) {
                        let (s_obj, _) = indexed_access_parts(c, source);
                        let __lift16 = get_indexed_access_type(c, s_obj, index_constraint);
                        result = self.is_related_to(c, __lift16, target, RecursionFlags::SOURCE, report_errors);
                        if result != Ternary::False {
                            return result;
                        }
                    }
                }
            }
        } else if s_flags.intersects(TypeFlags::INDEX) {
            let source_index_target = index_target(c, source);
            let source_index_flags = index_type_index_flags(c, source);
            let is_deferred_mapped_index = should_defer_index_type(c, source_index_target, source_index_flags)
                && c.type_(source_index_target).object_flags.intersects(ObjectFlags::MAPPED);
            let string_number_symbol_type = get_string_number_symbol_type(c);
            result = self.is_related_to(c, string_number_symbol_type, target, RecursionFlags::SOURCE, report_errors && !is_deferred_mapped_index);
            if result != Ternary::False {
                return result;
            }
            if is_deferred_mapped_index {
                let mapped_type = index_target(c, source);
                let name_type = get_name_type_from_mapped_type(c, mapped_type);
                // Unlike on the target side (Go comment, verbatim).
                let source_mapped_keys = if name_type.is_some() && is_mapped_type_with_keyof_constraint_declaration(c, mapped_type) {
                    // TODO(port M6 S7): getApparentMappedTypeKeys.
                    name_type
                } else {
    name_type.or_else(|| get_constraint_type_from_mapped_type(c, mapped_type))
                };
                if let Some(source_mapped_keys) = source_mapped_keys {
                    result = self.is_related_to(c, source_mapped_keys, target, RecursionFlags::SOURCE, report_errors);
                    if result != Ternary::False {
                        return result;
                    }
                }
            }
        } else if s_flags.intersects(TypeFlags::CONDITIONAL) {
            // If we reach 10 levels of nesting for the same conditional type
            // (Go comment, verbatim).
            if c.is_deeply_nested_type(source, &self.source_stack, 10) {
                return Ternary::Maybe;
            }
            if t_flags.intersects(TypeFlags::CONDITIONAL) {
                // Two conditional types (Go comment, verbatim).
                let source_params = conditional_infer_type_parameters(c, source);
                let (s_check, mut s_extends) = conditional_parts(c, source);
                let (_, t_extends) = conditional_parts(c, target);
                let mut mapper: Option<crate::todo_shapes::TypeMapperId> = None;
                if !source_params.is_empty() {
                    // If the source has infer type parameters (Go comment,
                    // verbatim).
                    // Go: `ctx := r.c.newInferenceContext(sourceParams, nil, InferenceFlagsNone, r.isRelatedToWorker)` —
                    // the comparer is the relater's worker (the relation captured; see
                    // inference.rs's InferenceComparer PORT note).
                    let ctx = c.new_inference_context(
                        &source_params,
                        None,
                        crate::inference::InferenceFlags::NONE,
                        crate::inference::InferenceComparer::Relater(self.relation),
                    );
                    c.infer_types(ctx, t_extends, s_extends, crate::inference::InferencePriority::NO_CONSTRAINTS | crate::inference::InferencePriority::ALWAYS_STRICT, false);
                    let __lift17 = crate::inference::inference_context_mapper(c, ctx);
                    s_extends = instantiate_type_ex(c, s_extends, __lift17);
                    mapper = crate::inference::inference_context_mapper(c, ctx);
                }
                let target_check_type = conditional_parts(c, target).0;
                if c.is_type_identical_to(s_extends, t_extends)
                    && (ternary_not_false(self.is_related_to(c, s_check, target_check_type, RecursionFlags::BOTH, false))
                        || ternary_not_false(self.is_related_to(c, target_check_type, s_check, RecursionFlags::BOTH, false)))
                {
                    let source_true_type = get_true_type_from_conditional_type(c, source);
                    let __lift19 = instantiate_type_opt(c, source_true_type, mapper);
                    result = self.is_related_to(c, __lift19, target, RecursionFlags::BOTH, report_errors);
                    if result != Ternary::False {
                        let __lift21 = get_false_type_from_conditional_type(c, source);
                        let __lift20 = get_false_type_from_conditional_type(c, target);
                        result = ternary_and(result, self.is_related_to(c, __lift21, __lift20, RecursionFlags::BOTH, report_errors));
                        if result != Ternary::False {
                            return result;
                        }
                    }
                }
            }
            // conditionals can be related to one another via normal
            // constraint (Go comment, verbatim).
            if let Some(default_constraint) = get_default_constraint_of_conditional_type(c, source) {
                result = self.is_related_to(c, default_constraint, target, RecursionFlags::SOURCE, report_errors);
                if result != Ternary::False {
 return result;
                }
            }
            // conditionals aren't related to one another via distributive
            // constraint (Go comment, verbatim).
            if !t_flags.intersects(TypeFlags::CONDITIONAL) && has_non_circular_base_constraint(c, source) {
                if let Some(distributive_constraint) = get_constraint_of_distributive_conditional_type(c, source) {
                    self.error_chain = save_error_state.error_chain.clone();
                    result = self.is_related_to(c, distributive_constraint, target, RecursionFlags::SOURCE, report_errors);
                    if result != Ternary::False {
                        return result;
                    }
                }
            }
        } else if s_flags.intersects(TypeFlags::TEMPLATE_LITERAL) && !t_flags.intersects(TypeFlags::OBJECT) {
            if !t_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                if let Some(constraint) = get_base_constraint_of_type(c, source) {
                    if constraint != source {
                        result = self.is_related_to(c, constraint, target, RecursionFlags::SOURCE, report_errors);
                        if result != Ternary::False {
                            return result;
                        }
                    }
                }
            }
        } else if s_flags.intersects(TypeFlags::STRING_MAPPING) {
            if t_flags.intersects(TypeFlags::STRING_MAPPING) {
                if string_mapping_symbol(c, source) != string_mapping_symbol(c, target) {
                    return Ternary::False;
                }
                let __lift23 = string_mapping_target(c, source);
                let __lift22 = string_mapping_target(c, target);
                result = self.is_related_to(c, __lift23, __lift22, RecursionFlags::BOTH, report_errors);
                if result != Ternary::False {
                    return result;
                }
            } else if let Some(constraint) = get_base_constraint_of_type(c, source) {
                result = self.is_related_to(c, constraint, target, RecursionFlags::SOURCE, report_errors);
                if result != Ternary::False {
                    return result;
                }
            }
        } else {
            // An empty object type is related to any mapped type that
            // includes a '?' modifier (Go comment, verbatim).
            if self.relation != RelationKind::Subtype
                && self.relation != RelationKind::StrictSubtype
                && is_partial_mapped_type(c, target)
                && is_empty_object_type(c, source)
            {
                return Ternary::True;
            }
            if is_generic_mapped_type(c, target) {
                if is_generic_mapped_type(c, source) {
                    result = self.mapped_type_related_to(c, source, target, report_errors);
                    if result != Ternary::False {
                        return result;
                    }
                }
                return Ternary::False;
            }
            let source_is_primitive = s_flags.intersects(TypeFlags::PRIMITIVE);
            if self.relation != RelationKind::Identity {
                source = get_apparent_type(c, source);
            } else if is_generic_mapped_type(c, source) {
                return Ternary::False;
            }
            let _s_flags = c.type_(source).flags;
            if c.type_(source).object_flags.intersects(ObjectFlags::REFERENCE)
                && c.type_(target).object_flags.intersects(ObjectFlags::REFERENCE)
                && type_reference_target(c, source) == type_reference_target(c, target)
                && !is_tuple_type(c, source)
                && !c.is_marker_type(source)
                && !c.is_marker_type(target)
            {
                // When strictNullChecks is disabled (Go comment, verbatim).
                if is_empty_array_literal_type(c, source) {
                    return Ternary::True;
                }
                // We have type references to the same generic type (Go
                // comment, verbatim).
                let variances = c.get_variances(type_reference_target(c, source));
                if variances.is_empty() {
                    return Ternary::Unknown;
                }
                let __lift25 = get_type_arguments(c, source);
                let __lift24 = get_type_arguments(c, target);
                let (variance_result, ok) = self.relate_variances(c, &__lift25, &__lift24, &variances, report_errors, intersection_state, &mut original_error_chain, &mut variance_check_failed);
                if ok {
                    return variance_result;
                }
            } else if is_array_type(c, target) {
                let source_types = distributed(c, source);
                let all_array_or_tuple = source_types.iter().all(|&t| is_array_or_tuple_type(c, t));
                let all_mutable_tuple = source_types.iter().all(|&t| is_mutable_tuple_type(c, t));
                if ((is_readonly_array_type(c, target) && all_array_or_tuple) || all_mutable_tuple)
                    && self.relation != RelationKind::Identity {
                        let number_type = c.number_type();
                        let any_type = c.any_type();
                        let source_index_type = get_index_type_of_type_ex(c, source, number_type, any_type);
                        let target_index_type = get_index_type_of_type_ex(c, target, number_type, any_type);
                        return self.is_related_to(
                            c,
                            source_index_type,
                            target_index_type,
                            RecursionFlags::BOTH,
                            report_errors,
                        );
                    }
                // By flags alone (Go comment, verbatim).
                return Ternary::False;
 } else if is_generic_tuple_type(c, source) && is_tuple_type(c, target) && !is_generic_tuple_type(c, target) {
                let constraint = get_base_constraint_or_type(c, source);
                if constraint != source {
                    return self.is_related_to(c, constraint, target, RecursionFlags::SOURCE, report_errors);
                }
            } else if (self.relation == RelationKind::Subtype || self.relation == RelationKind::StrictSubtype)
                && is_empty_object_type(c, target)
                && c.type_(target).object_flags.intersects(ObjectFlags::FRESH_LITERAL)
                && !is_empty_object_type(c, source)
            {
                return Ternary::False;
            }
            // Even if relationship doesn't hold for unions (Go comment,
            // verbatim).
            if c.type_(source).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && t_flags.intersects(TypeFlags::OBJECT)
            {
                // Report structural errors only if we haven't reported any
                // errors yet (Go comment, verbatim).
                let report_structural_errors = report_errors
                    && self.error_chain == save_error_state.error_chain
                    && !source_is_primitive;
                result = self.properties_related_to(c, source, target, report_structural_errors, &Default::default(), false, intersection_state);
                if result != Ternary::False {
                    result = ternary_and(result, self.signatures_related_to(c, source, target, SignatureKind::Call, report_structural_errors, intersection_state));
                    if result != Ternary::False {
                        result = ternary_and(result, self.signatures_related_to(c, source, target, SignatureKind::Construct, report_structural_errors, intersection_state));
                        if result != Ternary::False {
                            result = ternary_and(result, self.index_signatures_related_to(c, source, target, source_is_primitive, report_structural_errors, intersection_state));
                        }
                    }
                }
                if result != Ternary::False {
                    if !variance_check_failed {
                        return result;
                    }
                    if let Some(original) = original_error_chain.take() {
                        self.error_chain = Some(original);
                    } else if self.error_chain.is_none() {
                        self.error_chain = save_error_state.error_chain.clone();
                    }
                    // Use variance error (there is no structural one) and
                    // return false (Go comment, verbatim).
                }
            }
            // If S is an object type and T is a discriminated union (Go
            // comment, verbatim).
            if c.type_(source).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && t_flags.intersects(TypeFlags::UNION)
            {
                let object_only_target = extract_types_of_kind(c, target, TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::SUBSTITUTION);
                if c.type_(object_only_target).flags.intersects(TypeFlags::UNION) {
                    let result = self.type_related_to_discriminated_type(c, source, object_only_target);
                    if result != Ternary::False {
                        return result;
                    }
                }
            }
        }
        Ternary::False
    }

    /// Go: `func (r *Relater) typeArgumentsRelatedTo(...) Ternary`.
    fn type_arguments_related_to(
        &mut self,
        c: &mut Checker,
        sources: &[TypeId],
        targets: &[TypeId],
        variances: &[VarianceFlags],
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if sources.len() != targets.len() && self.relation == RelationKind::Identity {
            return Ternary::False;
        }
        let length = sources.len().min(targets.len());
        let mut result = Ternary::True;
        for i in 0..length {
            // When variance information isn't available we default to
            // covariance (Go comment, verbatim).
            let variance_flags = if i < variances.len() { variances[i] } else { VarianceFlags::COVARIANT };
            let variance = VarianceFlags(variance_flags.0 & VarianceFlags::VARIANCE_MASK.0);
            // We ignore arguments for independent type parameters (Go
            // comment, verbatim).
            if variance != VarianceFlags::INDEPENDENT {
                let s = sources[i];
                let t = targets[i];
                let related;
                if variance_flags.intersects(VarianceFlags::UNMEASURABLE) {
                    // Even an `Unmeasurable` variance works out (Go comment,
                    // verbatim).
                    if self.relation == RelationKind::Identity {
                        related = self.is_related_to(c, s, t, RecursionFlags::BOTH, false);
                    } else {
                        related = c.compare_types_identical(s, t);
                    }
                } else {
                    // Propagate unreliable variance flag in variance
                    // computations (Go comment, verbatim).
                    if !c.variance_stack.is_empty() && variance_flags.intersects(VarianceFlags::UNRELIABLE) {
                        report_unreliable_instantiate(c, s);
                    }
                    if variance == VarianceFlags::COVARIANT {
                        related = self.is_related_to_ex(c, s, t, RecursionFlags::BOTH, report_errors, None, intersection_state);
                    } else if variance == VarianceFlags::CONTRAVARIANT {
                        related = self.is_related_to_ex(c, t, s, RecursionFlags::BOTH, report_errors, None, intersection_state);
                    } else if variance == VarianceFlags::BIVARIANT {
                        // In the bivariant case (Go comment, verbatim).
                        let contra = self.is_related_to(c, t, s, RecursionFlags::BOTH, false);
                        related = if contra == Ternary::False {
                            self.is_related_to_ex(c, s, t, RecursionFlags::BOTH, report_errors, None, intersection_state)
                        } else {
                            contra
                        };
                    } else {
                        // In the invariant case (Go comment, verbatim).
                        let co = self.is_related_to_ex(c, s, t, RecursionFlags::BOTH, report_errors, None, intersection_state);
                        related = if co != Ternary::False {
                            ternary_and(co, self.is_related_to_ex(c, t, s, RecursionFlags::BOTH, report_errors, None, intersection_state))
                        } else {
                            co
                        };
                    }
                }
                if related == Ternary::False {
                    return Ternary::False;
                }
                result = ternary_and(result, related);
            }
        }
        result
    }

    /// Go: `func (r *Relater) mappedTypeRelatedTo(source, target, reportErrors) Ternary`.
    fn mapped_type_related_to(&mut self, c: &mut Checker, source: TypeId, target: TypeId, report_errors: bool) -> Ternary {
        let modifiers_related = self.relation == RelationKind::Comparable
            || self.relation == RelationKind::Identity
                && get_mapped_type_modifiers(c, source) == get_mapped_type_modifiers(c, target)
            || self.relation != RelationKind::Identity
                && get_combined_mapped_type_optionality(c, source) <= get_combined_mapped_type_optionality(c, target);
        if modifiers_related {
            let target_constraint =
                get_constraint_type_from_mapped_type(c, target).unwrap_or_else(|| c.never_type());
            let source_mapped_constraint =
                get_constraint_type_from_mapped_type(c, source).unwrap_or_else(|| c.never_type());
            let source_mapper = if get_combined_mapped_type_optionality(c, source) < 0 {
                report_unmeasurable_mapper(c)
            } else {
                report_unreliable_mapper(c)
            };
            let source_constraint = instantiate_type_ex(c, source_mapped_constraint, source_mapper);
            let result = self.is_related_to(c, target_constraint, source_constraint, RecursionFlags::BOTH, report_errors);
            if result != Ternary::False {
                let __lift27 = get_type_parameter_from_mapped_type(c, source);
                let __lift26 = get_type_parameter_from_mapped_type(c, target);
                let mapper = new_simple_type_mapper(c, __lift27, __lift26);
                let source_name_type = get_name_type_from_mapped_type(c, source);
                let target_name_type = get_name_type_from_mapped_type(c, target);
                // Go: both name types may be nil; nil == nil compares equal.
                let names_equal = match (source_name_type, target_name_type) {
                    (None, None) => true,
                    (Some(s), Some(t)) => {
                        let s_instantiated = instantiate_type_ex(c, s, Some(mapper));
                        let t_instantiated = instantiate_type_ex(c, t, Some(mapper));
                        s_instantiated == t_instantiated
                    }
                    _ => false,
                };
                if names_equal {
                    let source_template = get_template_type_from_mapped_type(c, source);
                    let source_template_inst = instantiate_type_ex(c, source_template, Some(mapper));
                    let target_template = get_template_type_from_mapped_type(c, target);
                    return ternary_and(
                        result,
                        self.is_related_to(
                            c,
                            source_template_inst,
                            target_template,
                            RecursionFlags::BOTH,
                            report_errors,
                        ),
                    );
                }
            }
        }
        Ternary::False
    }
}

/// Go: the `getTypeOfSourceProperty func(*ast.Symbol) *Type` parameter of
/// `propertyRelatedTo` — the two call sites are `c.getNonMissingTypeOfSymbol`
/// and a constant closure (the discriminant matrix comparison); the port
/// tags them (§5.4 closure precedent).
#[derive(Clone, Copy)]
pub(crate) enum GetTypeOfSourceProperty {
    NonMissingTypeOfSymbol,
    Constant(TypeId),
}

impl Relater {
    /// Go: `func (r *Relater) typeRelatedToDiscriminatedType(source, target) Ternary`
    /// (see Go's numbered doc comment, verbatim there).
    #[allow(clippy::too_many_lines)]
    fn type_related_to_discriminated_type(&mut self, c: &mut Checker, source: TypeId, target: TypeId) -> Ternary {
        let source_properties = get_properties_of_type(c, source);
        let source_properties_filtered = find_discriminant_properties(c, &source_properties, target);
        if source_properties_filtered.is_empty() {
            return Ternary::False;
        }
        // Though we could compute the number of combinations as we generate
        // the matrix (Go comment, verbatim).
        let mut num_combinations: usize = 1;
        for &source_property in &source_properties_filtered {
            let __lift28 = get_non_missing_type_of_symbol(c, source_property);
            num_combinations = num_combinations.saturating_mul(count_types(c, __lift28));
            if num_combinations > 25 {
                if let Some(tr) = &c.tracer {
                    tr.instant(
                        crate::tracing::Phase::CheckTypes,
                        "typeRelatedToDiscriminatedType_DepthLimit",
                        &{
                            let args = crate::tracing::Args::new();
                            args.insert("sourceId", crate::tracing::TraceValue::Uint(source.0 as u64));
                            args.insert("targetId", crate::tracing::TraceValue::Uint(target.0 as u64));
                            args.insert("numCombinations", crate::tracing::TraceValue::Int(num_combinations as i64));
                            args
                        },
                    );
                }
                return Ternary::False;
            }
            if num_combinations == 0 {
                return Ternary::False;
            }
        }
        // Compute the set of types for each discriminant property.
        let mut source_discriminant_types: Vec<Vec<TypeId>> = Vec::with_capacity(source_properties_filtered.len());
        let mut excluded_properties: FxHashSet<String> = FxHashSet::default();
        for &source_property in &source_properties_filtered {
            let source_property_type = get_non_missing_type_of_symbol(c, source_property);
            source_discriminant_types.push(distributed(c, source_property_type));
            excluded_properties.insert(c.program_symbol(source_property).name.clone());
        }
        // Build the cartesian product.
        let mut discriminant_combinations: Vec<Vec<TypeId>> = Vec::with_capacity(num_combinations);
        for i in 0..num_combinations {
            let mut combination = vec![TypeId(0); source_discriminant_types.len()];
            let mut n = i;
            for j in (0..source_discriminant_types.len()).rev() {
                let source_types = &source_discriminant_types[j];
                combination[j] = source_types[n % source_types.len()];
                n /= source_types.len();
            }
            discriminant_combinations.push(combination);
        }
        // Match each combination of the cartesian product (Go comment,
        // verbatim).
        let mut matching_types: Vec<TypeId> = Vec::new();
        let target_types: Vec<TypeId> = c.type_(target).types().to_vec();
        for combination in &discriminant_combinations {
            let mut has_match = false;
            'outer: for &t in &target_types {
                for (i, &source_property) in source_properties_filtered.iter().enumerate() {
                    let name = c.program_symbol(source_property).name.clone();
                    let Some(target_property) = get_property_of_type(c, t, &name) else { continue 'outer };
                    if source_property == target_property {
                        continue;
                    }
                    // We compare the source property to the target in the
                    // context of a single discriminant type (Go comment,
                    // verbatim).
                    let related = self.property_related_to(
                        c,
                        source,
                        target,
                        source_property,
                        target_property,
                        GetTypeOfSourceProperty::Constant(combination[i]),
                        false, /*reportErrors*/
                        IntersectionState::NONE,
                        c.strict_null_checks || self.relation == RelationKind::Comparable, /*skipOptional*/
                    );
                    if related == Ternary::False {
                        continue 'outer;
                    }
                }
                if !matching_types.contains(&t) {
                    matching_types.push(t);
                }
                has_match = true;
            }
            if !has_match {
                // We failed to match any type for this combination.
                return Ternary::False;
            }
        }
        // Compare the remaining non-discriminant properties of each match.
        let mut result = Ternary::True;
        for &t in &matching_types {
            result = ternary_and(result, self.properties_related_to(c, source, t, false, &excluded_properties, false, IntersectionState::NONE));
            if result != Ternary::False {
                result = ternary_and(result, self.signatures_related_to(c, source, t, SignatureKind::Call, false, IntersectionState::NONE));
                if result != Ternary::False {
                    result = ternary_and(result, self.signatures_related_to(c, source, t, SignatureKind::Construct, false, IntersectionState::NONE));
                    if result != Ternary::False && !(is_tuple_type(c, source) && is_tuple_type(c, t)) {
                        // Comparing numeric index types when both `source`
                        // and `type` are tuples is unnecessary (Go comment,
                        // verbatim).
                        result = ternary_and(result, self.index_signatures_related_to(c, source, t, false, false, IntersectionState::NONE));
                    }
                }
            }
            if result == Ternary::False {
                return result;
            }
        }
        result
    }

    /// Go: `func (r *Relater) propertiesRelatedTo(...) Ternary`.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    #[allow(clippy::needless_range_loop)] // TODO(port M6 W2): Go mirrors the indexed arity walk
    fn properties_related_to(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        excluded_properties: &FxHashSet<String>,
        optionals_only: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if self.relation == RelationKind::Identity {
            return self.properties_identical_to(c, source, target, excluded_properties);
        }
        let mut result = Ternary::True;
        if is_tuple_type(c, target) {
            if is_array_or_tuple_type(c, source) {
                if !tuple_type_readonly(c, target)
                    && (is_readonly_array_type(c, source) || (is_tuple_type(c, source) && tuple_type_readonly(c, source)))
                {
                    return Ternary::False;
                }
                let source_arity = get_type_reference_arity(c, source) as usize;
                let target_arity = get_type_reference_arity(c, target) as usize;
                let source_rest = if is_tuple_type(c, source) {
                    tuple_type_combined_flags(c, source).intersects(ElementFlags::REST)
                } else {
                    true
                };
                let target_has_rest_element = tuple_type_combined_flags(c, target).intersects(ElementFlags::REST);
                let target_has_variable_element = tuple_type_combined_flags(c, target).intersects(ElementFlags::VARIABLE);
                let source_min_length = if is_tuple_type(c, source) {
                    tuple_type_min_length(c, source)
                } else {
                    0
                };
                let target_min_length = tuple_type_min_length(c, target);
                if !source_rest && source_arity < target_min_length {
                    if report_errors {
                        self.report_error(c, &msg::SOURCE_HAS_0_ELEMENT_S_BUT_TARGET_REQUIRES_1, &[DiagArg::Uint(source_arity as u64), DiagArg::Uint(target_min_length as u64)]);
                    }
                    return Ternary::False;
                }
                if !target_has_variable_element && target_arity < source_min_length {
                    if report_errors {
                        self.report_error(c, &msg::SOURCE_HAS_0_ELEMENT_S_BUT_TARGET_ALLOWS_ONLY_1, &[DiagArg::Uint(source_min_length as u64), DiagArg::Uint(target_arity as u64)]);
                    }
                    return Ternary::False;
                }
                if !target_has_variable_element && (source_rest || target_arity < source_arity) {
                    if report_errors {
                        if source_min_length < target_min_length {
                            self.report_error(c, &msg::TARGET_REQUIRES_0_ELEMENT_S_BUT_SOURCE_MAY_HAVE_FEWER, &[DiagArg::Uint(target_min_length as u64)]);
                        } else {
                            self.report_error(c, &msg::TARGET_ALLOWS_ONLY_0_ELEMENT_S_BUT_SOURCE_MAY_HAVE_MORE, &[DiagArg::Uint(target_arity as u64)]);
                        }
                    }
                    return Ternary::False;
                }
                let source_type_arguments = get_type_arguments(c, source);
                let target_type_arguments = get_type_arguments(c, target);
                let target_start_count = get_start_element_count(c, target, ElementFlags::NON_REST);
                let target_end_count = get_end_element_count(c, target, ElementFlags::NON_REST);
                let mut can_exclude_discriminants = !excluded_properties.is_empty();
                for source_position in 0..source_arity {
                    let source_flags = if is_tuple_type(c, source) {
                        tuple_type_element_flags(c, source, source_position)
                    } else {
                        ElementFlags::REST
                    };
                    let source_position_from_end = source_arity - 1 - source_position;
                    let target_position = if target_has_rest_element && source_position >= target_start_count {
                        target_arity - 1 - source_position_from_end.min(target_end_count)
                    } else {
                        if source_position >= target_arity {
                            if report_errors {
                                self.report_error(c, &msg::TARGET_ALLOWS_ONLY_0_ELEMENT_S_BUT_SOURCE_MAY_HAVE_MORE, &[DiagArg::Uint(target_arity as u64)]);
                            }
                            return Ternary::False;
                        }
                        source_position
                    };
                    let mut target_flags = ElementFlags::NONE;
                    if target_position < target_arity {
                        target_flags = tuple_type_element_flags(c, target, target_position);
                    }
                    if target_flags.intersects(ElementFlags::VARIADIC) && !source_flags.intersects(ElementFlags::VARIADIC) {
                        if report_errors {
                            self.report_error(c, &msg::SOURCE_PROVIDES_NO_MATCH_FOR_VARIADIC_ELEMENT_AT_POSITION_0_IN_TARGET, &[DiagArg::Uint(target_position as u64)]);
                        }
                        return Ternary::False;
                    }
                    if source_flags.intersects(ElementFlags::VARIADIC) && !target_flags.intersects(ElementFlags::VARIABLE) {
                        if report_errors {
                            self.report_error(c, &msg::VARIADIC_ELEMENT_AT_POSITION_0_IN_SOURCE_DOES_NOT_MATCH_ELEMENT_AT_POSITION_1_IN_TARGET, &[DiagArg::Uint(source_position as u64), DiagArg::Uint(target_position as u64)]);
                        }
                        return Ternary::False;
                    }
                    if target_flags.intersects(ElementFlags::REQUIRED) && !source_flags.intersects(ElementFlags::REQUIRED) {
                        if report_errors {
                            self.report_error(c, &msg::SOURCE_PROVIDES_NO_MATCH_FOR_REQUIRED_ELEMENT_AT_POSITION_0_IN_TARGET, &[DiagArg::Uint(target_position as u64)]);
                        }
                        return Ternary::False;
                    }
                    // We can only exclude discriminant properties (Go
                    // comment, verbatim).
                    if can_exclude_discriminants {
                        if source_flags.intersects(ElementFlags::VARIABLE) || target_flags.intersects(ElementFlags::VARIABLE) {
                            can_exclude_discriminants = false;
                        }
                        if can_exclude_discriminants && excluded_properties.contains(&source_position.to_string()) {
                            continue;
                        }
                    }
                    let source_type = remove_missing_type(c, source_type_arguments[source_position], ElementFlags(source_flags.0 & target_flags.0).intersects(ElementFlags::OPTIONAL));
                    let target_type = target_type_arguments[target_position];
                    let target_check_type = if source_flags.intersects(ElementFlags::VARIADIC) && target_flags.intersects(ElementFlags::REST) {
                        c.create_array_type(target_type)
                    } else {
                        remove_missing_type(c, target_type, target_flags.intersects(ElementFlags::OPTIONAL))
                    };
                    let related = self.is_related_to_ex(c, source_type, target_check_type, RecursionFlags::BOTH, report_errors, None, intersection_state);
                    if related == Ternary::False {
                        if report_errors && (target_arity > 1 || source_arity > 1) {
                            if target_has_rest_element
                                && source_position >= target_start_count
                                && source_position_from_end >= target_end_count
                                && target_start_count != source_arity - target_end_count - 1
                            {
                                self.report_error(c, &msg::TYPE_AT_POSITIONS_0_THROUGH_1_IN_SOURCE_IS_NOT_COMPATIBLE_WITH_TYPE_AT_POSITION_2_IN_TARGET, &[DiagArg::Uint(target_start_count as u64), DiagArg::Uint((source_arity - target_end_count - 1) as u64), DiagArg::Uint(target_position as u64)]);
                            } else {
                                self.report_error(c, &msg::TYPE_AT_POSITION_0_IN_SOURCE_IS_NOT_COMPATIBLE_WITH_TYPE_AT_POSITION_1_IN_TARGET, &[DiagArg::Uint(source_position as u64), DiagArg::Uint(target_position as u64)]);
                            }
                        }
                        return Ternary::False;
                    }
                    result = ternary_and(result, related);
                }
                return result;
            }
            if tuple_type_combined_flags(c, target).intersects(ElementFlags::VARIABLE) {
                return Ternary::False;
            }
        }
        let require_optional_properties = (self.relation == RelationKind::Subtype || self.relation == RelationKind::StrictSubtype)
            && !is_object_literal_type(c, source)
            && !is_empty_array_literal_type(c, source)
            && !is_tuple_type(c, source);
        if let Some(unmatched_property) = get_unmatched_property(c, source, target, require_optional_properties, false /*matchDiscriminantProperties*/) {
            if report_errors && should_report_unmatched_property_error(c, source, target) {
                self.report_unmatched_property(c, source, target, unmatched_property, require_optional_properties);
            }
            return Ternary::False;
        }
        if is_object_literal_type(c, target) {
            let __lift29 = get_properties_of_type(c, source);
            for source_prop in exclude_properties(c, __lift29, excluded_properties) {
                let name = c.program_symbol(source_prop).name.clone();
                if get_property_of_object_type(c, target, &name).is_none() {
                    if report_errors {
                        let __ts0 = symbol_to_string(c, source_prop);
                        let __ts1 = type_to_string(c, target);
                        self.report_error(c, &msg::PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1)]);
                    }
                    return Ternary::False;
                }
            }
        }
        // We only call this for union target types (Go comment, verbatim).
        let properties = get_properties_of_type(c, target);
        let numeric_names_only = is_tuple_type(c, source) && is_tuple_type(c, target);
        for target_prop in exclude_properties(c, properties, excluded_properties) {
            let name = c.program_symbol(target_prop).name.clone();
            if !c.program_symbol(target_prop).flags.intersects(SymbolFlags::PROTOTYPE)
                && (!numeric_names_only || is_numeric_literal_name(&name) || name == "length")
                && (!optionals_only || c.program_symbol(target_prop).flags.intersects(SymbolFlags::OPTIONAL))
            {
                let source_prop = get_property_of_type(c, source, &name);
                if let Some(source_prop) = source_prop {
                    if source_prop != target_prop {
                        let related = self.property_related_to(
                            c,
                            source,
                            target,
                            source_prop,
                            target_prop,
                            GetTypeOfSourceProperty::NonMissingTypeOfSymbol,
                            report_errors,
                            intersection_state,
                            self.relation == RelationKind::Comparable, /*skipOptional*/
                        );
                        if related == Ternary::False {
                            return Ternary::False;
                        }
                        result = ternary_and(result, related);
                    }
                }
            }
        }
        result
    }

    /// Go: `func (r *Relater) propertyRelatedTo(...) Ternary`.
    #[allow(clippy::too_many_arguments)]
    fn property_related_to(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        source_prop: SymbolId,
        target_prop: SymbolId,
        get_type_of_source_property: GetTypeOfSourceProperty,
        report_errors: bool,
        intersection_state: IntersectionState,
        skip_optional: bool,
    ) -> Ternary {
        let source_prop_flags = get_declaration_modifier_flags_from_symbol(c, source_prop);
        let target_prop_flags = get_declaration_modifier_flags_from_symbol(c, target_prop);
        if source_prop_flags.intersects(ModifierFlags::PRIVATE) || target_prop_flags.intersects(ModifierFlags::PRIVATE) {
            if symbol_value_declaration(c, source_prop) != symbol_value_declaration(c, target_prop) {
                if report_errors {
                    if source_prop_flags.intersects(ModifierFlags::PRIVATE) && target_prop_flags.intersects(ModifierFlags::PRIVATE) {
                        let __lift30 = symbol_to_string(c, target_prop);
                        self.report_error(c, &msg::TYPES_HAVE_SEPARATE_DECLARATIONS_OF_A_PRIVATE_PROPERTY_0, &[DiagArg::Str(__lift30)]);
                    } else {
                        let (private_side, other_side) = if source_prop_flags.intersects(ModifierFlags::PRIVATE) { (source, target) } else { (target, source) };
                        let __ts0 = symbol_to_string(c, target_prop);
                        let __ts1 = type_to_string(c, private_side);
                        let __ts2 = type_to_string(c, other_side);
                        self.report_error(c, &msg::PROPERTY_0_IS_PRIVATE_IN_TYPE_1_BUT_NOT_IN_TYPE_2, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1), DiagArg::Str(__ts2)]);
                    }
                }
                return Ternary::False;
            }
        } else if target_prop_flags.intersects(ModifierFlags::PROTECTED) {
            if !is_valid_override_of(c, source_prop, target_prop) {
                if report_errors {
                    let source_type = get_declaring_class(c, source_prop).unwrap_or(source);
                    let target_type = get_declaring_class(c, target_prop).unwrap_or(target);
                    let __ts0 = symbol_to_string(c, target_prop);
                    let __ts1 = type_to_string(c, source_type);
   let __ts2 = type_to_string(c, target_type);
                    self.report_error(c, &msg::PROPERTY_0_IS_PROTECTED_BUT_TYPE_1_IS_NOT_A_CLASS_DERIVED_FROM_2, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1), DiagArg::Str(__ts2)]);
                }
                return Ternary::False;
            }
        } else if source_prop_flags.intersects(ModifierFlags::PROTECTED) {
            if report_errors {
                let __ts0 = symbol_to_string(c, target_prop);
                let __ts1 = type_to_string(c, source);
                let __ts2 = type_to_string(c, target);
                self.report_error(c, &msg::PROPERTY_0_IS_PROTECTED_IN_TYPE_1_BUT_PUBLIC_IN_TYPE_2, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1), DiagArg::Str(__ts2)]);
            }
            return Ternary::False;
        }
        // Ensure {readonly a: whatever} is not a subtype of {a: whatever}
        // (Go comment, verbatim).
        if self.relation == RelationKind::StrictSubtype && is_readonly_symbol(c, source_prop) && !is_readonly_symbol(c, target_prop) {
            return Ternary::False;
        }
        // If the target comes from a partial union prop (Go comment,
        // verbatim).
        let related = self.is_property_symbol_type_related(c, source_prop, target_prop, get_type_of_source_property, report_errors, intersection_state);
        if related == Ternary::False {
            if report_errors {
                let __lift31 = symbol_to_string(c, target_prop);
                self.report_error(c, &msg::TYPES_OF_PROPERTY_0_ARE_INCOMPATIBLE, &[DiagArg::Str(__lift31)]);
            }
            return Ternary::False;
        }
        // When checking for comparability, be more lenient with optional
        // properties (Go comment, verbatim).
        if !skip_optional
            && c.program_symbol(source_prop).flags.intersects(SymbolFlags::OPTIONAL)
            && c.program_symbol(target_prop).flags.intersects(SymbolFlags::CLASS_MEMBER)
            && !c.program_symbol(target_prop).flags.intersects(SymbolFlags::OPTIONAL)
        {
            // TypeScript 1.0 spec (April 2014): 3.8.3 (Go comment, verbatim).
            if report_errors {
                let __ts0 = symbol_to_string(c, target_prop);
                let __ts1 = type_to_string(c, source);
                let __ts2 = type_to_string(c, target);
                self.report_error(c, &msg::PROPERTY_0_IS_OPTIONAL_IN_TYPE_1_BUT_REQUIRED_IN_TYPE_2, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1), DiagArg::Str(__ts2)]);
            }
            return Ternary::False;
        }
        related
    }

    /// Go: `func (r *Relater) isPropertySymbolTypeRelated(...) Ternary`.
    fn is_property_symbol_type_related(
        &mut self,
        c: &mut Checker,
        source_prop: SymbolId,
        target_prop: SymbolId,
        get_type_of_source_property: GetTypeOfSourceProperty,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let target_is_optional = c.strict_null_checks
            && c.program_symbol(target_prop)
                .check_flags
                .intersects(CHECK_FLAGS_PARTIAL);
        let __lift32 = get_non_missing_type_of_symbol(c, target_prop);
        let effective_target = add_optionality_ex(c, __lift32, false /*isProperty*/, target_is_optional);
        // source could resolve to `any` and that's not related to `unknown`
        // target under strict subtype relation (Go comment, verbatim).
        let mask = if self.relation == RelationKind::StrictSubtype { TypeFlags::ANY } else { TypeFlags::ANY_OR_UNKNOWN };
        if c.type_(effective_target).flags.intersects(mask) {
            return Ternary::True;
        }
        let effective_source = match get_type_of_source_property {
            GetTypeOfSourceProperty::NonMissingTypeOfSymbol => get_non_missing_type_of_symbol(c, source_prop),
            GetTypeOfSourceProperty::Constant(t) => t,
        };
        self.is_related_to_ex(c, effective_source, effective_target, RecursionFlags::BOTH, report_errors, None, intersection_state)
    }

    /// Go: `func (r *Relater) reportUnmatchedProperty(...)`.
    fn report_unmatched_property(&mut self, c: &mut Checker, source: TypeId, target: TypeId, unmatched_property: SymbolId, require_optional_properties: bool) {
        // give specific error in case where private names have the same
        // description (Go comment, verbatim).
        // TODO(port M6 S6): the private-identifier description arm
        // (binder.GetSymbolNameForPrivateIdentifier).
        let props = get_unmatched_properties(c, source, target, require_optional_properties, false /*matchDiscriminantProperties*/);
        if props.len() == 1 {
            let (source_type, target_type) = c.get_type_names_for_error_display(source, target);
            let prop_name = symbol_to_string(c, unmatched_property);
            self.report_error(c, &msg::PROPERTY_0_IS_MISSING_IN_TYPE_1_BUT_REQUIRED_IN_TYPE_2, &[DiagArg::Str(prop_name.clone()), DiagArg::Str(source_type), DiagArg::Str(target_type)]);
            if let Some(decl) = c.program_symbol(unmatched_property).declarations.first().copied() {
                self.related_info.push(diagnostic_for_node(c, Some(decl), &msg::X_0_IS_DECLARED_HERE, &[DiagArg::Str(prop_name)]));
            }
        } else if self.try_elaborate_array_like_errors(c, source, target, false /*reportErrors*/) {
            let (source_type, target_type) = c.get_type_names_for_error_display(source, target);
            if props.len() > 5 {
                let prop_names = props.iter().take(4).map(|&p| symbol_to_string(c, p)).collect::<Vec<_>>().join(", ");
                self.report_error(c, &msg::TYPE_0_IS_MISSING_THE_FOLLOWING_PROPERTIES_FROM_TYPE_1_COLON_2_AND_3_MORE, &[DiagArg::Str(source_type), DiagArg::Str(target_type), DiagArg::Str(prop_names), DiagArg::Uint((props.len() - 4) as u64)]);
            } else {
                let prop_names = props.iter().map(|&p| symbol_to_string(c, p)).collect::<Vec<_>>().join(", ");
                self.report_error(c, &msg::TYPE_0_IS_MISSING_THE_FOLLOWING_PROPERTIES_FROM_TYPE_1_COLON_2, &[DiagArg::Str(source_type), DiagArg::Str(target_type), DiagArg::Str(prop_names)]);
            }
        }
    }

    /// Go: `func (r *Relater) tryElaborateArrayLikeErrors(...) bool`.
    fn try_elaborate_array_like_errors(&mut self, c: &mut Checker, source: TypeId, target: TypeId, report_errors: bool) -> bool {
        // The spec for elaboration is (Go doc comment, verbatim there).
        if is_tuple_type(c, source) {
            if tuple_type_readonly(c, source) && is_mutable_array_or_tuple(c, target) {
                if report_errors {
                    let __ts0 = type_to_string(c, source);
                    let __ts1 = type_to_string(c, target);
                    self.report_error(c, &msg::THE_TYPE_0_IS_READONLY_AND_CANNOT_BE_ASSIGNED_TO_THE_MUTABLE_TYPE_1, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1)]);
                }
                return false;
            }
            return is_array_or_tuple_type(c, target);
        }
        if is_readonly_array_type(c, source) && is_mutable_array_or_tuple(c, target) {
            if report_errors {
                let __ts0 = type_to_string(c, source);
                let __ts1 = type_to_string(c, target);
                self.report_error(c, &msg::THE_TYPE_0_IS_READONLY_AND_CANNOT_BE_ASSIGNED_TO_THE_MUTABLE_TYPE_1, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1)]);
            }
            return false;
        }
        if is_tuple_type(c, target) {
            return is_array_type(c, source);
        }
        true
    }

    /// Go: `func (r *Relater) tryElaborateErrorsForPrimitivesAndObjects(...)`.
    fn try_elaborate_errors_for_primitives_and_objects(&mut self, c: &mut Checker, source: TypeId, target: TypeId) {
        if (c.global_string_type == Some(source) && Some(target) == c.string_type)
            || (c.global_number_type == Some(source) && Some(target) == c.number_type)
            || (c.global_boolean_type == Some(source) && Some(target) == Some(boolean_type_placeholder(c)))
            || (get_global_es_symbol_type(c) == Some(source) && Some(target) == c.es_symbol_type)
        {
            let __ts0 = type_to_string(c, target);
            let __ts1 = type_to_string(c, source);
            self.report_error(c, &msg::X_0_IS_A_PRIMITIVE_BUT_1_IS_A_WRAPPER_OBJECT_PREFER_USING_0_WHEN_POSSIBLE, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1)]);
        }
    }

    /// Go: `func (r *Relater) propertiesIdenticalTo(...) Ternary`.
    fn properties_identical_to(&mut self, c: &mut Checker, source: TypeId, target: TypeId, excluded_properties: &FxHashSet<String>) -> Ternary {
        if !c.type_(source).flags.intersects(TypeFlags::OBJECT) || !c.type_(target).flags.intersects(TypeFlags::OBJECT) {
            return Ternary::False;
        }
        let __lift33 = get_properties_of_object_type(c, source);
        let source_properties = exclude_properties(c, __lift33, excluded_properties);
        let __lift34 = get_properties_of_object_type(c, target);
        let target_properties = exclude_properties(c, __lift34, excluded_properties);
        if source_properties.len() != target_properties.len() {
            return Ternary::False;
        }
        let mut result = Ternary::True;
        for &source_prop in &source_properties {
            let name = c.program_symbol(source_prop).name.clone();
            let Some(target_prop) = get_property_of_object_type(c, target, &name) else {
                return Ternary::False;
            };
            let related = compare_properties(c, source_prop, target_prop, TypeComparerKind::RelatedToSimple, Some(self));
            if related == Ternary::False {
                return Ternary::False;
            }
            result = ternary_and(result, related);
        }
        result
    }

    /// Go: `func (r *Relater) signaturesRelatedTo(...) Ternary`.
    fn signatures_related_to(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        kind: SignatureKind,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if self.relation == RelationKind::Identity {
            return self.signatures_identical_to(c, source, target, kind);
        }
        // With respect to signatures, the anyFunctionType wildcard (Go
        // comment, verbatim).
        if any_function_type(c) == Some(source) {
            return Ternary::True;
        }
        if any_function_type(c) == Some(target) {
            return Ternary::False;
        }
        let source_signatures = get_signatures_of_type(c, source, kind);
        let target_signatures = get_signatures_of_type(c, target, kind);
        if kind == SignatureKind::Construct && !source_signatures.is_empty() && !target_signatures.is_empty() {
            let source_is_abstract = sig(c, source_signatures[0]).flags.intersects(crate::types::SignatureFlags::ABSTRACT);
            let target_is_abstract = sig(c, target_signatures[0]).flags.intersects(crate::types::SignatureFlags::ABSTRACT);
            if source_is_abstract && !target_is_abstract {
                // An abstract constructor type is not assignable (Go
                // comment, verbatim).
                if report_errors {
                    self.report_error(c, &msg::CANNOT_ASSIGN_AN_ABSTRACT_CONSTRUCTOR_TYPE_TO_A_NON_ABSTRACT_CONSTRUCTOR_TYPE, &[]);
                }
                return Ternary::False;
            }
            if !self.constructor_visibilities_are_compatible(c, source_signatures[0], target_signatures[0], report_errors) {
                return Ternary::False;
            }
        }
        let mut result = Ternary::True;
        if (c.type_(source).object_flags.intersects(ObjectFlags::INSTANTIATED)
            && c.type_(target).object_flags.intersects(ObjectFlags::INSTANTIATED)
            && c.type_(source).symbol == c.type_(target).symbol)
            || (c.type_(source).object_flags.intersects(ObjectFlags::REFERENCE)
                && c.type_(target).object_flags.intersects(ObjectFlags::REFERENCE)
                && type_reference_target(c, source) == type_reference_target(c, target))
        {
            // We have instantiations of the same anonymous type (Go comment,
            // verbatim).
            for i in 0..target_signatures.len() {
                let related = self.signature_related_to(c, source_signatures[i], target_signatures[i], true /*erase*/, report_errors, intersection_state);
                if related == Ternary::False {
                    return Ternary::False;
                }
                result = ternary_and(result, related);
            }
        } else if source_signatures.len() == 1 && target_signatures.len() == 1 {
            // For simple functions (Go comment, verbatim).
            let erase_generics = self.relation == RelationKind::Comparable;
            result = self.signature_related_to(c, source_signatures[0], target_signatures[0], erase_generics, report_errors, intersection_state);
        } else {
            'outer: for &t in &target_signatures {
                let save_error_state = self.get_error_state();
                // Only elaborate errors from the first failure (Go comment,
                // verbatim).
                let mut should_elaborate_errors = report_errors;
                for &s in &source_signatures {
                    let related = self.signature_related_to(c, s, t, true /*erase*/, should_elaborate_errors, intersection_state);
                    if related != Ternary::False {
                        result = ternary_and(result, related);
                        self.restore_error_state(save_error_state);
                        continue 'outer;
                    }
                    should_elaborate_errors = false;
                }
                if should_elaborate_errors {
                    let __ts0 = type_to_string(c, source);
                    let __ts1 = signature_to_string(c, t);
                    self.report_error(c, &msg::TYPE_0_PROVIDES_NO_MATCH_FOR_THE_SIGNATURE_1, &[DiagArg::Str(__ts0), DiagArg::Str(__ts1)]);
                }
                return Ternary::False;
            }
        }
        result
    }

    /// Go: `func (r *Relater) constructorVisibilitiesAreCompatible(...) bool`.
    fn constructor_visibilities_are_compatible(&mut self, c: &mut Checker, source_signature: crate::types::SignatureId, target_signature: crate::types::SignatureId, report_errors: bool) -> bool {
        let (source_declaration, target_declaration) = (sig(c, source_signature).declaration, sig(c, target_signature).declaration);
        let (Some(source_declaration), Some(target_declaration)) = (source_declaration, target_declaration) else {
            return true;
        };
        let source_accessibility = ModifierFlags(modifier_flags_of_node(c, source_declaration).0 & ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER.0);
        let target_accessibility = ModifierFlags(modifier_flags_of_node(c, target_declaration).0 & ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER.0);
        // A public, protected and private signature is assignable to a
        // private signature (Go comment, verbatim).
        if target_accessibility == ModifierFlags::PRIVATE {
            return true;
        }
        // A public and protected signature is assignable to a protected
        // signature.
        if target_accessibility == ModifierFlags::PROTECTED && source_accessibility != ModifierFlags::PRIVATE {
            return true;
        }
        // Only a public signature is assignable to public signature.
        if target_accessibility != ModifierFlags::PROTECTED && source_accessibility == ModifierFlags::NONE {
            return true;
        }
        if report_errors {
            self.report_error(c, &msg::CANNOT_ASSIGN_A_0_CONSTRUCTOR_TYPE_TO_A_1_CONSTRUCTOR_TYPE, &[DiagArg::Str(visibility_to_string(source_accessibility).to_string()), DiagArg::Str(visibility_to_string(target_accessibility).to_string())]);
        }
        false
    }

    /// Go: `func (r *Relater) signatureRelatedTo(...) Ternary` (see
    /// signatureAssignableTo, compareSignaturesIdentical).
    fn signature_related_to(
        &mut self,
        c: &mut Checker,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
        erase: bool,
        report_errors: bool,
        _intersection_state: IntersectionState,
    ) -> Ternary {
        let mut check_mode = SignatureCheckMode::NONE;
        if self.relation == RelationKind::Subtype {
            check_mode = SignatureCheckMode::STRICT_TOP_SIGNATURE;
        } else if self.relation == RelationKind::StrictSubtype {
            check_mode = SignatureCheckMode::STRICT_TOP_SIGNATURE | SignatureCheckMode::STRICT_ARITY;
        }
        let source = if erase { get_erased_signature(c, source) } else { source };
        let target = if erase { get_erased_signature(c, target) } else { target };
        let unreliable_mapper = report_unreliable_mapper(c);
        c.compare_signatures_related(source, target, check_mode, report_errors, Some(ErrorReporterKind::Relater), TypeComparerKind::RelatedToWorker, unreliable_mapper)
    }

    /// Go: `func (r *Relater) signaturesIdenticalTo(...) Ternary`.
    fn signatures_identical_to(&mut self, c: &mut Checker, source: TypeId, target: TypeId, kind: SignatureKind) -> Ternary {
        let source_signatures = get_signatures_of_type(c, source, kind);
        let target_signatures = get_signatures_of_type(c, target, kind);
        if source_signatures.len() != target_signatures.len() {
            return Ternary::False;
        }
        let mut result = Ternary::True;
        for i in 0..source_signatures.len() {
            let related = c.compare_signatures_identical(source_signatures[i], target_signatures[i], false, false, false, TypeComparerKind::RelatedToSimple);
            if related == Ternary::False {
                return Ternary::False;
            }
            result = ternary_and(result, related);
        }
        result
    }

    /// Go: `func (r *Relater) indexSignaturesRelatedTo(...) Ternary`.
    fn index_signatures_related_to(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        source_is_primitive: bool,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if self.relation == RelationKind::Identity {
            return self.index_signatures_identical_to(c, source, target);
        }
        let index_infos = get_index_infos_of_type(c, target);
        let target_has_string_index = index_infos.iter().any(|&info| c.index_info_key_type(info) == c.string_type());
        let mut result = Ternary::True;
        for &target_info in &index_infos {
            
            let related = if self.relation != RelationKind::StrictSubtype
                && !source_is_primitive
                && target_has_string_index
                && c.type_(c.index_info_value_type(target_info)).flags.intersects(TypeFlags::ANY)
            {
                Ternary::True
            } else if is_generic_mapped_type(c, source) && target_has_string_index {
                let __lift35 = get_template_type_from_mapped_type(c, source);
                self.is_related_to(c, __lift35, c.index_info_value_type(target_info), RecursionFlags::BOTH, report_errors)
            } else {
                self.type_related_to_index_info(c, source, target_info, report_errors, intersection_state)
            };
            if related == Ternary::False {
                return Ternary::False;
            }
            result = ternary_and(result, related);
        }
        result
    }

    /// Go: `func (r *Relater) typeRelatedToIndexInfo(...) Ternary`.
    fn type_related_to_index_info(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target_info: crate::types::IndexInfoId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if let Some(source_info) = get_applicable_index_info(c, source, c.index_info_key_type(target_info)) {
            return self.index_info_related_to(c, source_info, target_info, report_errors, intersection_state);
        }
        // Intersection constituents are never considered to have an inferred
        // index signature (Go comment, verbatim).
        if !intersection_state.intersects(IntersectionState::SOURCE)
            && (self.relation != RelationKind::StrictSubtype
                || c.type_(source).object_flags.intersects(ObjectFlags::FRESH_LITERAL))
            && is_object_type_with_inferable_index(c, source)
        {
            return self.members_related_to_index_info(c, source, target_info, report_errors, intersection_state);
        }
        if report_errors {
            let __lift37 = type_to_string(c, c.index_info_key_type(target_info));
            let __lift36 = type_to_string(c, source);
            self.report_error(c, &msg::INDEX_SIGNATURE_FOR_TYPE_0_IS_MISSING_IN_TYPE_1, &[DiagArg::Str(__lift37), DiagArg::Str(__lift36)]);
        }
        Ternary::False
    }

    /// Go: `func (r *Relater) membersRelatedToIndexInfo(...) Ternary`.
    fn members_related_to_index_info(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target_info: crate::types::IndexInfoId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut result = Ternary::True;
        let key_type = c.index_info_key_type(target_info);
        let props = if c.type_(source).flags.intersects(TypeFlags::INTERSECTION) {
            get_properties_of_union_or_intersection_type(c, source)
        } else {
            get_properties_of_object_type(c, source)
        };
        for prop in props {
            // Skip over ignored JSX and symbol-named members.
            if is_ignored_jsx_property(c, source, prop) {
                continue;
            }
            let __lift38 = get_literal_type_from_property(c, prop, TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE, false);
            if is_applicable_index_type(c, __lift38, key_type) {
                let prop_type = get_non_missing_type_of_symbol(c, prop);
                let t = if c.exact_optional_property_types
                    || c.type_(prop_type).flags.intersects(TypeFlags::UNDEFINED)
                    || key_type == c.number_type()
                    || !c.program_symbol(prop).flags.intersects(SymbolFlags::OPTIONAL)
                {
                    prop_type
                } else {
                    get_type_with_facts(c, prop_type, TypeFacts::NE_UNDEFINED)
                };
                let related = self.is_related_to_ex(c, t, c.index_info_value_type(target_info), RecursionFlags::BOTH, report_errors, None, intersection_state);
                if related == Ternary::False {
                    if report_errors {
                        let __lift39 = symbol_to_string(c, prop);
                        self.report_error(c, &msg::PROPERTY_0_IS_INCOMPATIBLE_WITH_INDEX_SIGNATURE, &[DiagArg::Str(__lift39)]);
                    }
                    return Ternary::False;
                }
                result = ternary_and(result, related);
            }
        }
        for info in get_index_infos_of_type(c, source) {
            if is_applicable_index_type(c, Some(c.index_info_key_type(info)), key_type) {
                let related = self.index_info_related_to(c, info, target_info, report_errors, intersection_state);
                if related == Ternary::False {
                    return Ternary::False;
                }
                result = ternary_and(result, related);
        }
        }
        result
    }

    /// Go: `func (r *Relater) indexInfoRelatedTo(...) Ternary`.
    fn index_info_related_to(
        &mut self,
        c: &mut Checker,
        source_info: crate::types::IndexInfoId,
        target_info: crate::types::IndexInfoId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let related = self.is_related_to_ex(c, c.index_info_value_type(source_info), c.index_info_value_type(target_info), RecursionFlags::BOTH, report_errors, None, intersection_state);
        if related == Ternary::False && report_errors {
            if c.index_info_key_type(source_info) == c.index_info_key_type(target_info) {
                let __lift40 = type_to_string(c, c.index_info_key_type(source_info));
                self.report_error(c, &msg::X_0_INDEX_SIGNATURES_ARE_INCOMPATIBLE, &[DiagArg::Str(__lift40)]);
            } else {
                let __lift42 = type_to_string(c, c.index_info_key_type(source_info));
                let __lift41 = type_to_string(c, c.index_info_key_type(target_info));
                self.report_error(c, &msg::X_0_AND_1_INDEX_SIGNATURES_ARE_INCOMPATIBLE, &[DiagArg::Str(__lift42), DiagArg::Str(__lift41)]);
            }
        }
        related
    }

    /// Go: `func (r *Relater) indexSignaturesIdenticalTo(...) Ternary`.
    fn index_signatures_identical_to(&mut self, c: &mut Checker, source: TypeId, target: TypeId) -> Ternary {
        let source_infos = get_index_infos_of_type(c, source);
        let target_infos = get_index_infos_of_type(c, target);
        if source_infos.len() != target_infos.len() {
            return Ternary::False;
        }
        for &target_info in &target_infos {
            let Some(source_info) = get_index_info_of_type(c, source, c.index_info_key_type(target_info)) else {
                return Ternary::False;
            };
            if !(ternary_not_false(self.is_related_to(c, c.index_info_value_type(source_info), c.index_info_value_type(target_info), RecursionFlags::BOTH, false))
                && c.index_info_is_readonly(source_info) == c.index_info_is_readonly(target_info))
            {
                return Ternary::False;
            }
        }
        Ternary::True
    }

    /// Go: `func (r *Relater) reportErrorResults(...)`.
    fn report_error_results(
        &mut self,
        c: &mut Checker,
        original_source: TypeId,
        original_target: TypeId,
        source: TypeId,
        target: TypeId,
        head_message: Option<&'static Message>,
    ) {
        let mut source = source;
 let mut target = target;
        let source_has_base = get_single_base_for_non_augmenting_subtype(c, original_source).is_some();
        let target_has_base = get_single_base_for_non_augmenting_subtype(c, original_target).is_some();
        if c.type_(original_source).alias.is_some() || source_has_base {
            source = original_source;
        }
        if c.type_(original_target).alias.is_some() || target_has_base {
            target = original_target;
        }
        if c.type_(source).flags.intersects(TypeFlags::OBJECT) && c.type_(target).flags.intersects(TypeFlags::OBJECT) {
            self.try_elaborate_array_like_errors(c, source, target, true /*reportErrors*/);
        }
        let s_flags = c.type_(source).flags;
        let t_flags = c.type_(target).flags;
        if s_flags.intersects(TypeFlags::OBJECT) && t_flags.intersects(TypeFlags::PRIMITIVE) {
            self.try_elaborate_errors_for_primitives_and_objects(c, source, target);
        } else if c.type_(source).symbol.is_some() && s_flags.intersects(TypeFlags::OBJECT) && c.global_object_type == Some(source) {
            self.report_error(c, &msg::THE_OBJECT_TYPE_IS_ASSIGNABLE_TO_VERY_FEW_OTHER_TYPES_DID_YOU_MEAN_TO_USE_THE_ANY_TYPE_INSTEAD, &[]);
        } else if c.type_(source).object_flags.intersects(ObjectFlags::JSX_ATTRIBUTES) && t_flags.intersects(TypeFlags::INTERSECTION) {
            // TODO(port M6 W5 jsx): the JSX IntrinsicAttributes check.
        } else if c.type_(original_target).flags.intersects(TypeFlags::INTERSECTION)
            && c.type_(original_target).object_flags.intersects(ObjectFlags::IS_NEVER_INTERSECTION)
        {
            // TODO(port M6 S7): the was-reduced-to-never elaborations
            // (isDiscriminantWithNeverType / isConflictingPrivateProperty).
        }
        self.report_relation_error(c, head_message, source, target);
        if s_flags.intersects(TypeFlags::TYPE_PARAMETER)
            && c.type_(source).symbol.is_some()
            && !c.program_symbol(c.type_(source).symbol.expect("checked")).declarations.is_empty()
            && get_constraint_of_type(c, source).is_none()
        {
            let synthetic_param = clone_type_parameter(c, source);
            let mapper = new_simple_type_mapper(c, source, synthetic_param);
            let __lift43 = instantiate_type_ex(c, target, Some(mapper));
            set_type_parameter_constraint(c, synthetic_param, __lift43);
            if has_non_circular_base_constraint(c, synthetic_param) {
                let target_constraint_string = type_to_string(c, target);
                let decl = c.program_symbol(c.type_(source).symbol.expect("checked")).declarations[0];
                self.related_info.push(diagnostic_for_node(c, Some(decl), &msg::THIS_TYPE_PARAMETER_MIGHT_NEED_AN_EXTENDS_0_CONSTRAINT, &[DiagArg::Str(target_constraint_string)]));
            }
        }
    }

    /// Go: `func (r *Relater) reportRelationError(message, source, target)`.
    fn report_relation_error(&mut self, c: &mut Checker, message: Option<&'static Message>, source: TypeId, target: TypeId) {
        let (source_type, target_type) = c.get_type_names_for_error_display(source, target);
        let mut generalized_source = source;
        let mut generalized_source_type = source_type.clone();
        // Don't generalize on 'never' (Go comment, verbatim).
        if !c.type_(target).flags.intersects(TypeFlags::NEVER)
            && is_literal_type(c, source)
            && !c.type_could_have_top_level_singleton_types(target)
        {
            generalized_source = get_base_type_of_literal_type(c, source);
            generalized_source_type = c.get_type_name_for_error_display(generalized_source);
        }
        // If `target` is of indexed access type (Go comment, verbatim).
        let target_flags = if c.type_(target).flags.intersects(TypeFlags::INDEXED_ACCESS)
            && !c.type_(source).flags.intersects(TypeFlags::INDEXED_ACCESS)
        {
            c.type_(indexed_access_parts(c, target).0).flags
        } else {
            c.type_(target).flags
        };
        if target_flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let constraint = get_base_constraint_of_type(c, target);
            if is_distributed_type_parameter(c, target)
                && get_constraint_of_type_parameter(c, target).is_some()
                && c.is_type_assignable_to(generalized_source, get_constraint_of_type_parameter(c, target).expect("checked"))
            {
                self.report_error(c, &msg::X_0_IS_ONLY_ASSIGNABLE_TO_THE_NON_DISTRIBUTED_1_BUT_1_HAS_BEEN_DISTRIBUTED_HERE, &[DiagArg::Str(generalized_source_type.clone()), DiagArg::Str(target_type.clone())]);
            } else if constraint.is_some_and(|constraint| c.is_type_assignable_to(generalized_source, constraint)) {
                let __lift44 = type_to_string(c, constraint.expect("checked"));
                self.report_error(c, &msg::X_0_IS_ASSIGNABLE_TO_THE_CONSTRAINT_OF_TYPE_1_BUT_1_COULD_BE_INSTANTIATED_WITH_A_DIFFERENT_SUBTYPE_OF_CONSTRAINT_2, &[DiagArg::Str(generalized_source_type.clone()), DiagArg::Str(target_type.clone()), DiagArg::Str(__lift44)]);
            } else if constraint.is_some_and(|constraint| c.is_type_assignable_to(source, constraint)) {
                let __lift45 = type_to_string(c, constraint.expect("checked"));
                self.report_error(c, &msg::X_0_IS_ASSIGNABLE_TO_THE_CONSTRAINT_OF_TYPE_1_BUT_1_COULD_BE_INSTANTIATED_WITH_A_DIFFERENT_SUBTYPE_OF_CONSTRAINT_2, &[DiagArg::Str(source_type.clone()), DiagArg::Str(target_type.clone()), DiagArg::Str(__lift45)]);
            } else {
                self.error_chain = None; // Only report this error once
                self.report_error(c, &msg::X_0_COULD_BE_INSTANTIATED_WITH_AN_ARBITRARY_TYPE_WHICH_COULD_BE_UNRELATED_TO_1, &[DiagArg::Str(target_type.clone()), DiagArg::Str(generalized_source_type.clone())]);
            }
        }
        let mut message = message;
        if message.is_none() {
            if self.relation == RelationKind::Comparable {
                message = Some(&msg::TYPE_0_IS_NOT_COMPARABLE_TO_TYPE_1);
            } else if source_type == target_type {
                message = Some(&msg::TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1_TWO_DIFFERENT_TYPES_WITH_THIS_NAME_EXIST_BUT_THEY_ARE_UNRELATED);
            } else if c.exact_optional_property_types && !get_exact_optional_unassignable_properties(c, source, target).is_empty() {
                message = Some(&msg::TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1_WITH_EXACTOPTIONALPROPERTYTYPES_COLON_TRUE_CONSIDER_ADDING_UNDEFINED_TO_THE_TYPES_OF_THE_TARGET_S_PROPERTIES);
            } else {
                if c.type_(source).flags.intersects(TypeFlags::STRING_LITERAL) && c.type_(target).flags.intersects(TypeFlags::UNION) {
                    // TODO(port M6 S6): getSuggestedTypeForNonexistentStringLiteralType
                    // — the Did-you-mean-2 elaboration.
                }
                message = Some(&msg::TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1);
            }
        } else if message.is_some_and(|m| std::ptr::eq(m, &msg::ARGUMENT_OF_TYPE_0_IS_NOT_ASSIGNABLE_TO_PARAMETER_OF_TYPE_1))
            && c.exact_optional_property_types
&& !get_exact_optional_unassignable_properties(c, source, target).is_empty()
        {
            message = Some(&msg::ARGUMENT_OF_TYPE_0_IS_NOT_ASSIGNABLE_TO_PARAMETER_OF_TYPE_1_WITH_EXACTOPTIONALPROPERTYTYPES_COLON_TRUE_CONSIDER_ADDING_UNDEFINED_TO_THE_TYPES_OF_THE_TARGET_S_PROPERTIES);
        }
        // Suppress if next message is (Go comments, verbatim).
        match self.get_chain_message(0) {
            Some(m) if std::ptr::eq(m, &msg::OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_AND_0_DOES_NOT_EXIST_IN_TYPE_1) || std::ptr::eq(m, &msg::OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_BUT_0_DOES_NOT_EXIST_IN_TYPE_1_DID_YOU_MEAN_TO_WRITE_2) => return,
            Some(m) if std::ptr::eq(m, &msg::EXCESSIVE_COMPLEXITY_COMPARING_TYPES_0_AND_1) || std::ptr::eq(m, &msg::THE_TYPE_0_IS_READONLY_AND_CANNOT_BE_ASSIGNED_TO_THE_MUTABLE_TYPE_1) => {
                if self.chain_args_match(&[Some(&DiagArg::Str(generalized_source_type.clone())), Some(&DiagArg::Str(target_type.clone()))]) {
                    return;
                }
            }
            Some(m) if std::ptr::eq(m, &msg::PROPERTY_0_IS_MISSING_IN_TYPE_1_BUT_REQUIRED_IN_TYPE_2) => {
                if !message.is_some_and(is_conversion_or_interface_implementation_message)
                    && self.chain_args_match(&[None, Some(&DiagArg::Str(generalized_source_type.clone())), Some(&DiagArg::Str(target_type.clone()))])
                {
                    return;
                }
            }
            Some(m) if (std::ptr::eq(m, &msg::TYPE_0_IS_MISSING_THE_FOLLOWING_PROPERTIES_FROM_TYPE_1_COLON_2_AND_3_MORE) || std::ptr::eq(m, &msg::TYPE_0_IS_MISSING_THE_FOLLOWING_PROPERTIES_FROM_TYPE_1_COLON_2))
                && !message.is_some_and(is_conversion_or_interface_implementation_message)
                    && self.chain_args_match(&[Some(&DiagArg::Str(generalized_source_type.clone())), Some(&DiagArg::Str(target_type.clone()))])
                => {
                    return;
                }
            _ => {}
        }
        self.report_error(c, message.expect("message set above"), &[DiagArg::Str(generalized_source_type.clone()), DiagArg::Str(target_type.clone())]);
    }

    /// Go: `func (r *Relater) reportError(message, args...)` — the error-chain
    /// transformations (the `Types_of_property_0_are_incompatible` pyramid
    /// rewriting rules).
    #[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)] // Go mirrors the variadic report chain
    fn report_error(&mut self, c: &mut Checker, message: &'static Message, args: &[DiagArg]) {
        if std::ptr::eq(message, &msg::TYPES_OF_PROPERTY_0_ARE_INCOMPATIBLE) {
            // Suppress if next message is an excess property error.
            match self.get_chain_message(0) {
                Some(m) if std::ptr::eq(m, &msg::OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_AND_0_DOES_NOT_EXIST_IN_TYPE_1) || std::ptr::eq(m, &msg::OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_BUT_0_DOES_NOT_EXIST_IN_TYPE_1_DID_YOU_MEAN_TO_WRITE_2) => return,
                _ => {}
            }
            // Transform a property incompatibility message for property 'x'
            // followed by some elaboration message followed by a signature
            // return type incompatibility message (Go comment, verbatim).
            let mut message = message;
            let mut args: Vec<DiagArg> = args.to_vec();
            let mut arg = String::new();
            match self.get_chain_message(1) {
                Some(m) if std::ptr::eq(m, &msg::CALL_SIGNATURES_WITH_NO_ARGUMENTS_HAVE_INCOMPATIBLE_RETURN_TYPES_0_AND_1) => {
                    arg = get_property_name_arg(&args[0]) + "()";
                }
                Some(m) if std::ptr::eq(m, &msg::CONSTRUCT_SIGNATURES_WITH_NO_ARGUMENTS_HAVE_INCOMPATIBLE_RETURN_TYPES_0_AND_1) => {
                    arg = "new ".to_string() + &get_property_name_arg(&args[0]) + "()";
                }
                Some(m) if std::ptr::eq(m, &msg::CALL_SIGNATURE_RETURN_TYPES_0_AND_1_ARE_INCOMPATIBLE) => {
                    arg = get_property_name_arg(&args[0]) + "(...)";
                }
                Some(m) if std::ptr::eq(m, &msg::CONSTRUCT_SIGNATURE_RETURN_TYPES_0_AND_1_ARE_INCOMPATIBLE) => {
                    arg = "new ".to_string() + &get_property_name_arg(&args[0]) + "(...)";
                }
                _ => {}
            }
            if !arg.is_empty() {
                message = &msg::THE_TYPES_RETURNED_BY_0_ARE_INCOMPATIBLE_BETWEEN_THESE_TYPES;
                args[0] = DiagArg::Str(arg);
                if let Some(chain) = self.error_chain.as_mut() {
                    // r.errorChain = r.errorChain.next.next
                    let next_next = chain.next.as_deref().and_then(|n| n.next.clone());
                    chain.next = next_next;
                }
            }
            // Transform a property incompatibility message for property 'x'
            // followed by some elaboration message followed by a property
            // incompatibility message for property 'y' (Go comment,
            // verbatim).
            match self.get_chain_message(1) {
                Some(m) if std::ptr::eq(m, &msg::TYPES_OF_PROPERTY_0_ARE_INCOMPATIBLE) || std::ptr::eq(m, &msg::THE_TYPES_OF_0_ARE_INCOMPATIBLE_BETWEEN_THESE_TYPES) || std::ptr::eq(m, &msg::THE_TYPES_RETURNED_BY_0_ARE_INCOMPATIBLE_BETWEEN_THESE_TYPES) => {
                    let head = get_property_name_arg(&args[0]);
                    let tail = self
                        .error_chain
                        .as_ref()
                        .and_then(|e| e.next.as_ref())
                        .and_then(|n| n.args.first())
                        .map(get_property_name_arg)
                        .unwrap_or_default();
                    let arg = add_to_dotted_name(&head, &tail);
                    if let Some(chain) = self.error_chain.as_mut() {
                        let next_next = chain.next.as_deref().and_then(|n| n.next.clone());
                        chain.next = next_next;
                    }
                    if std::ptr::eq(message, &msg::TYPES_OF_PROPERTY_0_ARE_INCOMPATIBLE) {
                        message = &msg::THE_TYPES_OF_0_ARE_INCOMPATIBLE_BETWEEN_THESE_TYPES;
                    }
                    self.report_error(c, message, &[DiagArg::Str(arg)]);
                    return;
                }
                _ => {}
            }
            self.error_chain = Some(Box::new(ErrorChain {
                next: self.error_chain.take(),
                message,
                args,
            }));
            return;
        }
        self.error_chain = Some(Box::new(ErrorChain {
            next: self.error_chain.take(),
            message,
            args: args.to_vec(),
        }));
    }

    /// Go: `func (r *Relater) getChainMessage(index) *diagnostics.Message`.
    fn get_chain_message(&self, mut index: usize) -> Option<&'static Message> {
        let mut e = self.error_chain.as_deref();
        loop {
            let chain = e?;
            if index == 0 {
                return Some(chain.message);
            }
            e = chain.next.as_deref();
            index -= 1;
        }
    }

    /// Go: `func (r *Relater) chainArgsMatch(args...) bool` — nil acts as a
    /// wildcard.
    fn chain_args_match(&self, args: &[Option<&DiagArg>]) -> bool {
        let Some(chain) = self.error_chain.as_ref() else { return false };
        for (i, a) in args.iter().enumerate() {
            if let Some(a) = a {
                if chain.args.get(i) != Some(a) {
                    return false;
                }
            }
        }
        true
    }
}

/// Go: `func addToDottedName(head, tail) string`.
fn add_to_dotted_name(head: &str, tail: &str) -> String {
    let head = if head.starts_with("new ") { format!("({head})") } else { head.to_string() };
    let mut pos = 0;
    loop {
        if tail[pos..].starts_with('(') {
            pos += 1;
        } else if tail[pos..].starts_with("new ") {
            pos += 4;
        } else {
            break;
        }
    }
    let prefix = &tail[..pos];
    let suffix = &tail[pos..];
    if suffix.starts_with('[') {
        format!("{prefix}{head}{suffix}")
    } else {
        format!("{prefix}{head}.{suffix}")
    }
}

/// Go: `func getPropertyNameArg(arg) string`.
fn get_property_name_arg(arg: &DiagArg) -> String {
    let s = match arg {
        DiagArg::Str(s) => s.clone(),
        _ => String::new(),
    };
    if !s.is_empty() && (s.starts_with('"') || s.starts_with('\'') || s.starts_with('`')) {
        format!("[{s}]")
    } else {
        s
    }
}

/// Go: `func isConversionOrInterfaceImplementationMessage(message) bool`.
fn is_conversion_or_interface_implementation_message(message: &Message) -> bool {
    std::ptr::eq(message, &msg::CLASS_0_INCORRECTLY_IMPLEMENTS_INTERFACE_1)
        || std::ptr::eq(message, &msg::CLASS_0_INCORRECTLY_IMPLEMENTS_CLASS_1_DID_YOU_MEAN_TO_EXTEND_1_AND_INHERIT_ITS_MEMBERS_AS_A_SUBCLASS)
        || std::ptr::eq(message, &msg::CONVERSION_OF_TYPE_0_TO_TYPE_1_MAY_BE_A_MISTAKE_BECAUSE_NEITHER_TYPE_SUFFICIENTLY_OVERLAPS_WITH_THE_OTHER_IF_THIS_WAS_INTENTIONAL_CONVERT_THE_EXPRESSION_TO_UNKNOWN_FIRST)
        || std::ptr::eq(message, &msg::ITS_INSTANCE_TYPE_0_IS_NOT_A_VALID_JSX_ELEMENT)
        || std::ptr::eq(message, &msg::ITS_RETURN_TYPE_0_IS_NOT_A_VALID_JSX_ELEMENT)
        || std::ptr::eq(message, &msg::ITS_ELEMENT_TYPE_0_IS_NOT_A_VALID_JSX_ELEMENT)
}

/// Go: `func chainDepth(chain *ErrorChain) int`.
fn chain_depth(chain: Option<&ErrorChain>) -> usize {
    let mut depth = 0;
    let mut chain = chain;
    while let Some(c) = chain {
        depth += 1;
        chain = c.next.as_deref();
    }
    depth
}

/// Go: `func (c *Checker) isObjectTypeWithInferableIndex(t) bool` — the
/// crate-public form (inference.rs's inferFromIndexTypes is the W2 caller).
pub(crate) fn is_object_type_with_inferable_index_pub(c: &mut Checker, t: TypeId) -> bool {
    is_object_type_with_inferable_index(c, t)
}

/// Go: `func (c *Checker) isObjectTypeWithInferableIndex(t) bool`.
fn is_object_type_with_inferable_index(c: &mut Checker, t: TypeId) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
        let constituents: Vec<TypeId> = c.type_(t).types().to_vec();
        return constituents.iter().all(|&t| is_object_type_with_inferable_index(c, t));
    }
    let symbol = c.type_(t).symbol;
    let symbol_ok = match symbol {
        Some(symbol) => {
            let flags = c.program_symbol(symbol).flags;
            flags.intersects(SymbolFlags::OBJECT_LITERAL | SymbolFlags::TYPE_LITERAL | SymbolFlags::ENUM | SymbolFlags::VALUE_MODULE)
                && !flags.intersects(SymbolFlags::CLASS)
                && !type_has_call_or_construct_signatures(c, t)
        }
        None => false,
    };
    symbol_ok || c.type_(t).object_flags.intersects(ObjectFlags::JS_LITERAL | ObjectFlags::OBJECT_REST_TYPE)
        || (c.type_(t).object_flags.intersects(ObjectFlags::REVERSE_MAPPED)
            && c
                .type_(t)
                .as_reverse_mapped_type()
                .and_then(|d| d.source)
                .is_some_and(|s| is_object_type_with_inferable_index(c, s)))
}

// ────────────────────────────────────────────────────────────────────────────
// Best-matching-type / discriminant machinery (Go relater.go 872+)
// ────────────────────────────────────────────────────────────────────────────

/// Go: the `isRelatedTo func(*Type, *Type) Ternary` parameter of the
/// getBestMatchingType/findMatchingDiscriminantType family (the call sites
/// pass `r.isRelatedToSimple` or `c.compareTypesAssignableSimple`); the port
/// threads the active relater (None for the checker-level comparers).
fn invoke_comparer(
    c: &mut Checker,
    kind: TypeComparerKind,
    relater: Option<&mut Relater>,
    source: TypeId,
    target: TypeId,
    _report_errors: bool,
) -> Ternary {
    match kind {
        TypeComparerKind::Identical => c.compare_types_identical(source, target),
        TypeComparerKind::Assignable => c.compare_types_assignable_simple(source, target),
        TypeComparerKind::SubtypeOf => c.compare_types_subtype_of(source, target),
        TypeComparerKind::RelatedToSimple | TypeComparerKind::RelatedToWorker => {
            match relater {
                Some(r) => match kind {
                    TypeComparerKind::RelatedToSimple => r.is_related_to_simple(c, source, target),
                    _ => r.is_related_to_worker(c, source, target, false),
                },
                None => {
                    // PORT: unreachable through checker-level entry points
                    // (they construct their own relater); see the module note.
                    if c.check_type_related_to(source, target, RelationKind::Assignable, None) { Ternary::True } else { Ternary::False }
                }
            }
        }
    }
}

/// Go: `func (c *Checker) getBestMatchingType(source, target, isRelatedTo) *Type`.
fn get_best_matching_type(
    c: &mut Checker,
    source: TypeId,
    target: TypeId,
    kind: TypeComparerKind,
    relater: Option<&mut Relater>,
) -> Option<TypeId> {
    if let Some(t) = find_matching_discriminant_type(c, source, target, kind, relater) {
        return Some(t);
    }
    if let Some(t) = find_matching_type_reference_or_type_alias_reference(c, source, target) {
        return Some(t);
    }
    if let Some(t) = find_best_type_for_object_literal(c, source, target) {
        return Some(t);
    }
    if let Some(t) = find_best_type_for_invokable(c, source, target, SignatureKind::Call) {
        return Some(t);
    }
    if let Some(t) = find_best_type_for_invokable(c, source, target, SignatureKind::Construct) {
        return Some(t);
    }
    find_most_overlappy_type(c, source, target)
}

/// Go: `func (c *Checker) findMatchingTypeReferenceOrTypeAliasReference(source, unionTarget) *Type`.
fn find_matching_type_reference_or_type_alias_reference(c: &mut Checker, source: TypeId, union_target: TypeId) -> Option<TypeId> {
    let source_object_flags = c.type_(source).object_flags;
    if source_object_flags.intersects(ObjectFlags::REFERENCE | ObjectFlags::ANONYMOUS)
        && c.type_(union_target).flags.intersects(TypeFlags::UNION)
    {
        for &target in c.type_(union_target).types() {
            if c.type_(target).flags.intersects(TypeFlags::OBJECT) {
                let overlap_obj_flags = ObjectFlags::from_bits(source_object_flags.0 & c.type_(target).object_flags.0);
                if overlap_obj_flags.intersects(ObjectFlags::REFERENCE)
                    && type_reference_target_opt(c, source) == type_reference_target_opt(c, target)
                {
                    return Some(target);
                }
                if overlap_obj_flags.intersects(ObjectFlags::ANONYMOUS)
                    && c.type_(source).alias.is_some()
                    && c.type_(target).alias.is_some()
                    && c.type_(source).alias.as_ref().map(|a| a.symbol) == c.type_(target).alias.as_ref().map(|a| a.symbol)
                {
                    return Some(target);
                }
            }
        }
    }
    None
}

/// Go: `func (c *Checker) findBestTypeForInvokable(source, unionTarget, kind) *Type`.
fn find_best_type_for_invokable(c: &mut Checker, source: TypeId, union_target: TypeId, kind: SignatureKind) -> Option<TypeId> {
    if !get_signatures_of_type(c, source, kind).is_empty() {
        let constituents: Vec<TypeId> = c.type_(union_target).types().to_vec();
        for &t in &constituents {
            if !get_signatures_of_type(c, t, kind).is_empty() {
                return Some(t);
            }
        }
    }
    None
}

/// Go: `func (c *Checker) findMostOverlappyType(source, unionTarget) *Type`.
fn find_most_overlappy_type(c: &mut Checker, source: TypeId, union_target: TypeId) -> Option<TypeId> {
    let mut best_match: Option<TypeId> = None;
    if !c.type_(source).flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::INSTANTIABLE_PRIMITIVE) {
        let mut matching_count = 0usize;
        let constituents: Vec<TypeId> = c.type_(union_target).types().to_vec();
        for &target in &constituents {
            let target_flags = c.type_(target).flags;
            if !target_flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::INSTANTIABLE_PRIMITIVE) {
                let __lift47 = get_index_type(c, source);
                let __lift46 = get_index_type(c, target);
                let overlap = get_intersection_type(c, vec![__lift47, __lift46]);
                if c.type_(overlap).flags.intersects(TypeFlags::INDEX) {
                    // perfect overlap of keys
                    return Some(target);
                } else if is_unit_type(c, overlap) || c.type_(overlap).flags.intersects(TypeFlags::UNION) {
                    // We only want to account for literal types otherwise
                    // (Go comment, verbatim).
                    let length = if c.type_(overlap).flags.intersects(TypeFlags::UNION) {
                        let overlap_types: Vec<TypeId> = c.type_(overlap).types().to_vec();
                        overlap_types.iter().filter(|&&t| is_unit_type(c, t)).count()
                    } else {
                        1
                    };
                    if length >= matching_count {
                        best_match = Some(target);
                        matching_count = length;
                    }
                }
            }
        }
    }
    best_match
}

/// Go: `func (c *Checker) findBestTypeForObjectLiteral(source, unionTarget) *Type`.
fn find_best_type_for_object_literal(c: &mut Checker, source: TypeId, union_target: TypeId) -> Option<TypeId> {
    if c.type_(source).object_flags.intersects(ObjectFlags::OBJECT_LITERAL) {
        let constituents: Vec<TypeId> = c.type_(union_target).types().to_vec();
        if constituents.iter().any(|&t| is_array_like_type(c, t)) {
            for &t in &constituents {
                if !is_array_like_type(c, t) {
                    return Some(t);
                }
            }
        }
    }
    None
}

/// Go: `func (c *Checker) shouldReportUnmatchedPropertyError(source, target) bool`.
fn should_report_unmatched_property_error(c: &mut Checker, source: TypeId, target: TypeId) -> bool {
    let type_call_signatures = get_signatures_of_structured_type(c, source, SignatureKind::Call);
    let type_construct_signatures = get_signatures_of_structured_type(c, source, SignatureKind::Construct);
    let type_properties = get_properties_of_object_type(c, source);
    if (!type_call_signatures.is_empty() || !type_construct_signatures.is_empty()) && type_properties.is_empty() {
        if (!get_signatures_of_type(c, target, SignatureKind::Call).is_empty() && !type_call_signatures.is_empty())
            || (!get_signatures_of_type(c, target, SignatureKind::Construct).is_empty() && !type_construct_signatures.is_empty())
        {
            // target has similar signature kinds to source (Go comment,
            // verbatim).
            return true;
        }
        return false;
    }
    true
}

/// Go: `func (c *Checker) getUnmatchedProperty(source, target, requireOptionalProperties, matchDiscriminantProperties) *ast.Symbol`.
pub(crate) fn get_unmatched_property(c: &mut Checker, source: TypeId, target: TypeId, require_optional_properties: bool, match_discriminant_properties: bool) -> Option<SymbolId> {
    get_unmatched_properties_worker(c, source, target, require_optional_properties, match_discriminant_properties, None)
}

/// Go: `func (c *Checker) getUnmatchedProperties(...) []*ast.Symbol`.
fn get_unmatched_properties(c: &mut Checker, source: TypeId, target: TypeId, require_optional_properties: bool, match_discriminant_properties: bool) -> Vec<SymbolId> {
    let mut props = Vec::new();
    get_unmatched_properties_worker(c, source, target, require_optional_properties, match_discriminant_properties, Some(&mut props));
    props
}

/// Go: `func (c *Checker) getUnmatchedPropertiesWorker(...) *ast.Symbol`.
fn get_unmatched_properties_worker(
    c: &mut Checker,
    source: TypeId,
    target: TypeId,
    require_optional_properties: bool,
    match_discriminant_properties: bool,
    mut props_out: Option<&mut Vec<SymbolId>>,
) -> Option<SymbolId> {
    let properties = get_properties_of_type(c, target);
    for target_prop in properties {
        // TODO: remove this when we support static private identifier fields
        // (Go comment, verbatim).
        if is_static_private_identifier_property(c, target_prop) {
            continue;
        }
        if require_optional_properties
            || (!c.program_symbol(target_prop).flags.intersects(SymbolFlags::OPTIONAL)
                && !c.program_symbol(target_prop).check_flags.intersects(CHECK_FLAGS_PARTIAL))
        {
            let name = c.program_symbol(target_prop).name.clone();
            match get_property_of_type(c, source, &name) {
                None => match props_out.as_deref_mut() {
                    None => return Some(target_prop),
                    Some(out) => out.push(target_prop),
                },
                Some(source_prop) if match_discriminant_properties => {
                    let target_type = get_type_of_symbol(c, target_prop);
                    if c.type_(target_type).flags.intersects(TypeFlags::UNIT) {
                        let source_type = get_type_of_symbol(c, source_prop);
                        if !(c.type_(source_type).flags.intersects(TypeFlags::ANY)
                            || c.get_regular_type_of_literal_type(source_type) == c.get_regular_type_of_literal_type(target_type))
                        {
                            match props_out.as_deref_mut() {
                                None => return Some(target_prop),
                                Some(out) => out.push(target_prop),
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Go: `func excludeProperties(properties, excludedProperties) []*ast.Symbol`.
fn exclude_properties(c: &Checker, properties: Vec<SymbolId>, excluded_properties: &FxHashSet<String>) -> Vec<SymbolId> {
    if excluded_properties.is_empty() || properties.is_empty() {
        return properties;
    }
    let mut reduced: Vec<SymbolId> = Vec::new();
    let mut excluded = false;
    let properties = &properties;
    for (i, &prop) in properties.iter().enumerate() {
        if !excluded_properties.contains(&c.program_symbol(prop).name) {
            if excluded {
                reduced.push(prop);
            }
        } else if !excluded {
            reduced = properties[..i].to_vec();
            excluded = true;
        }
    }
    if excluded {
        return reduced;
    }
    properties.clone()
}

/// Go: `func (c *Checker) findMatchingDiscriminantType(source, target, isRelatedTo) *Type`
/// (keep up-to-date with the same logic in getApparentTypeOfContextualType —
/// Go doc comment, verbatim).
fn find_matching_discriminant_type(
    c: &mut Checker,
    source: TypeId,
    target: TypeId,
    kind: TypeComparerKind,
    relater: Option<&mut Relater>,
) -> Option<TypeId> {
    if c.type_(target).flags.intersects(TypeFlags::UNION)
        && c.type_(source).flags.intersects(TypeFlags::INTERSECTION | TypeFlags::OBJECT)
    {
        if let Some(match_) = get_matching_union_constituent_for_type(c, target, source) {
            return Some(match_);
        }
        let __lift48 = get_properties_of_type(c, source);
        let discriminant_properties = find_discriminant_properties(c, &__lift48, target);
        if !discriminant_properties.is_empty() {
            let discriminated = discriminate_type_by_discriminable_items(c, target, &discriminant_properties, kind, relater);
            if discriminated != Some(target) {
                return discriminated;
            }
        }
    }
    None
}

/// Go: `func (c *Checker) findDiscriminantProperties(sourceProperties, target) []*ast.Symbol`.
fn find_discriminant_properties(c: &mut Checker, source_properties: &[SymbolId], target: TypeId) -> Vec<SymbolId> {
    let mut result = Vec::new();
    for &source_property in source_properties {
        let name = c.program_symbol(source_property).name.clone();
        if is_discriminant_property(c, target, &name) {
            result.push(source_property);
        }
    }
    result
}

/// Go: `func (c *Checker) isDiscriminantProperty(t, name) bool`.
fn is_discriminant_property(c: &mut Checker, t: TypeId, _name: &str) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        // TODO(port M6 S8): getUnionOrIntersectionProperty — the synthetic
        // discriminant CheckFlags computation (nonUniformAndLiteral etc.).
        // Union property resolution lands with resolveStructuredTypeMembers.
        return false;
    }
    false
}

/// Go: `func (c *Checker) getMatchingUnionConstituentForType(unionType, t) *Type`.
fn get_matching_union_constituent_for_type(c: &mut Checker, union_type: TypeId, t: TypeId) -> Option<TypeId> {
    let key_property_name = get_key_property_name(c, union_type)?;
    let prop_type = get_type_of_property_of_type(c, t, &key_property_name)?;
    get_constituent_type_for_key_type(c, union_type, prop_type)
}

/// Go: `func (c *Checker) getKeyPropertyName(t) string` + `computeKeyPropertyNameAndMap`.
///
/// PORT: Go caches `keyPropertyName`/`constituentMap` on the UnionType; the
/// port keeps the cache checker-side (types.rs's UnionType carries no
/// mutation seam for the map), keyed by the union TypeId — same lifetime
/// semantics (union types are interned per checker).
fn get_key_property_name(c: &mut Checker, t: TypeId) -> Option<String> {
    if let Some(cached) = c.key_property_names.get(&t) {
        return cached.clone();
    }
    let computed = compute_key_property_name_and_map(c, t);
    c.key_property_names.insert(t, computed.clone());
    computed
}

/// Go: `func (c *Checker) computeKeyPropertyNameAndMap(t) (string, map[*Type]*Type)`.
fn compute_key_property_name_and_map(c: &mut Checker, t: TypeId) -> Option<String> {
    let types = c.type_(t).types().to_vec();
    if types.len() < 10
        || c.type_(t).object_flags.intersects(ObjectFlags::PRIMITIVE_UNION)
        || types.iter().filter(|&&t| is_object_or_instantiable_non_primitive(c, t)).count() < 10
    {
        return None;
    }
    let key_property_name = get_key_property_candidate_name(c, &types)?;
    map_types_by_key_property(c, &types, &key_property_name)?;
    Some(key_property_name)
}

/// Go: `func isObjectOrInstantiableNonPrimitive(t) bool`.
fn is_object_or_instantiable_non_primitive(c: &Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(TypeFlags::OBJECT | TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
}

/// Go: `func (c *Checker) getKeyPropertyCandidateName(types) string`.
fn get_key_property_candidate_name(c: &mut Checker, types: &[TypeId]) -> Option<String> {
    for &t in types {
        if c.type_(t).flags.intersects(TypeFlags::OBJECT | TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
            for p in get_properties_of_type(c, t) {
                let __lift49 = get_type_of_symbol(c, p);
                if is_unit_type(c, __lift49) {
                    return Some(c.program_symbol(p).name.clone());
                }
            }
        }
    }
    None
}

/// Go: `func (c *Checker) mapTypesByKeyProperty(types, keyPropertyName) map[*Type]*Type`.
fn map_types_by_key_property(c: &mut Checker, types: &[TypeId], key_property_name: &str) -> Option<FxHashMap<TypeId, TypeId>> {
    let mut types_by_key: FxHashMap<TypeId, TypeId> = FxHashMap::default();
    let mut count = 0usize;
    for &t in types {
        if c.type_(t).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
            let discriminant = get_type_of_property_of_type(c, t, key_property_name)?;
            if !is_literal_type(c, discriminant) {
                return None;
            }
            let mut duplicate = false;
            for d in distributed(c, discriminant) {
                let key = c.get_regular_type_of_literal_type(d);
                match types_by_key.get(&key) {
                    None => {
                        types_by_key.insert(key, t);
                    }
                    Some(existing) if *existing != c.unknown_type() => {
                        types_by_key.insert(key, c.unknown_type());
                        duplicate = true;
                    }
                    _ => {}
                }
            }
            if !duplicate {
                count += 1;
            }
        }
    }
    if count >= 10 && count * 2 >= types.len() {
        return Some(types_by_key);
    }
    None
}

/// Go: `func (c *Checker) getConstituentTypeForKeyType(t, keyType) *Type`.
fn get_constituent_type_for_key_type(c: &mut Checker, t: TypeId, key_type: TypeId) -> Option<TypeId> {
    let key = c.get_regular_type_of_literal_type(key_type);
    let key_property_name = get_key_property_name(c, t)?;
    //The map is rebuilt lazily (PORT note: Go caches it on the union).
    let types = c.type_(t).types().to_vec();
    let map = map_types_by_key_property(c, &types, &key_property_name)?;
    match map.get(&key) {
        Some(&result) if result != c.unknown_type() => Some(result),
        _ => None,
    }
}

/// Go: `func (c *Checker) discriminateTypeByDiscriminableItems(target, discriminator) *Type`.
fn discriminate_type_by_discriminable_items(
    c: &mut Checker,
    target: TypeId,
    props: &[SymbolId],
    kind: TypeComparerKind,
    mut relater: Option<&mut Relater>,
) -> Option<TypeId> {
    let types = c.type_(target).types().to_vec();
    let mut include = vec![Ternary::False; types.len()];
    for (i, &t) in types.iter().enumerate() {
        let reduced = get_reduced_type(c, t);
        if !c.type_(t).flags.intersects(TypeFlags::PRIMITIVE) && !c.type_(reduced).flags.intersects(TypeFlags::NEVER) {
            include[i] = Ternary::True;
        }
    }
    let prop_names: Vec<String> = props.iter().map(|&p| c.program_symbol(p).name.clone()).collect();
    for (n, prop) in props.iter().enumerate() {
        // If the remaining target types include at least one with a matching
        // discriminant (Go comment, verbatim).
        let mut matched = false;
        let prop_type = get_type_of_symbol(c, *prop);
        let prop_type_distributed = distributed(c, prop_type);
        for i in 0..types.len() {
            if include[i] != Ternary::False {
                let Some(target_type) = get_type_of_property_or_index_signature_of_type(c, types[i], &prop_names[n]) else {
                    continue;
                };
                let mut is_match = false;
                for &s in &prop_type_distributed {
                    if invoke_comparer(c, kind, relater.as_deref_mut(), s, target_type, false) != Ternary::False {
                        is_match = true;
                        break;
                    }
                }
                if is_match {
                    matched = true;
                } else {
                    include[i] = Ternary::Maybe;
                }
            }
        }
        // Turn each Ternary.Maybe into Ternary.False if there was a match
        // (Go comment, verbatim).
        #[allow(clippy::needless_range_loop)] // Go mirrors the index walk
        for i in 0..types.len() {
            if include[i] == Ternary::Maybe {
                if matched {
                    include[i] = Ternary::False;
                } else {
                    include[i] = Ternary::True;
                }
            }
        }
    }
    if include.contains(&Ternary::False) {
        let filtered_types: Vec<TypeId> = types
            .iter()
            .zip(&include)
            .filter(|(_, inc)| **inc == Ternary::True)
            .map(|(&t, _)| t)
            .collect();
        let filtered = get_union_type_ex(c, filtered_types, crate::todo_shapes::UnionReduction::None, None, None);
        if !c.type_(filtered).flags.intersects(TypeFlags::NEVER) {
            return Some(filtered);
        }
    }
    Some(target)
}

/// Go: `func (c *Checker) filterPrimitivesIfContainsNonPrimitive(unionType) *Type`.
fn filter_primitives_if_contains_non_primitive(c: &mut Checker, union_type: TypeId) -> TypeId {
    if maybe_type_of_kind(c, union_type, TypeFlags::NON_PRIMITIVE) {
        // PORT(note): filter_type's W2 stub only consults the top-level type,
        // so the constituent-level predicate runs against it directly until
        // S7 lands the real reducer (same behavior, borrow-clean).
        let keep = !c.type_(union_type).flags.intersects(TypeFlags::PRIMITIVE);
        let result = if keep { union_type } else { c.never_type() };
        if !c.type_(result).flags.intersects(TypeFlags::NEVER) {
            return result;
        }
    }
    union_type
}

// ────────────────────────────────────────────────────────────────────────────
// Template-literal / string-matching helpers (Go relater.go 2354+)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (c *Checker) templateLiteralTypesDefinitelyUnrelated(source, target) bool`.
fn template_literal_types_definitely_unrelated(_c: &Checker, source_texts: &[String], target_texts: &[String]) -> bool {
    let source_start = &source_texts[0];
    let target_start = &target_texts[0];
    let source_end = &source_texts[source_texts.len() - 1];
    let target_end = &target_texts[target_texts.len() - 1];
    let start_len = source_start.len().min(target_start.len());
    let end_len = source_end.len().min(target_end.len());
    source_start[..start_len] != target_start[..start_len]
        || source_end[source_end.len() - end_len..] != target_end[target_end.len() - end_len..]
}

/// Go: `func (c *Checker) isTypeMatchedByTemplateLiteralType(source, target, compareTypes) bool`.
fn is_type_matched_by_template_literal_type(
    c: &mut Checker,
    source: TypeId,
    target: TypeId,
    kind: TypeComparerKind,
    mut relater: Option<&mut Relater>,
) -> bool {
    let Some(inferences) = infer_types_from_template_literal_type(c, source, target, kind, relater.as_deref_mut()) else {
        return false;
    };
    let (_, target_types) = template_literal_parts(c, target);
    for (i, inference) in inferences.iter().enumerate() {
        if !is_valid_type_for_template_literal_placeholder(c, *inference, target_types[i], kind, relater.as_deref_mut()) {
            return false;
        }
    }
    true
}

/// Go: `func (c *Checker) inferTypesFromTemplateLiteralType(source, target, compareTypes) []*Type`.
pub(crate) fn infer_types_from_template_literal_type(
    c: &mut Checker,
    source: TypeId,
    target: TypeId,
    kind: TypeComparerKind,
    mut relater: Option<&mut Relater>,
) -> Option<Vec<TypeId>> {
    if c.type_(source).flags.intersects(TypeFlags::STRING_LITERAL) {
        let __lift50 = get_string_literal_value(c, source);
        return infer_from_literal_parts_to_template_literal(c, &[__lift50], &[], target);
    }
    if c.type_(source).flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
        let (source_texts, source_types) = template_literal_parts(c, source);
        let (target_texts, _) = template_literal_parts(c, target);
        if source_texts == target_texts {
            let (target_texts, target_types) = template_literal_parts(c, target);
            let _ = target_texts;
            return Some(
                source_types
                    .iter()
                    .zip(&target_types)
                    .map(|(&s, &t)| {
                        let __lift52 = get_base_constraint_or_type(c, s);
                        let __lift51 = get_base_constraint_or_type(c, t);
                        if invoke_comparer(c, kind, relater.as_deref_mut(), __lift52, __lift51, false) != Ternary::False {
                            s
                        } else {
                            get_string_like_type_for_type(c, s).expect("string-like")
                        }
                    })
                    .collect(),
            );
        }
        return infer_from_literal_parts_to_template_literal(c, &source_texts, &source_types, target);
    }
    None
}

/// Go: `func (c *Checker) inferFromLiteralPartsToTemplateLiteral(sourceTexts, sourceTypes, target) []*Type`
/// (see Go's long doc comment, verbatim there).
#[allow(clippy::too_many_lines)]
fn infer_from_literal_parts_to_template_literal(
    c: &mut Checker,
    source_texts: &[String],
    source_types: &[TypeId],
    target: TypeId,
) -> Option<Vec<TypeId>> {
    let last_source_index = source_texts.len() - 1;
    let source_start_text = &source_texts[0];
    let source_end_text = source_texts[last_source_index].clone();
    let (target_texts, _) = template_literal_parts(c, target);
    let last_target_index = target_texts.len() - 1;
    let target_start_text = &target_texts[0];
    let target_end_text = &target_texts[last_target_index];
    if last_source_index == 0 && source_start_text.len() < target_start_text.len() + target_end_text.len()
        || !source_start_text.starts_with(target_start_text.as_str())
        || !source_end_text.ends_with(target_end_text.as_str())
    {
        return None;
    }
    let remaining_end_text = source_end_text[..source_end_text.len() - target_end_text.len()].to_string();
    let mut seg = 0usize;
    let mut pos = target_start_text.len();
    let mut matches: Vec<TypeId> = Vec::new();
    // Go: getSourceText closure.
    let get_source_text = |index: usize, source_texts: &[String], remaining_end_text: &str| -> String {
        if index < last_source_index {
            source_texts[index].clone()
        } else {
    remaining_end_text.to_string()
        }
    };
    // Go: addMatch closure (borrow discipline: pass the mutable state
    // explicitly).
    #[allow(clippy::too_many_arguments)] // Go mirrors addMatch's parameter list
    fn add_match(
        c: &mut Checker,
        source_texts: &[String],
        source_types: &[TypeId],
        remaining_end_text: &str,
        last_source_index: usize,
        seg: &mut usize,
        pos: &mut usize,
        matches: &mut Vec<TypeId>,
        s: usize,
        p: usize,
    ) {
        let match_type = if s == *seg {
            let text = combine_surrogate_pairs(&source_texts[*seg][*pos..p]);
            c.get_string_literal_type(&text)
        } else {
            let mut match_texts: Vec<String> = vec![String::new(); s - *seg + 1];
            match_texts[0] = source_texts[*seg][*pos..].to_string();
            for (i, text) in source_texts[*seg + 1..s].iter().enumerate() {
                match_texts[1 + i] = text.clone();
            }
            match_texts[s - *seg] = if s < last_source_index {
                source_texts[s][..p].to_string()
            } else {
                remaining_end_text[..p].to_string()
            };
            get_template_literal_type(c, &match_texts, &source_types[*seg..s])
        };
        matches.push(match_type);
        *seg = s;
        *pos = p;
    }
    #[allow(clippy::needless_range_loop)] // Go mirrors the index walk over the delimiters
    for i in 1..last_target_index {
        let delim = &target_texts[i];
        if !delim.is_empty() {
            let mut s = seg;
            let mut p = pos;
            loop {
                let text = get_source_text(s, source_texts, &remaining_end_text);
                match text[p..].find(delim.as_str()) {
                    Some(d) => {
                        p += d;
                        break;
                    }
                    None => {
                        s += 1;
                        if s == source_texts.len() {
                            return None;
                        }
                        p = 0;
                    }
                }
            }
            add_match(c, source_texts, source_types, &remaining_end_text, last_source_index, &mut seg, &mut pos, &mut matches, s, p);
            pos += delim.len();
        } else {
            let source_text = get_source_text(seg, source_texts, &remaining_end_text);
            if pos < source_text.len() {
                // Consume one code point at a time (Go comment, verbatim).
                let size = decode_js_string_rune(&source_text[pos..]);
                let seg_snapshot = seg;
                let pos_snapshot = pos;
                add_match(c, source_texts, source_types, &remaining_end_text, last_source_index, &mut seg, &mut pos, &mut matches, seg_snapshot, pos_snapshot + size);
            } else if seg < last_source_index {
                let seg_snapshot = seg;
                add_match(c, source_texts, source_types, &remaining_end_text, last_source_index, &mut seg, &mut pos, &mut matches, seg_snapshot + 1, 0);
            } else {
                return None;
            }
        }
    }
    let end = get_source_text(last_source_index, source_texts, &remaining_end_text).len();
    add_match(c, source_texts, source_types, &remaining_end_text, last_source_index, &mut seg, &mut pos, &mut matches, last_source_index, end);
    Some(matches)
}

/// Go: `func (c *Checker) getStringLikeTypeForType(t) *Type`.
fn get_string_like_type_for_type(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    if c.type_(t).flags.intersects(TypeFlags::ANY | TypeFlags::STRING_LIKE) {
        return Some(t);
    }
    Some(get_template_literal_type(c, &["".to_string(), "".to_string()], &[t]))
}

/// Go: `func (c *Checker) isValidTypeForTemplateLiteralPlaceholder(source, target, compareTypes) bool`.
fn is_valid_type_for_template_literal_placeholder(
    c: &mut Checker,
    source: TypeId,
    target: TypeId,
    kind: TypeComparerKind,
    mut relater: Option<&mut Relater>,
) -> bool {
    if c.type_(target).flags.intersects(TypeFlags::INTERSECTION) {
        let constituents: Vec<TypeId> = c.type_(target).types().to_vec();
        for &t in &constituents {
            if Some(t) == c.empty_type_literal_type {
                continue;
            }
            if !is_valid_type_for_template_literal_placeholder(c, source, t, kind, relater.as_deref_mut()) {
                return false;
            }
        }
        return true;
    }
    if c.type_(target).flags.intersects(TypeFlags::STRING) || invoke_comparer(c, kind, relater.as_deref_mut(), source, target, false) != Ternary::False {
        return true;
    }
    if c.type_(source).flags.intersects(TypeFlags::STRING_LITERAL) {
        let value = get_string_literal_value(c, source);
        return (c.type_(target).flags.intersects(TypeFlags::NUMBER) && is_valid_number_string(&value, false))
            || (c.type_(target).flags.intersects(TypeFlags::BIGINT) && is_valid_big_int_string(&value, false))
            || (c.type_(target).flags.intersects(TypeFlags::BOOLEAN_LITERAL | TypeFlags::NULLABLE)
                && c.type_(target).as_intrinsic_type().is_some_and(|d| d.intrinsic_name == value))
            || (c.type_(target).flags.intersects(TypeFlags::STRING_MAPPING)
                && is_member_of_string_mapping(c, source, target))
            || (c.type_(target).flags.intersects(TypeFlags::TEMPLATE_LITERAL)
                && is_type_matched_by_template_literal_type(c, source, target, kind, relater));
    }
    if c.type_(source).flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
        let (texts, types) = template_literal_parts(c, source);
        return texts.len() == 2
            && texts[0].is_empty()
            && texts[1].is_empty()
            && invoke_comparer(c, kind, relater, types[0], target, false) != Ternary::False;
    }
    false
}

/// Go: `func (c *Checker) isMemberOfStringMapping(source, target) bool`.
fn is_member_of_string_mapping(c: &mut Checker, source: TypeId, target: TypeId) -> bool {
    if c.type_(target).flags.intersects(TypeFlags::ANY) {
        return true;
    }
    if c.type_(target).flags.intersects(TypeFlags::STRING | TypeFlags::TEMPLATE_LITERAL) {
        return c.is_type_assignable_to(source, target);
    }
    if c.type_(target).flags.intersects(TypeFlags::STRING_MAPPING) {
        // We need to see whether applying the same mappings (Go comment,
        // verbatim).
        let (mapped, inner) = apply_target_string_mapping_to_source(c, source, target);
        return mapped == source && is_member_of_string_mapping(c, source, inner);
    }
    false
}

/// Go: `func (c *Checker) applyTargetStringMappingToSource(source, target) (*Type, *Type)`.
fn apply_target_string_mapping_to_source(c: &mut Checker, source: TypeId, target: TypeId) -> (TypeId, TypeId) {
    let mut inner = string_mapping_target(c, target);
    let mut source = source;
    if c.type_(inner).flags.intersects(TypeFlags::STRING_MAPPING) {
        let (s, i) = apply_target_string_mapping_to_source(c, source, inner);
        source = s;
        inner = i;
    }
    let mapping_symbol = c.type_(target).symbol;
    (get_string_mapping_type(c, mapping_symbol, source), inner)
}

// ────────────────────────────────────────────────────────────────────────────
// Error elaboration (Go relater.go 438+) — the elaborate* family. The flow/
// jsx/array-literal contextual-type callees are S4/S5/S9 waves; the structure
// mirrors Go with permissive stubs at the wave seams.
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) elaborateError(node, source, target, relation, headMessage, diagnosticOutput) bool`.
    pub(crate) fn elaborate_error(
        &mut self,
        node: Option<NodeId>,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        head_message: Option<&'static Message>,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        let Some(node) = node else { return false };
        if self.is_or_has_generic_conditional(target) {
            return false;
        }
        if self.compiler_options.no_check == Tristate::TSTrue {
            return false;
        }
        if self.elaborate_did_you_mean_to_call_or_construct(node, source, target, relation, SignatureKind::Construct, head_message, diagnostic_output.as_deref_mut())
            || self.elaborate_did_you_mean_to_call_or_construct(node, source, target, relation, SignatureKind::Call, head_message, diagnostic_output.as_deref_mut())
        {
            return true;
        }
        let kind = self.store().node(node).kind;
        match kind {
            Kind::AsExpression | Kind::JsxExpression | Kind::ParenthesizedExpression => {
                // Go: AsExpression recurses only for const assertions (the
                // non-const case breaks out of the switch).
                if kind == Kind::AsExpression && !ast_is_const_assertion(self, node) {
                    return false;
                }
                // TODO(port M6 S4/S5): node.Expression() accessors — the
                // recursion into the inner expression needs the flow wave's
                // expression accessor seam.
                return false;
            }
            Kind::BinaryExpression => {
                // TODO(port M6 S4): the '=' / ',' operator recursion.
                return false;
            }
            Kind::ObjectLiteralExpression => {
                return self.elaborate_object_literal(node, source, target, relation, diagnostic_output);
            }
            Kind::ArrayLiteralExpression => {
                return self.elaborate_array_literal(node, source, target, relation, diagnostic_output);
            }
            Kind::ArrowFunction => {
                return self.elaborate_arrow_function(node, source, target, relation, diagnostic_output);
            }
            Kind::JsxAttributes => {
                // TODO(port M6 W5 jsx): elaborateJsxComponents.
                return false;
            }
            _ => {}
        }
        false
    }

    /// Go: `func (c *Checker) isOrHasGenericConditional(t) bool`.
    fn is_or_has_generic_conditional(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        flags.intersects(TypeFlags::CONDITIONAL)
            || (flags.intersects(TypeFlags::INTERSECTION) && {
                let constituents: Vec<TypeId> = self.type_(t).types().to_vec();
                constituents.iter().any(|&x| self.is_or_has_generic_conditional(x))
            })
    }

    /// Go: `func (c *Checker) elaborateDidYouMeanToCallOrConstruct(...) bool`.
    #[allow(clippy::too_many_arguments)]
    fn elaborate_did_you_mean_to_call_or_construct(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        kind: SignatureKind,
        head_message: Option<&'static Message>,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        let signatures = get_signatures_of_type(self, source, kind);
        for &s in &signatures {
            let return_type = get_return_type_of_signature(self, s);
            if !self.type_(return_type).flags.intersects(TypeFlags::ANY | TypeFlags::NEVER)
                && self.check_type_related_to(return_type, target, relation, None /*errorNode*/)
            {
                let mut diags: Vec<tsc_ast::Diagnostic> = Vec::new();
                if !self.check_type_related_to_ex(source, target, relation, Some(node), head_message, Some(&mut diags)) {
                    let mut diagnostic = diags[0].clone();
                    let message = if kind == SignatureKind::Construct {
                        &msg::DID_YOU_MEAN_TO_USE_NEW_WITH_THIS_EXPRESSION
                    } else {
                        &msg::DID_YOU_MEAN_TO_CALL_THIS_EXPRESSION
                    };
                    diagnostic.add_related_info(diagnostic_for_node(self, Some(node), message, &[]));
                    report_diagnostic(self, Some(diagnostic), diagnostic_output);
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) elaborateObjectLiteral(...) bool`.
    fn elaborate_object_literal(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        if self.type_(target).flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::NEVER) {
            return false;
        }
        // TODO(port M6 S4): the property walk (getLiteralTypeFromProperty of
        // the declaration symbols, the per-Kind elaborateElement dispatch,
        // the computed-name message). The object-literal members need the
        // symbol-of-declaration seam from the flow wave.
        let _ = (node, source, relation, diagnostic_output);
        false
    }

    /// Go: `func (c *Checker) elaborateArrayLiteral(...) bool`.
    fn elaborate_array_literal(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        if self.type_(target).flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::NEVER) {
            return false;
        }
        // TODO(port M6 S4/S9): checkArrayLiteral via the contextual-type
        // stack (pushContextualType/popContextualType) + isTupleLikeType +
        // getEffectiveCheckNode — the array-literal elaboration needs the
        // flow wave's contextual typing.
        let _ = (node, source, relation, diagnostic_output);
        false
    }

    /// Go: `func (c *Checker) elaborateArrowFunction(...) bool`.
    fn elaborate_arrow_function(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        // Don't elaborate blocks or functions with annotated parameter types
        // (Go comment, verbatim).
        // TODO(port M6 S4): the body/parameter accessors + getSingleCallSignature
        // (S8) + createPromiseType — the arrow-function elaboration needs the
        // flow wave.
        let _ = (node, source, target, relation, diagnostic_output);
        false
    }
}

/// Go: `func (c *Checker) getTypeOfPropertyInType`-family helper — the
/// `getPropertyOfType`/`getPropertyOfObjectType` names above route through
/// the property-stub seam.
fn ast_is_const_assertion(c: &Checker, node: NodeId) -> bool {
    // Go: ast.IsConstAssertion — KindAsExpression with a `const` type.
    tsc_binder::ast_shims::is_const_assertion(c.store(), node)
}

// ────────────────────────────────────────────────────────────────────────────
// Unported-wave callees (Go checker.go/utilities.go slice seams)
//
// Each stub carries the TODO(port M6 <slice>) banner naming the wave that
// replaces it, per the W1 stub convention: real body where the callee is
// arena-local, permissive direction where the owning wave is unported (the
// M6-DISPATCH cycle notes pin the directions: S7 union/instantiation calls
// get identity-or-errorType stubs, S8 member resolution gets empty/None
// stubs, S9 constraint reads stop at the payload, W4 type-to-string falls
// back to names).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `ResolvedTypeMembers`-shaped carrier (checker.go) — the S8 wave owns
/// the real struct; the permissive stub resolves to no members/signatures.
pub(crate) struct ResolvedTypeMembers {
    pub properties: Vec<SymbolId>,
    pub signatures: Vec<crate::types::SignatureId>,
    pub index_infos: Vec<crate::types::IndexInfoId>,
}

/// Go: `func (c *Checker) resolveStructuredTypeMembers(t) *ResolvedTypeMembers`.
pub(crate) fn resolve_structured_type_members(c: &mut Checker, t: TypeId) -> ResolvedTypeMembers {
    // TODO(port M6 S8): the members resolver (resolveObjectTypeMembers +
    // resolveUnionTypeMembers etc.) — permissive: no members.
    let _ = (c, t);
    ResolvedTypeMembers {
        properties: Vec::new(),
        signatures: Vec::new(),
        index_infos: Vec::new(),
    }
}

/// Go: `func (c *Checker) getPropertiesOfType(t) []*ast.Symbol`.
pub(crate) fn get_properties_of_type(c: &mut Checker, t: TypeId) -> Vec<SymbolId> {
    if c.type_(t).flags.intersects(TypeFlags::OBJECT) {
        return get_properties_of_object_type(c, t);
    }
    if c.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
        return get_properties_of_union_or_intersection_type(c, t);
    }
    // TODO(port M6 S8): intersection reduction etc.
    Vec::new()
}

/// Go: `func (c *Checker) getPropertiesOfObjectType(t) []*ast.Symbol`.
pub(crate) fn get_properties_of_object_type(c: &mut Checker, t: TypeId) -> Vec<SymbolId> {
    // TODO(port M6 S8): resolveStructuredTypeMembers — empty until then.
    let _ = (c, t);
    Vec::new()
}

/// Go: `func (c *Checker) getPropertiesOfUnionOrIntersectionType(t) []*ast.Symbol`.
pub(crate) fn get_properties_of_union_or_intersection_type(c: &mut Checker, t: TypeId) -> Vec<SymbolId> {
    // TODO(port M6 S8): the union/intersection property walk.
    let _ = (c, t);
    Vec::new()
}

/// Go: `func (c *Checker) getPropertyOfObjectType(t, name) *ast.Symbol`.
pub(crate) fn get_property_of_object_type(c: &mut Checker, t: TypeId, name: &str) -> Option<SymbolId> {
    // TODO(port M6 S8): the member lookup (case-insensitive fallback etc.).
    let _ = (c, t, name);
    None
}

/// Go: `func (c *Checker) getPropertyOfUnionOrIntersectionType(t, name, skipObjectFunctionPropertyAugment) *ast.Symbol`.
pub(crate) fn get_property_of_union_or_intersection_type(c: &mut Checker, t: TypeId, name: &str, _skip_object_function_property_augment: bool) -> Option<SymbolId> {
    // TODO(port M6 S8): the union/intersection synthetic property.
    let _ = (c, t, name);
    None
}

/// Go: `func (c *Checker) getPropertyOfType(t, name) *ast.Symbol`.
pub(crate) fn get_property_of_type(c: &mut Checker, t: TypeId, name: &str) -> Option<SymbolId> {
    if c.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
        return get_property_of_union_or_intersection_type(c, t, name, false);
    }
    let apparent = get_apparent_type(c, t);
    get_property_of_object_type(c, apparent, name)
}

/// Go: `func (c *Checker) getTypeOfPropertyOfType(t, name) *Type`.
pub(crate) fn get_type_of_property_of_type(c: &mut Checker, t: TypeId, name: &str) -> Option<TypeId> {
    get_property_of_type(c, t, name).map(|p| c.get_type_of_symbol(p))
}

/// Go: `func (c *Checker) getTypeOfPropertyOrIndexSignatureOfType(t, name) *Type`.
pub(crate) fn get_type_of_property_or_index_signature_of_type(c: &mut Checker, t: TypeId, name: &str) -> Option<TypeId> {
    if let Some(prop) = get_property_of_type(c, t, name) {
        return Some(get_type_of_symbol(c, prop));
    }
    if let Some(info) = get_applicable_index_info_for_name(c, t, name) {
        return Some(c.index_info_value_type(info));
    }
    None
}

/// Go: `func (c *Checker) getSignaturesOfType(t, kind) []*Signature`.
pub(crate) fn get_signatures_of_type(c: &mut Checker, _t: TypeId, kind: SignatureKind) -> Vec<crate::types::SignatureId> {
    // TODO(port M6 S8): resolveStructuredTypeMembers' signature lists.
    let _ = (c, kind);
    Vec::new()
}

/// Go: `func (c *Checker) getSignaturesOfStructuredType(t, kind) []*Signature`.
pub(crate) fn get_signatures_of_structured_type(c: &mut Checker, _t: TypeId, kind: SignatureKind) -> Vec<crate::types::SignatureId> {
    // TODO(port M6 S8): as above (the structured-type variant).
    let _ = (c, kind);
    Vec::new()
}

/// Go: `func (c *Checker) typeHasCallOrConstructSignatures(t) bool`.
pub(crate) fn type_has_call_or_construct_signatures(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S8): the signature-existence check.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) getIndexInfosOfType(t) []*IndexInfo`.
pub(crate) fn get_index_infos_of_type(c: &mut Checker, t: TypeId) -> Vec<crate::types::IndexInfoId> {
    // TODO(port M6 S8): resolveStructuredTypeMembers' indexInfos.
    let _ = (c, t);
    Vec::new()
}

/// Go: `func (c *Checker) getIndexInfoOfType(t, key) *IndexInfo`.
pub(crate) fn get_index_info_of_type(c: &mut Checker, _t: TypeId, key: TypeId) -> Option<crate::types::IndexInfoId> {
    // TODO(port M6 S8): the indexInfo map lookup.
    let _ = (c, key);
    None
}

/// Go: `func (c *Checker) getApplicableIndexInfo(t, key) *IndexInfo`.
pub(crate) fn get_applicable_index_info(c: &mut Checker, _t: TypeId, key: TypeId) -> Option<crate::types::IndexInfoId> {
    // TODO(port M6 S8): isApplicableIndexType over the indexInfos.
    let _ = (c, key);
    None
}

/// Go: `func (c *Checker) getApplicableIndexInfoForName(t, name) *IndexInfo`.
pub(crate) fn get_applicable_index_info_for_name(c: &mut Checker, t: TypeId, name: &str) -> Option<crate::types::IndexInfoId> {
    // TODO(port M6 S8): the name-keyed indexInfo path.
    let _ = (c, t, name);
    None
}

/// Go: `func (c *Checker) isApplicableIndexType(source, target) bool`.
pub(crate) fn is_applicable_index_type(c: &mut Checker, source: Option<TypeId>, target: TypeId) -> bool {
    // TODO(port M6 S8): getIndexType / isKeyTypeValidForIndex.
    let _ = (c, source, target);
    false
}

/// Go: `func (c *Checker) getLiteralTypeFromProperty(prop, flags, checkOverflow) *Type`.
pub(crate) fn get_literal_type_from_property(c: &mut Checker, prop: SymbolId, _flags: TypeFlags, _check_overflow: bool) -> Option<TypeId> {
    // TODO(port M6 S8): the declared-literal-from-name machinery.
    let _ = (c, prop);
    None
}

/// Go: `func (c *Checker) getNonMissingTypeOfSymbol(sym) *Type`.
pub(crate) fn get_non_missing_type_of_symbol(c: &mut Checker, sym: SymbolId) -> TypeId {
    // TODO(port M6 S8): the missing-type strip (exactOptionalPropertyTypes).
    c.get_type_of_symbol(sym)
}

/// Go: `func (c *Checker) getTypeOfSymbol(sym) *Type` (free-fn form).
pub(crate) fn get_type_of_symbol(c: &mut Checker, sym: SymbolId) -> TypeId {
    c.get_type_of_symbol(sym)
}

/// Go: `func (c *Checker) symbolToString(sym) string`.
pub(crate) fn symbol_to_string(c: &mut Checker, sym: SymbolId) -> String {
    // TODO(port M6 W4): symbolToString's escaping/mangling (nodebuilder wave).
    c.program_symbol(sym).name.clone()
}

/// Go: `func (c *Checker) signatureToString(s) string`.
pub(crate) fn signature_to_string(c: &mut Checker, s: crate::types::SignatureId) -> String {
    // TODO(port M6 W4): the signature printer.
    let _ = (c, s);
    String::new()
}

/// Go: `func (c *Checker) TypeToString(t) string` — W4 stub: the intrinsic
/// name (or a flag-derived placeholder); the nodebuilder wave owns the real
/// printer. Only error-message text depends on this.
pub(crate) fn type_to_string(c: &mut Checker, t: TypeId) -> String {
    // TODO(port M6 W4): TypeToString (nodebuilderimpl).
    if let Some(data) = c.type_(t).as_intrinsic_type() {
        return data.intrinsic_name.clone();
    }
    if let Some(data) = c.type_(t).as_literal_type() {
        return match data.value.as_ref() {
            Some(crate::types::LiteralValue::String(s)) => format!("\"{s}\""),
            Some(crate::types::LiteralValue::Number(n)) => format!("{}", n.0),
            Some(crate::types::LiteralValue::BigInt(b)) => b.to_string(),
            Some(crate::types::LiteralValue::Boolean(b)) => b.to_string(),
            // Go's `nil` LiteralValue sentinel (only fresh-type widening
            // produces it; unreachable on the error-display path).
            Some(crate::types::LiteralValue::None) | None => String::new(),
        };
    }
    "unknown".to_string()
}

/// Go: `func isObjectLiteralType(t) bool` (utilities.go — `t.flags&(TypeFlagsObject|TypeFlagsIntersection) != 0 && t.symbol != nil && t.symbol.Flags&SymbolFlagsObjectLiteral != 0`).
pub(crate) fn is_object_literal_type(c: &Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
        && c.type_(t).symbol.is_some_and(|s| c.program_symbol(s).flags.intersects(SymbolFlags::OBJECT_LITERAL))
}

/// Go: `func isUnitType(t) bool` (utilities.go).
pub(crate) fn is_unit_type(c: &Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(TypeFlags::UNIT)
}

/// Go: `func isLiteralType(t) bool` (utilities.go — the widened form without
/// unique symbols).
pub(crate) fn is_literal_type(c: &Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(
        TypeFlags::STRING_LITERAL
            | TypeFlags::NUMBER_LITERAL
            | TypeFlags::BIGINT_LITERAL
            | TypeFlags::BOOLEAN_LITERAL,
    )
}

/// Go: `func (c *Checker) isEmptyObjectType(t) bool`.
pub(crate) fn is_empty_object_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): t == emptyObjectType — the empty anonymous object
    // type is created by the newAnonymousType wave.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) IsEmptyAnonymousObjectType(t) bool`.
pub(crate) fn is_empty_anonymous_object_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): as above.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isEmptyArrayLiteralType(t) bool`.
pub(crate) fn is_empty_array_literal_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): the empty-array-literal marker on array types.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isArrayType(t) bool`.
pub(crate) fn is_array_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): the globalArrayType-target reference check.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isReadonlyArrayType(t) bool`.
pub(crate) fn is_readonly_array_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): the globalReadonlyArrayType-target check.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isArrayOrTupleType(t) bool`.
pub(crate) fn is_array_or_tuple_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isMutableArrayOrTuple(t) bool`.
pub(crate) fn is_mutable_array_or_tuple(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func isMutableTupleType(t) bool`.
pub(crate) fn is_mutable_tuple_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func isTupleType(t) bool`.
pub(crate) fn is_tuple_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): the instantiable-target-tuple check.
    let _ = (c, t);
    false
}

/// Go: `func isTupleLikeType(t) bool` (utilities.go).
#[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn is_tuple_like_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isArrayLikeType(t) bool`.
pub(crate) fn is_array_like_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isGenericTupleType(t) bool`.
pub(crate) fn is_generic_tuple_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isGenericMappedType(t) bool`.
pub(crate) fn is_generic_mapped_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): the contains-marker check on mapped types.
    let _ = (c, t);
    false
}

/// Go: `func isPartialMappedType(t) bool`.
pub(crate) fn is_partial_mapped_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isGenericObjectType(t) bool`.
pub(crate) fn is_generic_object_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): the containsGenericLocal check.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isNonGenericObjectType(t) bool`.
pub(crate) fn is_non_generic_object_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isGenericIndexType(t) bool`.
pub(crate) fn is_generic_index_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isMappedTypeGenericIndexedAccess(t) bool`.
pub(crate) fn is_mapped_type_generic_indexed_access(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): the mapped-type + generic-index combination.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isErrorType(t) bool`.
#[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn is_error_type(c: &Checker, t: TypeId) -> bool {
    Some(t) == c.error_type
}

/// Go: `func IsDistributedTypeParameter(t) bool` (utilities.go).
pub(crate) fn is_distributed_type_parameter(c: &Checker, t: TypeId) -> bool {
    // TODO(port M6 S10): the distributed-marker check.
    let _ = (c, t);
    false
}

/// Go: `func isSingleElementGenericTupleType(t) bool`.
pub(crate) fn is_single_element_generic_tuple_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7): arity-1 tuple target with a generic element.
    let _ = (c, t);
    false
}

/// Go: `func isStaticPrivateIdentifierProperty(prop) bool`.
fn is_static_private_identifier_property(c: &Checker, prop: SymbolId) -> bool {
    // TODO(port M6 S6): the static-private check.
    let _ = (c, prop);
    false
}

/// Go: `func isNumericLiteralName(name) bool` (utilities.go).
fn is_numeric_literal_name(name: &str) -> bool {
    // Go: a canonical parse of the name as a numeric literal.
    !name.is_empty() && name.parse::<f64>().is_ok()
}

// ── Normalization / constraints / instantiation (S7/S9/S10 seams) ──

/// Go: `func (c *Checker) getNormalizedType(t, writing) *Type` — the fresh /
/// deferred-reference normalization. The fresh-literal arm is real; the
/// deferred-reference, tuple and conditional arms are S7.
pub(crate) fn get_normalized_type(c: &mut Checker, t: TypeId, writing: bool) -> TypeId {
    if !writing && crate::is_fresh_literal_type(c, t) {
        return c.get_regular_type_of_literal_type(t);
    }
    // TODO(port M6 S7): deferred type references, tuples/conditional
    // simplification, substitution resolution.
    t
}

/// Go: `func (c *Checker) getConstraintOfType(t) *Type` — reads the payload
/// constraint; the type-alias and mapped-type arms are S9.
pub(crate) fn get_constraint_of_type(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    let ty = c.type_(t);
    if ty.flags.intersects(TypeFlags::TYPE_PARAMETER | TypeFlags::INDEX | TypeFlags::CONDITIONAL) {
        if let Some(data) = ty.as_type_parameter() {
            return data.constraint;
        }
        // TODO(port M6 S10): the index-type constraint (stringNumberSymbol).
        if ty.flags.intersects(TypeFlags::INDEX) {
            return None;
        }
        if let Some(data) = ty.as_conditional_type() {
            return data.base.resolved_base_constraint;
        }
    }
    // TODO(port M6 S9): getConstraintFromTypeReference, mapped types,
    // `infer`-parameter constraints.
    None
}

/// Go: `func (c *Checker) getConstraintOfTypeParameter(t) *Type`.
pub(crate) fn get_constraint_of_type_parameter(c: &Checker, t: TypeId) -> Option<TypeId> {
    c.type_(t).as_type_parameter().and_then(|d| d.constraint)
}

/// Go: `func (c *Checker) getBaseConstraintOfType(t) *Type` — the cycle-safe
/// constraint walk.
pub(crate) fn get_base_constraint_of_type(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    // TODO(port M6 S9): the getBaseConstraint worker with the
    // circular-constraint sentinel; the payload read suffices until the
    // constrained-instantiation waves construct deeper chains.
    get_constraint_of_type(c, t)
}

/// Go: `func (c *Checker) getBaseConstraintOrType(t) *Type`.
pub(crate) fn get_base_constraint_or_type(c: &mut Checker, t: TypeId) -> TypeId {
    get_base_constraint_of_type(c, t).unwrap_or(t)
}

/// Go: `func (c *Checker) getDefaultConstraintOfConditionalType(t) *Type`.
pub(crate) fn get_default_constraint_of_conditional_type(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    c.type_(t).as_conditional_type().and_then(|d| d.resolved_default_constraint)
}

/// Go: `func (c *Checker) getConstraintOfDistributiveConditionalType(t) *Type`.
pub(crate) fn get_constraint_of_distributive_conditional_type(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    c.type_(t).as_conditional_type().and_then(|d| d.resolved_constraint_of_distributive)
}

/// Go: `func (c *Checker) hasNonCircularBaseConstraint(t) bool`.
pub(crate) fn has_non_circular_base_constraint(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S9): the circularity detection; permissive until then.
    get_base_constraint_of_type(c, t).is_some()
}

/// Go: `func (c *Checker) getTypeWithThisArgument(t, thisArgument, needApparentType) *Type`.
pub(crate) fn get_type_with_this_argument(_c: &mut Checker, t: TypeId, this_arg: TypeId, _need_apparent_type: bool) -> TypeId {
    // TODO(port M6 S9): the this-mapper instantiation.
    let _ = this_arg;
    t
}

/// Go: `func (c *Checker) removeMissingType(t, isOptional) *Type`.
pub(crate) fn remove_missing_type(_c: &mut Checker, t: TypeId, _is_optional: bool) -> TypeId {
    // TODO(port M6 S7): the `missing`-strip union construction.
    t
}

/// Go: `func (c *Checker) addOptionalityEx(t, isProperty, isOptional) *Type`.
pub(crate) fn add_optionality_ex(c: &mut Checker, t: TypeId, is_property: bool, is_optional: bool) -> TypeId {
    c.add_optionality_ex(t, is_property, is_optional)
}

/// Go: `func (c *Checker) getApparentType(t) *Type`.
pub(crate) fn get_apparent_type(_c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S8): apparentTypeOfObjectLiteral/enum etc.
    t
}

/// Go: `func (c *Checker) getRegularTypeOfObjectLiteral(t) *Type`.
pub(crate) fn get_regular_type_of_object_literal(c: &mut Checker, t: TypeId) -> TypeId {
    if c.type_(t).symbol.is_some_and(|s| c.program_symbol(s).flags.intersects(SymbolFlags::OBJECT_LITERAL)) {
        return c.get_regular_type_of_literal_type(t);
    }
    t
}

/// Go: `func (c *Checker) getUnionType(types) *Type` — S7 permissive stub.
pub(crate) fn get_union_type(c: &mut Checker, types: Vec<TypeId>) -> TypeId {
    // TODO(port M6 S7): getUnionType (subtype reduction, origin tracking).
    if types.len() == 1 {
        return types[0];
    }
    if types.is_empty() {
        return c.never_type();
    }
    c.error_type()
}

/// Go: `func (c *Checker) getUnionTypeEx(types, unionReduction, alias, origin) *Type`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn get_union_type_ex(
    c: &mut Checker,
    types: Vec<TypeId>,
    _union_reduction: crate::todo_shapes::UnionReduction,
    _alias: Option<Box<crate::types::TypeAlias>>,
    _origin: Option<TypeId>,
) -> TypeId {
    // TODO(port M6 S7): the real union constructor.
    get_union_type(c, types)
}

/// Go: `func (c *Checker) getIntersectionType(types) *Type` — S7 permissive
/// stub.
pub(crate) fn get_intersection_type(c: &mut Checker, types: Vec<TypeId>) -> TypeId {
    // TODO(port M6 S7): getIntersectionType.
    if types.len() == 1 {
        return types[0];
    }
    if types.is_empty() {
        return c.unknown_type();
    }
    c.error_type()
}

/// Go: `func (c *Checker) getIntersectionTypeEx(types, intersectionFlags, alias) *Type`.
pub(crate) fn get_intersection_type_ex(
    c: &mut Checker,
    types: Vec<TypeId>,
    _flags: IntersectionFlags,
    _alias: Option<Box<crate::types::TypeAlias>>,
) -> TypeId {
    // TODO(port M6 S7): the real intersection constructor.
    get_intersection_type(c, types)
}

/// Go: `func (c *Checker) intersectTypes(t, u) *Type`.
pub(crate) fn intersect_types(c: &mut Checker, t: TypeId, u: TypeId) -> Option<TypeId> {
    // TODO(port M6 S7): the intersection-with-primitive reduction.
    Some(get_intersection_type(c, vec![t, u]))
}

/// Go: `func (c *Checker) extractTypesOfKind(t, kind) *Type` — returns the
/// never type when nothing matches (Go returns nil; the callers all read
/// flags immediately, and never carries the same "no constituents"
/// semantics).
pub(crate) fn extract_types_of_kind(c: &mut Checker, t: TypeId, kind: TypeFlags) -> TypeId {
    // TODO(port M6 S7): the filter construction.
    if c.type_(t).flags.intersects(kind) {
        t
    } else {
        c.never_type()
    }
}

/// Go: `func (c *Checker) extractTypesOfKind(target, ^TypeFlagsUndefined)` —
/// the complement form.
pub(crate) fn extract_types_of_kind_not(c: &mut Checker, t: TypeId, kind: TypeFlags) -> TypeId {
    // TODO(port M6 S7): the filter construction (complement).
    if !c.type_(t).flags.intersects(!kind) {
        t
    } else {
        c.never_type()
    }
}

/// Go: `func (c *Checker) filterType(t, f) *Type`.
#[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn filter_type(c: &mut Checker, t: TypeId, mut f: impl FnMut(TypeId) -> bool) -> TypeId {
    // TODO(port M6 S7): the real filter (sameType reduction).
    if f(t) { t } else { c.never_type() }
}

/// Go: `func (c *Checker) maybeTypeOfKind(t, kind) bool`.
pub(crate) fn maybe_type_of_kind(c: &mut Checker, t: TypeId, kind: TypeFlags) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        return c.type_(t).types().iter().any(|&t| c.type_(t).flags.intersects(kind));
    }
    c.type_(t).flags.intersects(kind)
}

/// Go: `func (c *Checker) countTypes(t) int`.
pub(crate) fn count_types(c: &mut Checker, t: TypeId) -> usize {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        return c.type_(t).types().len();
    }
    1
}

/// Go: `func (c *Checker) getReducedType(t) *Type`.
pub(crate) fn get_reduced_type(_c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7): subtype-reduced union forms.
    t
}

/// Go: the `Distributed()` accessor — a union distributes to its
/// constituents, anything else to itself.
pub(crate) fn distributed(c: &Checker, t: TypeId) -> Vec<TypeId> {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        c.type_(t).types().to_vec()
    } else {
        vec![t]
    }
}

/// Go: `func everyType(t, f) bool`.
pub(crate) fn every_type(c: &mut Checker, t: TypeId, mut f: impl FnMut(TypeId) -> bool) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        return c.type_(t).types().iter().all(|&t| f(t));
    }
    f(t)
}

/// Go: `func someType(t, f) bool`.
#[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn some_type(c: &mut Checker, t: TypeId, mut f: impl FnMut(TypeId) -> bool) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        return c.type_(t).types().iter().any(|&t| f(t));
    }
    f(t)
}

/// Go: `func containsType(types, t) bool`.
pub(crate) fn contains_type(_c: &Checker, types: &[TypeId], t: TypeId) -> bool {
    types.contains(&t)
}

/// Go: `func (c *Checker) getTrueTypeFromConditionalType(t) *Type` — payload
/// read (the instantiation path is S10).
pub(crate) fn get_true_type_from_conditional_type(c: &mut Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_conditional_type()
        .and_then(|d| d.resolved_true_type)
        .unwrap_or_else(|| c.error_type())
}

/// Go: `func (c *Checker) getFalseTypeFromConditionalType(t) *Type`.
pub(crate) fn get_false_type_from_conditional_type(c: &mut Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_conditional_type()
        .and_then(|d| d.resolved_false_type)
        .unwrap_or_else(|| c.error_type())
}

/// Go: the ConditionalRoot payload readers (identity = Box pointer).
pub(crate) fn conditional_root_is_distributive(c: &Checker, t: TypeId) -> bool {
    c.type_(t)
        .as_conditional_type()
        .and_then(|d| d.root.as_deref().map(|r| r.is_distributive))
        .unwrap_or(false)
}

/// Go: `root == root` pointer identity (the conditional `source.AsConditionalType().root == c.root` guard).
pub(crate) fn conditional_roots_identical(c: &Checker, s: TypeId, t: TypeId) -> bool {
    let sp = c
        .type_(s)
        .as_conditional_type()
        .and_then(|d| d.root.as_deref().map(|r| r as *const _ as usize));
    let tp = c
        .type_(t)
        .as_conditional_type()
        .and_then(|d| d.root.as_deref().map(|r| r as *const _ as usize));
    sp.is_some() && sp == tp
}

/// Go: `isDistributionDependent(root)` — the isTypeParameterPossiblyReferenced
/// walk is S10; the distributive flag is real.
pub(crate) fn conditional_is_distribution_dependent(c: &mut Checker, t: TypeId) -> bool {
    let root = c
        .type_(t)
        .as_conditional_type()
        .and_then(|d| d.root.clone());
    root.as_deref()
        .map(|r| c.is_distribution_dependent(r))
        .unwrap_or(false)
}

/// Go: `root.inferTypeParameters`.
pub(crate) fn conditional_infer_type_parameters(c: &Checker, t: TypeId) -> Vec<TypeId> {
    c.type_(t)
        .as_conditional_type()
        .and_then(|d| d.root.as_deref().map(|r| r.infer_type_parameters.clone()))
        .unwrap_or_default()
}

/// Go: `(checkType, extendsType)` of a conditional.
pub(crate) fn conditional_parts(c: &Checker, t: TypeId) -> (TypeId, TypeId) {
    let data = c.type_(t).as_conditional_type().expect("ConditionalType data");
    (
        data.check_type.unwrap_or_else(|| c.error_type()),
        data.extends_type.unwrap_or_else(|| c.error_type()),
    )
}

/// Go: the IndexedAccess payload `(objectType, indexType)`.
pub(crate) fn indexed_access_parts(c: &Checker, t: TypeId) -> (TypeId, TypeId) {
    let data = c.type_(t).as_indexed_access_type().expect("IndexedAccessType data");
    (
        data.object_type.expect("indexed-access objectType"),
        data.index_type.expect("indexed-access indexType"),
    )
}

/// Go: the Index payload `target`.
pub(crate) fn index_target(c: &Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_index_type()
        .expect("IndexType data")
        .target
        .expect("index target")
}

/// Go: the Index payload `indexFlags`.
pub(crate) fn index_type_index_flags(c: &Checker, t: TypeId) -> IndexFlags {
    c.type_(t).as_index_type().expect("IndexType data").index_flags
}

/// Go: the Substitution payload `(baseType, constraint)`.
pub(crate) fn substitution_parts(c: &Checker, t: TypeId) -> (TypeId, TypeId) {
    let data = c.type_(t).as_substitution_type().expect("SubstitutionType data");
    (
        data.base_type.expect("substitution baseType"),
        data.constraint.expect("substitution constraint"),
    )
}

/// Go: the TemplateLiteral payload `(texts, types)`.
pub(crate) fn template_literal_parts(c: &Checker, t: TypeId) -> (Vec<String>, Vec<TypeId>) {
    let data = c.type_(t).as_template_literal_type().expect("TemplateLiteralType data");
    (data.texts.clone(), data.types.clone())
}

/// Go: the StringMapping payload `symbol`.
pub(crate) fn string_mapping_symbol(c: &Checker, t: TypeId) -> Option<SymbolId> {
    c.type_(t).symbol
}

/// Go: the StringMapping payload `target`.
pub(crate) fn string_mapping_target(c: &Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_string_mapping_type()
        .expect("StringMappingType data")
        .target
        .expect("string-mapping target")
}

// ── Index / indexed-access / keys (S10 seams) ──

/// Go: `func (c *Checker) getIndexType(t) *Type` — keyof construction.
pub(crate) fn get_index_type(c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S10): getIndexedAccessType with AccessFlagsWriting.
    let _ = t;
    c.never_type()
}

/// Go: `func (c *Checker) getIndexTypeEx(t, indexFlags) *Type`.
pub(crate) fn get_index_type_ex(c: &mut Checker, t: TypeId, _index_flags: IndexFlags) -> TypeId {
    // TODO(port M6 S10).
    let _ = t;
    c.never_type()
}

/// Go: `func (c *Checker) getIndexTypeOfTypeEx(t, key, fallback) *Type`.
pub(crate) fn get_index_type_of_type_ex(c: &mut Checker, t: TypeId, _key: TypeId, _fallback: TypeId) -> TypeId {
    // TODO(port M6 S10).
    let _ = t;
    c.never_type()
}

/// Go: `func (c *Checker) getIndexedAccessType(t, key) *Type`.
pub(crate) fn get_indexed_access_type(c: &mut Checker, t: TypeId, key: TypeId) -> TypeId {
    // TODO(port M6 S10): the indexed-access constructor.
    let _ = (t, key);
    c.error_type()
}

/// Go: `func (c *Checker) getIndexedAccessTypeOrUndefined(objectType, indexType, accessFlags, accessNode, errorNode) *Type`
/// — the `AccessFlagsWriting | IfElse(baseObjectType != objectType, AccessFlagsNoIndexSignatures, 0)` call.
pub(crate) fn get_indexed_access_type_or_undefined_writing(_c: &mut Checker, object_type: TypeId, index_type: TypeId, _no_index_signatures: bool) -> Option<TypeId> {
    // TODO(port M6 S10): the constraint-of-indexed-access resolution.
    let _ = (object_type, index_type);
    None
}

/// Go: `func (c *Checker) getKnownKeysOfTupleType(t) *Type`.
pub(crate) fn get_known_keys_of_tuple_type(c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7/S10).
    let _ = t;
    c.never_type()
}

/// Go: `func (c *Checker) getSimplifiedTypeOrConstraint(t) *Type`.
pub(crate) fn get_simplified_type_or_constraint(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    // TODO(port M6 S10): getSimplifiedType.
    get_base_constraint_of_type(c, t)
}

// ── Type references / tuples / aliases (S2/S7 seams) ──

/// Go: the TypeReference payload `Target()` (the instantiable target).
pub(crate) fn type_reference_target(c: &Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_type_reference()
        .and_then(|d| d.base.target)
        .expect("TypeReference target")
}

/// Go: as above, `Option` form (the anonymous-vs-reference overlap checks).
pub(crate) fn type_reference_target_opt(c: &Checker, t: TypeId) -> Option<TypeId> {
    c.type_(t).as_type_reference().and_then(|d| d.base.target)
}

/// Go: `func (c *Checker) getTypeArguments(t) []*Type` — reads the resolved
/// type arguments (the deferred form falls back to the node's).
pub(crate) fn get_type_arguments(c: &Checker, t: TypeId) -> Vec<TypeId> {
    c.type_(t)
        .as_type_reference()
        .map(|d| d.resolved_type_arguments.clone())
        .unwrap_or_default()
}

/// Go: `func (c *Checker) getTypeReferenceArity(t) int`.
pub(crate) fn get_type_reference_arity(c: &Checker, t: TypeId) -> i32 {
    get_type_arguments(c, t).len() as i32
}

/// Go: `func (c *Checker) getMinTypeArgumentCount(typeParameters) int`.
pub(crate) fn get_min_type_argument_count(c: &mut Checker, params: &[TypeId]) -> usize {
    // TODO(port M6 S9): the defaults-aware minimum.
    let _ = c;
    params.len()
}

/// Go: `func (c *Checker) fillMissingTypeArguments(typeArguments, typeParameters, minTypeArgumentCount, includeEmptyArray) []*Type`.
pub(crate) fn fill_missing_type_arguments(c: &mut Checker, type_arguments: &[TypeId], _params: &[TypeId], min_type_argument_count: usize, _include_empty_array: bool) -> Vec<TypeId> {
    // TODO(port M6 S7): the defaults-filling instantiation.
    if type_arguments.len() >= min_type_argument_count {
        return type_arguments.to_vec();
    }
    let mut filled = type_arguments.to_vec();
    while filled.len() < min_type_argument_count {
        filled.push(c.unknown_type());
    }
    filled
}

/// Go: the typeAliasLinks `typeParameters` read.
pub(crate) fn get_type_alias_type_parameters(_c: &mut Checker, _symbol: SymbolId) -> Vec<TypeId> {
    // TODO(port M6 S2/S7): the declared type-parameter list.
    Vec::new()
}

/// Go: `ast.IsInJSFile(symbol.ValueDeclaration)`.
pub(crate) fn alias_symbol_value_declaration_in_js_file(c: &Checker, symbol: SymbolId) -> bool {
    // TODO(port M6 S4): IsInJSFile.
    let _ = (c, symbol);
    false
}

// ── Mapped-type accessors (S7 seams) ──

/// Go: `func (c *Checker) getMappedTypeModifiers(t) MappedTypeModifiers`.
pub(crate) fn get_mapped_type_modifiers(_c: &mut Checker, t: TypeId) -> MappedTypeModifiers {
    // TODO(port M6 S7): the declaration-driven modifier computation.
    let _ = t;
    MappedTypeModifiers::NONE
}

/// Go: `func (c *Checker) getCombinedMappedTypeOptionality(t) int`.
pub(crate) fn get_combined_mapped_type_optionality(_c: &mut Checker, t: TypeId) -> i32 {
    // TODO(port M6 S7).
    let _ = t;
    0
}

/// Go: `func (c *Checker) getConstraintTypeFromMappedType(t) *Type`.
pub(crate) fn get_constraint_type_from_mapped_type(_c: &mut Checker, t: TypeId) -> Option<TypeId> {
    // TODO(port M6 S7): the mapped-type constraint type parameter (Go's
    // nullable return ports as Option; None until the mapped-type
    // constructor lands).
    let _ = t;
    None
}

/// Go: `func (c *Checker) getTemplateTypeFromMappedType(t) *Type`.
pub(crate) fn get_template_type_from_mapped_type(c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7).
    let _ = t;
    c.never_type()
}

/// Go: `func (c *Checker) getTypeParameterFromMappedType(t) *Type`.
pub(crate) fn get_type_parameter_from_mapped_type(c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7).
    let _ = t;
    c.never_type()
}

/// Go: `func (c *Checker) getNameTypeFromMappedType(t) *Type`.
pub(crate) fn get_name_type_from_mapped_type(_c: &mut Checker, t: TypeId) -> Option<TypeId> {
    // TODO(port M6 S7).
    let _ = t;
    None
}

/// Go: `target.AsMappedType().declaration.NameType == nil`.
pub(crate) fn mapped_type_name_type_is_none(c: &mut Checker, t: TypeId) -> bool {
    get_name_type_from_mapped_type(c, t).is_none()
}

/// Go: `func (c *Checker) isMappedTypeWithKeyofConstraintDeclaration(t) bool`.
pub(crate) fn is_mapped_type_with_keyof_constraint_declaration(_c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7).
    let _ = t;
    false
}

/// Go: `func (c *Checker) shouldDeferIndexType(target, indexFlags) bool`.
pub(crate) fn should_defer_index_type(_c: &mut Checker, t: TypeId, _index_flags: IndexFlags) -> bool {
    // TODO(port M6 S10).
    let _ = t;
    false
}

// ── Instantiation seams (S7) ──

/// Go: `func (c *Checker) instantiateType(t, mapper) *Type` — the W1
/// checker.rs identity stub threaded as a free function.
pub(crate) fn instantiate_type_ex(c: &mut Checker, t: TypeId, mapper: Option<crate::todo_shapes::TypeMapperId>) -> TypeId {
    match mapper {
        Some(mapper) => c.instantiate_type(t, mapper),
        None => t,
    }
}

/// Go: the `mapper` variable (possibly nil) in the conditional-vs-conditional
/// arm — the same call with the Option surfaced.
pub(crate) fn instantiate_type_opt(c: &mut Checker, t: TypeId, mapper: Option<crate::todo_shapes::TypeMapperId>) -> TypeId {
    instantiate_type_ex(c, t, mapper)
}

/// Go: `c.instantiateType(s, c.reportUnreliableMapper)` — the mapper is nil
/// until S7 constructs the report mappers; nil-instantiation is identity.
pub(crate) fn report_unreliable_instantiate(c: &mut Checker, t: TypeId) {
    // TODO(port M6 S7): the reportUnreliableMapper instantiation.
    let _ = instantiate_type_ex(c, t, None);
}

/// Go: `c.reportUnreliableMapper` / `c.reportUnmeasurableMapper`.
pub(crate) fn report_unreliable_mapper(c: &mut Checker) -> Option<crate::todo_shapes::TypeMapperId> {
    // TODO(port M6 S7): the mapper.go report mappers.
    let _ = c;
    None
}

pub(crate) fn report_unmeasurable_mapper(c: &mut Checker) -> Option<crate::todo_shapes::TypeMapperId> {
    // TODO(port M6 S7).
    let _ = c;
    None
}

/// Go: `newSimpleTypeMapper(source, target)` (mapper.go).
pub(crate) fn new_simple_type_mapper(c: &mut Checker, source: TypeId, target: TypeId) -> crate::todo_shapes::TypeMapperId {
    c.new_simple_type_mapper(source, target)
}

/// Go: `func (c *Checker) getErasedSignature(s) *Signature`.
pub(crate) fn get_erased_signature(c: &mut Checker, s: crate::types::SignatureId) -> crate::types::SignatureId {
    // TODO(port M6 S6): eraseTypeParameters.
    let _ = c;
    s
}

/// Go: `c.stringNumberSymbolType` — the keyof domain union.
pub(crate) fn get_string_number_symbol_type(c: &mut Checker) -> TypeId {
    // TODO(port M6 S7): the string | number | symbol union.
    c.string_type()
}

/// Go: `c.anyFunctionType` (None until the signature-construction wave).
pub(crate) fn any_function_type(c: &mut Checker) -> Option<TypeId> {
    // TODO(port M6 S7): the anyFunctionType wildcard signature type.
    let _ = c;
    None
}

/// Go: `func (c *Checker) getGlobalESSymbolType() *Type`.
pub(crate) fn get_global_es_symbol_type(c: &mut Checker) -> Option<TypeId> {
    // TODO(port M6 S2): the getGlobal* resolver.
    let _ = c;
    None
}

/// Go: `func (c *Checker) getExactOptionalUnassignableProperties(source, target) []*ast.Symbol`.
pub(crate) fn get_exact_optional_unassignable_properties(c: &mut Checker, source: TypeId, target: TypeId) -> Vec<SymbolId> {
    // TODO(port M6 S8): the exactOptionalPropertyTypes property walk.
    let _ = (c, source, target);
    Vec::new()
}

/// Go: `func (c *Checker) getSingleBaseForNonAugmentingSubtype(t) *Type`.
pub(crate) fn get_single_base_for_non_augmenting_subtype(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    // TODO(port M6 S7/S8): the single-base resolution.
    let _ = (c, t);
    None
}

/// Go: `func (c *Checker) cloneTypeParameter(t) *Type`.
pub(crate) fn clone_type_parameter(c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7): the clone-and-clear for the synthetic
    // this-type-parameter-might-need constraint elaboration; identity (the
    // constraint write below is harmless on the original until then).
    let _ = c;
    t
}

/// Go: the synthetic type-parameter constraint write.
pub(crate) fn set_type_parameter_constraint(c: &mut Checker, t: TypeId, constraint: TypeId) {
    if let Some(data) = c.type_mut(t).as_type_parameter_mut() {
        data.constraint = Some(constraint);
    }
}

// ── Symbol/declaration helpers ──

/// Go: `prop.ValueDeclaration`.
pub(crate) fn symbol_value_declaration(c: &Checker, sym: SymbolId) -> Option<NodeId> {
    c.program_symbol(sym).value_declaration
}

/// Go: `getDeclarationModifierFlagsFromSymbol(sym)`.
pub(crate) fn get_declaration_modifier_flags_from_symbol(c: &Checker, sym: SymbolId) -> ModifierFlags {
    // TODO(port M6 S6): the declaration modifier-flags read.
    let _ = (c, sym);
    ModifierFlags::NONE
}

/// Go: `func (c *Checker) isValidOverrideOf(sourceProp, targetProp) bool`.
pub(crate) fn is_valid_override_of(c: &mut Checker, source_prop: SymbolId, target_prop: SymbolId) -> bool {
    // TODO(port M6 S8): the protected-override relation check. Permissive.
    let _ = (c, source_prop, target_prop);
    true
}

/// Go: `func (c *Checker) isReadonlySymbol(sym) bool`.
pub(crate) fn is_readonly_symbol(c: &mut Checker, sym: SymbolId) -> bool {
    // TODO(port M6 S6): the readonly modifier read.
    let _ = (c, sym);
    false
}

/// Go: `func (c *Checker) getDeclaringClass(prop) *Type`.
pub(crate) fn get_declaring_class(c: &mut Checker, prop: SymbolId) -> Option<TypeId> {
    // TODO(port M6 S8): the protected-property's declaring class.
    let _ = (c, prop);
    None
}

/// Go: `declaration.ModifierFlags()` (the node-level modifier read).
pub(crate) fn modifier_flags_of_node(c: &Checker, node: NodeId) -> ModifierFlags {
    c.store().node(node).modifier_flags()
}

/// Go: `func (c *Checker) getTypeWithFacts(t, facts) *Type` — S10 stub.
pub(crate) fn get_type_with_facts(_c: &mut Checker, t: TypeId, _facts: TypeFacts) -> TypeId {
    // TODO(port M6 S10): the facts-based type construction.
    t
}

/// Go: `func (c *Checker) hasCovariantVoidArgument(typeArguments, variances) bool`
/// (checker method form).
pub(crate) fn has_covariant_void_argument(c: &mut Checker, type_arguments: &[TypeId], variances: &[VarianceFlags]) -> bool {
    c.has_covariant_void_argument(type_arguments, variances)
}

/// Go: `func (c *Checker) getBaseTypeOfLiteralType(t) *Type`.
pub(crate) fn get_base_type_of_literal_type(c: &mut Checker, t: TypeId) -> TypeId {
    let flags = c.type_(t).flags;
    if flags.intersects(TypeFlags::STRING_LIKE) {
        return c.string_type();
    }
    if flags.intersects(TypeFlags::NUMBER_LIKE) {
        return c.number_type();
    }
    if flags.intersects(TypeFlags::BIGINT_LIKE) {
        return c.bigint_type.expect("bigintType");
    }
    if flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
        // Go: booleanType — the true | false union (S7). The regular boolean
        // literals route through their base; the union isn't constructible
        // yet, so the strict-subtype surface stays permissive.
        // TODO(port M6 S7): booleanType union.
        return c.error_type();
    }
    c.error_type()
}

/// Go: `func (c *Checker) getBaseTypeOfEnumLikeType(t) *Type` — S2 stub.
pub(crate) fn get_base_type_of_enum_like_type(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    // TODO(port M6 S2): getBaseTypeOfEnumLikeType.
    let _ = (c, t);
    None
}

/// Go: `func (c *Checker) hasBaseType(t, s) bool` — S8 stub.
#[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn has_base_type(c: &mut Checker, t: TypeId, s: TypeId) -> bool {
    // TODO(port M6 S8): the base-type walk.
    let _ = (c, t, s);
    false
}

/// Go: `func (c *Checker) isFunctionObjectType(t) bool` — S8 stub.
#[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn is_function_object_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S8): the Function-global check.
    let _ = (c, t);
    false
}

/// Go: `getStringLiteralValue(t)` (utilities.go).
pub(crate) fn get_string_literal_value(c: &Checker, t: TypeId) -> String {
    match c.type_(t).as_literal_type().and_then(|d| d.value.as_ref()) {
        Some(crate::types::LiteralValue::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Go: `func (c *Checker) getTemplateLiteralType(texts, types) *Type` — S7
/// stub.
pub(crate) fn get_template_literal_type(c: &mut Checker, texts: &[String], types: &[TypeId]) -> TypeId {
    // TODO(port M6 S7): the template-literal constructor.
    let _ = (texts, types);
    c.never_type()
}

/// Go: `func (c *Checker) getStringMappingType(symbol, t) *Type` — S7 stub.
pub(crate) fn get_string_mapping_type(c: &mut Checker, _symbol: Option<SymbolId>, t: TypeId) -> TypeId {
    // TODO(port M6 S7): the string-mapping constructor.
    let _ = t;
    c.never_type()
}

/// Go: stringutil.CombineSurrogatePairs.
fn combine_surrogate_pairs(s: &str) -> String {
    let bytes = tsc_stringutil::combine_surrogate_pairs(s.as_bytes());
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Go: `_, size := stringutil.DecodeJSStringRune(s)` — the consumed byte
/// count of the next code point.
fn decode_js_string_rune(s: &str) -> usize {
    tsc_stringutil::decode_js_string_rune(s.as_bytes()).1
}

/// Go: `isValidNumberString(value, roundTripOnly)` — jsnum seam.
fn is_valid_number_string(_value: &str, _round_trip_only: bool) -> bool {
    // TODO(port M6 W4/jsnum): the number-string round-trip validator.
    false
}

/// Go: `isValidBigIntString(value, roundTripOnly)`.
fn is_valid_big_int_string(_value: &str, _round_trip_only: bool) -> bool {
    // TODO(port M6 W4/jsnum): the big-int-string validator.
    false
}

/// Go: `func (c *Checker) compareProperties(sourceProp, targetProp, compareTypes) Ternary`
/// (checker.go's property-identity fastpath + comparer).
pub(crate) fn compare_properties(c: &mut Checker, source_prop: SymbolId, target_prop: SymbolId, kind: TypeComparerKind, relater: Option<&mut Relater>) -> Ternary {
    if source_prop == target_prop {
        return Ternary::True;
    }
    let source_type = c.get_type_of_symbol(source_prop);
    let target_type = c.get_type_of_symbol(target_prop);
    match kind {
        TypeComparerKind::Identical => c.compare_types_identical(source_type, target_type),
        TypeComparerKind::Assignable => c.compare_types_assignable_simple(source_type, target_type),
        TypeComparerKind::SubtypeOf => c.compare_types_subtype_of(source_type, target_type),
        TypeComparerKind::RelatedToSimple => invoke_comparer(c, kind, relater, source_type, target_type, false),
        TypeComparerKind::RelatedToWorker => invoke_comparer(c, kind, relater, source_type, target_type, false),
    }
}

/// Go: `func (c *Checker) getSingleCallSignatureOfType(t) *Signature` — S8
/// stub (the arrow-function elaboration gate).
pub(crate) fn get_single_call_signature_of_type(_c: &mut Checker, t: TypeId) -> Option<crate::types::SignatureId> {
    // TODO(port M6 S8).
    let _ = t;
    None
}

// ── Inference seams ──
// (M6 W2: the real inference engine lives in inference.rs — the
// InferencePriority ladder's provisional stub values here were WRONG vs Go's
// signed int32 ladder (NO_CONSTRAINTS is 1<<9, ALWAYS_STRICT is 1<<10, and
// InferenceFlags's real bits are the checker.go five); the conditional arm
// below now routes through crate::inference's real surface.)

// ────────────────────────────────────────────────────────────────────────────
// Checker method bridges — the free-function stubs above surface as Checker
// methods (Go's method syntax; the W1 code in the upper half of this file and
// the signature ladder call them as methods). Each delegates to the bannered
// free stub unless the body is arena-local.
// ────────────────────────────────────────────────────────────────────────────

// TODO(port M6 W2): the W2 stub bridges are called from the inference and
// services waves that follow.
#[allow(dead_code)]
impl Checker {
    /// Go: `func (c *Checker) createTypeReference(target, typeArguments, aliasSymbol, aliasTypeArguments) *Type`.
    pub(crate) fn create_type_reference(&mut self, target: TypeId, type_arguments: Vec<TypeId>) -> TypeId {
        // TODO(port M6 S7): the type-reference constructor; the identity
        // keeps the marker-type recursion keys stable for the relater.
        let _ = type_arguments;
        target
    }

    /// Go: `func (c *Checker) getTypeAliasInstantiation(alias, typeArguments) *Type`.
    pub(crate) fn get_type_alias_instantiation(&mut self, symbol: SymbolId, type_arguments: Vec<TypeId>) -> TypeId {
        // TODO(port M6 S7/W4): the alias instantiation.
        let _ = (symbol, type_arguments);
        self.error_type()
    }

    /// Go: `func (c *Checker) TypeToString(t) string` (checker.go) — the
    /// W4-stub free fn, surfaced as the Go method form.
    pub(crate) fn type_to_string(&mut self, t: TypeId) -> String {
        type_to_string(self, t)
    }

    /// Go: `func (c *Checker) TypeToStringEx(t, enclosingDeclaration, flags, tracker) string`
    /// — W4 stub (the error-display form).
    pub(crate) fn type_to_string_ex(&mut self, t: TypeId) -> String {
        type_to_string(self, t)
    }

    /// Go: `func (c *Checker) isEmptyAnonymousObjectType(t) bool`.
    pub(crate) fn is_empty_anonymous_object_type(&mut self, t: TypeId) -> bool {
        is_empty_anonymous_object_type(self, t)
    }

    /// Go: `func (c *Checker) getNonNullableType(t) *Type` (utilities.go).
    pub(crate) fn get_non_nullable_type(&mut self, t: TypeId) -> TypeId {
        get_non_nullable_type(self, t)
    }

    /// Go: `func (c *Checker) getTypeWithFacts`-shaped `getTypeFacts(t, facts)`
    /// — S10 stub (equal-facts fallback: the facts query is only consulted
    /// on undefined-containing types the current surface can't build).
    pub(crate) fn get_type_facts(&mut self, t: TypeId, facts: TypeFacts) -> TypeFacts {
        // TODO(port M6 S10): the TypeFacts lattice worker.
        let _ = t;
        facts
    }

    /// Go: `func (c *Checker) getSingleCallSignatureOfType(t) *Signature`.
    pub(crate) fn get_single_call_signature_of_type(&mut self, t: TypeId) -> Option<crate::types::SignatureId> {
        get_single_call_signature_of_type(self, t)
    }

    /// Go: `func (c *Checker) getIndexedAccessType(t, key) *Type`.
    pub(crate) fn get_indexed_access_type(&mut self, t: TypeId, key: TypeId) -> TypeId {
        get_indexed_access_type(self, t, key)
    }

    /// Go: `func (c *Checker) getElementTypeOfArrayType(t) *Type`.
    pub(crate) fn get_element_type_of_array_type(&mut self, t: TypeId) -> Option<TypeId> {
        // TODO(port M6 S7): the resolvedTypeArguments[0] read.
        let _ = t;
        None
    }

    /// Go: `func (c *Checker) createArrayType(elementType) *Type`.
    pub(crate) fn create_array_type(&mut self, element_type: TypeId) -> TypeId {
        // TODO(port M6 S7): the globalArrayType reference construction; the
        // identity preserves element type for the rest-type ladder.
        element_type
    }

    /// Go: `func (c *Checker) createTupleTypeEx(elementTypes, elementInfos, readonly) *Type`.
    pub(crate) fn create_tuple_type_ex(
        &mut self,
        element_types: &[TypeId],
        _element_infos: &[crate::types::TupleElementInfo],
        _readonly: bool,
    ) -> TypeId {
        // TODO(port M6 S7): the tuple constructor.
        if let Some(&first) = element_types.first() {
            first
        } else {
            self.never_type()
        }
    }

    /// Go: `func (c *Checker) sliceTupleType(t, index, endSkipCount) *Type`.
    pub(crate) fn slice_tuple_type(&mut self, t: TypeId, _index: usize, _end_skip_count: usize) -> TypeId {
        // TODO(port M6 S7): the tuple-slice construction.
        t
    }

    /// Go: `func (c *Checker) isGenericType(t) bool`.
    pub(crate) fn is_generic_type(&mut self, t: TypeId) -> bool {
        // TODO(port M6 S7): the IS_GENERIC object-flags check.
        let _ = t;
        false
    }

    /// Go: the predicate combiner's union/intersection construction.
    pub(crate) fn get_union_or_intersection_type(&mut self, types: &[TypeId], _is_union: bool) -> TypeId {
        // TODO(port M6 S7): the union/intersection constructors.
        if let Some(&first) = types.first() {
            first
        } else {
            self.never_type()
        }
    }

    /// Go: `func (c *Checker) getIntersectionTypeEx(types, intersectionFlags, alias) *Type`.
    pub(crate) fn get_intersection_type_ex(
        &mut self,
        types: &[TypeId],
        flags: IntersectionFlags,
        alias: Option<Box<crate::types::TypeAlias>>,
    ) -> TypeId {
        get_intersection_type_ex(self, types.to_vec(), flags, alias)
    }

    /// Go: `func (c *Checker) getNormalizedType(t, writing) *Type`.
    pub(crate) fn get_normalized_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        get_normalized_type(self, t, writing)
    }

    /// Go: `func (c *Checker) getBaseTypeOfEnumLikeType(t) *Type`.
    pub(crate) fn get_base_type_of_enum_like_type(&mut self, t: TypeId) -> Option<TypeId> {
        get_base_type_of_enum_like_type(self, t)
    }

    /// Go: `func (c *Checker) isFunctionObjectType(t) bool`.
    pub(crate) fn is_function_object_type(&mut self, t: TypeId) -> bool {
        is_function_object_type(self, t)
    }

    /// Go: `func (c *Checker) hasBaseType(t, s) bool`.
    pub(crate) fn has_base_type(&mut self, t: TypeId, s: TypeId) -> bool {
        has_base_type(self, t, s)
    }

    /// Go: `func (c *Checker) isReadonlyArrayType(t) bool`.
    pub(crate) fn is_readonly_array_type(&mut self, t: TypeId) -> bool {
        is_readonly_array_type(self, t)
    }

    // Go: `func (c *Checker) getConstraintOrUnknownFromTypeParameter(t) *Type`
    // — REMOVED (M6 S2): superseded by typeofsymbol.rs's real body (same
    // signature; this module's call sites resolve there).

    /// Go: `func (c *Checker) getDefaultOrUnknownFromTypeParameter(t) *Type`.
    pub(crate) fn get_default_or_unknown_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        // Go: the resolvedDefaultType payload read.
        self.type_(t)
            .as_type_parameter()
            .and_then(|d| d.resolved_default_type)
            .unwrap_or_else(|| self.unknown_type())
    }

    /// Go: `func (c *Checker) instantiateSignatureEx(s, mapper, eraseTypeParameters) *Signature`.
    pub(crate) fn instantiate_signature_ex(
        &mut self,
        s: crate::types::SignatureId,
        _mapper: Option<crate::todo_shapes::TypeMapperId>,
        _erase_type_parameters: bool,
    ) -> crate::types::SignatureId {
        // TODO(port M6 S7): the signature instantiator.
        s
    }

    /// Go: `func (c *Checker) getCanonicalSignature(s) *Signature`.
    pub(crate) fn get_canonical_signature(&mut self, s: crate::types::SignatureId) -> crate::types::SignatureId {
        // TODO(port M6 S6): the canonical-signature resolution.
        s
    }

    /// Go: `func (c *Checker) instantiateSignatureInContextOf(source, target, compareTypes) *Signature`.
    pub(crate) fn instantiate_signature_in_context_of(
        &mut self,
        source: crate::types::SignatureId,
        _target: crate::types::SignatureId,
        _compare_types: TypeComparerKind,
    ) -> crate::types::SignatureId {
        // TODO(port M6 S6): the type-parameter fixing instantiator.
        source
    }

    /// Go: the IndexInfo payload readers (the `index_infos` arena accessors).
    pub(crate) fn index_info_key_type(&self, info: crate::types::IndexInfoId) -> TypeId {
        self.index_infos[info.0 as usize].key_type.expect("IndexInfo key type")
    }

    pub(crate) fn index_info_value_type(&self, info: crate::types::IndexInfoId) -> TypeId {
        self.index_infos[info.0 as usize].value_type.expect("IndexInfo value type")
    }

    pub(crate) fn index_info_is_readonly(&self, info: crate::types::IndexInfoId) -> bool {
        self.index_infos[info.0 as usize].is_readonly
    }

    /// Go: the IndexInfo payload `declaration` read (the elaboration path).
    pub(crate) fn index_info_declaration(&self, info: crate::types::IndexInfoId) -> Option<NodeId> {
        self.index_infos[info.0 as usize].declaration
    }

    // ── Error reporting (checker.go's report* family) — the errorReporter
    // threading needs the live Relater; the S6 diagnostic plumbing wave owns
    // the real bodies. The signature ladder's calls become no-ops until
    // then (error elaboration is already gated behind typeToString (W4) and
    // member resolution (S8) seams).

    /// Go: `func (c *Checker) reportSignatureArityError(errorReporter, source, targetCount)`.
    pub(crate) fn report_signature_arity_error(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
        _source: crate::types::SignatureId,
        _target_count: usize,
    ) {
        // TODO(port M6 S6): the errorReporter threading.
    }

    /// Go: `func (c *Checker) reportIncompatibleThisTypes(errorReporter, source, target)`.
    pub(crate) fn report_incompatible_this_types(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
    ) {
        // TODO(port M6 S6): as above.
    }

    /// Go: `func (c *Checker) reportParameterTypeMismatch(errorReporter, source, target, i)`.
    pub(crate) fn report_parameter_type_mismatch(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
        _source: crate::types::SignatureId,
        _target: crate::types::SignatureId,
        _i: usize,
    ) {
        // TODO(port M6 S6): as above.
    }

    /// Go: `func (c *Checker) reportPredicateRequired(errorReporter, signature)`.
    pub(crate) fn report_predicate_required(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
        _source: crate::types::SignatureId,
    ) {
        // TODO(port M6 S6): as above.
    }

    /// Go: `func (c *Checker) reportReturnTypeMismatch(errorReporter, source, sourceReturnType, targetReturnType)`.
    pub(crate) fn report_return_type_mismatch(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
        _source: crate::types::SignatureId,
        _source_return_type: TypeId,
        _target_return_type: TypeId,
    ) {
        // TODO(port M6 S6): as above.
    }

    /// Go: `func (c *Checker) reportPredicateKindMismatch(errorReporter, source, target)`.
    pub(crate) fn report_predicate_kind_mismatch(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
        _source: &TypePredicate,
        _target: &TypePredicate,
    ) {
        // TODO(port M6 S6): as above.
    }

    /// Go: `func (c *Checker) reportPredicatePositionMismatch(errorReporter, source, target)`.
    pub(crate) fn report_predicate_position_mismatch(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
        _source: &TypePredicate,
        _target: &TypePredicate,
    ) {
        // TODO(port M6 S6): as above.
    }

    /// Go: `func (c *Checker) reportPredicateIncompatible(errorReporter, source, target)`.
    pub(crate) fn report_predicate_incompatible(
        &mut self,
        _error_reporter: Option<ErrorReporterKind>,
        _source: &TypePredicate,
        _target: &TypePredicate,
    ) {
        // TODO(port M6 S6): as above.
    }
}

// ── Missing free helpers (referenced above) ──

/// Go: `func isUnknownLikeUnionType(t) bool` (utilities.go).
pub(crate) fn is_unknown_like_union_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S7/S8): the union-contains-undefined/null/{} check.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) getReturnTypeOfSignature(sig) *Type` — S8 stub
/// (free-fn form; the method lives in exports.rs).
pub(crate) fn get_return_type_of_signature(c: &mut Checker, sig: crate::types::SignatureId) -> TypeId {
    // TODO(port M6 S8): resolvedReturnType + getReturnTypeFromBody.
    c.get_return_type_of_signature(sig)
}

/// Go: `func (c *Checker) getDeclaredTypeOfSymbol(symbol) *Type`.
/// PORT (M6 S2): the W2 error-type stub flipped to the real family
/// (typeofsymbol.rs, via the exports.rs Get* twin).
pub(crate) fn get_declared_type_of_symbol(c: &mut Checker, symbol: SymbolId) -> TypeId {
    c.get_declared_type_of_symbol(symbol)
}

/// Go: `func (c *Checker) getTypeOfParameter(p) *Type` — the parameter symbol
/// read (getTypeOfSymbol on a parameter symbol).
pub(crate) fn get_type_of_parameter(c: &mut Checker, p: SymbolId) -> TypeId {
    c.get_type_of_symbol(p)
}

/// Go: `func (c *Checker) getConstraintFromTypeParameter(t) *Type` — the
/// S9 constrained-reference arm; payload read.
#[allow(dead_code)] // TODO(port M6 W2): Go-mirrored; callers land with inference.rs
pub(crate) fn get_constraint_from_type_parameter(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    get_constraint_of_type_parameter(c, t)
}

/// Go: `c.filterType(reducedTarget, isExcessPropertyCheckTarget)`.
pub(crate) fn filter_type_is_excess_property_check_target(c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7): the filter construction; the first match or the
    // reduced type stands in.
    if is_excess_property_check_target(c, t) {
        t
    } else {
        c.never_type()
    }
}

/// Go: `func (c *Checker) getPermissiveInstantiation(t) *Type` — S7 stub.
pub(crate) fn get_permissive_instantiation(_c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7): the permissive mapper.
    t
}

/// Go: `func (c *Checker) getRestrictiveInstantiation(t) *Type` — S7 stub.
pub(crate) fn get_restrictive_instantiation(_c: &mut Checker, t: TypeId) -> TypeId {
    // TODO(port M6 S7): the restrictive mapper.
    t
}

/// Go: the tuple payload readers (the tuple-comparison ladder; TupleType
/// fields behind the S7 constructor, but the payload reads are real).
pub(crate) fn tuple_type_readonly(c: &Checker, t: TypeId) -> bool {
    // Go: TargetTupleType().readonly — the tuple payload's target read.
    c.type_(t)
        .target()
        .and_then(|target| c.type_(target).as_tuple_type().map(|d| d.readonly))
        .unwrap_or(false)
}

/// Go: `target.TargetTupleType().combinedFlags`.
pub(crate) fn tuple_type_combined_flags(c: &Checker, t: TypeId) -> crate::types::ElementFlags {
    c.type_(t)
        .target()
        .and_then(|target| c.type_(target).as_tuple_type().map(|d| d.combined_flags))
        .unwrap_or(crate::types::ElementFlags::NONE)
}

/// Go: `target.TargetTupleType().minLength`.
pub(crate) fn tuple_type_min_length(c: &Checker, t: TypeId) -> usize {
    c.type_(t)
        .target()
        .and_then(|target| c.type_(target).as_tuple_type().map(|d| d.min_length as usize))
        .unwrap_or(0)
}

/// Go: `tuple.TargetTupleType().elementInfos[pos].flags`.
pub(crate) fn tuple_type_element_flags(c: &Checker, t: TypeId, pos: usize) -> crate::types::ElementFlags {
    c.type_(t)
        .target()
        .and_then(|target| {
            c.type_(target)
                .as_tuple_type()
                .and_then(|d| d.element_infos.get(pos).map(|e| e.flags))
        })
        .unwrap_or(crate::types::ElementFlags::NONE)
}

/// Go: `getStartElementCount(tuple, flags)` (utilities.go).
pub(crate) fn get_start_element_count(c: &Checker, t: TypeId, flags: crate::types::ElementFlags) -> usize {
    // TODO(port M6 S7): the element-flag prefix scan.
    let _ = flags;
    let _ = c.type_(t);
    0
}

/// Go: `getEndElementCount(tuple, flags)` (utilities.go).
pub(crate) fn get_end_element_count(c: &Checker, t: TypeId, flags: crate::types::ElementFlags) -> usize {
    // TODO(port M6 S7): the element-flag suffix scan.
    let _ = flags;
    let _ = c.type_(t);
    0
}

/// Go: `conditional_parts_ex` — the same `(checkType, extendsType)` pair
/// (the worker's conditional arm; kept as a distinct name for grep parity).
pub(crate) fn conditional_parts_ex(c: &Checker, t: TypeId) -> (TypeId, TypeId) {
    conditional_parts(c, t)
}

/// Go: the booleanType placeholder — `booleanType` is the `true | false`
/// union (S7's getUnionType); the wrapper-object elaboration that reads it
/// is unreachable until the S2 globals resolve, so the error type stands in.
pub(crate) fn boolean_type_placeholder(c: &Checker) -> TypeId {
    // TODO(port M6 S7): the booleanType union.
    c.error_type()
}
