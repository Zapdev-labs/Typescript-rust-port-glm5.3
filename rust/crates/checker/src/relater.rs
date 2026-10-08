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

use rustc_hash::FxHashMap;
use tsc_ast::{ModifierFlags, NodeId, SymbolFlags, SymbolId};
use tsc_diagnostics::Message;
use tsc_diagnostics::messages_generated as msg;

use crate::types::{
    format_type_flags, ElementFlags, IndexFlags, IndexInfo, SignatureKind, Ternary, TypeFlags,
    TypeId, TypePredicate, TypePredicateKind, VarianceFlags,
};
use crate::utilities as u;
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// Flag/enum types (Go relater.go top-of-file)
// ────────────────────────────────────────────────────────────────────────────

define_flags!(SignatureCheckMode, u32);
impl SignatureCheckMode {
    pub const NONE: SignatureCheckMode = SignatureCheckMode(0);
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
    pub const NONE: MinArgumentCountFlags = MinArgumentCountFlags(0);
    pub const STRONG_ARITY_FOR_UNTYPED_JS: MinArgumentCountFlags = MinArgumentCountFlags(1 << 0);
    pub const VOID_IS_NON_OPTIONAL: MinArgumentCountFlags = MinArgumentCountFlags(1 << 1);
}

define_flags!(IntersectionState, u32);
impl IntersectionState {
    pub const NONE: IntersectionState = IntersectionState(0);
    /// Source type is a constituent of an outer intersection
    pub const SOURCE: IntersectionState = IntersectionState(1 << 0);
    /// Target type is a constituent of an outer intersection
    pub const TARGET: IntersectionState = IntersectionState(1 << 1);
}

define_flags!(RecursionFlags, u32);
impl RecursionFlags {
    pub const NONE: RecursionFlags = RecursionFlags(0);
    pub const SOURCE: RecursionFlags = RecursionFlags(1 << 0);
    pub const TARGET: RecursionFlags = RecursionFlags(1 << 1);
    pub const BOTH: RecursionFlags = RecursionFlags(Self::SOURCE.0 | Self::TARGET.0);
}

define_flags!(ExpandingFlags, u8);
impl ExpandingFlags {
    pub const NONE: ExpandingFlags = ExpandingFlags(0);
    pub const SOURCE: ExpandingFlags = ExpandingFlags(1 << 0);
    pub const TARGET: ExpandingFlags = ExpandingFlags(1 << 1);
    pub const BOTH: ExpandingFlags = ExpandingFlags(Self::SOURCE.0 | Self::TARGET.0);
}

define_flags!(RelationComparisonResult, u32);
impl RelationComparisonResult {
    pub const NONE: RelationComparisonResult = RelationComparisonResult(0);
    pub const SUCCEEDED: RelationComparisonResult = RelationComparisonResult(1 << 0);
    pub const FAILED: RelationComparisonResult = RelationComparisonResult(1 << 1);
    pub const REPORTS_UNMEASURABLE: RelationComparisonResult = RelationComparisonResult(1 << 3);
    pub const REPORTS_UNRELIABLE: RelationComparisonResult = RelationComparisonResult(1 << 4);
    pub const COMPLEXITY_OVERFLOW: RelationComparisonResult = RelationComparisonResult(1 << 5);
    pub const REPORTS_MASK: RelationComparisonResult =
        RelationComparisonResult(Self::REPORTS_UNMEASURABLE.0 | Self::REPORTS_UNRELIABLE.0);
    pub const OVERFLOW: RelationComparisonResult = Self::COMPLEXITY_OVERFLOW;
}

/// Go: checker.go `type MappedTypeModifiers uint32` (values pinned).
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
struct KeyBuilder {
    inline_length: usize,
    overflow_buffer: Vec<u8>,
    inline_buffer: [u8; 192],
}

