// Ported from tsc/internal/checker/checker.go @ bec77507cc (M6 S10)
//
// facts.rs — THE FACTS LATTICE (Go checker.go:399-482 constants + the
// 31523-31765 worker family + the 29604-29636 falsy/missing extractors).
//
// NOTE (parent-task correction, honestly scoped): this Go snapshot has NO
// checkFacts/ExprFact/setFact/forEachFactInFactTable family (those are
// upstream-TS 5.7+ checker.ts names that postdate the Go port's snapshot).
// The facts lattice HERE is the TypeFacts bitset + getTypeFacts/hasTypeFacts/
// getTypeFactsWorker/getIntersectionTypeFacts/getTypeWithFacts/
// getAdjustedTypeWithFacts family below — the flow-fact structures W3's
// flow.go wave consumes (flow.go's narrowTypeByTypeFacts/
// getNotEqualFactsFromTypeofSwitch all route through these entry points).
//
// The TypeFacts constant set lives on `crate::relater::TypeFacts` (the
// define_flags! newtype the S2/S4/S9 waves already read); the full Go lattice
// landed with this wave (checker.go:402-482 mirrored bit-for-bit).

use crate::checker::Checker;
use crate::relater::TypeFacts;
use crate::types::{LiteralValue, TypeFlags, TypeId};

impl Checker {
    // ────────────────────────────────────────────────────────────────────────
    // The facts lattice (Go checker.go 31523-31765)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getTypeFacts(t *Type, mask TypeFacts) TypeFacts`
    /// (checker.go:31523) — REAL (M6 S10; replaces relater.rs's equal-facts
    /// stub, which returned `mask` unmasked).
    pub(crate) fn get_type_facts_impl(&mut self, t: TypeId, mask: TypeFacts) -> TypeFacts {
        let facts = self.get_type_facts_worker(t, mask);
        TypeFacts(facts.0 & mask.0)
    }

    /// Go: `func (c *Checker) hasTypeFacts(t *Type, mask TypeFacts) bool`
    /// (checker.go:31527) — `getTypeFacts(t, mask) != 0` (ANY bit; the prior
    /// typeofsymbol.rs stub computed contains-all — a misport, fixed here).
    pub(crate) fn has_type_facts_impl(&mut self, t: TypeId, mask: TypeFacts) -> bool {
        !self.get_type_facts_impl(t, mask).is_empty()
    }

