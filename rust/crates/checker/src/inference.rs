// Ported from tsc/internal/checker/inference.go @ 51c3d2dcae (M6 W2).
//
// The inference engine: InferenceKey/InferenceState (the pooled engine
// state), the inferTypes/inferFromTypes ladder, the InferenceContext family
// (which owns the InferenceInfo per type parameter), and getInferredType (the
// covariant/contravariant candidate resolution).
//
// File map (Go → Rust), mirroring inference.go top to bottom:
//   InferenceKey/InferenceState ................. structs below
//   getInferenceState/putInferenceState ......... Checker::get_inference_state/put_inference_state
//   inferTypes/inferFromTypes ................... Checker::infer_types/infer_from_types
//   inferFromAliasTypeArguments etc. ............ Checker::infer_from_*
//   invokeOnce/inferFromMatchingTypes ........... Checker::invoke_once
//   inferToMultipleTypes/Conditional/Template ... Checker::infer_to_*
//   inferFromObjectTypes/Properties/Signatures .. Checker::infer_from_*
//   applyToParameterTypes/applyToReturnTypes ..... Checker::apply_to_*
//   inferToMappedType/reverse-mapped family ...... Checker::infer_to_mapped_type etc.
//   newInferenceContext family .................. Checker::new_inference_context_worker
//   getInferredType/getInferredTypes ............ Checker::get_inferred_type (replaces the
//                                                   W1 errorType stub; mapper.rs's
//                                                   InferenceTypeMapper is the other caller)
//   getCovariant/ContravariantInference .......... Checker::get_covariant_inference etc.
//   getInferenceInfoForType ..................... free fn returning the INDEX (Go's
//                                                   *InferenceInfo pointer → §5.4 index idiom)
//
// PORT notes (the big four):
//
// 1. Ownership. Go's `*InferenceState` holds `inferences []*InferenceInfo` —
//    a slice SHARED with the owning InferenceContext, so engine mutations
//    (candidate appends, priority updates, clearCachedInferences) land in the
//    context's InferenceInfo objects directly. The port's contexts live in
//    `Checker::inference_contexts` and every W2-real caller of inferTypes
//    passes a context (the relater's conditional arm and the intra-expression
//    sites); the state therefore carries the [`InferenceContextId`] and all
//    mutations index the arena. Go's one standalone caller
//    (inferReverseMappedTypeWorker's bare newInferenceInfo) allocates a
//    temporary context in the arena (contexts are append-only, like Go's
//    GC-kept contexts).
//
// 2. The state pool. Go keeps `freeinferenceState *InferenceState` as an
//    intrusively-linked free list. The port's states are plain values (no
//    `next` link — a `Vec<InferenceState>` free stack on the checker);
//    getInferenceState pops and putInferenceState clears (visited map +
//    stacks) and pushes, the same reuse contract.
//
// 3. `compareTypes TypeComparer`. Go stores a function value — either
//    `c.compareTypesAssignable` or a relater method expression
//    (`r.isRelatedToWorker` in the relater's conditional arm). A Rust
//    context in the checker's arena cannot borrow the creating relater, so
//    the port stores [`InferenceComparer`]: Assignable dispatches the plain
//    checker comparer, Relater(RelationKind) re-enters the relater with a
//    fresh relater of the creating relation (isRelatedToWorker consults only
//    the relation, not outer relater state; getInferredType's constraint
//    checks are the only consumer).
//
// 4. InferencePriority is a signed Go int32 (InferencePriorityCircularity =
//    -1 sorts below every other value and the engine takes min() over it).
//    The provisional W2 relater stub modeled it as a u64 flag set with
//    NO_CONSTRAINTS=1<<6/ALWAYS_STRICT=1<<9 — WRONG values; the real ladder
//    below (checker.go's) corrects the relater's
//    NO_CONSTRAINTS|ALWAYS_STRICT to 1<<9|1<<10.
#![allow(dead_code)] // Go-mirrored surface; consumers land with the S waves

use std::cmp::Ordering;

use rustc_hash::FxHashMap;
use tsc_ast::{Kind, NodeId, SymbolFlags};

use crate::relater::{
    get_base_constraint_of_type, get_constraint_of_type, get_erased_signature, get_index_infos_of_type,
    get_index_type_ex, get_literal_type_from_property,
    get_name_type_from_mapped_type, get_properties_of_object_type, get_signatures_of_type,
    get_string_literal_value, get_template_type_from_mapped_type, get_type_alias_type_parameters,
    get_type_arguments, get_constraint_type_from_mapped_type,
    get_type_with_this_argument, get_union_type, get_union_type_ex,
    alias_symbol_value_declaration_in_js_file, conditional_parts,
    get_default_constraint_of_conditional_type, get_true_type_from_conditional_type,
    get_false_type_from_conditional_type, index_target, infer_types_from_template_literal_type,
    is_applicable_index_type, is_array_or_tuple_type, is_array_type, is_generic_mapped_type,
    is_literal_type, is_mutable_array_or_tuple, maybe_type_of_kind, remove_missing_type,
    string_mapping_symbol, string_mapping_target, template_literal_parts,
    tuple_type_combined_flags, tuple_type_element_flags, tuple_type_min_length,
    type_reference_target, ExpandingFlags, RelationKind, TypeComparerKind,
};
use crate::utilities::is_object_or_array_literal_type;
use crate::todo_shapes::{InferenceContextId, TypeMapperId, UnionReduction};
use crate::types::{ElementFlags, ObjectFlags, SignatureFlags, SignatureKind, Ternary, TypeFlags, TypeId};
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// InferencePriority (Go checker.go) + InferenceFlags
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type InferencePriority int32` + the const ladder (checker.go).
///
/// PORT: a signed newtype (CIRCULARITY is -1 and the engine relies on min()
/// ordering, so a plain unsigned flags type cannot express it). Bit ops are
/// the Go `|`/`&` uses; Ord is the min() uses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct InferencePriority(pub i32);
impl InferencePriority {
    /// Go: `InferencePriorityNone`.
    pub const NONE: InferencePriority = InferencePriority(0);
    /// Go: `InferencePriorityNakedTypeVariable` — naked type variable in
    /// union or intersection type.
    pub const NAKED_TYPE_VARIABLE: InferencePriority = InferencePriority(1 << 0);
    /// Go: `InferencePrioritySpeculativeTuple`.
    pub const SPECULATIVE_TUPLE: InferencePriority = InferencePriority(1 << 1);
    /// Go: `InferencePrioritySubstituteSource` — source of inference
    /// originated within a substitution type's substitute.
    pub const SUBSTITUTE_SOURCE: InferencePriority = InferencePriority(1 << 2);
    /// Go: `InferencePriorityHomomorphicMappedType` — reverse inference for
    /// homomorphic mapped type.
    pub const HOMOMORPHIC_MAPPED_TYPE: InferencePriority = InferencePriority(1 << 3);
    /// Go: `InferencePriorityPartialHomomorphicMappedType`.
    pub const PARTIAL_HOMOMORPHIC_MAPPED_TYPE: InferencePriority = InferencePriority(1 << 4);
    /// Go: `InferencePriorityMappedTypeConstraint` — reverse inference for
    /// mapped type.
    pub const MAPPED_TYPE_CONSTRAINT: InferencePriority = InferencePriority(1 << 5);
    /// Go: `InferencePriorityContravariantConditional` — conditional type in
    /// contravariant position.
    pub const CONTRAVARIANT_CONDITIONAL: InferencePriority = InferencePriority(1 << 6);
    /// Go: `InferencePriorityReturnType` — inference made from return type
    /// of generic function.
    pub const RETURN_TYPE: InferencePriority = InferencePriority(1 << 7);
    /// Go: `InferencePriorityLiteralKeyof` — inference made from a string
    /// literal to a keyof T.
    pub const LITERAL_KEYOF: InferencePriority = InferencePriority(1 << 8);
    /// Go: `InferencePriorityNoConstraints` — don't infer from constraints of
    /// instantiable types.
    pub const NO_CONSTRAINTS: InferencePriority = InferencePriority(1 << 9);
    /// Go: `InferencePriorityAlwaysStrict` — always use strict rules for
    /// contravariant inferences.
    pub const ALWAYS_STRICT: InferencePriority = InferencePriority(1 << 10);
    /// Go: `InferencePriorityMaxValue` — seed for inference priority tracking.
    pub const MAX_VALUE: InferencePriority = InferencePriority(1 << 11);
    /// Go: `InferencePriorityCircularity` (value less than all other priorities).
    pub const CIRCULARITY: InferencePriority = InferencePriority(-1);
    /// Go: `InferencePriorityPriorityImpliesCombination` — these priorities
    /// imply that the resulting type should be a combination of all candidates.
    pub const PRIORITY_IMPLIES_COMBINATION: InferencePriority = InferencePriority(
        Self::RETURN_TYPE.0 | Self::MAPPED_TYPE_CONSTRAINT.0 | Self::LITERAL_KEYOF.0,
    );
}
impl std::ops::BitOr for InferencePriority {
    type Output = InferencePriority;
    fn bitor(self, rhs: Self) -> Self {
        InferencePriority(self.0 | rhs.0)
    }
}
impl std::ops::BitOrAssign for InferencePriority {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
impl std::ops::BitAnd for InferencePriority {
    type Output = InferencePriority;
    fn bitand(self, rhs: Self) -> Self {
        InferencePriority(self.0 & rhs.0)
    }
}
impl InferencePriority {
    /// Go: the `priority&FLAG != 0` tests.
    pub fn intersects(self, other: InferencePriority) -> bool {
        self.0 & other.0 != 0
    }
    /// Go: `min(a, b)` over priorities.
    pub fn min(self, other: InferencePriority) -> InferencePriority {
        if self < other {
            self
        } else {
            other
        }
    }
}

// Go: `type InferenceFlags uint32` + consts (checker.go).
define_flags!(InferenceFlags, u32);
// (NONE is the define_flags! macro's generated zero value — InferenceFlagsNone.)
impl InferenceFlags {
    /// Go: `InferenceFlagsNoDefault` — infer silentNeverType for no inferences
    /// (otherwise anyType or unknownType).
    pub const NO_DEFAULT: InferenceFlags = InferenceFlags(1 << 0);
    /// Go: `InferenceFlagsAnyDefault` — infer anyType (in JS files) for no
    /// inferences (otherwise unknownType).
    pub const ANY_DEFAULT: InferenceFlags = InferenceFlags(1 << 1);
    /// Go: `InferenceFlagsSkippedGenericFunction` — a generic function was
    /// skipped during inference.
    pub const SKIPPED_GENERIC_FUNCTION: InferenceFlags = InferenceFlags(1 << 2);
    /// Go: `InferenceFlagsNoConstraintChecks`.
    pub const NO_CONSTRAINT_CHECKS: InferenceFlags = InferenceFlags(1 << 3);
}

/// Go: the context's `compareTypes TypeComparer` — the function value the
/// context's constraint checks call (`n.compareTypes(t, u, false)` in
/// getInferredType).
///
/// PORT: Go stores `c.compareTypesAssignable` (checker-level) or a bound
/// relater method (`r.isRelatedToWorker`, relater.go's conditional arm). The
/// port cannot borrow the creating relater from an arena-stored context, so
/// the relation kind is captured and the worker re-entered through a fresh
/// relater (`check_type_related_to`); isRelatedToWorker consults only the
/// relation, not outer relater state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InferenceComparer {
    /// Go: `c.compareTypesAssignable`.
    #[default]
    Assignable,
    /// Go: a relater method expression (`r.isRelatedToWorker`); the payload
    /// is the creating relater's relation.
    Relater(RelationKind),
}

impl InferenceComparer {
    /// Go: invoking the stored `TypeComparer`.
    fn compare(&self, c: &mut Checker, source: TypeId, target: TypeId, _report_errors: bool) -> Ternary {
        match self {
            InferenceComparer::Assignable => c.compare_types_assignable_simple(source, target),
            InferenceComparer::Relater(relation) => {
                if c.check_type_related_to(source, target, *relation, None) {
                    Ternary::True
                } else {
                    Ternary::False
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// InferenceContext / InferenceInfo (Go checker.go's structs)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type InferenceContext struct` (checker.go) — replaces the W1
/// todo_shapes stub shape wholesale (mapper.rs's InferenceTypeMapper reaches
/// it through [`InferenceContextId`], unchanged).
#[derive(Debug, Default)]
pub struct InferenceContext {
    /// Go: `inferences []*InferenceInfo` — inferences made for each type
    /// parameter (PORT: by value; the mapper's clearCachedInferences writes
    /// `.inferredType` through the arena).
    pub inferences: Vec<InferenceInfo>,
    /// Go: `signature *Signature` — generic signature for which inferences
    /// are made (if any).
    pub signature: Option<crate::types::SignatureId>,
    /// Go: `flags InferenceFlags`.
    pub flags: InferenceFlags,
    /// Go: `compareTypes TypeComparer` (see [`InferenceComparer`]).
    pub compare_types: InferenceComparer,
    /// Go: `mapper *TypeMapper` — mapper that fixes inferences.
    pub mapper: Option<TypeMapperId>,
    /// Go: `nonFixingMapper *TypeMapper`.
    pub non_fixing_mapper: Option<TypeMapperId>,
    /// Go: `returnMapper *TypeMapper` — type mapper for inferences from
    /// return types (if any).
    pub return_mapper: Option<TypeMapperId>,
    /// Go: `outerReturnMapper *TypeMapper`.
    pub outer_return_mapper: Option<TypeMapperId>,
    /// Go: `inferredTypeParameters []*Type` — inferred type parameters for
    /// the function result.
    pub inferred_type_parameters: Vec<TypeId>,
    /// Go: `intraExpressionInferenceSites []IntraExpressionInferenceSite`.
    pub intra_expression_inference_sites: Vec<IntraExpressionInferenceSite>,
}


/// Go: `type InferenceInfo struct` (checker.go). The first three field names
/// are the W1 mapper.rs contract (`typeParameter`/`isFixed`/`inferredType`).
#[derive(Debug, Default)]
pub struct InferenceInfo {
    /// Go: `typeParameter *Type`.
    pub type_parameter: TypeId,
    /// Go: `candidates []*Type` — candidates in covariant positions in
    /// decreasing depth order.
    pub candidates: Vec<TypeId>,
    /// Go: `contraCandidates []*Type` — candidates in contravariant positions.
    pub contra_candidates: Vec<TypeId>,
    /// Go: `inferredType *Type` — cache for resolved inferred type.
    pub inferred_type: Option<TypeId>,
    /// Go: `priority InferencePriority` — priority of current inference set.
    pub priority: InferencePriority,
    /// Go: `topLevel bool` — true if all inferences are to top level occurrences.
    pub top_level: bool,
    /// Go: `isFixed bool` — true if inferences are fixed.
    pub is_fixed: bool,
    /// Go: `impliedArity int` — implied arity (or -1).
    pub implied_arity: i32,
}

/// Go: `type IntraExpressionInferenceSite struct`.
#[derive(Debug, Clone, Copy)]
pub struct IntraExpressionInferenceSite {
    /// Go: `node *ast.Node`.
    pub node: NodeId,
    /// Go: `t *Type`.
    pub t: TypeId,
}