impl KeyBuilder {
    fn new() -> Self {
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
    fn hash(&self) -> crate::todo_shapes::CacheHashKey {
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

    fn write_byte(&mut self, c: u8) {
        if self.inline_length == self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length] = c;
        self.inline_length += 1;
    }

    fn write_string(&mut self, s: &str) {
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

    fn write_uint32(&mut self, v: u32) {
        if self.inline_length + 4 > self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length..self.inline_length + 4]
            .copy_from_slice(&v.to_le_bytes());
        self.inline_length += 4;
    }

    fn write_uint64(&mut self, v: u64) {
        if self.inline_length + 8 > self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length..self.inline_length + 8]
            .copy_from_slice(&v.to_le_bytes());
        self.inline_length += 8;
    }

    fn write_int(&mut self, value: i64) {
        self.write_uint64(value as u64);
    }

    fn write_symbol(&mut self, s: SymbolId) {
        self.write_uint64(s.0 as u64);
    }

    fn write_type(&mut self, t: TypeId) {
        self.write_uint32(t.0);
    }

    fn write_types(&mut self, types: &[TypeId]) {
        self.write_int(types.len() as i64);
        for &t in types {
            self.write_type(t);
        }
    }

    fn write_alias(&mut self, alias: Option<&crate::types::TypeAlias>) {
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
                        let index = type_parameters
                            .iter()
                            .position(|&tp| tp == t)
                            .unwrap_or_else(|| {
                                type_parameters.push(t);
                                type_parameters.len() - 1
                            });
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
        if u::is_fresh_literal(c, source) {
            source = c
                .type_(source)
                .as_literal_type()
                .expect("LiteralType data")
                .regular_type
                .expect("regularType");
        }
        if u::is_fresh_literal(c, target) {
            target = c
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
                && !c.type_(target).flags.intersects(TypeFlags::NEVER)
                && c.is_simple_type_related_to(target, source, relation, None)
                || c.is_simple_type_related_to(source, target, relation, None)
            {
                return true;
            }
        } else if !c
            .type_(source)
            .flags
            .union(c.type_(target).flags)
            .intersects(
                TypeFlags::UNION_OR_INTERSECTION
                    | TypeFlags::INDEXED_ACCESS
                    | TypeFlags::CONDITIONAL
                    | TypeFlags::SUBSTITUTION,
            )
        {
            // We have excluded types that may simplify to other forms, so
            // types must have identical flags (Go comment, verbatim).
            if c.type_(source).flags != c.type_(target).flags {
                return false;
            }
            if c.type_(source).flags.intersects(TypeFlags::SINGLETON) {
                return true;
            }
        }
        if c.type_(source).flags.intersects(TypeFlags::OBJECT)
            && c.type_(target).flags.intersects(TypeFlags::OBJECT)
        {
            let (id, _) = get_relation_key(c, source, target, IntersectionState::NONE, relation == RelationKind::Identity, false);
            let related = c.relation_ref(relation).get(id);
            if related != RelationComparisonResult::NONE {
                return related.intersects(RelationComparisonResult::SUCCEEDED);
            }
        }
        if c.type_(source).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
            || c.type_(target).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
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
            self.get_parent_of_symbol(source)
        } else {
            source
        };
        let target_symbol = if self.program_symbol(target).flags.intersects(SymbolFlags::ENUM_MEMBER) {
            self.get_parent_of_symbol(target)
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
            let diagnostic = u::new_diagnostic_for_node(
                self,
                error_node,
                tsc_diagnostics::messages_generated::EXCESSIVE_COMPLEXITY_COMPARING_TYPES_0_AND_1,
                &[],
            );
            report_diagnostic(self, diagnostic, diagnostic_output.as_deref_mut());
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
            let diagnostic = create_diagnostic_chain_from_error_chain(chain, r.error_node, related_info);
            report_diagnostic(self, diagnostic, diagnostic_output.as_deref_mut());
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
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        if self.is_type_related_to(source, target, relation) {
            return true;
        }
        if let Some(error_node) = error_node {
            let mut r = self.get_relater();
            let elaborated = r.elaborate_error(
                self,
                expr,
                source,
                target,
                relation,
                head_message,
                diagnostic_output.as_deref_mut(),
            );
            self.put_relater(r);
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

/// Go: `func createDiagnosticChainFromErrorChain(chain, errorNode, relatedInfo) *ast.Diagnostic`.
fn create_diagnostic_chain_from_error_chain(
    mut chain: Option<Box<ErrorChain>>,
    error_node: Option<NodeId>,
    related_info: Vec<tsc_ast::Diagnostic>,
) -> Option<tsc_ast::Diagnostic> {
    while chain.is_some_and(|c| c.message.elided_in_compatibility_pyramid) {
        chain = chain.and_then(|c| c.next);
    }
    let chain = chain?;
    let next = create_diagnostic_chain_from_error_chain(chain.next, error_node, related_info);
    match next {
        None => Some(
            u::new_diagnostic_for_node_from_parts(
                error_node,
                chain.message,
                &chain.args,
            )
            .set_related_info(related_info),
        ),
        Some(next) => Some(u::new_diagnostic_chain_for_node(next, chain.message, &chain.args)),
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
        None => c.add_diagnostic(diagnostic),
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
            return resolved.signatures.is_empty()
                && resolved.index_infos.is_empty()
                && !resolved.properties.is_empty()
                && resolved.properties.iter().all(|&p| {
                    self.program_symbol(p).flags.intersects(SymbolFlags::OPTIONAL)
                });
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
            return self.type_(t).types().iter().all(|&t| self.is_weak_type(t));
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
        if flags.intersects(TypeFlags::OBJECT) {
            if get_property_of_object_type(self, target_type, name).is_some()
                || get_applicable_index_info_for_name(self, target_type, name).is_some()
                || u::is_late_bound_name(name) && get_index_info_of_type(self, target_type, self.string_type()).is_some()
                || is_comparing_jsx_attributes && is_hyphenated_jsx_name(name)
            {
                return true;
            }
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
            for &t in self.type_(target_type).types() {
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
            return self.type_(t).types().iter().any(|&t| {
                self.type_could_have_top_level_singleton_types(t)
            });
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
        if let Some(tr) = &self.tracer {
            let args = crate::tracing::Args::new();
            args.insert("arity", crate::tracing::TraceValue::Int(type_parameters.len() as i64));
            args.insert(
                "id",
                crate::tracing::TraceValue::Uint(get_declared_type_of_symbol(self, symbol).0 as u64),
            );
            trace_args = Some((args, tr.push(crate::tracing::Phase::CheckTypes, "getVariancesWorker", &args, true)));
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
            let mut min_index = stack_index as usize;
            for i in (stack_index as usize + 1)..self.variance_stack.len() {
                if u::compare_symbols_worker(
                    self,
                    self.variance_stack[i].symbol,
                    self.variance_stack[min_index].symbol,
                ) == Ordering::Less
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
        c.get_type_alias_instantiation(symbol, instantiated)
    } else {
        let type_parameters = c
            .type_(t)
            .as_interface_type()
            .map(|i| i.type_parameters().to_vec())
            .unwrap_or_default();
        let instantiated = instantiate_types(c, type_parameters, mapper);
        c.create_type_reference(t, instantiated)
    };
    c.marker_types.add(result);
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
fn sig(c: &Checker, id: crate::types::SignatureId) -> &crate::types::Signature {
    &c.signatures[id.0 as usize]
}

fn sig_mut(c: &mut Checker, id: crate::types::SignatureId) -> &mut crate::types::Signature {
    &mut c.signatures[id.0 as usize]
}

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
                self.report_signature_arity_error(error_reporter, source, target_count);
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
                        related = invoke_comparer(self, None, compare_types, source_this_type, target_this_type, false);
                    }
                    if related == Ternary::False {
                        related = invoke_comparer(self, None, compare_types, target_this_type, source_this_type, report_errors);
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
                Some(self.get_rest_or_any_type_at_position(source, i))
            } else {
                self.try_get_type_at_position(source, i)
            };
            let target_type = if i == rest_index {
                Some(self.get_rest_or_any_type_at_position(target, i))
            } else {
                self.try_get_type_at_position(target, i)
            };
            let (Some(source_type), Some(target_type)) = (source_type, target_type) else {
                continue;
            };
            if source_type != target_type || check_mode.intersects(SignatureCheckMode::STRICT_ARITY) {
                // In order to ensure that any generic type Foo<T> is at least
                // co-variant with respect to T … we need to relate parameters
                // bi-variantly (Go comment, verbatim).
                let source_sig = if !check_mode.intersects(SignatureCheckMode::CALLBACK)
                    && !self.is_instantiated_generic_parameter(source, i)
                {
                    self.get_single_call_signature_of_type(self.get_non_nullable_type(source_type))
                } else {
                    None
                };
                let target_sig = if !check_mode.intersects(SignatureCheckMode::CALLBACK)
                    && !self.is_instantiated_generic_parameter(target, i)
                {
                    self.get_single_call_signature_of_type(self.get_non_nullable_type(target_type))
                } else {
                    None
                };
                let callbacks = source_sig.is_some()
                    && target_sig.is_some()
                    && self.get_type_predicate_of_signature(source_sig).is_none()
                    && self.get_type_predicate_of_signature(target_sig).is_none()
                    && self.get_type_facts(source_type, TypeFacts::IS_UNDEFINED_OR_NULL)
                        == self.get_type_facts(target_type, TypeFacts::IS_UNDEFINED_OR_NULL);
                let related;
                if callbacks {
                    let mut callback_mode = check_mode & SignatureCheckMode::STRICT_ARITY;
                    callback_mode |= if strict_variance {
                        SignatureCheckMode::STRICT_CALLBACK
                    } else {
                        SignatureCheckMode::BIVARIANT_CALLBACK
                    };
                    related = self.compare_signatures_related(
                        target_sig.expect("checked"),
                        source_sig.expect("checked"),
                        callback_mode,
                        report_errors,
                        error_reporter,
                        compare_types,
                        report_unreliable_markers,
                    );
                } else {
                    let mut r = Ternary::False;
                    if !check_mode.intersects(SignatureCheckMode::CALLBACK) && !strict_variance {
                        r = invoke_comparer(self, None, compare_types, source_type, target_type, false);
                    }
                    related = if r == Ternary::False {
                        invoke_comparer(self, None, compare_types, target_type, source_type, report_errors)
                    } else {
                        r
                    };
                }
                // With strict arity, (x: number | undefined) => void is a
                // subtype of (x?: number | undefined) => void (Go, verbatim).
                if ternary_not_false(related)
                    && check_mode.intersects(SignatureCheckMode::STRICT_ARITY)
                    && i as i32 >= self.get_min_argument_count(source)
                    && (i as i32) < self.get_min_argument_count(target)
                    && invoke_comparer(self, None, compare_types, source_type, target_type, false) != Ternary::False
                {
                    return Ternary::False;
                }
                if related == Ternary::False {
                    if report_errors {
                        self.report_parameter_type_mismatch(error_reporter, source, target, i);
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
                                source_type_predicate,
                                target_type_predicate,
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
                    related = invoke_comparer(self, None, compare_types, target_return_type, source_return_type, false);
                }
                if related == Ternary::False {
                    related = invoke_comparer(self, None, compare_types, source_return_type, target_return_type, report_errors);
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
        if source.kind == TypePredicateKind::Identifier || source.kind == TypePredicateKind::AssertsIdentifier {
            if source.parameter_index != target.parameter_index {
                if report_errors {
                    self.report_predicate_position_mismatch(error_reporter, source, target);
                }
                return Ternary::False;
            }
        }
        let related = match (source.t, target.t) {
            (Some(a), Some(b)) if a == b => Ternary::True,
            (Some(a), Some(b)) => invoke_comparer(self, None, compare_types, a, b, report_errors),
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
        if signature.type_parameters.is_empty()
            && (signature.this_parameter.is_none()
                || is_type_any(self, get_type_of_parameter(self, signature.this_parameter.expect("checked"))))
            && signature.parameters.len() == 1
            && signature.has_rest_parameter()
        {
            let param_type = get_type_of_parameter(self, signature.parameters[0]);
            let rest_type = if self.is_array_type(param_type) {
                get_type_arguments(self, param_type)[0]
            } else {
                param_type
            };
            return self.type_(rest_type).flags.intersects(TypeFlags::ANY | TypeFlags::NEVER)
                && self
                    .type_(self.get_return_type_of_signature(s))
                    .flags
                    .intersects(TypeFlags::ANY_OR_UNKNOWN);
        }
        false
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
            let mut min_argument_count = min_argument_count;
            let mut i = min_argument_count - 1;
            while i >= 0 {
                let t = self.get_type_at_position(signature, i as usize);
                if !some_type(self, t, |c, t| c.type_(t).flags.intersects(TypeFlags::VOID)) {
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
                Some(
                    self.type_(self.type_(rest_type).as_type_reference().expect("ref").base.target.expect("target"))
                        .as_tuple_type()
                        .expect("TupleType data")
                        .clone(),
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
            let indexed = self.get_indexed_access_type(rest_type.expect("checked"), self.number_type());
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
            let flags;
            if rest_type.is_none() || i < length - 1 {
                types[i] = self.get_type_at_position(source, i + pos);
                flags = if i + pos < min_argument_count {
                    ElementFlags::REQUIRED
                } else {
                    ElementFlags::OPTIONAL
                };
            } else {
                types[i] = rest_type.expect("checked");
                flags = ElementFlags::VARIADIC;
            }
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
                    .expect("TupleType data")
                    .clone();
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
                    return self.any_array_type;
                }
                return Some(rest_type);
            }
            let tuple = self
                .type_(self.type_(rest_type).as_type_reference().expect("ref").base.target.expect("target"))
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
                .expect("TupleType data")
                .clone();
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
                    let elements = tsc_ast::binding_pattern_elements(self.store(), name);
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
                types.push(pred.t?);
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
        let t = match predicate.t {
            Some(t) => match mapper {
                Some(mapper) => self.instantiate_type(t, mapper),
                None => t,
            },
            None => None,
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
        &self,
        signature: crate::types::SignatureId,
    ) -> bool {
        let mut i = self.type_resolutions.len() as isize - 1;
        while i >= self.resolution_start as isize {
            let resolution = self.type_resolutions[i as usize];
            if resolution.target == crate::TypeSystemEntity::Signature(signature)
                && resolution.property_name == crate::TypeSystemPropertyName::ResolvedReturnType
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
                if self
                    .find_matching_signature(list, signature, false, false, false)
                    .is_none()
                {
                    return None;
                }
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
        for &s in signature_list {
            if self.compare_signatures_identical(
                s,
                signature,
                partial_match,
                ignore_this_types,
                ignore_return_types,
                compare_types,
            ) != Ternary::False
            {
                return Some(s);
            }
        }
        None
    }

    /// Go: `func (c *Checker) compareSignaturesIdentical(source, target, partialMatch, ignoreThisTypes, ignoreReturnTypes, compareTypes) Ternary`.
    pub fn compare_signatures_identical(
        &mut self,
        source: crate::types::SignatureId,
        mut target: crate::types::SignatureId,
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
                let constraints_ok = invoke_comparer(
                    self,
                    None,
                    compare_types,
                    self.instantiate_type(s_constraint, mapper),
                    t_constraint,
                    true,
                ) != Ternary::False;
                let defaults_ok = invoke_comparer(
                    self,
                    None,
                    compare_types,
                    self.instantiate_type(s_default, mapper),
                    t_default,
                    true,
                ) != Ternary::False;
                if !(s == t || constraints_ok && defaults_ok) {
                    return Ternary::False;
                }
            }
            source = self.instantiate_signature_ex(source, mapper, true /*eraseTypeParameters*/);
        }
        let mut result = Ternary::True;
        if !ignore_this_types {
            if let Some(source_this_type) = self.get_this_type_of_signature(source) {
                if let Some(target_this_type) = self.get_this_type_of_signature(target) {
                    let related = invoke_comparer(self, None, compare_types, source_this_type, target_this_type, true);
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
            let related = invoke_comparer(self, None, compare_types, t, s, true);
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
                    invoke_comparer(self, None, compare_types, source_return, target_return, true),
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
            if !self.is_type_identical_to(source_constraint, self.instantiate_type(target_constraint, Some(mapper))) {
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
                    (Some(a), Some(b)) => invoke_comparer(self, None, compare_types, a, b, true),
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
            return self.type_(source).types().iter().all(|&t| {
                contains_type(self, self.type_(target).types(), t)
            });
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
        if source_flags.intersects(TypeFlags::UNION) {
            return self.type_(source).types().iter().all(|&t| {
                self.is_type_derived_from(t, target)
            });
        }
        if target_flags.intersects(TypeFlags::UNION) {
            return self.type_(target).types().iter().any(|&t| {
                self.is_type_derived_from(source, t)
            });
        }
        if source_flags.intersects(TypeFlags::INTERSECTION) {
            return self.type_(source).types().iter().any(|&t| {
                self.is_type_derived_from(t, target)
            });
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
pub(crate) fn get_non_nullable_type(c: &mut Checker, t: TypeId) -> TypeId {
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
// facts wave is S10; only the two the signature ladder uses are carried).
define_flags!(TypeFacts, u32);
impl TypeFacts {
    pub const IS_UNDEFINED_OR_NULL: TypeFacts = TypeFacts(1 << 3);
    pub const NE_UNDEFINED: TypeFacts = TypeFacts(1 << 4);
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
pub struct ErrorChain {
    pub next: Option<Box<ErrorChain>>,
    pub message: &'static Message,
    pub args: Vec<DiagArg>,
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
            let is_performing_common_property_checks = (self.relation != RelationKind::Comparable || is_unit_type(c, source))
                && !intersection_state.intersects(IntersectionState::TARGET)
                && c.type_(source).flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && c.global_object_type != Some(source)
                && c.type_(target).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && c.is_weak_type(target)
                && (!get_properties_of_type(c, source).is_empty()
                    || type_has_call_or_construct_signatures(c, source));
            let is_comparing_jsx_attributes_flags = c.type_(source).object_flags.intersects(ObjectFlags::JSX_ATTRIBUTES);
            if is_performing_common_property_checks
                && !c.has_common_properties(source, target, is_comparing_jsx_attribute_flags)
            {
                if report_errors {
                    let source_string = type_to_string(c, if c.type_(original_source).alias.is_some() { original_source } else { source });
                    let target_string = type_to_string(c, if c.type_(original_target).alias.is_some() { original_target } else { target });
                    let calls = get_signatures_of_type(c, source, SignatureKind::Call);
                    let constructs = get_signatures_of_type(c, source, SignatureKind::Construct);
                    if (!calls.is_empty()
                        && ternary_not_false(self.is_related_to(c, get_return_type_of_signature(c, calls[0]), target, RecursionFlags::SOURCE, false)))
                        || (!constructs.is_empty()
                            && ternary_not_false(self.is_related_to(c, get_return_type_of_signature(c, constructs[0]), target, RecursionFlags::SOURCE, false)))
                    {
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
        let is_comparing_jsx_attribute_flags = c.type_(source).object_flags.intersects(ObjectFlags::JSX_ATTRIBUTES);
        if (self.relation == RelationKind::Assignable || self.relation == RelationKind::Comparable)
            && (c.is_type_subset_of(c.global_object_type.unwrap_or(c.never_type()), target)
                || (!is_comparing_jsx_attribute_flags && is_empty_object_type(c, target)))
        {
            return false;
        }
        let mut reduced_target = target;
        let mut check_types: Option<Vec<TypeId>> = None;
        if c.type_(target).flags.intersects(TypeFlags::UNION) {
            reduced_target = find_matching_discriminant_type(c, source, target, TypeComparerKind::RelatedToSimple, self)
                .unwrap_or_else(|| filter_primitives_if_contains_non_primitive(c, target));
            check_types = Some(distributed(c, reduced_target));
        }
        for prop in get_properties_of_type(c, source) {
            if should_check_as_excess_property(c, prop, c.type_(source).symbol) && !is_ignored_jsx_property(c, source, prop) {
                let name = c.program_symbol(prop).name.clone();
                if !c.is_known_property(reduced_target, &name, is_comparing_jsx_attribute_flags) {
                    if report_errors {
                        // TODO(port M6 W5 jsx): the JSX-attribute error arms
                        // (getSuggestedSymbolForNonexistentJSXAttribute,
                        // IsJsxAttributes/IsJsxOpeningLikeElement routing).
                        // The non-JSX arm below is ported; until the jsx wave,
                        // object-literal comparisons take it unconditionally.
                        // TODO(port M6 S6): getSuggestionForNonexistentProperty
                        // — the Did-you-mean elaboration.
                        let error_target = filter_type_is_excess_property_check_target(c, reduced_target);
                        self.report_error(c, &msg::OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_AND_0_DOES_NOT_EXIST_IN_TYPE_1, &[DiagArg::Str(symbol_to_string(c, prop)), DiagArg::Str(type_to_string(c, error_target))]);
                    }
                    return true;
                }
                if let Some(check_types) = &check_types {
                    let prop_type = get_type_of_symbol(c, prop);
                    let target_prop_type = get_type_of_property_in_types(c, check_types, &name);
                    if self.is_related_to(c, prop_type, target_prop_type, RecursionFlags::BOTH, report_errors) == Ternary::False {
                        if report_errors {
                            self.report_error(c, &msg::TYPES_OF_PROPERTY_0_ARE_INCOMPATIBLE, &[DiagArg::Str(symbol_to_string(c, prop))]);
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
    u::get_parent_of_node(c, prop_decl) == cont
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
            if let Some(best_matching_type) = get_best_matching_type(c, source, target, TypeComparerKind::RelatedToSimple, self) {
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
        for &target_type in c.type_(target).types() {
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
        for &source_type in c.type_(source).types() {
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
                    self.report_error(c, &msg::EXCESSIVE_COMPLEXITY_COMPARING_TYPES_0_AND_1, &[DiagArg::Str(type_to_string(c, source)), DiagArg::Str(type_to_string(c, target))]);
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
        let result;
        if self.expanding_flags == ExpandingFlags::BOTH {
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
            result = Ternary::Maybe;
        } else {
            let _pop = c.tracer.as_ref().map(|tr| {
                let args = crate::tracing::Args::new();
                args.insert("sourceId", crate::tracing::TraceValue::Uint(source.0 as u64));
                args.insert("targetId", crate::tracing::TraceValue::Uint(target.0 as u64));
                tr.push(crate::tracing::Phase::CheckTypes, "structuredTypeRelatedTo", &args, false /*separateBeginAndEnd*/)
            });
            result = self.structured_type_related_to(c, source, target, report_errors, intersection_state);
            // Go: defer pop — the TracerPop drop above handles it.
        }
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
        c.type_(source).flags.intersects(TypeFlags::INTERSECTION)
            && c.type_(get_apparent_type(c, source)).flags.intersects(TypeFlags::STRUCTURED_TYPE)
            && !c.type_(source).types().iter().any(|&t| {
                t == target || c.type_(t).object_flags.intersects(ObjectFlags::NON_INFERRABLE_TYPE)
            })
    }

    /// Go: `func (r *Relater) traceUnionsOrIntersectionsTooLarge(source, target)`.
    fn trace_unions_or_intersections_too_large(&mut self, c: &Checker, source: TypeId, target: TypeId) {        let Some(tr) = &c.tracer else { return };
        if c.type_(source).flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
            && c.type_(target).flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            if c.type_(source).object_flags.intersect(c.type_(target).object_flags).intersects(ObjectFlags::PRIMITIVE_UNION) {
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
}