    /// Go: `func (c *Checker) getTypeFactsWorker(t *Type, callerOnlyNeeds TypeFacts) TypeFacts`
    /// (checker.go:31531).
    pub(crate) fn get_type_facts_worker(&mut self, t: TypeId, caller_only_needs: TypeFacts) -> TypeFacts {
        // Intersection and instantiable types are represented by their
        // base constraint (Go's first two lines).
        let mut t = t;
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION.union(TypeFlags::INSTANTIABLE)) {
            t = crate::relater::get_base_constraint_of_type(self, t)
                .unwrap_or_else(|| self.unknown_type());
        }
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::STRING.union(TypeFlags::STRING_MAPPING)) {
            if self.strict_null_checks {
                return TypeFacts::STRING_STRICT_FACTS;
            }
            return TypeFacts::STRING_FACTS;
        }
        if flags.intersects(TypeFlags::STRING_LITERAL.union(TypeFlags::TEMPLATE_LITERAL)) {
            let is_empty = flags.intersects(TypeFlags::STRING_LITERAL)
                && crate::relater::get_string_literal_value(self, t).is_empty();
            if self.strict_null_checks {
                if is_empty {
                    return TypeFacts::EMPTY_STRING_STRICT_FACTS;
                }
                return TypeFacts::NON_EMPTY_STRING_STRICT_FACTS;
            }
            if is_empty {
                return TypeFacts::EMPTY_STRING_FACTS;
            }
            return TypeFacts::NON_EMPTY_STRING_FACTS;
        }
        if flags.intersects(TypeFlags::NUMBER.union(TypeFlags::ENUM)) {
            if self.strict_null_checks {
                return TypeFacts::NUMBER_STRICT_FACTS;
            }
            return TypeFacts::NUMBER_FACTS;
        }
        if flags.intersects(TypeFlags::NUMBER_LITERAL) {
            let is_zero = get_number_literal_value_of_type(self.type_(t)).0 == 0.0;
            if self.strict_null_checks {
                if is_zero {
                    return TypeFacts::ZERO_NUMBER_STRICT_FACTS;
                }
                return TypeFacts::NON_ZERO_NUMBER_STRICT_FACTS;
            }
            if is_zero {
                return TypeFacts::ZERO_NUMBER_FACTS;
            }
            return TypeFacts::NON_ZERO_NUMBER_FACTS;
        }
        if flags.intersects(TypeFlags::BIGINT) {
            if self.strict_null_checks {
                return TypeFacts::BIGINT_STRICT_FACTS;
            }
            return TypeFacts::BIGINT_FACTS;
        }
        if flags.intersects(TypeFlags::BIGINT_LITERAL) {
            let is_zero = is_zero_big_int(self.type_(t));
            if self.strict_null_checks {
                if is_zero {
                    return TypeFacts::ZERO_BIGINT_STRICT_FACTS;
                }
                return TypeFacts::NON_ZERO_BIGINT_STRICT_FACTS;
            }
            if is_zero {
                return TypeFacts::ZERO_BIGINT_FACTS;
            }
            return TypeFacts::NON_ZERO_BIGINT_FACTS;
        }
        if flags.intersects(TypeFlags::BOOLEAN) {
            if self.strict_null_checks {
                return TypeFacts::BOOLEAN_STRICT_FACTS;
            }
            return TypeFacts::BOOLEAN_FACTS;
        }
        if flags.intersects(TypeFlags::BOOLEAN_LIKE) {
            let is_false = t == self.false_type() || t == self.regular_false_type();
            if self.strict_null_checks {
                if is_false {
                    return TypeFacts::FALSE_STRICT_FACTS;
                }
                return TypeFacts::TRUE_STRICT_FACTS;
            }
            if is_false {
                return TypeFacts::FALSE_FACTS;
            }
            return TypeFacts::TRUE_FACTS;
        }
        if flags.intersects(TypeFlags::OBJECT) {
            let possible_facts = if self.strict_null_checks {
                TypeFacts::EMPTY_OBJECT_STRICT_FACTS
                    .union(TypeFacts::FUNCTION_STRICT_FACTS)
                    .union(TypeFacts::OBJECT_STRICT_FACTS)
            } else {
                TypeFacts::EMPTY_OBJECT_FACTS
                    .union(TypeFacts::FUNCTION_FACTS)
                    .union(TypeFacts::OBJECT_FACTS)
            };
            if !caller_only_needs.intersects(possible_facts) {
                // If the caller doesn't care about any of the facts that we
                // could possibly produce, return zero so we can skip resolving
                // members (Go's comment).
                return TypeFacts::NONE;
            }
            if self.type_(t).object_flags.intersects(crate::types::ObjectFlags::ANONYMOUS)
                && self.is_empty_object_type(t)
            {
                if self.strict_null_checks {
                    return TypeFacts::EMPTY_OBJECT_STRICT_FACTS;
                }
                return TypeFacts::EMPTY_OBJECT_FACTS;
            }
            if self.is_function_object_type(t) {
                if self.strict_null_checks {
                    return TypeFacts::FUNCTION_STRICT_FACTS;
                }
                return TypeFacts::FUNCTION_FACTS;
            }
            if self.strict_null_checks {
                return TypeFacts::OBJECT_STRICT_FACTS;
            }
            return TypeFacts::OBJECT_FACTS;
        }
        if flags.intersects(TypeFlags::VOID) {
            return TypeFacts::VOID_FACTS;
        }
        if flags.intersects(TypeFlags::UNDEFINED) {
            return TypeFacts::UNDEFINED_FACTS;
        }
        if flags.intersects(TypeFlags::NULL) {
            return TypeFacts::NULL_FACTS;
        }
        if flags.intersects(TypeFlags::ESSYMBOL_LIKE) {
            if self.strict_null_checks {
                return TypeFacts::SYMBOL_STRICT_FACTS;
            }
            return TypeFacts::SYMBOL_FACTS;
        }
        if flags.intersects(TypeFlags::NON_PRIMITIVE) {
            if self.strict_null_checks {
                return TypeFacts::OBJECT_STRICT_FACTS;
            }
            return TypeFacts::OBJECT_FACTS;
        }
        if flags.intersects(TypeFlags::NEVER) {
            return TypeFacts::NONE;
        }
        if flags.intersects(TypeFlags::UNION) {
            let constituents = self.type_(t).types().to_vec();
            let mut facts = TypeFacts::NONE;
            for u in constituents {
                facts |= self.get_type_facts_worker(u, caller_only_needs);
            }
            return facts;
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            return self.get_intersection_type_facts(t, caller_only_needs);
        }
        TypeFacts::UNKNOWN_FACTS
    }

    /// Go: `func (c *Checker) getIntersectionTypeFacts(t *Type, callerOnlyNeeds TypeFacts) TypeFacts`
    /// (checker.go:31667).
    pub(crate) fn get_intersection_type_facts(&mut self, t: TypeId, caller_only_needs: TypeFacts) -> TypeFacts {
        // When an intersection contains a primitive type we ignore object
        // type constituents as they are presumably type tags (Go's comment).
        let ignore_objects = crate::relater::maybe_type_of_kind(self, t, TypeFlags::PRIMITIVE);
        let constituents = self.type_(t).types().to_vec();
        let mut ored_facts = TypeFacts::NONE;
        let mut anded_facts = TypeFacts::ALL;
        for u in constituents {
            if !(ignore_objects && self.type_(u).flags.intersects(TypeFlags::OBJECT)) {
                let f = self.get_type_facts_worker(u, caller_only_needs);
                ored_facts |= f;
                anded_facts &= f;
            }
        }
        TypeFacts(ored_facts.0 & TypeFacts::OR_FACTS_MASK.0 | anded_facts.0 & TypeFacts::AND_FACTS_MASK.0)
    }

    /// Go: `func (c *Checker) isFunctionObjectType(t *Type) bool` (checker.go:31695)
    /// — the quick "bind" probe before the globalFunctionType subtype check.
    pub(crate) fn is_function_object_type(&mut self, t: TypeId) -> bool {
        if self.type_(t).object_flags.intersects(crate::types::ObjectFlags::EVOLVING_ARRAY) {
            return false;
        }
        let resolved = self.resolve_structured_type_members_impl(t);
        let has_bind = resolved.members.get("bind").is_some();
        let signature_count = resolved.signatures.len();
        // We do a quick check for a "bind" property before performing the
        // more expensive subtype check (Go's comment).
        signature_count != 0
            || has_bind && {
                let global_function_type = self.global_function_type.expect("globalFunctionType");
                self.is_type_subtype_of(t, global_function_type)
            }
    }

    /// Go: `func (c *Checker) getTypeWithFacts(t *Type, include TypeFacts) *Type`
    /// (checker.go:31699) — REAL (M6 S10; replaces relater.rs's identity
    /// stub). PORT: the predicate resolves members (the object arm), so the
    /// union walk rides uniontypes.rs's `&mut`-taking filterType twin
    /// (`filter_type_mut` — Go-identical body) rather than the `&Checker` form.
    pub(crate) fn get_type_with_facts_impl(&mut self, t: TypeId, include: TypeFacts) -> TypeId {
        self.filter_type_mut(t, |c, u| c.has_type_facts_impl(u, include))
    }

    /// Go: `func (c *Checker) getAdjustedTypeWithFacts(t *Type, facts TypeFacts) *Type`
    /// (checker.go:31708) — REAL (M6 S10; replaces the S9 identity seam the
    /// awaited family routed through). In-crate consumers land with W3
    /// (flow.go's narrowing arms) — the W2 relater facts reads ride
    /// getTypeWithFacts/hasTypeFacts.
    #[allow(dead_code)] // TODO(port M6 W3): flow.go's narrowTypeByTypeFacts
    pub(crate) fn get_adjusted_type_with_facts_impl(&mut self, t: TypeId, facts: TypeFacts) -> TypeId {
        let strict = self.strict_null_checks && self.type_(t).flags.intersects(TypeFlags::UNKNOWN);
        let subject = if strict { self.unknown_union_type() } else { t };
        let type_with_facts = self.get_type_with_facts_impl(subject, facts);
        let reduced = self.recombine_unknown_type(type_with_facts);
        if self.strict_null_checks {
            if facts == TypeFacts::NE_UNDEFINED {
                return self.remove_nullable_by_intersection(
                    reduced,
                    TypeFacts::EQ_UNDEFINED,
                    TypeFacts::EQ_NULL,
                    TypeFacts::IS_NULL,
                    self.null_type(),
                );
            }
            if facts == TypeFacts::NE_NULL {
                return self.remove_nullable_by_intersection(
                    reduced,
                    TypeFacts::EQ_NULL,
                    TypeFacts::EQ_UNDEFINED,
                    TypeFacts::IS_UNDEFINED,
                    self.undefined_type(),
                );
            }
            if facts == TypeFacts::NE_UNDEFINED_OR_NULL || facts == TypeFacts::TRUTHY {
                let eq_facts = TypeFacts::EQ_UNDEFINED_OR_NULL;
                let mapped = self
                    .map_type(
                        reduced,
                        &mut |c, u| {
                            if c.has_type_facts_impl(u, eq_facts) {
                                Some(c.get_global_non_nullable_type_instantiation(u))
                            } else {
                                Some(u)
                            }
                        },
                    )
                    .unwrap_or(reduced);
                return mapped;
            }
        }
        reduced
    }

    /// Go: `func (c *Checker) removeNullableByIntersection(t, targetFacts, otherFacts, otherIncludesFacts, otherType) *Type`
    /// (checker.go:31737).
    pub(crate) fn remove_nullable_by_intersection(
        &mut self,
        t: TypeId,
        target_facts: TypeFacts,
        other_facts: TypeFacts,
        other_includes_facts: TypeFacts,
        other_type: TypeId,
    ) -> TypeId {
        let facts = self.get_type_facts_impl(
            t,
            TypeFacts::EQ_UNDEFINED
                .union(TypeFacts::EQ_NULL)
                .union(TypeFacts::IS_UNDEFINED)
                .union(TypeFacts::IS_NULL),
        );
        // Simply return the type if it never compares equal to the target
        // nullable (Go's comment).
        if !facts.intersects(target_facts) {
            return t;
        }
        // By default we intersect with a union of {} and the opposite
        // nullable (Go's comment).
        let empty_object_type = self.empty_object_type();
        let empty_and_other_union = self.get_union_type(&[empty_object_type, other_type]);
        let other_includes = facts.intersects(other_includes_facts);
        self.map_type(t, &mut |c, u| {
                if c.has_type_facts_impl(u, target_facts) {
                    if !other_includes && c.has_type_facts_impl(u, other_facts) {
                        return Some(c.get_intersection_type(&[u, empty_and_other_union]));
                    }
                    return Some(c.get_intersection_type(&[u, empty_object_type]));
                }
                Some(u)
            })
            .unwrap_or(t)
    }

    /// Go: `func (c *Checker) recombineUnknownType(t *Type) *Type` (checker.go:31759).
    pub(crate) fn recombine_unknown_type(&mut self, t: TypeId) -> TypeId {
        if t == self.unknown_union_type() {
            return self.unknown_type();
        }
        t
    }

    /// Go: `func (c *Checker) getGlobalNonNullableTypeInstantiation(t *Type) *Type`
    /// (checker.go:31763) — without lib.d.ts the NonNullable alias symbol is
    /// absent (Go's `getGlobalNonNullableTypeAliasOrNil` answers nil), so the
    /// {}-intersection fallback is the Go-faithful answer.
    pub(crate) fn get_global_non_nullable_type_instantiation(&mut self, t: TypeId) -> TypeId {
        let alias = self
            .get_global_type_alias_symbol("NonNullable", 1, false);
        if let Some(alias) = alias {
            return self.get_type_alias_instantiation(alias, vec![t], None);
        }
        let empty_object_type = self.empty_object_type();
        self.get_intersection_type(&[t, empty_object_type])
    }

    // ────────────────────────────────────────────────────────────────────────
    // The falsy/missing extractors (Go checker.go 29604-29636)
    // ────────────────────────────────────────────────────────────────────────
    // (maybeTypeOfKindConsideringBaseConstraint — the facts-adjacent kind
    // probe — lives in expressions.rs (S7-landed REAL body); this module's
    // extractors below are the S10 slices.)

    /// Go: `func (c *Checker) removeMissingOrUndefinedType(t *Type) *Type`
    /// (checker.go:29616) — REAL (M6 S10; the exports.go
    /// RemoveMissingOrUndefinedType entry delegates here).
    pub(crate) fn remove_missing_or_undefined_type_impl(&mut self, t: TypeId) -> TypeId {
        if self.exact_optional_property_types {
            let missing = self.missing_type();
            return self.remove_type(t, missing);
        }
        self.get_type_with_facts_impl(t, TypeFacts::NE_UNDEFINED)
    }

    /// Go: `func (c *Checker) removeDefinitelyFalsyTypes(t *Type) *Type`
    /// (checker.go:29624).
    pub(crate) fn remove_definitely_falsy_types(&mut self, t: TypeId) -> TypeId {
        let truthy = TypeFacts::TRUTHY;
        self.filter_type_mut(t, |c, u| c.has_type_facts_impl(u, truthy))
    }

    /// Go: `func (c *Checker) extractDefinitelyFalsyTypes(t *Type) *Type`
    /// (checker.go:29628) — `mapType(t, getDefinitelyFalsyPartOfType)`.
    pub(crate) fn extract_definitely_falsy_types(&mut self, t: TypeId) -> TypeId {
        self.map_type(t, &mut |c, u| Some(c.get_definitely_falsy_part_of_type(u)))
            .unwrap_or(t)
    }

    /// Go: `func (c *Checker) getDefinitelyFalsyPartOfType(t *Type) *Type`
    /// (checker.go:29632).
    pub(crate) fn get_definitely_falsy_part_of_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::STRING) {
            return self.empty_string_type();
        }
        if flags.intersects(TypeFlags::NUMBER) {
            return self.zero_type();
        }
        if flags.intersects(TypeFlags::BIGINT) {
            return self.zero_bigint_type();
        }
        if t == self.regular_false_type()
            || t == self.false_type()
            || flags.intersects(
                TypeFlags::VOID
                    .union(TypeFlags::UNDEFINED)
                    .union(TypeFlags::NULL)
                    .union(TypeFlags::ANY_OR_UNKNOWN),
            )
            || (flags.intersects(TypeFlags::STRING_LITERAL)
                && crate::relater::get_string_literal_value(self, t).is_empty())
            || (flags.intersects(TypeFlags::NUMBER_LITERAL)
                && get_number_literal_value_of_type(self.type_(t)).0 == 0.0)
            || (flags.intersects(TypeFlags::BIGINT_LITERAL) && is_zero_big_int(self.type_(t)))
        {
            return t;
        }
        self.never_type()
    }
}

/// Go: `func isZeroBigInt(t *Type) bool` (checker.go:31693) —
/// `getBigIntLiteralValue(t) == jsnum.PseudoBigInt{}`.
pub(crate) fn is_zero_big_int(t: &crate::types::Type) -> bool {
    match t.as_literal_type().and_then(|d| d.value.as_ref()) {
        Some(LiteralValue::BigInt(b)) => *b == tsc_jsnum::PseudoBigInt::default(),
        _ => false,
    }
}

/// Go: `getNumberLiteralValue(t)` (utilities.go) — the literal payload read.
pub(crate) fn get_number_literal_value_of_type(t: &crate::types::Type) -> tsc_jsnum::Number {
    match t.as_literal_type().and_then(|d| d.value.as_ref()) {
        Some(LiteralValue::Number(n)) => *n,
        _ => tsc_jsnum::Number::from(0.0),
    }
}