// ────────────────────────────────────────────────────────────────────────────
// InferenceKey / InferenceState
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type InferenceKey struct`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InferenceKey {
    /// Go: `source TypeId`.
    source: TypeId,
    /// Go: `target TypeId`.
    target: TypeId,
    /// Go: `priority InferencePriority`.
    priority: InferencePriority,
    /// Go: `contravariant bool`.
    contravariant: bool,
    /// Go: `bivariant bool`.
    bivariant: bool,
}

/// Go: `type ReverseMappedTypeKey struct` (checker.go).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ReverseMappedTypeKey {
    /// Go: `sourceId TypeId`.
    pub source_id: TypeId,
    /// Go: `targetId TypeId`.
    pub target_id: TypeId,
    /// Go: `constraintId TypeId`.
    pub constraint_id: TypeId,
}

/// Go: `type InferenceState struct`.
///
/// PORT: `inferences []*InferenceInfo` → the owning context's arena index
/// (see the module note); `next *InferenceState` (the free-list link) is
/// dropped — the pool is `Checker::free_inference_states` (a value stack).
pub(crate) struct InferenceState {
    /// Go: `inferences []*InferenceInfo` (shared with the context).
    inferences: InferenceContextId,
    /// Go: `originalSource *Type`.
    original_source: TypeId,
    /// Go: `originalTarget *Type`.
    original_target: TypeId,
    /// Go: `priority InferencePriority`.
    priority: InferencePriority,
    /// Go: `inferencePriority InferencePriority`.
    inference_priority: InferencePriority,
    /// Go: `contravariant bool`.
    contravariant: bool,
    /// Go: `bivariant bool`.
    bivariant: bool,
    /// Go: `expandingFlags ExpandingFlags`.
    expanding_flags: ExpandingFlags,
    /// Go: `propagationType *Type`.
    propagation_type: Option<TypeId>,
    /// Go: `visited map[InferenceKey]InferencePriority`.
    visited: FxHashMap<InferenceKey, InferencePriority>,
    /// Go: `sourceStack []*Type`.
    source_stack: Vec<TypeId>,
    /// Go: `targetStack []*Type`.
    target_stack: Vec<TypeId>,
}

impl Default for InferenceState {
    fn default() -> Self {
        InferenceState {
            inferences: InferenceContextId(0),
            original_source: TypeId(0),
            original_target: TypeId(0),
            priority: InferencePriority::NONE,
            inference_priority: InferencePriority::MAX_VALUE,
            contravariant: false,
            bivariant: false,
            expanding_flags: ExpandingFlags::NONE,
            propagation_type: None,
            visited: FxHashMap::default(),
            source_stack: Vec::new(),
            target_stack: Vec::new(),
        }
    }
}

impl InferenceState {
    /// Go: `clear(n.visited)` + the resets of putInferenceState's
    /// `*n = InferenceState{...}` (the pool reuse contract).
    fn clear_for_pool(&mut self) {
        self.visited.clear();
        self.source_stack.clear();
        self.target_stack.clear();
        self.expanding_flags = ExpandingFlags::NONE;
        self.propagation_type = None;
        self.bivariant = false;
    }
}

/// Go: `func getInferenceInfoForType(n *InferenceState, t *Type) *InferenceInfo`
/// — PORT: the §5.4 index idiom (Go's *InferenceInfo pointer → index into the
/// state's inference list).
fn get_inference_info_for_type(c: &Checker, n: &InferenceState, t: TypeId) -> Option<usize> {
    if c.type_(t).flags.intersects(TypeFlags::TYPE_VARIABLE) {
        let t = c.get_non_distributed_type_parameter(t);
        let inferences = &c.inference_contexts[n.inferences.0 as usize].inferences;
        for (i, inference) in inferences.iter().enumerate() {
            if t == inference.type_parameter {
                return Some(i);
            }
        }
    }
    None
}

/// Go: `func getSingleTypeVariableFromIntersectionTypes(n, types) *Type`.
fn get_single_type_variable_from_intersection_types(
    c: &Checker,
    n: &InferenceState,
    types: &[TypeId],
) -> Option<TypeId> {
    let mut type_variable: Option<TypeId> = None;
    for &t in types {
        if !c.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            return None;
        }
        let v = c
            .type_(t)
            .distributed()
            .into_iter()
            .find(|t| get_inference_info_for_type(c, n, *t).is_some());
        match v {
            None => return None,
            Some(v) if type_variable.is_some() && Some(v) != type_variable => return None,
            Some(v) => type_variable = Some(v),
        }
    }
    type_variable
}

// ────────────────────────────────────────────────────────────────────────────
// The engine (Checker methods)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getInferenceState() *InferenceState`.
    fn get_inference_state(&mut self) -> InferenceState {
        self.free_inference_states.pop().unwrap_or_default()
    }

    /// Go: `func (c *Checker) putInferenceState(n *InferenceState)`.
    fn put_inference_state(&mut self, mut n: InferenceState) {
        n.clear_for_pool();
        self.free_inference_states.push(n);
    }

    /// Go: `func (c *Checker) inferTypes(inferences, originalSource, originalTarget, priority, contravariant)`.
    ///
    /// PORT: the `inferences` slice is the context's (see the module note);
    /// the state is taken from the pool by value. bivariant/expandingFlags/
    /// propagationType are reset defensively — Go relies on the
    /// putInferenceState invariant that the previous user restored them.
    pub(crate) fn infer_types(
        &mut self,
        inferences: InferenceContextId,
        original_source: TypeId,
        original_target: TypeId,
        priority: InferencePriority,
        contravariant: bool,
    ) {
        let mut n = self.get_inference_state();
        n.inferences = inferences;
        n.original_source = original_source;
        n.original_target = original_target;
        n.priority = priority;
        n.inference_priority = InferencePriority::MAX_VALUE;
        n.contravariant = contravariant;
        self.infer_from_types(&mut n, original_source, original_target);
        self.put_inference_state(n);
    }

    /// Go: `func (c *Checker) inferFromTypes(n *InferenceState, source, target *Type)`.
    fn infer_from_types(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        if !self.could_contain_type_variables(target) || self.is_no_infer_type(target) {
            return;
        }
        if source == self.wildcard_type() || source == self.blocked_string_type() {
            // We are inferring from an 'any' type. We want to infer this type for every type parameter
            // referenced in the target type, so we record it as the propagation type and infer from the
            // target to itself. Then, as we find candidates we substitute the propagation type. (Go comment, verbatim.)
            let save_propagation_type = n.propagation_type;
            n.propagation_type = Some(source);
            self.infer_from_types(n, target, target);
            n.propagation_type = save_propagation_type;
            return;
        }
        let source_alias_symbol = self.type_(source).alias.as_ref().map(|a| a.symbol);
        let target_alias_symbol = self.type_(target).alias.as_ref().map(|a| a.symbol);
        if source_alias_symbol.is_some() && source_alias_symbol == target_alias_symbol {
            let source_arg_count = self.type_(source).alias.as_ref().expect("alias").type_arguments.len();
            let target_arg_count = self.type_(target).alias.as_ref().expect("alias").type_arguments.len();
            if source_arg_count != 0 || target_arg_count != 0 {
                self.invoke_once(n, source, target, Checker::infer_from_alias_type_arguments);
            }
            // And if there weren't any type arguments, there's no reason to run inference as the types must be the same. (Go comment, verbatim.)
            return;
        }
        if source == target && self.type_(source).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            // When source and target are the same union or intersection type, just relate each constituent
            // type to itself. (Go comment, verbatim.)
            let constituents = self.type_(source).distributed();
            for t in constituents {
                self.infer_from_types(n, t, t);
            }
            return;
        }
        let mut source = source;
        let mut target = target;
        if self.type_(target).flags.intersects(TypeFlags::UNION) {
            let source_types: Vec<TypeId> = if self.type_(source).flags.intersects(TypeFlags::UNION) {
                self.type_(source).distributed()
            } else {
                vec![source]
            };
            // First, infer between identically matching source and target constituents and remove the
            // matching types. (Go comment, verbatim.)
            let target_types = self.type_(target).distributed();
            let (temp_sources, temp_targets) = self.infer_from_matching_types(
                n,
                source_types,
                target_types,
                Self::is_type_or_base_identical_to,
                false, /*sort*/
            );
            // Next, infer between closely matching source and target constituents and remove
            // the matching types. Types closely match when they are instantiations of the same
            // object type or instantiations of the same type alias. (Go comment, verbatim.)
            let (sources, targets) = self.infer_from_matching_types(
                n,
                temp_sources,
                temp_targets,
                Self::is_type_closely_matched_by,
                true, /*sort*/
            );
            if targets.is_empty() {
                return;
            }
            target = get_union_type(self, targets);
            if sources.is_empty() {
                // All source constituents have been matched and there is nothing further to infer from.
                // However, simply making no inferences is undesirable because it could ultimately mean
                // inferring a type parameter constraint. Instead, make a lower priority inference from
                // the full source to whatever remains in the target. For example, when inferring from
                // string to 'string | T', make a lower priority inference of string for T. (Go comment, verbatim.)
                self.infer_with_priority(n, source, target, InferencePriority::NAKED_TYPE_VARIABLE);
                return;
            }
            source = get_union_type(self, sources);
        } else if self.type_(target).flags.intersects(TypeFlags::INTERSECTION)
            && !self
                .type_(target)
                .distributed()
                .iter()
                .all(|&t| self.is_non_generic_object_type(t))
        {
            // We reduce intersection types unless they're simple combinations of object types. For example,
            // when inferring from 'string[] & { extra: any }' to 'string[] & T' we want to remove string[] and
            // infer { extra: any } for T. But when inferring to 'string[] & Iterable<T>' we want to keep the
            // string[] on the source side and infer string for T. (Go comment, verbatim.)
            if !self.type_(source).flags.intersects(TypeFlags::UNION) {
                let source_types: Vec<TypeId> =
                    if self.type_(source).flags.intersects(TypeFlags::INTERSECTION) {
                        self.type_(source).distributed()
                    } else {
                        vec![source]
                    };
                // Infer between identically matching source and target constituents and remove the matching types. (Go comment, verbatim.)
                let target_types = self.type_(target).distributed();
                let (sources, targets) = self.infer_from_matching_types(
                    n,
                    source_types,
                    target_types,
                    Self::is_type_identical_to,
                    false, /*sort*/
                );
                if sources.is_empty() || targets.is_empty() {
                    return;
                }
                source = crate::relater::get_intersection_type(self, sources);
                target = crate::relater::get_intersection_type(self, targets);
            }
        }
        if self.type_(target).flags.intersects(TypeFlags::INDEXED_ACCESS | TypeFlags::SUBSTITUTION) {
            if self.is_no_infer_type(target) {
                return;
            }
            target = self.get_actual_type_variable(target);
        }
        if self.type_(target).flags.intersects(TypeFlags::TYPE_VARIABLE) {
            // Skip inference if the source is "blocked", which is used by the language service to
            // prevent inference on nodes currently being edited. (Go comment, verbatim.)
            if self.is_from_inference_blocked_source(source) {
                return;
            }
            if let Some(index) = get_inference_info_for_type(self, n, target) {
                // If target is a type parameter, make an inference, unless the source type contains
                // a "non-inferrable" type. Types with this flag set are markers used to prevent inference.
                // (Go's long comment covers anyFunctionType/autoType/silentNeverType — see inference.go.)
                if self.type_(source).object_flags.intersects(ObjectFlags::NON_INFERRABLE_TYPE)
                    || source == self.non_inferrable_any_type()
                {
                    return;
                }
                let inference_fixed = self.inference_contexts[n.inferences.0 as usize].inferences[index].is_fixed;
                let inference_priority = self.inference_contexts[n.inferences.0 as usize].inferences[index].priority;
                if !inference_fixed {
                    let candidate = n.propagation_type.unwrap_or(source);
                    if candidate == self.blocked_string_type() {
                        return;
                    }
                    if n.priority < inference_priority {
                        let inference = &mut self.inference_contexts[n.inferences.0 as usize].inferences[index];
                        inference.candidates.clear();
                        inference.contra_candidates.clear();
                        inference.top_level = true;
                        inference.priority = n.priority;
                    }
                    if n.priority == self.inference_contexts[n.inferences.0 as usize].inferences[index].priority {
                        // We make contravariant inferences only if we are in a pure contravariant position,
                        // i.e. only if we have not descended into a bivariant position. (Go comment, verbatim.)
                        if n.contravariant && !n.bivariant {
                            if !self.inference_contexts[n.inferences.0 as usize].inferences[index]
                                .contra_candidates
                                .contains(&candidate)
                            {
                                self.inference_contexts[n.inferences.0 as usize].inferences[index]
                                    .contra_candidates
                                    .push(candidate);
                                crate::mapper::clear_cached_inferences(
                                    &mut self.inference_contexts[n.inferences.0 as usize].inferences,
                                );
                            }
                        } else if !self.inference_contexts[n.inferences.0 as usize].inferences[index]
                            .candidates
                            .contains(&candidate)
                        {
                            self.inference_contexts[n.inferences.0 as usize].inferences[index]
                                .candidates
                                .push(candidate);
                            crate::mapper::clear_cached_inferences(
                                &mut self.inference_contexts[n.inferences.0 as usize].inferences,
                            );
                        }
                    }
                    if !n.priority.intersects(InferencePriority::RETURN_TYPE)
                        && self.type_(target).flags.intersects(TypeFlags::TYPE_PARAMETER)
                        && self.inference_contexts[n.inferences.0 as usize].inferences[index].top_level
                        && !self.is_type_parameter_at_top_level(n.original_target, target, 0)
                    {
                        self.inference_contexts[n.inferences.0 as usize].inferences[index].top_level = false;
                        crate::mapper::clear_cached_inferences(
                            &mut self.inference_contexts[n.inferences.0 as usize].inferences,
                        );
                    }
                }
            }
            n.inference_priority = n.inference_priority.min(n.priority);
            return;
        }
        // Infer to the simplified version of an indexed access, if possible, to (hopefully) expose more bare type parameters to the inference engine (Go comment, verbatim.)
        let simplified = self.get_simplified_type(target, false /*writing*/);
        if simplified != target {
            self.infer_from_types(n, source, simplified);
        } else if self.type_(target).flags.intersects(TypeFlags::INDEXED_ACCESS) {
            // TODO(port M6 S7): the indexType simplification +
            // distributeIndexOverObjectType ladder (getSimplifiedType's real
            // body owns the index/object simplifications); permissive skip.
        }
        // ── The structural switch (Go's switch{}). ──
        let source_object_flags = self.type_(source).object_flags;
        let target_object_flags = self.type_(target).object_flags;
        if source_object_flags.intersects(ObjectFlags::REFERENCE)
            && target_object_flags.intersects(ObjectFlags::REFERENCE)
            && (type_reference_target(self, source) == type_reference_target(self, target)
                || is_array_type(self, source) && is_array_type(self, target))
            && !(type_reference_node(self, source).is_some()
                && type_reference_node(self, target).is_some())
        {
            // If source and target are references to the same generic type, infer from type arguments (Go comment, verbatim.)
            self.invoke_once(n, source, target, Checker::infer_from_reference_type_arguments);
        } else if self.type_(source).flags.intersects(TypeFlags::INDEX)
            && self.type_(target).flags.intersects(TypeFlags::INDEX)
        {
            let s_target = index_target(self, source);
            let t_target = index_target(self, target);
            self.infer_from_contravariant_types(n, s_target, t_target);
        } else if (is_literal_type(self, source) || self.type_(source).flags.intersects(TypeFlags::STRING))
            && self.type_(target).flags.intersects(TypeFlags::INDEX)
        {
            let empty = self.create_empty_object_type_from_string_literal(source);
            let t_target = index_target(self, target);
            self.infer_from_contravariant_types_with_priority(n, empty, t_target, InferencePriority::LITERAL_KEYOF);
        } else if self.type_(source).flags.intersects(TypeFlags::INDEXED_ACCESS)
            && self.type_(target).flags.intersects(TypeFlags::INDEXED_ACCESS)
        {
            let s_object = indexed_access_object_type(self, source);
            let t_object = indexed_access_object_type(self, target);
            self.infer_from_types(n, s_object, t_object);
            let s_index = indexed_access_index_type(self, source);
            let t_index = indexed_access_index_type(self, target);
            self.infer_from_types(n, s_index, t_index);
        } else if self.type_(source).flags.intersects(TypeFlags::STRING_MAPPING)
            && self.type_(target).flags.intersects(TypeFlags::STRING_MAPPING)
        {
            if self.type_(source).symbol == self.type_(target).symbol {
                let s_target = string_mapping_target(self, source);
                let t_target = string_mapping_target(self, target);
                self.infer_from_types(n, s_target, t_target);
            }
        } else if self.type_(source).flags.intersects(TypeFlags::SUBSTITUTION) {
            let base = substitution_base_type(self, source);
            self.infer_from_types(n, base, target);
            // Make substitute inference at a lower priority (Go comment, verbatim.)
            let intersection = self.get_substitution_intersection(source);
            self.infer_with_priority(n, intersection, target, InferencePriority::SUBSTITUTE_SOURCE);
        } else if self.type_(target).flags.intersects(TypeFlags::CONDITIONAL) {
            self.invoke_once(n, source, target, Checker::infer_to_conditional_type);
        } else if self.type_(target).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let target_types = self.type_(target).distributed();
            let target_flags = self.type_(target).flags;
            self.infer_to_multiple_types(n, source, target_types, target_flags);
        } else if self.type_(source).flags.intersects(TypeFlags::UNION) {
            // Source is a union or intersection type, infer from each constituent type (Go comment, verbatim.)
            let source_types = self.type_(source).distributed();
            for source_type in source_types {
                self.infer_from_types(n, source_type, target);
            }
        } else if self.type_(target).flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            self.infer_to_template_literal_type(n, source, target);
        } else {
            // Go reassigns `source` (getReducedType then getApparentType);
            // shadowing keeps the outer param untouched.
            let mut source = crate::relater::get_reduced_type(self, source);
            if is_generic_mapped_type(self, source) && is_generic_mapped_type(self, target) {
                self.invoke_once(n, source, target, Checker::infer_from_generic_mapped_types);
            }
            if !(n.priority.intersects(InferencePriority::NO_CONSTRAINTS)
                && self.type_(source).flags.intersects(TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE))
            {
                let apparent_source = self.get_apparent_type(source);
                // getApparentType can return _any_ type, since an indexed access or conditional may simplify to any other type.
                // If that occurs and it doesn't simplify to an object or intersection, we'll need to restart `inferFromTypes`
                // with the simplified source. (Go comment, verbatim.)
                if apparent_source != source
                    && !self
                        .type_(apparent_source)
                        .flags
                        .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                {
                    self.infer_from_types(n, apparent_source, target);
                    return;
                }
                source = apparent_source;
            }
            if self.type_(source).flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION) {
                self.invoke_once(n, source, target, Checker::infer_from_object_types);
            }
        }
    }

    /// Go: `func (c *Checker) inferFromAliasTypeArguments(n, source, target)`.
    fn infer_from_alias_type_arguments(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        // Source and target are types originating in the same generic type alias declaration.
        // Simply infer from source type arguments to target type arguments, with defaults applied. (Go comment, verbatim.)
        let source_alias_symbol = self.type_(source).alias.as_ref().expect("alias").symbol.expect("alias symbol");
        let params = get_type_alias_type_parameters(self, source_alias_symbol);
        let min_params = self.get_min_type_argument_count(&params);
        let node_is_in_js_file = alias_symbol_value_declaration_in_js_file(self, source_alias_symbol);
        let source_arguments = self
            .type_(source)
            .alias
            .as_ref()
            .expect("alias")
            .type_arguments
            .clone();
        let target_arguments = self
            .type_(target)
            .alias
            .as_ref()
            .expect("alias")
            .type_arguments
            .clone();
        let source_types =
            self.fill_missing_type_arguments(&source_arguments, &params, min_params, node_is_in_js_file);
        let target_types =
            self.fill_missing_type_arguments(&target_arguments, &params, min_params, node_is_in_js_file);
        let variances = self.get_alias_variances(source_alias_symbol);
        self.infer_from_type_arguments(n, source_types, target_types, variances);
    }

    /// Go: `func (c *Checker) inferFromReferenceTypeArguments(n, source, target)`.
    fn infer_from_reference_type_arguments(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        let source_types = get_type_arguments(self, source);
        let target_types = get_type_arguments(self, target);
        let reference_target = type_reference_target(self, source);
        let variances = self.get_variances(reference_target);
        self.infer_from_type_arguments(n, source_types, target_types, variances);
    }

    /// Go: `func (c *Checker) inferFromTypeArguments(n, sourceTypes, targetTypes, variances)`.
    fn infer_from_type_arguments(
        &mut self,
        n: &mut InferenceState,
        source_types: Vec<TypeId>,
        target_types: Vec<TypeId>,
        variances: Vec<crate::types::VarianceFlags>,
    ) {
        for i in 0..source_types.len().min(target_types.len()) {
            if i < variances.len()
                && variances[i].intersects(crate::types::VarianceFlags::CONTRAVARIANT)
            {
                self.infer_from_contravariant_types(n, source_types[i], target_types[i]);
            } else {
                self.infer_from_types(n, source_types[i], target_types[i]);
            }
        }
    }

    /// Go: `func (c *Checker) inferWithPriority(n, source, target, newPriority)`.
    fn infer_with_priority(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        new_priority: InferencePriority,
    ) {
        let save_priority = n.priority;
        n.priority |= new_priority;
        self.infer_from_types(n, source, target);
        n.priority = save_priority;
    }

    /// Go: `func (c *Checker) inferFromContravariantTypesWithPriority(n, source, target, newPriority)`.
    fn infer_from_contravariant_types_with_priority(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        new_priority: InferencePriority,
    ) {
        let save_priority = n.priority;
        n.priority |= new_priority;
        self.infer_from_contravariant_types(n, source, target);
        n.priority = save_priority;
    }

    /// Go: `func (c *Checker) inferFromContravariantTypes(n, source, target)`.
    fn infer_from_contravariant_types(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        n.contravariant = !n.contravariant;
        self.infer_from_types(n, source, target);
        n.contravariant = !n.contravariant;
    }

    /// Go: `func (c *Checker) inferFromContravariantTypesIfStrictFunctionTypes(n, source, target)`.
    fn infer_from_contravariant_types_if_strict_function_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        if self.strict_function_types || n.priority.intersects(InferencePriority::ALWAYS_STRICT) {
            self.infer_from_contravariant_types(n, source, target);
        } else {
            self.infer_from_types(n, source, target);
        }
    }

    /// Go: `func (c *Checker) invokeOnce(n, source, target, action)`.
    ///
    /// Ensure an inference action is performed only once for the given source and target types.
    /// (See Go's long doc comment for the circularity rationale.)
    fn invoke_once(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        action: fn(&mut Checker, &mut InferenceState, TypeId, TypeId),
    ) {
        let key = InferenceKey {
            source,
            target,
            priority: n.priority,
            contravariant: n.contravariant,
            bivariant: n.bivariant,
        };
        if let Some(&status) = n.visited.get(&key) {
            n.inference_priority = n.inference_priority.min(status);
            return;
        }
        n.visited.insert(key, InferencePriority::CIRCULARITY);
        let save_inference_priority = n.inference_priority;
        n.inference_priority = InferencePriority::MAX_VALUE;
        // We stop inferring and report a circularity if we encounter duplicate recursion identities on both
        // the source side and the target side. (Go comment, verbatim.)
        let save_expanding_flags = n.expanding_flags;
        n.source_stack.push(source);
        n.target_stack.push(target);
        if self.is_deeply_nested_type(source, &n.source_stack, 2) {
            n.expanding_flags |= ExpandingFlags::SOURCE;
        }
        if self.is_deeply_nested_type(target, &n.target_stack, 2) {
            n.expanding_flags |= ExpandingFlags::TARGET;
        }
        if n.expanding_flags != ExpandingFlags::BOTH {
            action(self, n, source, target);
        } else {
            n.inference_priority = InferencePriority::CIRCULARITY;
        }
        n.target_stack.pop();
        n.source_stack.pop();
        n.expanding_flags = save_expanding_flags;
        n.visited.insert(key, n.inference_priority);
        n.inference_priority = n.inference_priority.min(save_inference_priority);
    }

    /// Go: `func (c *Checker) inferFromMatchingTypes(n, sources, targets, matches, sort) ([]*Type, []*Type)`.
    fn infer_from_matching_types(
        &mut self,
        n: &mut InferenceState,
        sources: Vec<TypeId>,
        targets: Vec<TypeId>,
        matches: fn(&mut Checker, TypeId, TypeId) -> bool,
        sort: bool,
    ) -> (Vec<TypeId>, Vec<TypeId>) {
        let mut matched_sources: Vec<TypeId> = Vec::new();
        let mut matched_targets: Vec<TypeId> = Vec::new();
        for &t in &targets {
            for &s in &sources {
                if matches(self, s, t) {
                    if !sort {
                        self.infer_from_types(n, s, t);
                    }
                    if !matched_sources.contains(&s) {
                        matched_sources.push(s);
                    }
                    if !matched_targets.contains(&t) {
                        matched_targets.push(t);
                    }
                }
            }
        }
        if sort {
            // Sort target types by decreasing depth of generic instantiations. Intuitively, a successful
            // inference from a type argument with deeper nesting is of higher quality because we've stripped
            // away more layers of type instantiations that otherwise might skew the results. For example,
            // when inferring from string[] | string[][] to T[] | T[][], the inference of string we make from
            // relating string[][] to T[][] is of higher quality than the inference of string[] we make relating
            // string[][] to T[]. (Go comment, verbatim.)
            let checker = &*self;
            matched_targets.sort_by(|&t1, &t2| compare_types_and_depth(checker, t1, t2));
            for &t in matched_targets.clone().iter() {
                for &s in matched_sources.clone().iter() {
                    if matches(self, s, t) {
                        self.infer_from_types(n, s, t);
                    }
                }
            }
        }
        let sources: Vec<TypeId> = if !matched_sources.is_empty() {
            sources.into_iter().filter(|t| !matched_sources.contains(t)).collect()
        } else {
            sources
        };
        let targets: Vec<TypeId> = if !matched_targets.is_empty() {
            targets.into_iter().filter(|t| !matched_targets.contains(t)).collect()
        } else {
            targets
        };
        (sources, targets)
    }

    /// Go: `func (c *Checker) inferToMultipleTypes(n, source, targets, targetFlags)`.
    fn infer_to_multiple_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        targets: Vec<TypeId>,
        target_flags: TypeFlags,
    ) {
        let mut type_variable_count = 0;
        if target_flags.intersects(TypeFlags::UNION) {
            let mut naked_type_variable: Option<TypeId> = None;
            let sources: Vec<TypeId> = if self.type_(source).flags.intersects(TypeFlags::UNION) {
                self.type_(source).distributed()
            } else {
                vec![source]
            };
            let mut matched = vec![false; sources.len()];
            let mut inference_circularity = false;
            // First infer to types that are not naked type variables. For each source type we
            // track whether inferences were made from that particular type to some target with
            // equal priority (i.e. of equal quality) to what we would infer for a naked type
            // parameter. (Go comment, verbatim.)
            for &t in &targets {
                if get_inference_info_for_type(self, n, t).is_some() {
                    naked_type_variable = Some(t);
                    type_variable_count += 1;
                } else {
                    for i in 0..sources.len() {
                        let save_inference_priority = n.inference_priority;
                        n.inference_priority = InferencePriority::MAX_VALUE;
                        self.infer_from_types(n, sources[i], t);
                        if n.inference_priority == n.priority {
                            matched[i] = true;
                        }
                        inference_circularity = inference_circularity
                            || n.inference_priority == InferencePriority::CIRCULARITY;
                        n.inference_priority = n.inference_priority.min(save_inference_priority);
                    }
                }
            }
            if type_variable_count == 0 {
                // If every target is an intersection of types containing a single naked type variable,
                // make a lower priority inference to that type variable. This handles inferring from
                // 'A | B' to 'T & (X | Y)' where we want to infer 'A | B' for T. (Go comment, verbatim.)
                if let Some(intersection_type_variable) =
                    get_single_type_variable_from_intersection_types(self, n, &targets)
                {
                    self.infer_with_priority(
                        n,
                        source,
                        intersection_type_variable,
                        InferencePriority::NAKED_TYPE_VARIABLE,
                    );
                }
                return;
            }
            // If the target has a single naked type variable and no inference circularities were
            // encountered above (meaning we explored the types fully), create a union of the source
            // types from which no inferences have been made so far and infer from that union to the
            // naked type variable. (Go comment, verbatim.)
            if type_variable_count == 1 && !inference_circularity {
                let unmatched: Vec<TypeId> = sources
                    .iter()
                    .zip(&matched)
                    .filter(|(_, m)| !*m)
                    .map(|(s, _)| *s)
                    .collect();
                if !unmatched.is_empty() {
                    let union = get_union_type(self, unmatched);
                    self.infer_from_types(n, union, naked_type_variable.expect("checked above"));
                    return;
                }
            }
        } else {
            // We infer from types that are not naked type variables first so that inferences we
            // make from nested naked type variables and given slightly higher priority by virtue
            // of being first in the candidates array. (Go comment, verbatim.)
            for &t in &targets {
                if get_inference_info_for_type(self, n, t).is_some() {
                    type_variable_count += 1;
                } else {
                    self.infer_from_types(n, source, t);
                }
            }
        }
        // Inferences directly to naked type variables are given lower priority as they are
        // less specific. For example, when inferring from Promise<string> to T | Promise<T>,
        // we want to infer string for T, not Promise<string> | string. For intersection types
        // we only infer to single naked type variables. (Go comment, verbatim.)
        if target_flags.intersects(TypeFlags::INTERSECTION) && type_variable_count == 1
            || !target_flags.intersects(TypeFlags::INTERSECTION) && type_variable_count > 0
        {
            for &t in &targets {
                if get_inference_info_for_type(self, n, t).is_some() {
                    self.infer_with_priority(n, source, t, InferencePriority::NAKED_TYPE_VARIABLE);
                }
            }
        }
    }

    /// Go: `func (c *Checker) inferToMultipleTypesWithPriority(n, source, targets, targetFlags, newPriority)`.
    fn infer_to_multiple_types_with_priority(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        targets: Vec<TypeId>,
        target_flags: TypeFlags,
        new_priority: InferencePriority,
    ) {
        let save_priority = n.priority;
        n.priority |= new_priority;
        self.infer_to_multiple_types(n, source, targets, target_flags);
        n.priority = save_priority;
    }

    /// Go: `func (c *Checker) inferToConditionalType(n, source, target)`.
    fn infer_to_conditional_type(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        if self.type_(source).flags.intersects(TypeFlags::CONDITIONAL) {
            let (s_check, s_extends) = conditional_parts(self, source);
            let (t_check, t_extends) = conditional_parts(self, target);
            let s_true = get_true_type_from_conditional_type(self, source);
            let t_true = get_true_type_from_conditional_type(self, target);
            let s_false = get_false_type_from_conditional_type(self, source);
            let t_false = get_false_type_from_conditional_type(self, target);
            let s_check = self.get_non_distributed_type_parameter(s_check);
            let s_extends = self.get_non_distributed_type_parameter(s_extends);
            let s_true = self.get_non_distributed_type_parameter(s_true);
            let s_false = self.get_non_distributed_type_parameter(s_false);
            self.infer_from_types(n, s_check, t_check);
            self.infer_from_types(n, s_extends, t_extends);
            self.infer_from_types(n, s_true, t_true);
            self.infer_from_types(n, s_false, t_false);
        } else {
            let t_true = get_true_type_from_conditional_type(self, target);
            let t_false = get_false_type_from_conditional_type(self, target);
            let target_flags = self.type_(target).flags;
            let new_priority = if n.contravariant {
                InferencePriority::CONTRAVARIANT_CONDITIONAL
            } else {
                InferencePriority::NONE
            };
            self.infer_to_multiple_types_with_priority(
                n,
                source,
                vec![t_true, t_false],
                target_flags,
                new_priority,
            );
        }
    }

    /// Go: `func (c *Checker) inferToTemplateLiteralType(n, source, target)`.
    fn infer_to_template_literal_type(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        let matches = infer_types_from_template_literal_type(
            self,
            source,
            target,
            TypeComparerKind::Assignable,
            None,
        );
        let (target_texts, types) = template_literal_parts(self, target);
        // When the target template literal contains only placeholders (meaning that inference is intended to extract
        // single characters and remainder strings) and inference fails to produce matches, we want to infer 'never' for
        // each placeholder such that instantiation with the inferred value(s) produces 'never', a type for which an
        // assignment check will fail. If we make no inferences, we'll likely end up with the constraint 'string' which,
        // upon instantiation, would collapse all the placeholders to just 'string', and an assignment check might
        // succeed. That would be a pointless and confusing outcome. (Go comment, verbatim.)
        if matches.is_some() || target_texts.iter().all(|s| s.is_empty()) {
            let matches = matches.unwrap_or_default();
            for (i, &target) in types.iter().enumerate() {
                let source: TypeId = if !matches.is_empty() {
                    matches[i]
                } else {
                    self.never_type()
                };
                // If we are inferring from a string literal type to a type variable whose constraint includes one of the
                // allowed template literal placeholder types, infer from a literal type corresponding to the constraint. (Go comment condensed.)
                if self.type_(source).flags.intersects(TypeFlags::STRING_LITERAL)
                    && self.type_(target).flags.intersects(TypeFlags::TYPE_VARIABLE)
                {
                    if let Some(inference_index) = get_inference_info_for_type(self, n, target) {
                        let type_parameter =
                            self.inference_contexts[n.inferences.0 as usize].inferences[inference_index].type_parameter;
                        if let Some(constraint) = get_base_constraint_of_type(self, type_parameter) {
                            if !constraint_is_any(self, constraint) {
                                let mut all_type_flags = TypeFlags::NONE;
                                for t in self.type_(constraint).distributed() {
                                    all_type_flags |= self.type_(t).flags;
                                }
                                // If the constraint contains `string`, we don't need to look for a more preferred type (Go comment, verbatim.)
                                if !all_type_flags.intersects(TypeFlags::STRING) {
                                    let str = get_string_literal_value(self, source);
                                    // If the type contains `number` or a number literal and the string isn't a valid number, exclude numbers (Go comment, verbatim.)
                                    if all_type_flags.intersects(TypeFlags::NUMBER_LIKE)
                                        && !crate::utilities::is_valid_number_string(&str, true /*roundTripOnly*/)
                                    {
                                        all_type_flags &= !TypeFlags::NUMBER_LIKE;
                                    }
                                    // If the type contains `bigint` or a bigint literal and the string isn't a valid bigint, exclude bigints (Go comment, verbatim.)
                                    if all_type_flags.intersects(TypeFlags::BIGINT_LIKE)
                                        && !is_valid_bigint_string(&str, true /*roundTripOnly*/)
                                    {
                                        all_type_flags &= !TypeFlags::BIGINT_LIKE;
                                    }
                                    let mut matching_type = self.never_type();
                                    for t in self.type_(constraint).distributed() {
                                        matching_type = self.choose_template_literal_constraint_type(
                                            all_type_flags, &str, source, matching_type, t,
                                        );
                                    }
                                    if !self.type_(matching_type).flags.intersects(TypeFlags::NEVER) {
                                        self.infer_from_types(n, matching_type, target);
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                self.infer_from_types(n, source, target);
            }
        }
    }

    /// Go: the `choose` closure of inferToTemplateLiteralType — the
    /// constraint-preference ladder (left = the running choice, right = the
    /// constraint constituent under consideration).
    fn choose_template_literal_constraint_type(
        &mut self,
        all_type_flags: TypeFlags,
        str: &str,
        source: TypeId,
        left: TypeId,
        right: TypeId,
    ) -> TypeId {
        let left_flags = self.type_(left).flags;
        let right_flags = self.type_(right).flags;
        if !right_flags.intersects(all_type_flags) {
            return left;
        }
        if left_flags.intersects(TypeFlags::STRING) {
            return left;
        }
        if right_flags.intersects(TypeFlags::STRING) {
            return source;
        }
        if left_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            return left;
        }
        if right_flags.intersects(TypeFlags::TEMPLATE_LITERAL)
            && is_type_matched_by_template_literal_type(self, source, right)
        {
            return source;
        }
        if left_flags.intersects(TypeFlags::STRING_MAPPING) {
            return left;
        }
        if right_flags.intersects(TypeFlags::STRING_MAPPING)
            && str == string_mapping_symbol(self, right)
                .map(|s| self.program_symbol(s).name.clone())
                .unwrap_or_default()
        {
            return source;
        }
        if left_flags.intersects(TypeFlags::STRING_LITERAL) {
            return left;
        }
        if right_flags.intersects(TypeFlags::STRING_LITERAL) && get_string_literal_value(self, right) == str {
            return right;
        }
        if left_flags.intersects(TypeFlags::NUMBER) {
            return left;
        }
        if right_flags.intersects(TypeFlags::NUMBER) {
            let value = tsc_jsnum::from_string(str);
            return self.get_number_literal_type(value);
        }
        if left_flags.intersects(TypeFlags::ENUM) {
            return left;
        }
        if right_flags.intersects(TypeFlags::ENUM) {
            let value = tsc_jsnum::from_string(str);
            return self.get_number_literal_type(value);
        }
        if left_flags.intersects(TypeFlags::NUMBER_LITERAL) {
            return left;
        }
        if right_flags.intersects(TypeFlags::NUMBER_LITERAL)
            && get_number_literal_value(self, right) == tsc_jsnum::from_string(str)
        {
            return right;
        }
        if left_flags.intersects(TypeFlags::BIGINT) {
            return left;
        }
        if right_flags.intersects(TypeFlags::BIGINT) {
            return self.parse_bigint_literal_type(str);
        }
        if left_flags.intersects(TypeFlags::BIGINT_LITERAL) {
            return left;
        }
        if right_flags.intersects(TypeFlags::BIGINT_LITERAL)
            && get_bigint_literal_string(self, right) == str
        {
            return right;
        }
        if left_flags.intersects(TypeFlags::BOOLEAN) {
            return left;
        }
        if right_flags.intersects(TypeFlags::BOOLEAN) {
            return match str {
                "true" => self.true_type(),
                "false" => self.false_type(),
                _ => self.boolean_type(),
            };
        }
        if left_flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
            return left;
        }
        if right_flags.intersects(TypeFlags::BOOLEAN_LITERAL)
            && if get_boolean_literal_value(self, right) { "true" } else { "false" } == str
        {
            return right;
        }
        if left_flags.intersects(TypeFlags::UNDEFINED) {
            return left;
        }
        if right_flags.intersects(TypeFlags::UNDEFINED)
            && self
                .type_(right)
                .as_intrinsic_type()
                .map(|d| d.intrinsic_name.clone())
                .as_deref()
                == Some(str)
        {
            return right;
        }
        if left_flags.intersects(TypeFlags::NULL) {
            return left;
        }
        if right_flags.intersects(TypeFlags::NULL)
            && self
                .type_(right)
                .as_intrinsic_type()
                .map(|d| d.intrinsic_name.clone())
                .as_deref()
                == Some(str)
        {
            return right;
        }
        left
    }

    /// Go: `func (c *Checker) inferFromGenericMappedTypes(n, source, target)`.
    fn infer_from_generic_mapped_types(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        // The source and target types are generic types { [P in S]: X } and { [P in T]: Y }, so we infer
        // from S to T and from X to Y. (Go comment, verbatim.)
        let s_constraint = get_constraint_type_from_mapped_type(self, source);
        let t_constraint = get_constraint_type_from_mapped_type(self, target);
        self.infer_from_types(n, s_constraint, t_constraint);
        let s_template = get_template_type_from_mapped_type(self, source);
        let t_template = get_template_type_from_mapped_type(self, target);
        self.infer_from_types(n, s_template, t_template);
        let source_name_type = get_name_type_from_mapped_type(self, source);
        let target_name_type = get_name_type_from_mapped_type(self, target);
        if let Some(source_name_type) = source_name_type {
            if let Some(target_name_type) = target_name_type {
                self.infer_from_types(n, source_name_type, target_name_type);
            }
        }
    }

    /// Go: `func (c *Checker) inferFromObjectTypes(n, source, target)`.
    fn infer_from_object_types(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        if self.type_(source).object_flags.intersects(ObjectFlags::REFERENCE)
            && self.type_(target).object_flags.intersects(ObjectFlags::REFERENCE)
            && (type_reference_target(self, source) == type_reference_target(self, target)
                || is_array_type(self, source) && is_array_type(self, target))
        {
            // If source and target are references to the same generic type, infer from type arguments (Go comment, verbatim.)
            self.infer_from_reference_type_arguments(n, source, target);
            return;
        }
        if is_generic_mapped_type(self, source) && is_generic_mapped_type(self, target) {
            self.infer_from_generic_mapped_types(n, source, target);
        }
        if self.type_(target).object_flags.intersects(ObjectFlags::MAPPED)
            && get_name_type_from_mapped_type(self, target).is_none()
        {
            let constraint_type = get_constraint_type_from_mapped_type(self, target);
            if self.infer_to_mapped_type(n, source, target, constraint_type) {
                return;
            }
        }
        // Infer from the members of source and target only if the two types are possibly related (Go comment, verbatim.)
        if self.types_definitely_unrelated(source, target) {
            return;
        }
        if is_array_or_tuple_type(self, source) {
            if crate::relater::is_tuple_type(self, target) {
                let source_arity = crate::relater::get_type_reference_arity(self, source);
                let target_arity = crate::relater::get_type_reference_arity(self, target);
                let element_types = get_type_arguments(self, target);
                let element_infos = tuple_element_infos(self, target);
                let source_is_tuple = crate::relater::is_tuple_type(self, source);
                // When source and target are tuple types with the same structure (fixed, variadic, and rest are matched
                // to the same kind in each position), simply infer between the element types. (Go comment, verbatim.)
                if source_is_tuple && self.is_tuple_type_structure_matching(source, target) {
                    let source_args = get_type_arguments(self, source);
                    for i in 0..target_arity as usize {
                        self.infer_from_types(n, source_args[i], element_types[i]);
                    }
                    return;
                }
                let mut start_length = 0usize;
                let mut end_length = 0usize;
                if source_is_tuple {
                    start_length =
                        tuple_type_fixed_length(self, source).min(tuple_type_fixed_length(self, target)) as usize;
                    if tuple_type_combined_flags(self, target).intersects(ElementFlags::VARIADIC) {
                        end_length = crate::relater::get_end_element_count(self, source, ElementFlags::FIXED)
                            .min(crate::relater::get_end_element_count(self, target, ElementFlags::FIXED));
                    }
                }
                // Infer between starting fixed elements. (Go comment, verbatim.)
                {
                    let source_args = get_type_arguments(self, source);
                    for i in 0..start_length {
                        self.infer_from_types(n, source_args[i], element_types[i]);
                    }
                }
                if !source_is_tuple
                    || source_arity as usize - start_length - end_length == 1
                        && tuple_type_element_flags(self, source, start_length).intersects(ElementFlags::REST)
                {
                    // Single rest element remains in source, infer from that to every element in target (Go comment, verbatim.)
                    let source_args = get_type_arguments(self, source);
                    let rest_type = source_args[start_length];
                    for i in start_length..target_arity as usize - end_length {
                        let mut t = rest_type;
                        if element_infos[i].flags.intersects(ElementFlags::VARIADIC) {
                            t = self.create_array_type(t);
                        }
                        self.infer_from_types(n, t, element_types[i]);
                    }
                } else {
                    let middle_length = target_arity as usize - start_length - end_length;
                    if middle_length == 2
                        && element_infos[start_length]
                            .flags
                            .intersects(element_infos[start_length + 1].flags & ElementFlags::VARIADIC)
                    {
                        // Middle of target is [...T, ...U] and source is tuple type (Go comment, verbatim.)
                        if let Some(target_info_index) =
                            get_inference_info_for_type(self, n, element_types[start_length])
                        {
                            let implied_arity =
                                self.inference_contexts[n.inferences.0 as usize].inferences[target_info_index].implied_arity;
                            if implied_arity >= 0 {
                                // Infer slices from source based on implied arity of T. (Go comment, verbatim.)
                                let slice1 = self.slice_tuple_type(
                                    source,
                                    start_length,
                                    end_length + source_arity as usize - implied_arity as usize,
                                );
                                self.infer_from_types(n, slice1, element_types[start_length]);
                                let slice2 =
                                    self.slice_tuple_type(source, start_length + implied_arity as usize, end_length);
                                self.infer_from_types(n, slice2, element_types[start_length + 1]);
                            }
                        }
                    } else if middle_length == 2
                        && element_infos[start_length].flags.intersects(ElementFlags::VARIADIC)
                        && element_infos[start_length + 1].flags.intersects(ElementFlags::REST)
                    {
                        // Middle of target is [...T, ...rest] and source is tuple type
                        // if T is constrained by a fixed-size tuple we might be able to use its arity to infer T (Go comments, verbatim.)
                        if let Some(info_index) = get_inference_info_for_type(self, n, element_types[start_length]) {
                            let type_parameter =
                                self.inference_contexts[n.inferences.0 as usize].inferences[info_index].type_parameter;
                            if let Some(constraint) = get_base_constraint_of_type(self, type_parameter) {
                                if crate::relater::is_tuple_type(self, constraint)
                                    && !tuple_type_combined_flags(self, constraint).intersects(ElementFlags::VARIABLE)
                                {
                                    let implied_arity = tuple_type_fixed_length(self, constraint) as usize;
                                    let slice = self.slice_tuple_type(
                                        source,
                                        start_length,
                                        source_arity as usize - (start_length + implied_arity),
                                    );
                                    self.infer_from_types(n, slice, element_types[start_length]);
                                    if let Some(rest_type) = self.get_element_type_of_slice_of_tuple_type(
                                        source, start_length + implied_arity, end_length, false, false,
                                    ) {
                                        self.infer_from_types(n, rest_type, element_types[start_length + 1]);
                                    }
                                }
                            }
                        }
                    } else if middle_length == 2
                        && element_infos[start_length].flags.intersects(ElementFlags::REST)
                        && element_infos[start_length + 1].flags.intersects(ElementFlags::VARIADIC)
                    {
                        // Middle of target is [...rest, ...T] and source is tuple type
                        // if T is constrained by a fixed-size tuple we might be able to use its arity to infer T (Go comments, verbatim.)
                        if let Some(info_index) = get_inference_info_for_type(self, n, element_types[start_length + 1]) {
                            let type_parameter =
                                self.inference_contexts[n.inferences.0 as usize].inferences[info_index].type_parameter;
                            if let Some(constraint) = get_base_constraint_of_type(self, type_parameter) {
                                if crate::relater::is_tuple_type(self, constraint)
                                    && !tuple_type_combined_flags(self, constraint).intersects(ElementFlags::VARIABLE)
                                {
                                    let implied_arity = tuple_type_fixed_length(self, constraint) as usize;
                                    let end_index = source_arity as usize
                                        - crate::relater::get_end_element_count(self, target, ElementFlags::FIXED);
                                    let start_index = end_index - implied_arity;
                                    if start_index >= start_length {
                                        let source_args = get_type_arguments(self, source);
                                        let slice_types: Vec<TypeId> = source_args[start_index..end_index].to_vec();
                                        let slice_infos: Vec<crate::types::TupleElementInfo> =
                                            tuple_element_infos(self, source)[start_index..end_index].to_vec();
                                        let trailing_slice = self.create_tuple_type_ex(
                                            &slice_types,
                                            &slice_infos,
                                            false, /*readonly*/
                                        );
                                        if let Some(rest_type) = self.get_element_type_of_slice_of_tuple_type(
                                            source, start_length, end_length + implied_arity, false, false,
                                        ) {
                                            self.infer_from_types(n, rest_type, element_types[start_length]);
                                        }
                                        self.infer_from_types(n, trailing_slice, element_types[start_length + 1]);
                                    }
                                }
                            }
                        }
                    } else if middle_length == 1
                        && element_infos[start_length].flags.intersects(ElementFlags::VARIADIC)
                    {
                        // Middle of target is exactly one variadic element. Infer the slice between the fixed parts in the source.
                        // If target ends in optional element(s), make a lower priority a speculative inference. (Go comment, verbatim.)
                        let priority = if element_infos[target_arity as usize - 1].flags.intersects(ElementFlags::OPTIONAL) {
                            InferencePriority::SPECULATIVE_TUPLE
                        } else {
                            InferencePriority::NONE
                        };
                        let source_slice = self.slice_tuple_type(source, start_length, end_length);
                        self.infer_with_priority(n, source_slice, element_types[start_length], priority);
                    } else if middle_length == 1
                        && element_infos[start_length].flags.intersects(ElementFlags::REST)
                    {
                        // Middle of target is exactly one rest element. If middle of source is not empty, infer union of middle element types. (Go comment, verbatim.)
                        if let Some(rest_type) =
                            self.get_element_type_of_slice_of_tuple_type(source, start_length, end_length, false, false)
                        {
                            self.infer_from_types(n, rest_type, element_types[start_length]);
                        }
                    }
                }
                // Infer between ending fixed elements (Go comment, verbatim.)
                {
                    let source_args = get_type_arguments(self, source);
                    for i in 0..end_length {
                        self.infer_from_types(
                            n,
                            source_args[source_arity as usize - i - 1],
                            element_types[target_arity as usize - i - 1],
                        );
                    }
                }
                return;
            }
            if is_array_type(self, target) {
                self.infer_from_index_types(n, source, target);
                return;
            }
        }
        self.infer_from_properties(n, source, target);
        self.infer_from_signatures(n, source, target, SignatureKind::Call);
        self.infer_from_signatures(n, source, target, SignatureKind::Construct);
        self.infer_from_index_types(n, source, target);
    }

    /// Go: `func (c *Checker) inferFromProperties(n, source, target)`.
    fn infer_from_properties(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        let properties = get_properties_of_object_type(self, target);
        for target_prop in properties {
            let prop_name = self.program_symbol(target_prop).name.clone();
            if let Some(source_prop) = crate::relater::get_property_of_type(self, source, &prop_name) {
                if !self.is_skip_direct_inference_node_for_symbol(source_prop) {
                    let source_optional =
                        self.program_symbol(source_prop).flags.intersects(SymbolFlags::OPTIONAL);
                    let source_type = self.get_type_of_symbol(source_prop);
                    let source_type = remove_missing_type(self, source_type, source_optional);
                    let target_optional =
                        self.program_symbol(target_prop).flags.intersects(SymbolFlags::OPTIONAL);
                    let target_type = self.get_type_of_symbol(target_prop);
                    let target_type = remove_missing_type(self, target_type, target_optional);
                    self.infer_from_types(n, source_type, target_type);
                }
            }
        }
    }

    /// Go: `func (c *Checker) inferFromSignatures(n, source, target, kind)`.
    fn infer_from_signatures(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        kind: SignatureKind,
    ) {
        let source_signatures = get_signatures_of_type(self, source, kind);
        let source_len = source_signatures.len();
        if source_len > 0 {
            // We match source and target signatures from the bottom up, and if the source has fewer signatures
            // than the target, we infer from the first source signature to the excess target signatures. (Go comment, verbatim.)
            let target_signatures = get_signatures_of_type(self, target, kind);
            let target_len = target_signatures.len();
            for (i, &target_signature) in target_signatures.iter().enumerate() {
                let source_index = source_len.saturating_sub(target_len) + i;
                let base = self.get_base_signature(source_signatures[source_index]);
                let erased = get_erased_signature(self, target_signature);
                self.infer_from_signature(n, base, erased);
            }
        }
    }

    /// Go: `func (c *Checker) inferFromSignature(n, source, target)`.
    fn infer_from_signature(
        &mut self,
        n: &mut InferenceState,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
    ) {
        if !crate::relater::sig(self, source)
            .flags
            .intersects(SignatureFlags::IS_NON_INFERRABLE)
        {
            let save_bivariant = n.bivariant;
            let kind = crate::relater::sig(self, target)
                .declaration
                .map(|d| self.store().node(d).kind)
                .unwrap_or(Kind::Unknown);
            // Once we descend into a bivariant signature we remain bivariant for all nested inferences (Go comment, verbatim.)
            n.bivariant = n.bivariant
                || kind == Kind::MethodDeclaration
                || kind == Kind::MethodSignature
                || kind == Kind::Constructor;
            self.apply_to_parameter_types(n, source, target, |c, n, s, t| {
                c.infer_from_contravariant_types_if_strict_function_types(n, s, t)
            });
            n.bivariant = save_bivariant;
        }
        self.apply_to_return_types(n, source, target, |c, n, s, t| c.infer_from_types(n, s, t));
    }

    /// Go: `func (c *Checker) applyToParameterTypes(source, target, callback)`.
    ///
    /// PORT: Go's callback closes over `n *InferenceState`; a Rust fn pointer
    /// cannot capture it, so the state is an explicit parameter (the one
    /// signature divergence; the two call shapes in inferFromSignature pass
    /// the same state through).
    fn apply_to_parameter_types(
        &mut self,
        n: &mut InferenceState,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
        callback: fn(&mut Checker, &mut InferenceState, TypeId, TypeId),
    ) {
        let source_count = self.get_parameter_count(source);
        let target_count = self.get_parameter_count(target);
        let source_rest_type = self.get_effective_rest_type(source);
        let target_rest_type = self.get_effective_rest_type(target);
        let mut target_non_rest_count = target_count;
        if target_rest_type.is_some() {
            target_non_rest_count -= 1;
        }
        let mut param_count = target_non_rest_count;
        if source_rest_type.is_none() {
            param_count = source_count.min(target_non_rest_count);
        }
        let source_this_type = self.get_this_type_of_signature(source);
        if source_this_type.is_some() {
            let target_this_type = self.get_this_type_of_signature(target);
            if let (Some(source_this_type), Some(target_this_type)) = (source_this_type, target_this_type) {
                callback(self, n, source_this_type, target_this_type);
            }
        }
        for i in 0..param_count as usize {
            let s = self.get_type_at_position(source, i);
            let t = self.get_type_at_position(target, i);
            callback(self, n, s, t);
        }
        if let Some(target_rest_type) = target_rest_type {
            let readonly = self.is_const_type_variable(target_rest_type, 0)
                && !some_type_mutable_array_like(self, target_rest_type);
            // PORT: Go's getRestTypeAtPosition may return nil here; Go would
            // crash on the nil source inside inferFromTypes, so the situation
            // is out-of-contract — the port skips the callback.
            if let Some(source_rest) = self.get_rest_type_at_position(source, param_count as usize, readonly) {
                callback(self, n, source_rest, target_rest_type);
            }
        }
    }

    /// Go: `func (c *Checker) applyToReturnTypes(source, target, callback)`.
    fn apply_to_return_types(
        &mut self,
        n: &mut InferenceState,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
        callback: fn(&mut Checker, &mut InferenceState, TypeId, TypeId),
    ) {
        let target_type_predicate = self.get_type_predicate_of_signature(target);
        if let Some(target_type_predicate) = target_type_predicate {
            let source_type_predicate = self.get_type_predicate_of_signature(source);
            let matches = source_type_predicate.as_ref().is_some_and(|source_type_predicate| {
                self.type_predicate_kinds_match(source_type_predicate, &target_type_predicate)
                    && source_type_predicate.t.is_some()
                    && target_type_predicate.t.is_some()
            });
            if matches {
                let s = source_type_predicate.as_ref().expect("checked").t.expect("checked");
                let t = target_type_predicate.t.expect("checked");
                callback(self, n, s, t);
                return;
            }
        }
        let target_return_type = self.get_return_type_of_signature(target);
        if self.could_contain_type_variables(target_return_type) {
            let source_return_type = self.get_return_type_of_signature(source);
            callback(self, n, source_return_type, target_return_type);
        }
    }

    /// Go: `func (c *Checker) inferFromIndexTypes(n, source, target)`.
    fn infer_from_index_types(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        // Inferences across mapped type index signatures are pretty much the same a inferences to homomorphic variables (Go comment, verbatim.)
        let mut priority = InferencePriority::NONE;
        if self.type_(source).object_flags.intersects(ObjectFlags::MAPPED)
            && self.type_(target).object_flags.intersects(ObjectFlags::MAPPED)
        {
            priority = InferencePriority::HOMOMORPHIC_MAPPED_TYPE;
        }
        let index_infos = get_index_infos_of_type(self, target);
        if self.is_object_type_with_inferable_index(source) {
            for &target_info in &index_infos {
                let target_key_type = self.index_info_key_type(target_info);
                let mut prop_types: Vec<TypeId> = Vec::new();
                let properties = get_properties_of_object_type(self, source);
                for prop in properties {
                    let literal_type =
                        get_literal_type_from_property(self, prop, TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE, false);
                    if is_applicable_index_type(self, literal_type, target_key_type) {
                        let mut prop_type = self.get_type_of_symbol(prop);
                        if self.program_symbol(prop).flags.intersects(SymbolFlags::OPTIONAL) {
                            prop_type = self.remove_missing_or_undefined_type(prop_type);
                        }
                        prop_types.push(prop_type);
                    }
                }
                for &info in &get_index_infos_of_type(self, source) {
                    let info_key_type = self.index_info_key_type(info);
                    if is_applicable_index_type(self, Some(info_key_type), target_key_type) {
                        prop_types.push(self.index_info_value_type(info));
                    }
                }
                if !prop_types.is_empty() {
                    let union = get_union_type(self, prop_types);
                    let target_value_type = self.index_info_value_type(target_info);
                    self.infer_with_priority(n, union, target_value_type, priority);
                }
            }
        }
        for &target_info in &index_infos {
            let target_key_type = self.index_info_key_type(target_info);
            if let Some(source_info) = self.get_applicable_index_info_impl(source, target_key_type) {
                let source_value_type = self.index_info_value_type(source_info);
                let target_value_type = self.index_info_value_type(target_info);
                self.infer_with_priority(n, source_value_type, target_value_type, priority);
            }
        }
    }

    /// Go: `func (c *Checker) inferToMappedType(n, source, target, constraintType) bool`.
    fn infer_to_mapped_type(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        constraint_type: TypeId,
    ) -> bool {
        if self.type_(constraint_type).flags.intersects(TypeFlags::UNION)
            || self.type_(constraint_type).flags.intersects(TypeFlags::INTERSECTION)
        {
            let mut result = false;
            for t in self.type_(constraint_type).distributed() {
                result = self.infer_to_mapped_type(n, source, target, t) || result;
            }
            return result;
        }
        if self.type_(constraint_type).flags.intersects(TypeFlags::INDEX) {
            // We're inferring from some source type S to a homomorphic mapped type { [P in keyof T]: X },
            // where T is a type variable. Use inferTypeForHomomorphicMappedType to infer a suitable source
            // type and then make a secondary inference from that type to T. We make a secondary inference
            // such that direct inferences to T get priority over inferences to Partial<T>, for example. (Go comment, verbatim.)
            let index_target_type = index_target(self, constraint_type);
            if let Some(inference_index) = get_inference_info_for_type(self, n, index_target_type) {
                let inference_fixed = self.inference_contexts[n.inferences.0 as usize].inferences[inference_index].is_fixed;
                if !inference_fixed && !self.is_from_inference_blocked_source(source) {
                    if let Some(inferred_type) = self.infer_type_for_homomorphic_mapped_type(source, target, constraint_type) {
                        // We assign a lower priority to inferences made from types containing non-inferrable
                        // types because we may only have a partial result (i.e. we may have failed to make
                        // reverse inferences for some properties). (Go comment, verbatim.)
                        let priority = if self.type_(source).object_flags.intersects(ObjectFlags::NON_INFERRABLE_TYPE) {
                            InferencePriority::PARTIAL_HOMOMORPHIC_MAPPED_TYPE
                        } else {
                            InferencePriority::HOMOMORPHIC_MAPPED_TYPE
                        };
                        let type_parameter =
                            self.inference_contexts[n.inferences.0 as usize].inferences[inference_index].type_parameter;
                        self.infer_with_priority(n, inferred_type, type_parameter, priority);
                    }
                }
            }
            return true;
        }
        if self.type_(constraint_type).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            // We're inferring from some source type S to a mapped type { [P in K]: X }, where K is a type
            // parameter. First infer from 'keyof S' to K. (Go comment condensed.)
            // TODO(port M6 W4): c.patternForType[source] — the nodebuilder's
            // pattern cache gates IndexFlagsNoIndexSignatures; permissive: none.
            let index_type = get_index_type_ex(self, source, crate::types::IndexFlags::NONE);
            self.infer_with_priority(n, index_type, constraint_type, InferencePriority::MAPPED_TYPE_CONSTRAINT);
            // If K is constrained to a type C, also infer to C. Thus, for a mapped type { [P in K]: X },
            // where K extends keyof T, we make the same inferences as for a homomorphic mapped type
            // { [P in keyof T]: X }. This enables us to make meaningful inferences when the target is a
            // Pick<T, K>. (Go comment, verbatim.)
            if let Some(extended_constraint) = get_constraint_of_type(self, constraint_type) {
                if self.infer_to_mapped_type(n, source, target, extended_constraint) {
                    return true;
                }
            }
            // If no inferences can be made to K's constraint, infer from a union of the property types
            // in the source to the template type X. (Go comment, verbatim.)
            let mut type_union: Vec<TypeId> = Vec::new();
            for prop in get_properties_of_object_type(self, source) {
                type_union.push(self.get_type_of_symbol(prop));
            }
            for info in get_index_infos_of_type(self, source) {
                // TODO(port M6 S7): c.enumNumberIndexInfo — the enum-number
                // index-info identity; permissive: the info's value type.
                type_union.push(self.index_info_value_type(info));
            }
            let union = get_union_type(self, type_union);
            let template_type = get_template_type_from_mapped_type(self, target);
            self.infer_from_types(n, union, template_type);
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) inferTypeForHomomorphicMappedType(source, target, constraint) *Type`.
    fn infer_type_for_homomorphic_mapped_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> Option<TypeId> {
        // TODO(port M6 S7): the reverse-homomorphic cache + createReverseMappedType —
        // needs newObjectType/reverse-mapped payload construction (S7's type
        // constructors); permissive None (no secondary inference).
        let _ = (source, target, constraint);
        None
    }

    /// Go: `func (c *Checker) createReverseMappedType(source, target, constraint) *Type`.
    fn create_reverse_mapped_type(&mut self, source: TypeId, target: TypeId, constraint: TypeId) -> Option<TypeId> {
        // TODO(port M6 S7): newObjectType(ObjectFlagsReverseMapped|ObjectFlagsAnonymous)
        // + the reverse-mapped payload; permissive None.
        let _ = (source, target, constraint);
        None
    }

    /// Go: `func (c *Checker) isPartiallyInferableType(t) bool`.
    fn is_partially_inferable_type(&mut self, t: TypeId) -> bool {
        // We consider a type to be partially inferable if it isn't marked non-inferable or if it is
        // an object literal type with at least one property of an inferable type. For example, an object
        // literal { a: 123, b: x => true } is marked non-inferable because it contains a context sensitive
        // arrow function, but is considered partially inferable because property 'a' has an inferable type. (Go comment, verbatim.)
        if !self.type_(t).object_flags.intersects(ObjectFlags::NON_INFERRABLE_TYPE) {
            return true;
        }
        if crate::relater::is_object_literal_type(self, t) {
            let properties = get_properties_of_object_type(self, t);
            for prop in properties {
                let prop_type = self.get_type_of_symbol(prop);
                if self.is_partially_inferable_type(prop_type) {
                    return true;
                }
            }
        }
        if crate::relater::is_tuple_type(self, t) {
            for element_type in self.get_element_types(t) {
                if self.is_partially_inferable_type(element_type) {
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) inferReverseMappedType(source, target, constraint) *Type`.
    fn infer_reverse_mapped_type(&mut self, source: TypeId, target: TypeId, constraint: TypeId) -> Option<TypeId> {
        // TODO(port M6 S7): the reverse-mapped cache/stacks + the worker
        // (getIndexedAccessType + the standalone-inference shape); the
        // arena-context variant of inferTypes is the W2 seam; permissive None.
        let _ = (source, target, constraint);
        None
    }

    /// Go: `func (c *Checker) resolveReverseMappedTypeMembers(t) *Type` (inference.go:1110)
    /// — the REAL body (S8: setStructuredTypeMembers + the mapped-member
    /// plumbing landed; `inferReverseMappedType` stays the engines-wave
    /// seam, permissive None → unknownType).
    pub(crate) fn resolve_reverse_mapped_type_members(&mut self, t: TypeId) {
        let (source, mapped_type, constraint_type) = {
            let r = self
                .type_(t)
                .as_reverse_mapped_type()
                .expect("ReverseMappedType data");
            (
                r.source.expect("reverse-mapped source"),
                r.mapped_type.expect("reverse-mapped mapped type"),
                r.constraint_type.expect("reverse-mapped constraint"),
            )
        };
        let source_index_infos = self.get_index_infos_of_type_impl(source);
        let string_type = self.string_type();
        let index_info = crate::members::find_index_info(
            self,
            &source_index_infos,
            string_type,
        );
        let modifiers = self.mapped_type_modifiers_impl(mapped_type);
        let readonly_mask = !modifiers
            .intersects(crate::relater::MappedTypeModifiers::INCLUDE_READONLY);
        let optional_mask = if modifiers
            .intersects(crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL)
        {
            tsc_ast::SymbolFlags::NONE
        } else {
            tsc_ast::SymbolFlags::OPTIONAL
        };
        let mut index_infos: Vec<crate::types::IndexInfoId> = Vec::new();
        if let Some(index_info) = index_info {
            let value_type = self.index_info_value_type(index_info);
            let inferred = self
                .infer_reverse_mapped_type(value_type, mapped_type, constraint_type)
                .unwrap_or_else(|| self.unknown_type());
            let is_readonly =
                readonly_mask && self.index_info_is_readonly(index_info);
            index_infos.push(crate::types::IndexInfoId(
                self.new_index_info(self.string_type(), inferred, is_readonly, None, Vec::new())
                    as u32,
            ));
        }
        let mut members = tsc_collections::OrderedMap::new();
        let limited_constraint = self.get_limited_constraint(t);
        for prop in self.get_properties_of_type_impl(source) {
            // In case of a reverse mapped type with an intersection
            // constraint, if we were able to extract the filtering type
            // literals we skip those properties that are not assignable to
            // them, because the extra properties wouldn't get through the
            // application of the mapped type anyway (Go's comment).
            if let Some(limited_constraint) = limited_constraint {
                if let Some(property_name_type) = self.get_literal_type_from_property_impl(
                    prop,
                    TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                    false,
                ) {
                    if !self.is_type_assignable_to(property_name_type, limited_constraint) {
                        continue;
                    }
                }
            }
            let check_flags = tsc_ast::CheckFlags::REVERSE_MAPPED
                | if readonly_mask && self.is_readonly_symbol(prop) {
                    tsc_ast::CheckFlags::READONLY
                } else {
                    tsc_ast::CheckFlags::NONE
                };
            let prop_flags = tsc_ast::SymbolFlags::PROPERTY
                | (self.program_symbol(prop).flags & optional_mask);
            let prop_name = self.program_symbol(prop).name.clone();
            let inferred_prop = self.new_symbol_ex(prop_flags, &prop_name, check_flags);
            let declarations = self.program_symbol(prop).declarations.clone();
            self.program_symbol_mut(inferred_prop).declarations = declarations;
            let name_type = self.value_symbol_links.get(prop).name_type;
            self.value_symbol_links.get(inferred_prop).name_type = name_type;
            let property_type = self.get_type_of_symbol(prop);
            self.reverse_mapped_symbol_links.get(inferred_prop).property_type = Some(property_type);
            let constraint_target = self
                .type_(constraint_type)
                .as_index_type()
                .expect("IndexType data")
                .target
                .expect("IndexType target");
            let (object_type, index_type) = {
                let d = self
                    .type_(constraint_target)
                    .as_indexed_access_type()
                    .expect("IndexedAccessType data");
                (
                    d.object_type.expect("objectType"),
                    d.index_type.expect("indexType"),
                )
            };
            if self.type_(constraint_target).flags.intersects(TypeFlags::INDEXED_ACCESS)
                && self.type_(object_type).flags.intersects(TypeFlags::TYPE_PARAMETER)
                && self.type_(index_type).flags.intersects(TypeFlags::TYPE_PARAMETER)
            {
                // A reverse mapping of `{[K in keyof T[K_1]]: T[K_1]}` is the
                // same as that of `{[K in keyof T]: T}`, since all we care
                // about is inferring to the "type parameter" (or indexed
                // access) shared by the constraint and template. So, to
                // reduce the number of type identities produced, we simplify
                // such indexed access occurrences (Go's comment).
                let new_type_param = object_type;
                let new_mapped_type = self.replace_indexed_access(mapped_type, constraint_target, new_type_param);
                let new_constraint = crate::relater::get_index_type(self, new_type_param);
                let links = self.reverse_mapped_symbol_links.get(inferred_prop);
                links.mapped_type = Some(new_mapped_type);
                links.constraint_type = Some(new_constraint);
            } else {
                let links = self.reverse_mapped_symbol_links.get(inferred_prop);
                links.mapped_type = Some(mapped_type);
                links.constraint_type = Some(constraint_type);
            }
            members.set(prop_name, inferred_prop);
        }
        self.set_structured_type_members(t, members, Vec::new(), Vec::new(), index_infos);
    }

    // Go: `func (c *Checker) getTypeOfReverseMappedSymbol(symbol) *Type` —
    // REMOVED (M6 S2): the W2 permissive stub is superseded by the real
    // reverse-mapped links read in typeofsymbol.rs; this module's call sites
    // resolve there.
    /// Go: `func (c *Checker) getLimitedConstraint(t) *Type`.
    fn get_limited_constraint(&mut self, t: TypeId) -> Option<TypeId> {
        // If the original mapped type had an intersection constraint we extract its components,
        // and we make an attempt to do so even if the intersection has been reduced to a union.
        // This entire process allows us to possibly retrieve the filtering type literals.
        // e.g. { [K in keyof U & ("a" | "b") ] } -> "a" | "b" (Go comment, verbatim.)
        let mapped_type = self
            .type_(t)
            .as_reverse_mapped_type()
            .and_then(|d| d.mapped_type)
            .expect("reverse mapped");
        let constraint = get_constraint_type_from_mapped_type(self, mapped_type);
        if !(self.type_(constraint).flags.intersects(TypeFlags::UNION)
            || self.type_(constraint).flags.intersects(TypeFlags::INTERSECTION))
        {
            return None;
        }
        let mut origin = constraint;
        if self.type_(constraint).flags.intersects(TypeFlags::UNION) {
            origin = self
                .type_(constraint)
                .as_union_type()
                .and_then(|d| d.origin)
                .unwrap_or(constraint);
        }
        if !self.type_(origin).flags.intersects(TypeFlags::INTERSECTION) {
            return None;
        }
        let constraint_type = self
            .type_(t)
            .as_reverse_mapped_type()
            .and_then(|d| d.constraint_type)
            .expect("reverse mapped");
        let constituents: Vec<TypeId> = self
            .type_(origin)
            .distributed()
            .into_iter()
            .filter(|&t| t != constraint_type)
            .collect();
        let limited_constraint = crate::relater::get_intersection_type(self, constituents);
        if limited_constraint != self.never_type() {
            return Some(limited_constraint);
        }
        None
    }

    /// Go: `func (c *Checker) replaceIndexedAccess(instantiable, t, replacement) *Type`.
    fn replace_indexed_access(&mut self, instantiable: TypeId, t: TypeId, replacement: TypeId) -> TypeId {
        // map type.indexType to 0
        // map type.objectType to `[TReplacement]`
        // thus making the indexed access `[TReplacement][0]` or `TReplacement` (Go comment, verbatim.)
        let index_type = indexed_access_index_type(self, t);
        let object_type = indexed_access_object_type(self, t);
        let zero = self.get_number_literal_type(tsc_jsnum::Number::from(0.0));
        let tuple = self.create_tuple_type_ex(&[replacement], &[], false /*readonly*/);
        let mapper = self.new_type_mapper(&[index_type, object_type], &[zero, tuple]);
        self.instantiate_type(instantiable, mapper)
    }

    /// Go: `func (c *Checker) typesDefinitelyUnrelated(source, target) bool`.
    fn types_definitely_unrelated(&mut self, source: TypeId, target: TypeId) -> bool {
        // Two tuple types with incompatible arities are definitely unrelated.
        // Two object types that each have a property that is unmatched in the other are definitely unrelated. (Go comment, verbatim.)
        if crate::relater::is_tuple_type(self, source) && crate::relater::is_tuple_type(self, target) {
            return tuple_types_definitely_unrelated(self, source, target);
        }
        crate::relater::get_unmatched_property(self, source, target, false /*requireOptionalProperties*/, true /*matchDiscriminantProperties*/)
            .is_some()
            && crate::relater::get_unmatched_property(self, target, source, false /*requireOptionalProperties*/, false /*matchDiscriminantProperties*/)
                .is_some()
    }

    /// Go: `func (c *Checker) isTupleTypeStructureMatching(t1, t2) bool`.
    fn is_tuple_type_structure_matching(&mut self, t1: TypeId, t2: TypeId) -> bool {
        if crate::relater::get_type_reference_arity(self, t1) != crate::relater::get_type_reference_arity(self, t2) {
            return false;
        }
        let t1_infos = tuple_element_infos(self, t1);
        let t2_infos = tuple_element_infos(self, t2);
        for (i, e) in t1_infos.iter().enumerate() {
            if e.flags.intersects(ElementFlags::VARIABLE) != t2_infos[i].flags.intersects(ElementFlags::VARIABLE) {
                return false;
            }
        }
        true
    }

    /// Go: `func (c *Checker) isTypeOrBaseIdenticalTo(s, t) bool`.
    fn is_type_or_base_identical_to(&mut self, s: TypeId, t: TypeId) -> bool {
        if t == self.missing_type() {
            return s == t;
        }
        self.is_type_identical_to(s, t)
            || self.type_(t).flags.intersects(TypeFlags::STRING)
                && self.type_(s).flags.intersects(TypeFlags::STRING_LITERAL)
            || self.type_(t).flags.intersects(TypeFlags::NUMBER)
                && self.type_(s).flags.intersects(TypeFlags::NUMBER_LITERAL)
    }

    /// Go: `func (c *Checker) isTypeCloselyMatchedBy(s, t) bool`.
    fn is_type_closely_matched_by(&mut self, s: TypeId, t: TypeId) -> bool {
        (self.type_(s).flags.intersects(TypeFlags::OBJECT)
            && self.type_(t).flags.intersects(TypeFlags::OBJECT)
            && self.type_(s).symbol.is_some()
            && self.type_(s).symbol == self.type_(t).symbol)
            || (self.type_(s).alias.is_some()
                && self.type_(t).alias.is_some()
                && !self.type_(s).alias.as_ref().expect("alias").type_arguments.is_empty()
                && self.type_(s).alias.as_ref().map(|a| a.symbol)
                    == self.type_(t).alias.as_ref().map(|a| a.symbol))
    }

    /// Go: `func (c *Checker) createEmptyObjectTypeFromStringLiteral(t) *Type`.
    fn create_empty_object_type_from_string_literal(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S7): newAnonymousType + the any-typed literal properties
        // (+ the string index info for `string` targets); permissive errorType.
        let _ = t;
        self.error_type()
    }

    /// Go: `func (c *Checker) isTypeParameterAtTopLevel(t, tp, depth) bool`.
    fn is_type_parameter_at_top_level(&mut self, t: TypeId, tp: TypeId, depth: usize) -> bool {
        t == tp
            || (self.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) && {
                let constituents = self.type_(t).distributed();
                constituents
                    .iter()
                    .any(|&t| self.is_type_parameter_at_top_level(t, tp, depth))
            })
            || {
                let flags = self.type_(t).flags;
                depth < 3
                    && flags.intersects(TypeFlags::CONDITIONAL)
                    && {
                        let true_type = get_true_type_from_conditional_type(self, t);
                        let false_type = get_false_type_from_conditional_type(self, t);
                        self.is_type_parameter_at_top_level(true_type, tp, depth + 1)
                            || self.is_type_parameter_at_top_level(false_type, tp, depth + 1)
                    }
            }
    }

    /// Go: `func (c *Checker) isTypeParameterAtTopLevelInReturnType(signature, typeParameter) bool`.
    fn is_type_parameter_at_top_level_in_return_type(
        &mut self,
        signature: crate::types::SignatureId,
        type_parameter: TypeId,
    ) -> bool {
        let type_predicate = self.get_type_predicate_of_signature(signature);
        if let Some(type_predicate) = type_predicate {
            return type_predicate
                .t
                .is_some_and(|t| self.is_type_parameter_at_top_level(t, type_parameter, 0));
        }
        let return_type = self.get_return_type_of_signature(signature);
        self.is_type_parameter_at_top_level(return_type, type_parameter, 0)
    }

    // ── Inference contexts ──

    /// Go: `func (c *Checker) newInferenceContext(typeParameters, signature, flags, compareTypes) *InferenceContext`.
    pub(crate) fn new_inference_context(
        &mut self,
        type_parameters: &[TypeId],
        signature: Option<crate::types::SignatureId>,
        flags: InferenceFlags,
        compare_types: InferenceComparer,
    ) -> InferenceContextId {
        // Go: `if compareTypes == nil { compareTypes = c.compareTypesAssignable }` —
        // the port's InferenceComparer makes the default explicit at call sites.
        let inferences: Vec<InferenceInfo> = type_parameters.iter().map(|&tp| new_inference_info(tp)).collect();
        self.new_inference_context_worker(inferences, signature, flags, compare_types)
    }

    /// Go: `func (c *Checker) cloneInferenceContext(n, extraFlags) *InferenceContext`.
    pub(crate) fn clone_inference_context(
        &mut self,
        n: Option<InferenceContextId>,
        extra_flags: InferenceFlags,
    ) -> Option<InferenceContextId> {
        let n = n?;
        let (inferences, signature, flags, compare_types) = {
            let ctx = &self.inference_contexts[n.0 as usize];
            (
                ctx.inferences.iter().map(clone_inference_info).collect::<Vec<_>>(),
                ctx.signature,
                ctx.flags,
                ctx.compare_types,
            )
        };
        Some(self.new_inference_context_worker(inferences, signature, flags | extra_flags, compare_types))
    }

    /// Go: `func (c *Checker) cloneInferredPartOfContext(n) *InferenceContext`.
    pub(crate) fn clone_inferred_part_of_context(
        &mut self,
        n: InferenceContextId,
    ) -> Option<InferenceContextId> {
        let (inferences, signature, flags, compare_types) = {
            let ctx = &self.inference_contexts[n.0 as usize];
            let inferences: Vec<InferenceInfo> = ctx
                .inferences
                .iter()
                .filter(|i| has_inference_candidates(i))
                .map(clone_inference_info)
                .collect();
            (inferences, ctx.signature, ctx.flags, ctx.compare_types)
        };
        if inferences.is_empty() {
            return None;
        }
        Some(self.new_inference_context_worker(inferences, signature, flags, compare_types))
    }

    /// Go: `func (c *Checker) newInferenceContextWorker(inferences, signature, flags, compareTypes) *InferenceContext`.
    fn new_inference_context_worker(
        &mut self,
        inferences: Vec<InferenceInfo>,
        signature: Option<crate::types::SignatureId>,
        flags: InferenceFlags,
        compare_types: InferenceComparer,
    ) -> InferenceContextId {
        let id = InferenceContextId(self.inference_contexts.len() as u32);
        self.inference_contexts.push(InferenceContext {
            inferences,
            signature,
            flags,
            compare_types,
            mapper: None,
            non_fixing_mapper: None,
            return_mapper: None,
            outer_return_mapper: None,
            inferred_type_parameters: Vec::new(),
            intra_expression_inference_sites: Vec::new(),
        });
        // Go: `n.mapper = c.newInferenceTypeMapper(n, true /*fixing*/)` etc.
        let mapper = self.new_inference_type_mapper(id, true /*fixing*/);
        let non_fixing_mapper = self.new_inference_type_mapper(id, false /*fixing*/);
        let ctx = &mut self.inference_contexts[id.0 as usize];
        ctx.mapper = Some(mapper);
        ctx.non_fixing_mapper = Some(non_fixing_mapper);
        id
    }

    /// Go: `func (c *Checker) addIntraExpressionInferenceSite(n, node, t)`.
    pub(crate) fn add_intra_expression_inference_site(
        &mut self,
        n: InferenceContextId,
        node: NodeId,
        t: TypeId,
    ) {
        self.inference_contexts[n.0 as usize]
            .intra_expression_inference_sites
            .push(IntraExpressionInferenceSite { node, t });
    }

    /// Go: `func (c *Checker) inferFromIntraExpressionSites(n *InferenceContext)`.
    ///
    /// (Replaces the W1 no-op stub; see the module header for the ownership
    /// note — mapper.rs's InferenceTypeMapper is the other caller.)
    pub(crate) fn infer_from_intra_expression_sites(&mut self, n: InferenceContextId) {
        // We collect intra-expression inference sites within object and array literals to handle cases where
        // inferred types flow between context sensitive element expressions. (See Go's long doc comment.)
        let sites: Vec<IntraExpressionInferenceSite> = std::mem::take(
            &mut self.inference_contexts[n.0 as usize].intra_expression_inference_sites,
        );
        for site in sites {
            // TODO(port M6 W3): getContextualType /
            // getContextualTypeForObjectLiteralMethod — the contextual-typing
            // seam (ContextFlagsNoConstraints); permissive: no contextual
            // type, so no inference for the site.
            let _ = site;
        }
    }

    /// Go: `func (c *Checker) getInferredType(n *InferenceContext, index int) *Type`.
    ///
    /// (Replaces the W1 errorType stub; the real body.)
    pub(crate) fn get_inferred_type(&mut self, n: InferenceContextId, index: usize) -> TypeId {
        let cached = self.inference_contexts[n.0 as usize].inferences[index].inferred_type;
        if let Some(cached) = cached {
            return cached;
        }
        let type_parameter = self.inference_contexts[n.0 as usize].inferences[index].type_parameter;
        if type_parameter == self.error_type() {
            return type_parameter;
        }
        let context_flags = self.inference_contexts[n.0 as usize].flags;
        let context_signature = self.inference_contexts[n.0 as usize].signature;
        let inference_candidates = self.inference_contexts[n.0 as usize].inferences[index].candidates.clone();
        let inference_contra_candidates =
            self.inference_contexts[n.0 as usize].inferences[index].contra_candidates.clone();
        let mut inferred_type: Option<TypeId>;
        let mut fallback_type: Option<TypeId> = None;
        if let Some(signature) = context_signature {
            // Go: the covariant/contravariant candidate resolution + the
            // prefer-covariant policy (the long Go comment covers it).
            let mut inferred_covariant_type: Option<TypeId> = None;
            if !inference_candidates.is_empty() {
                inferred_covariant_type = Some(self.get_covariant_inference(n, index, signature));
            }
            let mut inferred_contravariant_type: Option<TypeId> = None;
            if !inference_contra_candidates.is_empty() {
                inferred_contravariant_type = Some(self.get_contravariant_inference(n, index));
            }
            if inferred_covariant_type.is_some() || inferred_contravariant_type.is_some() {
                // If we have both co- and contra-variant inferences, we prefer the co-variant inference if it is not 'never',
                // all co-variant inferences are assignable to it (i.e. it isn't one of a conflicting set of candidates), it is
                // assignable to some contra-variant inference, and no other type parameter is constrained to this type parameter
                // and has inferences that would conflict. Otherwise, we prefer the contra-variant inference.
                // Similarly ignore co-variant `any` inference when both are available as almost everything is assignable to it
                // and it would spoil the overall inference. (Go comment, verbatim.)
                // PORT: Go's `every`/`some` closures become explicit loops —
                // the Rust closures would need two live `&mut self` captures
                // (isTypeAssignableTo inside every() inside the outer block).
                let prefer_covariant_type = match inferred_covariant_type {
                    Some(covariant) if inferred_contravariant_type.is_none() || {
                        let covariant_flags = self.type_(covariant).flags;
                        !covariant_flags.intersects(TypeFlags::NEVER | TypeFlags::ANY)
                    } =>
                    {
                        // Go: some(inferenceContraCandidates, t => isTypeAssignableTo(covariant, t))
                        let mut contra_matched = false;
                        for &t in &inference_contra_candidates {
                            if self.is_type_assignable_to(covariant, t) {
                                contra_matched = true;
                                break;
                            }
                        }
                        if !contra_matched {
                            false
                        } else {
                            // Go: every(range, other => other == index ||
                            // getConstraintOfTypeParameter(...) != typeParameter ||
                            // every(...candidates, t => isTypeAssignableTo(t, covariant)))
                            let others: Vec<(TypeId, Vec<TypeId>)> = self.inference_contexts[n.0 as usize]
                                .inferences
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| *i != index)
                                .map(|(_, inf)| (inf.type_parameter, inf.candidates.clone()))
                                .collect();
                            let mut all_ok = true;
                            for (other_type_parameter, candidates) in others {
                                if self.get_constraint_of_type_parameter(other_type_parameter)
                                    != Some(type_parameter)
                                {
                                    continue;
                                }
                                for &t in &candidates {
                                    if !self.is_type_assignable_to(t, covariant) {
                                        all_ok = false;
                                        break;
                                    }
                                }
                                if !all_ok {
                                    break;
                                }
                            }
                            all_ok
                        }
                    }
                    _ => false,
                };
                if prefer_covariant_type {
                    inferred_type = inferred_covariant_type;
                    fallback_type = inferred_contravariant_type;
                } else {
                    inferred_type = inferred_contravariant_type;
                    fallback_type = inferred_covariant_type;
                }
            } else if context_flags.intersects(InferenceFlags::NO_DEFAULT) {
                // We use silentNeverType as the wildcard that signals no inferences. (Go comment, verbatim.)
                inferred_type = Some(self.silent_never_type());
            } else {
                // Infer either the default or the empty object type when no inferences were
                // made. It is important to remember that in this case, inference still
                // succeeds, meaning there is no error for not having inference candidates. An
                // inference error only occurs when there are *conflicting* candidates, i.e.
                // candidates with no common supertype. (Go comment, verbatim.)
                inferred_type = None;
                if let Some(default_type) = self.get_default_from_type_parameter(type_parameter) {
                    // Instantiate the default type. Any forward reference to a type
                    // parameter should be instantiated to the empty object type. (Go comment, verbatim.)
                    let backreference = self.new_backreference_mapper(n, index);
                    let non_fixing = self.inference_contexts[n.0 as usize].non_fixing_mapper;
                    let merged = self.merge_type_mappers(Some(backreference), non_fixing);
                    if let Some(mapper) = merged {
                        inferred_type = Some(self.instantiate_type(default_type, mapper));
                    }
                }
            }
        } else {
            inferred_type = self.get_type_from_inference(n, index);
        }
        if inferred_type.is_none() {
            if context_flags.intersects(InferenceFlags::ANY_DEFAULT) {
                inferred_type = Some(self.any_type());
            } else {
                inferred_type = Some(self.unknown_type());
            }
        }
        self.inference_contexts[n.0 as usize].inferences[index].inferred_type = inferred_type;
        let inference_priority = self.inference_contexts[n.0 as usize].inferences[index].priority;
        let constraint = self.get_constraint_of_type_parameter(type_parameter);
        if let Some(constraint) = constraint {
            let non_fixing_mapper = self.inference_contexts[n.0 as usize].non_fixing_mapper;
            let instantiated_constraint = match non_fixing_mapper {
                Some(mapper) => self.instantiate_type(constraint, mapper),
                None => constraint,
            };
            // A pure return type inference is still filtered in a recursive call resolution, whose result can become the type of the enclosing declaration. (Go comment, verbatim.)
            if let Some(current) = inferred_type {
                if !context_flags.intersects(InferenceFlags::NO_CONSTRAINT_CHECKS)
                    || inference_priority == InferencePriority::RETURN_TYPE
                {
                    let constraint_with_this =
                        get_type_with_this_argument(self, instantiated_constraint, Some(current), false);
                    let compare_types = self.inference_contexts[n.0 as usize].compare_types;
                    if compare_types.compare(self, current, constraint_with_this, false) == Ternary::False {
                        let filtered_by_constraint: Option<TypeId> = None;
                        if inference_priority == InferencePriority::RETURN_TYPE {
                            // If we have a pure return type inference, we may succeed by removing constituents of the inferred type
                            // that aren't assignable to the constraint type (pure return type inferences are speculation anyway). (Go comment, verbatim.)
                            // TODO(port M6 S7): c.mapType — permissive: the
                            // whole type fails the constraint check instead of
                            // constituent filtering.
                        }
                        inferred_type = match filtered_by_constraint {
                            Some(filtered)
                                if !self.type_(filtered).flags.intersects(TypeFlags::NEVER) =>
                            {
                                Some(filtered)
                            }
                            _ => None,
                        };
                    }
                }
            }
            if inferred_type.is_none() {
                // If the fallback type satisfies the constraint, we pick it. Otherwise, we pick the constraint. (Go comment, verbatim.)
                inferred_type = match fallback_type {
                    Some(fallback) => {
                        let constraint_with_this =
                            get_type_with_this_argument(self, instantiated_constraint, Some(fallback), false);
                        let compare_types = self.inference_contexts[n.0 as usize].compare_types;
                        if compare_types.compare(self, fallback, constraint_with_this, false) != Ternary::False {
                            Some(fallback)
                        } else {
                            Some(instantiated_constraint)
                        }
                    }
                    None => Some(instantiated_constraint),
                };
            }
            self.inference_contexts[n.0 as usize].inferences[index].inferred_type = inferred_type;
        }
        self.clear_active_mapper_caches();
        inferred_type.expect("set above")
    }

    /// Go: `func (c *Checker) getCovariantInference(inference, signature) *Type`.
    fn get_covariant_inference(
        &mut self,
        n: InferenceContextId,
        index: usize,
        signature: crate::types::SignatureId,
    ) -> TypeId {
        let type_parameter = self.inference_contexts[n.0 as usize].inferences[index].type_parameter;
        let candidates = self.inference_contexts[n.0 as usize].inferences[index].candidates.clone();
        let inference_top_level = self.inference_contexts[n.0 as usize].inferences[index].top_level;
        let inference_is_fixed = self.inference_contexts[n.0 as usize].inferences[index].is_fixed;
        let inference_priority = self.inference_contexts[n.0 as usize].inferences[index].priority;
        // Extract all object and array literal types and replace them with a single widened and normalized type. (Go comment, verbatim.)
        let candidates = self.union_object_and_array_literal_candidates(candidates);
        // We widen inferred literal types if
        // all inferences were made to top-level occurrences of the type parameter, and
        // the type parameter has no constraint or its constraint includes no primitive or literal types, and
        // the type parameter was fixed during inference or does not occur at top-level in the return type. (Go comment, verbatim.)
        let primitive_constraint = self.has_primitive_constraint(type_parameter)
            || is_const_type_variable(self, type_parameter, 0);
        let widen_literal_types = !primitive_constraint
            && inference_top_level
            && (inference_is_fixed
                || !self.is_type_parameter_at_top_level_in_return_type(signature, type_parameter));
        let base_candidates: Vec<TypeId> = if primitive_constraint {
            candidates.iter().map(|&t| self.get_regular_type_of_literal_type(t)).collect()
        } else if widen_literal_types {
            candidates.iter().map(|&t| self.get_widened_literal_type(t)).collect()
        } else {
            candidates
        };
        // If all inferences were made from a position that implies a combined result, infer a union type.
        // Otherwise, infer a common supertype. (Go comment, verbatim.)
        let unwidened_type = if inference_priority.intersects(InferencePriority::PRIORITY_IMPLIES_COMBINATION) {
            get_union_type_ex(self, base_candidates, UnionReduction::Subtype, None, None)
        } else {
            self.get_common_supertype(&base_candidates)
        };
        self.get_widened_type(unwidened_type)
    }

    /// Go: `func (c *Checker) getContravariantInference(inference) *Type`.
    fn get_contravariant_inference(&mut self, n: InferenceContextId, index: usize) -> TypeId {
        let inference_priority = self.inference_contexts[n.0 as usize].inferences[index].priority;
        let contra_candidates =
            self.inference_contexts[n.0 as usize].inferences[index].contra_candidates.clone();
        if inference_priority.intersects(InferencePriority::PRIORITY_IMPLIES_COMBINATION) {
            crate::relater::get_intersection_type(self, contra_candidates)
        } else {
            self.get_common_subtype(&contra_candidates)
        }
    }

    /// Go: `func (c *Checker) unionObjectAndArrayLiteralCandidates(candidates) []*Type`.
    fn union_object_and_array_literal_candidates(&mut self, candidates: Vec<TypeId>) -> Vec<TypeId> {
        if candidates.len() > 1 {
            let object_literals: Vec<TypeId> = candidates
                .iter()
                .copied()
                .filter(|&t| is_object_or_array_literal_type(self.type_(t)))
                .collect();
            if !object_literals.is_empty() {
                let literals_type = get_union_type_ex(self, object_literals.clone(), UnionReduction::Subtype, None, None);
                let non_literal_types: Vec<TypeId> = candidates
                    .iter()
                    .copied()
                    .filter(|&t| !is_object_or_array_literal_type(self.type_(t)))
                    .collect();
                let mut result = non_literal_types;
                result.push(literals_type);
                return result;
            }
        }
        candidates
    }

    /// Go: `func (c *Checker) hasPrimitiveConstraint(t) bool`.
    fn has_primitive_constraint(&mut self, t: TypeId) -> bool {
        let constraint = self.get_constraint_of_type_parameter(t);
        if let Some(mut constraint) = constraint {
            if self.type_(constraint).flags.intersects(TypeFlags::CONDITIONAL) {
                constraint = get_default_constraint_of_conditional_type(self, constraint)
                    .unwrap_or(constraint);
            }
            return maybe_type_of_kind(
                self,
                constraint,
                TypeFlags::PRIMITIVE | TypeFlags::INDEX | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
            );
        }
        false
    }

    /// Go: `func (c *Checker) getCommonSupertype(types) *Type`.
    fn get_common_supertype(&mut self, types: &[TypeId]) -> TypeId {
        if types.len() == 1 {
            return types[0];
        }
        // Remove nullable types from each of the candidates. (Go comment, verbatim.)
        // PORT: Go's `if c.strictNullChecks { ... filterType ... }` branch
        // reduces to the identity here — filter_type's W2 stub consults only
        // the top-level type, so the nullable filtering lands with S7's
        // reducer (primary_types == types either way).
        let primary_types: Vec<TypeId> = types.to_vec();
        // When the candidate types are all literal types with the same base type, return a union
        // of those literal types. Otherwise, return the leftmost type for which no type to the
        // right is a supertype. (Go comment, verbatim.)
        let supertype = if self.literal_types_with_same_base_type(&primary_types) {
            get_union_type(self, primary_types.clone())
        } else {
            self.get_single_common_supertype(&primary_types)
        };
        // Add any nullable types that occurred in the candidates back to the result. (Go comment, verbatim.)
        // PORT: with filter_type's permissive identity, primary_types == types,
        // so the getNullableType re-addition is never reached (S7 owns it).
        supertype
    }

    /// Go: `func (c *Checker) getSingleCommonSupertype(types) *Type`.
    ///
    /// First, find the leftmost type for which no type to the right is a strict supertype, and if that
    /// type is a strict supertype of all other candidates, return it. Otherwise, return the leftmost type
    /// for which no type to the right is a (regular) supertype. (Go comment, verbatim.)
    fn get_single_common_supertype(&mut self, types: &[TypeId]) -> TypeId {
        let candidate = self.find_leftmost_type(types, Checker::is_type_strict_subtype_of);
        let candidate_ok = types.iter().all(|&t| {
            t == candidate || self.is_type_strict_subtype_of(t, candidate)
        });
        if candidate_ok {
            return candidate;
        }
        self.find_leftmost_type(types, Checker::is_type_subtype_of)
    }

    /// Go: `func (c *Checker) findLeftmostType(types, f) *Type`.
    fn find_leftmost_type(&mut self, types: &[TypeId], f: fn(&mut Checker, TypeId, TypeId) -> bool) -> TypeId {
        let mut candidate: Option<TypeId> = None;
        for &t in types {
            if candidate.is_none() || f(self, candidate.expect("set in this loop"), t) {
                candidate = Some(t);
            }
        }
        candidate.expect("non-empty types")
    }

    /// Go: `func (c *Checker) getCommonSubtype(types) *Type` — the leftmost
    /// type for which no type to the right is a subtype.
    fn get_common_subtype(&mut self, types: &[TypeId]) -> TypeId {
        let mut subtype: Option<TypeId> = None;
        for &t in types {
            if subtype.is_none() || self.is_type_subtype_of(t, subtype.expect("set in this loop")) {
                subtype = Some(t);
            }
        }
        subtype.expect("non-empty types")
    }

    /// Go: `func (c *Checker) getCombinedTypeFlags(types) TypeFlags`.
    fn get_combined_type_flags(&mut self, types: &[TypeId]) -> TypeFlags {
        let mut flags = TypeFlags::NONE;
        for &t in types {
            if self.type_(t).flags.intersects(TypeFlags::UNION) {
                let constituents = self.type_(t).distributed();
                flags |= self.get_combined_type_flags(&constituents);
            } else {
                flags |= self.type_(t).flags;
            }
        }
        flags
    }

    /// Go: `func (c *Checker) literalTypesWithSameBaseType(types) bool`.
    fn literal_types_with_same_base_type(&mut self, types: &[TypeId]) -> bool {
        let mut common_base_type: Option<TypeId> = None;
        for &t in types {
            if !self.type_(t).flags.intersects(TypeFlags::NEVER) {
                let base_type = self.get_base_type_of_literal_type(t);
                if common_base_type.is_none() {
                    common_base_type = Some(base_type);
                }
                if base_type == t || Some(base_type) != common_base_type {
                    return false;
                }
            }
        }
        true
    }

    /// Go: `func (c *Checker) getInferredTypes(n) []*Type`.
    pub(crate) fn get_inferred_types(&mut self, n: InferenceContextId) -> Vec<TypeId> {
        let count = self.inference_contexts[n.0 as usize].inferences.len();
        (0..count).map(|i| self.get_inferred_type(n, i)).collect()
    }

    /// Go: `func (c *Checker) getMapperFromContext(n) *TypeMapper`.
    pub(crate) fn get_mapper_from_context(&self, n: Option<InferenceContextId>) -> Option<TypeMapperId> {
        n.and_then(|n| self.inference_contexts[n.0 as usize].mapper)
    }

    /// Go: `func (c *Checker) createOuterReturnMapper(context) *TypeMapper`.
    pub(crate) fn create_outer_return_mapper(&mut self, context: InferenceContextId) -> Option<TypeMapperId> {
        if self.inference_contexts[context.0 as usize].outer_return_mapper.is_none() {
            let clone = self.clone_inference_context(Some(context), InferenceFlags::NONE);
            let mapper = clone.and_then(|c| self.inference_contexts[c.0 as usize].mapper);
            let return_mapper = self.inference_contexts[context.0 as usize].return_mapper;
            let merged = self.merge_type_mappers(return_mapper, mapper);
            self.inference_contexts[context.0 as usize].outer_return_mapper = merged;
        }
        self.inference_contexts[context.0 as usize].outer_return_mapper
    }

    /// Go: `func (c *Checker) getTypeFromInference(inference) *Type` — the
    /// standalone (signature-less) resolution.
    fn get_type_from_inference(&mut self, n: InferenceContextId, index: usize) -> Option<TypeId> {
        let candidates = self.inference_contexts[n.0 as usize].inferences[index].candidates.clone();
        let contra_candidates =
            self.inference_contexts[n.0 as usize].inferences[index].contra_candidates.clone();
        if !candidates.is_empty() {
            Some(get_union_type_ex(
                self,
                candidates,
                UnionReduction::Subtype,
                None,
                None,
            ))
        } else if !contra_candidates.is_empty() {
            Some(crate::relater::get_intersection_type(self, contra_candidates))
        } else {
            None
        }
    }
}

/// Go: the context's `mapper` field (the relater's conditional arm reads it
/// after inferTypes — `ctx.mapper`).
pub(crate) fn inference_context_mapper(c: &Checker, ctx: InferenceContextId) -> Option<TypeMapperId> {
    c.inference_contexts[ctx.0 as usize].mapper
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers (Go's package-level functions)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func compareTypesAndDepth(t1, t2) int` — compare two types first by
/// depth and then by the regular type ordering.
///
/// PORT: Go's `CompareTypes` is the core package's TypeId ordering; the port
/// compares the arena ids (the same numeric identity).
fn compare_types_and_depth(c: &Checker, t1: TypeId, t2: TypeId) -> Ordering {
    let d1 = get_type_depth(c, t1, 3);
    let d2 = get_type_depth(c, t2, 3);
    if d1 != d2 {
        d2.cmp(&d1) // Largest depth sorts first (Go comment, verbatim.)
    } else {
        t1.0.cmp(&t2.0)
    }
}

/// Go: `func getTypeDepth(t, maxDepth) int` — the depth of the given type up
/// to the given maximum depth. For generic aliased types and type references,
/// the depth is one plus the largest type argument depth. For union and
/// intersection types, the depth is the largest constituent type depth. For
/// all other types, the depth is zero. The maximum depth limits infinite
/// recursion of circular types. (Go comment, verbatim.)
fn get_type_depth(c: &Checker, t: TypeId, max_depth: i32) -> i32 {
    if max_depth != 0 {
        let alias_type_arguments_len = c
            .type_(t)
            .alias
            .as_ref()
            .map(|a| a.type_arguments.len())
            .unwrap_or(0);
        if alias_type_arguments_len != 0 {
            let args = c.type_(t).alias.as_ref().expect("alias").type_arguments.clone();
            return get_type_list_depth(c, &args, max_depth - 1) + 1;
        }
        if c.type_(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            let type_arguments = get_type_arguments(c, t);
            if !type_arguments.is_empty() {
                return get_type_list_depth(c, &type_arguments, max_depth - 1) + 1;
            }
        }
        if c.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let types = c.type_(t).distributed();
            return get_type_list_depth(c, &types, max_depth);
        }
    }
    0
}

/// Go: `func getTypeListDepth(types, maxDepth) int`.
fn get_type_list_depth(c: &Checker, types: &[TypeId], max_depth: i32) -> i32 {
    let mut depth = 0;
    for &t in types {
        depth = depth.max(get_type_depth(c, t, max_depth));
    }
    depth
}

/// Go: `func tupleTypesDefinitelyUnrelated(source, target) bool`.
fn tuple_types_definitely_unrelated(c: &Checker, source: TypeId, target: TypeId) -> bool {
    let t_combined = tuple_type_combined_flags(c, target);
    let s_combined = tuple_type_combined_flags(c, source);
    (!t_combined.intersects(ElementFlags::VARIADIC)
        && tuple_type_min_length(c, target) > tuple_type_min_length(c, source))
        || (!t_combined.intersects(ElementFlags::VARIABLE)
            && (s_combined.intersects(ElementFlags::VARIABLE)
                || tuple_type_fixed_length(c, target) < tuple_type_fixed_length(c, source)))
}

/// Go: `func newInferenceInfo(typeParameter) *InferenceInfo`.
fn new_inference_info(type_parameter: TypeId) -> InferenceInfo {
    InferenceInfo {
        type_parameter,
        priority: InferencePriority::MAX_VALUE,
        top_level: true,
        implied_arity: -1,
        ..Default::default()
    }
}

/// Go: `func cloneInferenceInfo(info) *InferenceInfo`.
fn clone_inference_info(info: &InferenceInfo) -> InferenceInfo {
    InferenceInfo {
        type_parameter: info.type_parameter,
        candidates: info.candidates.clone(),
        contra_candidates: info.contra_candidates.clone(),
        inferred_type: info.inferred_type,
        priority: info.priority,
        top_level: info.top_level,
        is_fixed: info.is_fixed,
        implied_arity: info.implied_arity,
    }
}

/// Go: `func hasInferenceCandidates(info) bool`.
fn has_inference_candidates(info: &InferenceInfo) -> bool {
    !info.candidates.is_empty() || !info.contra_candidates.is_empty()
}

/// Go: `func hasInferenceCandidatesOrDefault(info) bool`.
#[allow(dead_code)]
pub(crate) fn has_inference_candidates_or_default(info: &InferenceInfo) -> bool {
    has_inference_candidates(info) || has_type_parameter_default(info.type_parameter)
}

/// Go: `func hasTypeParameterDefault(tp) bool`.
#[allow(dead_code)]
fn has_type_parameter_default(tp: TypeId) -> bool {
    // TODO(port M6 S2): the TypeParameterDeclaration default-type resolution —
    // permissive false (the S2 declaration surface).
    let _ = tp;
    false
}

/// Go: `func hasOverlappingInferences(a, b) bool`.
#[allow(dead_code)]
fn has_overlapping_inferences(a: &[InferenceInfo], b: &[InferenceInfo]) -> bool {
    for i in 0..a.len() {
        if has_inference_candidates(&a[i]) && has_inference_candidates(&b[i]) {
            return true;
        }
    }
    false
}

/// Go: `func (c *Checker) mergeInferences(target, source)`.
#[allow(dead_code)]
fn merge_inferences(target: &mut [InferenceInfo], source: &[InferenceInfo]) {
    for i in 0..target.len() {
        if !has_inference_candidates(&target[i]) && has_inference_candidates(&source[i]) {
            target[i] = clone_inference_info(&source[i]);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Permissive seams for not-yet-ported callees (S-wave banners)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (c *Checker) couldContainTypeVariables(t) bool` (checker.go —
/// the memoized worker). The real worker's cache bits
/// (ObjectFlagsCouldContainTypeVariablesComputed) and the
/// isNonGenericTopLevelType alias-declaration walk land with S4/S7; the
/// structural test below is the worker's body with the memo elided.
fn could_contain_type_variables_helper(c: &mut Checker, t: TypeId) -> bool {
    if !c.type_(t).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE) {
        return false;
    }
    let object_flags = c.type_(t).object_flags;
    if object_flags.intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED) {
        return object_flags.intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES);
    }
    let type_arguments = get_type_arguments(c, t);
    let mut result = c.type_(t).flags.intersects(TypeFlags::INSTANTIABLE);
    if !result && c.type_(t).flags.intersects(TypeFlags::OBJECT) && !is_non_generic_top_level_type(c, t) {
        result = object_flags.intersects(ObjectFlags::REFERENCE)
            && type_arguments.iter().any(|&ta| could_contain_type_variables_helper(c, ta))
            || object_flags.intersects(
                ObjectFlags::MAPPED
                    | ObjectFlags::REVERSE_MAPPED
                    | ObjectFlags::OBJECT_REST_TYPE
                    | ObjectFlags::INSTANTIATION_EXPRESSION_TYPE,
            );
    }
    if !result
        && c.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
        && !c.type_(t).flags.intersects(TypeFlags::ENUM_LITERAL)
        && !is_non_generic_top_level_type(c, t)
    {
        let constituents = c.type_(t).distributed();
        result = constituents.iter().any(|&t| could_contain_type_variables_helper(c, t));
    }
    result
}

/// Go: `func (c *Checker) isNonGenericTopLevelType(t) bool` — the alias
/// declaration's ancestor walk (SourceFile → non-generic; Module → generic).
fn is_non_generic_top_level_type(c: &mut Checker, t: TypeId) -> bool {
    // TODO(port M6 S4): GetDeclarationOfKind + FindAncestorOrQuit — the
    // declaration-ancestor walk; permissive false (treated as potentially
    // generic, so inference stays conservative/active).
    let _ = c;
    if let Some(alias) = &c.type_(t).alias {
        return alias.type_arguments.is_empty();
    }
    false
}

/// Go: `func (c *Checker) isNoInferType(t) bool`.
pub(crate) fn is_no_infer_type(c: &Checker, t: TypeId) -> bool {
    // A NoInfer<T> type is represented as a substitution type with a
    // TypeFlags.Unknown constraint. (Go comment, verbatim.)
    c.type_(t).flags.intersects(TypeFlags::SUBSTITUTION)
        && c
            .type_(t)
            .as_substitution_type()
            .and_then(|d| d.constraint)
            .is_some_and(|constraint| c.type_(constraint).flags.intersects(TypeFlags::UNKNOWN))
}

// Go: `func (c *Checker) getNoInferType(t) *Type` + `isNoInferTargetType`
// (checker.go:27928/27932) — uniontypes.rs owns the REAL bodies (same impl
// block); the NoInfer<T> intrinsic's call sites reach them as Checker methods.

/// Go: `func (c *Checker) getActualTypeVariable(t) *Type` (checker.go:32116)
/// — REAL (M6 S10): the substitution unwrap + indexed-access recombination +
/// getNonDistributedTypeParameter.
pub(crate) fn get_actual_type_variable(c: &mut Checker, t: TypeId) -> TypeId {
    let flags = c.type_(t).flags;
    if flags.intersects(TypeFlags::SUBSTITUTION) {
        let base = substitution_base_type(c, t);
        return get_actual_type_variable(c, base);
    }
    if flags.intersects(TypeFlags::INDEXED_ACCESS) {
        let (object_type, index_type) = {
            let d = c.type_(t).as_indexed_access_type().expect("IndexedAccessType data");
            (
                d.object_type.expect("objectType"),
                d.index_type.expect("indexType"),
            )
        };
        let new_object_type = get_actual_type_variable(c, object_type);
        let new_index_type = get_actual_type_variable(c, index_type);
        if new_object_type != object_type || new_index_type != index_type {
            return c.get_indexed_access_type_impl(new_object_type, new_index_type);
        }
    }
    c.get_non_distributed_type_parameter(t)
}

/// Go: `func (c *Checker) getSimplifiedType(t, writing) *Type` — REAL (M6 S10):
/// delegates to index.rs's body.
pub(crate) fn get_simplified_type(c: &mut Checker, t: TypeId, writing: bool) -> TypeId {
    c.get_simplified_type_impl(t, writing)
}

/// Go: `func (c *Checker) getSubstitutionIntersection(t) *Type` — pub(crate)
/// (M6 S10: typeofsymbol.rs's computeBaseConstraint substitution arm calls it).
pub(crate) fn get_substitution_intersection(c: &mut Checker, t: TypeId) -> TypeId {
    if is_no_infer_type(c, t) {
        return substitution_base_type(c, t);
    }
    let constraint = c
        .type_(t)
        .as_substitution_type()
        .expect("substitution")
        .constraint
        .expect("constraint");
    let base = substitution_base_type(c, t);
    crate::relater::get_intersection_type(c, vec![constraint, base])
}

/// Go: `func (c *Checker) isConstTypeVariable(t, depth) bool`.
pub(crate) fn is_const_type_variable(c: &mut Checker, t: TypeId, depth: usize) -> bool {
    // TODO(port M6 S2/S7): the const-modifier/homomorphic-variable walk;
    // permissive false.
    let _ = (c, t, depth);
    false
}

/// Go: `someType(t, c.isMutableArrayLikeType)`.
fn some_type_mutable_array_like(c: &mut Checker, t: TypeId) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        let constituents = c.type_(t).distributed();
        return constituents.iter().any(|&x| is_mutable_array_like_type(c, x));
    }
    is_mutable_array_like_type(c, t)
}

/// Go: `func (c *Checker) isMutableArrayLikeType(t) bool` — a type is
/// mutable-array-like if it is a reference to the global Array type, or if
/// it is not the any, undefined, null or never type and if it is assignable
/// to Array<any>.
/// Go: `func (c *Checker) isMutableArrayLikeType(t) bool` (checker.go:24060)
/// — pub(crate) since contextual.rs's getSpreadArgumentType port consumes it.
pub(crate) fn is_mutable_array_like_type(c: &mut Checker, t: TypeId) -> bool {
    is_mutable_array_or_tuple(c, t)
        || !c.type_(t).flags.intersects(TypeFlags::ANY | TypeFlags::NULLABLE | TypeFlags::NEVER)
            && c.is_type_assignable_to(t, c.any_array_type)
}

/// Go: `func (c *Checker) getApplicableIndexInfo(t, keyType) *IndexInfo` —
/// S8's REAL body (members.rs find_applicable_index_info) replaced the
/// S10-banner seam.
fn get_applicable_index_info(c: &mut Checker, t: TypeId, key_type: TypeId) -> Option<crate::types::IndexInfoId> {
    c.get_applicable_index_info_impl(t, key_type)
}

/// Go: `func (c *Checker) isFromInferenceBlockedSource(t) bool`.
fn is_from_inference_blocked_source(c: &Checker, t: TypeId) -> bool {
    // Go: t.symbol != nil && Some(t.symbol.Declarations, isSkipDirectInferenceNode)
    // — the skipDirectInferenceNodes collection is the W3/flow seam.
    // TODO(port M6 W3): the skip-direct-inference node set; permissive false.
    let _ = (c, t);
    false
}

/// Go: `func (c *Checker) isSkipDirectInferenceNode(node) bool` — the symbol
/// declarations' filter form (inferFromProperties).
fn is_skip_direct_inference_node_for_symbol(_c: &Checker, _symbol: tsc_ast::SymbolId) -> bool {
    // TODO(port M6 W3): the skip-direct-inference node set; permissive false.
    false
}

impl Checker {
    /// Go: `func (c *Checker) isSkipDirectInferenceNode(node *ast.Node) bool`
    /// (inference.go:1634) — `c.skipDirectInferenceNodes.Has(node)`. The set
    /// is the JSDoc satisfies-attributes registration (the field lives on
    /// the Checker, activated by S1's spine port).
    pub(crate) fn is_skip_direct_inference_node(&self, node: tsc_ast::NodeId) -> bool {
        self.skip_direct_inference_nodes.has(&node)
    }

    /// Go: `func (c *Checker) isFromInferenceBlockedSource(t *Type) bool`
    /// (inference.go:1630) — a type whose declaring symbol has a
    /// skip-direct-inference declaration.
    pub(crate) fn is_from_inference_blocked_source(&self, t: TypeId) -> bool {
        if let Some(symbol) = self.type_(t).symbol {
            self.program
                .symbol(symbol)
                .declarations
                .iter()
                .any(|&decl| self.is_skip_direct_inference_node(decl))
        } else {
            false
        }
    }
}

// (M6 S5): `getElementTypes` and `getElementTypeOfSliceOfTupleType` MOVED —
// the REAL bodies live in typenodes.rs (checker.go:23990/25347); the
// inference engine's call sites resolve through the Checker methods.

/// Go: `func (c *Checker) isObjectTypeWithInferableIndex(t) bool` — the
/// relater.rs free fn (re-exported for the engine's call shape).
fn is_object_type_with_inferable_index_helper(c: &mut Checker, t: TypeId) -> bool {
    // The relater.rs port owns the real body (the intersection/object-literal
    // ladder); reach it through the crate surface.
    crate::relater::is_object_type_with_inferable_index_pub(c, t)
}

/// Go: `func isValidBigIntString(s, roundTripOnly) bool`.
fn is_valid_bigint_string(_s: &str, _round_trip_only: bool) -> bool {
    // TODO(port M6 S5): the bigint literal validation; permissive false.
    false
}

/// Go: `c.parseBigIntLiteralType(str)`.
fn parse_bigint_literal_type(c: &mut Checker, _str: &str) -> TypeId {
    // TODO(port M6 S5): the bigint literal factory; permissive bigintType.
    c.bigint_type()
}

/// Go: `getNumberLiteralValue(right)` compared against jsnum values.
fn get_number_literal_value(c: &Checker, t: TypeId) -> tsc_jsnum::Number {
    c.type_(t)
        .as_literal_type()
        .and_then(|d| d.value.clone())
        .map(|v| match v {
            crate::types::LiteralValue::Number(n) => n,
            _ => tsc_jsnum::Number::from(0.0),
        })
        .unwrap_or_else(|| tsc_jsnum::Number::from(0.0))
}

/// Go: `pseudoBigIntToString(getBigIntLiteralValue(right))`.
fn get_bigint_literal_string(c: &Checker, t: TypeId) -> String {
    // TODO(port M6 S5): pseudoBigIntToString; permissive empty.
    let _ = c.type_(t).as_literal_type();
    String::new()
}

/// Go: `getBooleanLiteralValue(right)`.
pub(crate) fn get_boolean_literal_value(c: &Checker, t: TypeId) -> bool {
    c.type_(t)
        .as_literal_type()
        .and_then(|d| d.value.clone())
        .map(|v| match v {
            crate::types::LiteralValue::Boolean(b) => b,
            _ => false,
        })
        .unwrap_or(false)
}

/// Go: `c.isTypeMatchedByTemplateLiteralType(source, right, comparer)`.
fn is_type_matched_by_template_literal_type(_c: &mut Checker, _source: TypeId, _right: TypeId) -> bool {
    // TODO(port M6 S7): the template-literal matching; permissive false.
    false
}

/// Go: `IsTypeAny(constraint)`.
fn constraint_is_any(c: &Checker, constraint: TypeId) -> bool {
    c.type_(constraint).flags.intersects(TypeFlags::ANY)
}

/// Go: `t.AsIndexedAccessType().objectType` / `.indexType` (the payload reads).
fn indexed_access_object_type(c: &Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_indexed_access_type()
        .expect("indexed access")
        .object_type
        .expect("object type")
}

/// Go: `t.AsIndexedAccessType().indexType`.
fn indexed_access_index_type(c: &Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_indexed_access_type()
        .expect("indexed access")
        .index_type
        .expect("index type")
}

/// Go: `t.AsSubstitutionType().baseType`.
fn substitution_base_type(c: &Checker, t: TypeId) -> TypeId {
    c.type_(t)
        .as_substitution_type()
        .expect("substitution")
        .base_type
        .expect("base type")
}

/// Go: `t.AsTypeReference().node`.
fn type_reference_node(c: &Checker, t: TypeId) -> Option<NodeId> {
    c.type_(t).as_type_reference().and_then(|d| d.node)
}

/// Go: `target.TargetTupleType().elementInfos` (the full-info reader).
fn tuple_element_infos(c: &Checker, t: TypeId) -> Vec<crate::types::TupleElementInfo> {
    c.type_(t)
        .target()
        .and_then(|target| c.type_(target).as_tuple_type().map(|d| d.element_infos.clone()))
        .unwrap_or_default()
}

/// Go: `target.TargetTupleType().fixedLength`.
fn tuple_type_fixed_length(c: &Checker, t: TypeId) -> i32 {
    c.type_(t)
        .target()
        .and_then(|target| c.type_(target).as_tuple_type().map(|d| d.fixed_length))
        .unwrap_or(0)
}

impl Checker {
    /// Go: the checker's `couldContainTypeVariables` field (the bound worker).
    pub(crate) fn could_contain_type_variables(&mut self, t: TypeId) -> bool {
        could_contain_type_variables_helper(self, t)
    }

    /// Go: `func (c *Checker) isNoInferType(t) bool`.
    pub(crate) fn is_no_infer_type(&self, t: TypeId) -> bool {
        is_no_infer_type(self, t)
    }

    /// Go: `func (c *Checker) getActualTypeVariable(t) *Type`.
    pub(crate) fn get_actual_type_variable(&mut self, t: TypeId) -> TypeId {
        get_actual_type_variable(self, t)
    }

    /// Go: `func (c *Checker) getSimplifiedType(t, writing) *Type`.
    fn get_simplified_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        get_simplified_type(self, t, writing)
    }

    /// Go: `func (c *Checker) getSubstitutionIntersection(t) *Type`.
    fn get_substitution_intersection(&mut self, t: TypeId) -> TypeId {
        get_substitution_intersection(self, t)
    }

    /// Go: `func (c *Checker) isConstTypeVariable(t, depth) bool`.
    fn is_const_type_variable(&mut self, t: TypeId, depth: usize) -> bool {
        is_const_type_variable(self, t, depth)
    }

    // Go: `func (c *Checker) getBaseSignature(signature) *Signature` — S8's
    // REAL body (members.rs) replaced the S7 twin; resolves there.
    // Go: `func (c *Checker) getApplicableIndexInfo(t, keyType) *IndexInfo`
    // — S8's REAL body (members.rs) replaced the S10-banner seam; resolves
    // through the free-fn delegate above.

    /// Go: `func (c *Checker) isSkipDirectInferenceNode(node) bool` — the
    /// symbol-declaration filter form (inferFromProperties).
    fn is_skip_direct_inference_node_for_symbol(&self, symbol: tsc_ast::SymbolId) -> bool {
        is_skip_direct_inference_node_for_symbol(self, symbol)
    }

    /// Go: `c.parseBigIntLiteralType(str)`.
    fn parse_bigint_literal_type(&mut self, s: &str) -> TypeId {
        parse_bigint_literal_type(self, s)
    }

    /// Go: `c.isTypeMatchedByTemplateLiteralType(source, right, comparer)`.
    fn is_type_matched_by_template_literal_type(&mut self, source: TypeId, right: TypeId) -> bool {
        is_type_matched_by_template_literal_type(self, source, right)
    }

    /// Go: `func (c *Checker) isNonGenericObjectType(t) bool`.
    pub(crate) fn is_non_generic_object_type(&mut self, t: TypeId) -> bool {
        self.type_(t).flags.intersects(TypeFlags::OBJECT) && !is_generic_mapped_type(self, t)
    }

    /// Go: the `getElementTypeOfSliceOfTupleType` method call sites.
    fn get_element_type_of_slice_of_tuple_type(
        &mut self,
        t: TypeId,
        index: usize,
        end_skip_count: usize,
        writing: bool,
        no_reductions: bool,
    ) -> Option<TypeId> {
        crate::typenodes::get_element_type_of_slice_of_tuple_type(
            self, t, index, end_skip_count, writing, no_reductions,
        )
    }

    /// Go: `func (c *Checker) isObjectTypeWithInferableIndex(t) bool`.
    fn is_object_type_with_inferable_index(&mut self, t: TypeId) -> bool {
        is_object_type_with_inferable_index_helper(self, t)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests (Go's checker tests reach inference only through whole-program checks;
// these pin the engine's data-plane contracts the Go ladder relies on)
// ────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapper::tests_support::new_empty_checker;

    #[test]
    fn inference_priority_ladder_matches_go() {
        // checker.go's const ladder — CIRCULARITY is the signed -1 sentinel
        // that sorts below every other value (the engine min()s over it).
        assert_eq!(InferencePriority::CIRCULARITY.0, -1);
        assert_eq!(InferencePriority::NONE.0, 0);
        assert_eq!(InferencePriority::NAKED_TYPE_VARIABLE.0, 1 << 0);
        assert_eq!(InferencePriority::SPECULATIVE_TUPLE.0, 1 << 1);
        assert_eq!(InferencePriority::SUBSTITUTE_SOURCE.0, 1 << 2);
        assert_eq!(InferencePriority::HOMOMORPHIC_MAPPED_TYPE.0, 1 << 3);
        assert_eq!(InferencePriority::PARTIAL_HOMOMORPHIC_MAPPED_TYPE.0, 1 << 4);
        assert_eq!(InferencePriority::MAPPED_TYPE_CONSTRAINT.0, 1 << 5);
        assert_eq!(InferencePriority::CONTRAVARIANT_CONDITIONAL.0, 1 << 6);
        assert_eq!(InferencePriority::RETURN_TYPE.0, 1 << 7);
        assert_eq!(InferencePriority::LITERAL_KEYOF.0, 1 << 8);
        assert_eq!(InferencePriority::NO_CONSTRAINTS.0, 1 << 9);
        assert_eq!(InferencePriority::ALWAYS_STRICT.0, 1 << 10);
        assert_eq!(InferencePriority::MAX_VALUE.0, 1 << 11);
        // The relater's conditional arm combines exactly these two (the
        // provisional W2 stub had the WRONG bits — 1<<6/1<<9).
        assert_eq!(
            (InferencePriority::NO_CONSTRAINTS | InferencePriority::ALWAYS_STRICT).0,
            (1 << 9) | (1 << 10)
        );
        // PRIORITY_IMPLIES_COMBINATION — RETURN_TYPE|MAPPED_TYPE_CONSTRAINT|LITERAL_KEYOF.
        assert_eq!(
            InferencePriority::PRIORITY_IMPLIES_COMBINATION.0,
            (1 << 7) | (1 << 5) | (1 << 8)
        );
        // The min() ordering that seeds inferencePriority and drives circularity.
        assert!(InferencePriority::CIRCULARITY < InferencePriority::NONE);
        assert_eq!(
            InferencePriority::CIRCULARITY.min(InferencePriority::NAKED_TYPE_VARIABLE),
            InferencePriority::CIRCULARITY
        );
    }

    #[test]
    fn inference_flags_match_go() {
        assert_eq!(InferenceFlags::NONE.bits(), 0);
        assert_eq!(InferenceFlags::NO_DEFAULT.bits(), 1 << 0);
        assert_eq!(InferenceFlags::ANY_DEFAULT.bits(), 1 << 1);
        assert_eq!(InferenceFlags::SKIPPED_GENERIC_FUNCTION.bits(), 1 << 2);
        assert_eq!(InferenceFlags::NO_CONSTRAINT_CHECKS.bits(), 1 << 3);
    }

    #[test]
    fn new_inference_context_seeds_go_shaped_inference_infos() {
        let mut c = new_empty_checker();
        let (string, number) = (c.string_type(), c.number_type());
        let ctx = c.new_inference_context(
            &[string, number],
            None,
            InferenceFlags::NONE,
            InferenceComparer::Assignable,
        );
        let inferences = &c.inference_contexts[ctx.0 as usize].inferences;
        assert_eq!(inferences.len(), 2);
        for (i, tp) in [string, number].into_iter().enumerate() {
            assert_eq!(inferences[i].type_parameter, tp);
            // Go newInferenceInfo: priority MaxValue, topLevel true, impliedArity -1,
            // no candidates and no cached inferredType.
            assert_eq!(inferences[i].priority, InferencePriority::MAX_VALUE);
            assert!(inferences[i].top_level);
            assert_eq!(inferences[i].implied_arity, -1);
            assert!(inferences[i].candidates.is_empty());
            assert!(inferences[i].contra_candidates.is_empty());
            assert_eq!(inferences[i].inferred_type, None);
            assert!(!inferences[i].is_fixed);
        }
        assert_eq!(c.inference_contexts[ctx.0 as usize].flags, InferenceFlags::NONE);
    }

    #[test]
    fn infer_types_records_covariant_candidate_and_gets_it_back() {
        let mut c = new_empty_checker();
        let tp = c.alloc_test_type_parameter(false);
        let string = c.string_type();
        let ctx = c.new_inference_context(&[tp], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        c.infer_types(ctx, string, tp, InferencePriority::NAKED_TYPE_VARIABLE, false /*contravariant*/);
        let inference = &c.inference_contexts[ctx.0 as usize].inferences[0];
        assert_eq!(inference.candidates, vec![string]);
        assert!(inference.contra_candidates.is_empty());
        // A single naked-type-variable candidate resolves to itself through
        // getInferredType's covariant path (no constraint to check).
        assert_eq!(c.get_inferred_type(ctx, 0), string);
    }

    #[test]
    fn infer_types_contravariant_records_contra_candidates() {
        let mut c = new_empty_checker();
        let tp = c.alloc_test_type_parameter(false);
        let string = c.string_type();
        let ctx = c.new_inference_context(&[tp], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        c.infer_types(ctx, string, tp, InferencePriority::NAKED_TYPE_VARIABLE, true /*contravariant*/);
        let inference = &c.inference_contexts[ctx.0 as usize].inferences[0];
        assert_eq!(inference.contra_candidates, vec![string]);
        assert!(inference.candidates.is_empty());
    }

    #[test]
    fn infer_types_round_trips_states_through_the_pool() {
        let mut c = new_empty_checker();
        let tp = c.alloc_test_type_parameter(false);
        let string = c.string_type();
        let ctx = c.new_inference_context(&[tp], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        c.infer_types(ctx, string, tp, InferencePriority::NAKED_TYPE_VARIABLE, false);
        c.infer_types(ctx, string, tp, InferencePriority::NAKED_TYPE_VARIABLE, false);
        // Two inferTypes calls: the second pops the first's pooled state —
        // the free stack never grows past one entry.
        assert_eq!(c.free_inference_states.len(), 1);
        // And the second run did not duplicate the candidate (clear_for_pool
        // cleared the visited map, but the context's candidate set is dedup'd).
        let inference = &c.inference_contexts[ctx.0 as usize].inferences[0];
        assert_eq!(inference.candidates.len(), 1);
    }

    #[test]
    fn put_inference_state_honors_the_pool_reuse_contract() {
        let mut c = new_empty_checker();
        let (s, t) = (c.string_type(), c.number_type());
        assert!(c.free_inference_states.is_empty());
        let mut n = c.get_inference_state();
        n.visited.insert(
            InferenceKey { source: s, target: t, priority: InferencePriority::NONE, contravariant: false, bivariant: false },
            InferencePriority::NONE,
        );
        n.source_stack.push(s);
        n.target_stack.push(t);
        n.bivariant = true;
        n.propagation_type = Some(t);
        c.put_inference_state(n);
        assert_eq!(c.free_inference_states.len(), 1);
        // clear_for_pool: visited/stacks/expanding/propagation/bivariant reset.
        let n2 = c.get_inference_state();
        assert!(c.free_inference_states.is_empty());
        assert!(n2.visited.is_empty());
        assert!(n2.source_stack.is_empty());
        assert!(n2.target_stack.is_empty());
        assert!(!n2.bivariant);
        assert_eq!(n2.propagation_type, None);
    }

    #[test]
    fn invoke_once_is_guarded_by_the_visited_map() {
        let mut c = new_empty_checker();
        let (s, t) = (c.string_type(), c.number_type());
        let mut n = c.get_inference_state();
        // Non-object types: inferFromObjectTypes records only the visit.
        c.invoke_once(&mut n, s, t, Checker::infer_from_object_types);
        assert_eq!(n.visited.len(), 1);
        // The second invoke for the same key short-circuits (the action
        // would re-insert nothing — Go's `if priority, ok := n.visited[key]`).
        c.invoke_once(&mut n, s, t, Checker::infer_from_object_types);
        assert_eq!(n.visited.len(), 1);
        // A different key visits again.
        c.invoke_once(&mut n, t, s, Checker::infer_from_object_types);
        assert_eq!(n.visited.len(), 2);
        c.put_inference_state(n);
    }

    #[test]
    fn get_inferred_type_defaults_follow_the_context_flags() {
        let mut c = new_empty_checker();
        let tp = c.alloc_test_type_parameter(false);
        // No candidates: InferenceFlagsNone → unknownType (the anyDefault /
        // noDefault ladder's fall-through).
        let ctx = c.new_inference_context(&[tp], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        assert_eq!(c.get_inferred_type(ctx, 0), c.unknown_type());
        // NoDefault without a signature: Go's silentNeverType branch lives
        // inside getInferredType's `if n.signature != nil` ladder, so the
        // signature-less fall-through is the unknownType default here.
        let ctx = c.new_inference_context(&[tp], None, InferenceFlags::NO_DEFAULT, InferenceComparer::Assignable);
        assert_eq!(c.get_inferred_type(ctx, 0), c.unknown_type());
        // AnyDefault → anyType.
        let ctx = c.new_inference_context(&[tp], None, InferenceFlags::ANY_DEFAULT, InferenceComparer::Assignable);
        assert_eq!(c.get_inferred_type(ctx, 0), c.any_type());
    }

    #[test]
    fn clone_inference_context_deep_copies_inferences_and_or_flags() {
        let mut c = new_empty_checker();
        let tp = c.alloc_test_type_parameter(false);
        let string = c.string_type();
        let ctx = c.new_inference_context(&[tp], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        c.infer_types(ctx, string, tp, InferencePriority::NAKED_TYPE_VARIABLE, false);
        let clone = c.clone_inference_context(Some(ctx), InferenceFlags::NO_DEFAULT)
            .expect("clone of a live context");
        // The clone carries the recorded candidate AND the OR'd extra flag.
        assert_eq!(c.inference_contexts[clone.0 as usize].inferences[0].candidates, vec![string]);
        assert!(c.inference_contexts[clone.0 as usize].flags.intersects(InferenceFlags::NO_DEFAULT));
        // Deep copy: a second inference through the clone leaves the original untouched.
        let number = c.number_type();
        c.infer_types(clone, number, tp, InferencePriority::NAKED_TYPE_VARIABLE, false);
        assert_eq!(c.inference_contexts[clone.0 as usize].inferences[0].candidates.len(), 2);
        assert_eq!(c.inference_contexts[ctx.0 as usize].inferences[0].candidates, vec![string]);
    }

    #[test]
    fn clone_inferred_part_keeps_only_inferred_informations() {
        let mut c = new_empty_checker();
        let tp1 = c.alloc_test_type_parameter(false);
        let tp2 = c.alloc_test_type_parameter(false);
        let string = c.string_type();
        let ctx = c.new_inference_context(&[tp1, tp2], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        // Only tp1 gets a candidate.
        c.infer_types(ctx, string, tp1, InferencePriority::NAKED_TYPE_VARIABLE, false);
        let part = c.clone_inferred_part_of_context(ctx).expect("one inferred info");
        assert_eq!(c.inference_contexts[part.0 as usize].inferences.len(), 1);
        assert_eq!(c.inference_contexts[part.0 as usize].inferences[0].type_parameter, tp1);
        assert_eq!(c.inference_contexts[part.0 as usize].inferences[0].candidates, vec![string]);
        // An empty context yields nil (Go's `if len(inferences) == 0 return nil`).
        let ctx2 = c.new_inference_context(&[tp2], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        assert!(c.clone_inferred_part_of_context(ctx2).is_none());
    }

    #[test]
    fn get_inference_info_for_type_indexes_the_context_inferences() {
        let mut c = new_empty_checker();
        let tp1 = c.alloc_test_type_parameter(false);
        let tp2 = c.alloc_test_type_parameter(false);
        let ctx = c.new_inference_context(&[tp1, tp2], None, InferenceFlags::NONE, InferenceComparer::Assignable);
        let mut n = c.get_inference_state();
        n.inferences = ctx;
        // §5.4 index idiom: the match is by type-parameter identity.
        assert_eq!(get_inference_info_for_type(&c, &n, tp1), Some(0));
        assert_eq!(get_inference_info_for_type(&c, &n, tp2), Some(1));
        // Non-variables never match.
        assert_eq!(get_inference_info_for_type(&c, &n, c.string_type()), None);
        c.put_inference_state(n);
    }
}
