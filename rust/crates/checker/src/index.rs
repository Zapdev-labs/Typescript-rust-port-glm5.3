// Ported from tsc/internal/checker/checker.go @ bec77507cc (M6 S10)
//
// index.rs — THE INDEX/KEYOF/INDEXED-ACCESS/PATTERN/SIMPLIFIED FAMILIES
// (Go checker.go:27033-27800 pattern+index slice, 28399-28621 simplified
// slice, 29839-29860 substitution slice, plus relater.go's
// getKnownKeysOfTupleType and checker.go:17605-17632's
// getConstraintOfIndexedAccess/From).
//
// This is the slice the dispatch table pinned as equal-facts/no-op stubs in
// the earlier waves (relater.rs's S10 seam block, typenodes.rs's pattern
// stubs, typeofsymbol.rs's computeBaseConstraint arms) — S10 makes them REAL:
//   - keyof: getIndexType(Ex) → getIndexTypeForGenericType/ForMappedType →
//     getLiteralTypeFromProperties (the propertiesTypes cache).
//   - T[K]: getIndexedAccessType(Ex/OrUndefined) → the interned deferred
//     indexedAccessTypes table + getPropertyTypeForIndexType (Go's 222-line
//     resolver, diagnostics included).
//   - simplification: getSimplifiedType family over cachedTypes
//     (IndexedAccessForReading/ForWriting).
//   - substitution: getSubstitutionIntersection + substituteIndexedMappedType
//     + couldAccessOptionalProperty.
// The S7 union engine, S8 member/index-info machinery, S9 constraint family
// and the S2 circularity protocol are reused as-is — no new state beyond
// checker.rs's `indexed_access_types`/`properties_types` maps (Go
// checker.go:630/666).

use crate::checker::Checker;
use crate::relater::{self, KeyBuilder, TypeFacts};
use crate::todo_shapes::{CacheHashKey, CachedTypeKind, CachedTypeKey};
use crate::types::{self, AccessFlags, IndexFlags, ObjectFlags, TypeFlags, TypeId};
use tsc_ast::{Kind, NodeId, SymbolFlags};
use tsc_diagnostics::{
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_READ_ONLY_PROPERTY,
    ELEMENT_IMPLICITLY_HAS_AN_ANY_TYPE_BECAUSE_EXPRESSION_OF_TYPE_0_CAN_T_BE_USED_TO_INDEX_TYPE_1,
    ELEMENT_IMPLICITLY_HAS_AN_ANY_TYPE_BECAUSE_INDEX_EXPRESSION_IS_NOT_OF_TYPE_NUMBER,
    ELEMENT_IMPLICITLY_HAS_AN_ANY_TYPE_BECAUSE_TYPE_0_HAS_NO_INDEX_SIGNATURE_DID_YOU_MEAN_TO_CALL_1,
    INDEX_SIGNATURE_IN_TYPE_0_ONLY_PERMITS_READING, NO_INDEX_SIGNATURE_WITH_A_PARAMETER_OF_TYPE_0_WAS_FOUND_ON_TYPE_1,
    PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1, PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_2,
    PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_TO_ACCESS_THE_STATIC_MEMBER_2_INSTEAD,
    TUPLE_TYPE_0_OF_LENGTH_1_HAS_NO_ELEMENT_AT_INDEX_2, A_TUPLE_TYPE_CANNOT_BE_INDEXED_WITH_A_NEGATIVE_VALUE,
    TYPE_0_CANNOT_BE_USED_AS_AN_INDEX_TYPE, TYPE_0_CANNOT_BE_USED_TO_INDEX_TYPE_1,
    TYPE_0_IS_GENERIC_AND_CAN_ONLY_BE_INDEXED_FOR_READING, TYPE_0_HAS_NO_MATCHING_INDEX_SIGNATURE_FOR_TYPE_1,
};
use crate::utilities as u;

/// Go: `type PropertiesTypesKey struct { typeId TypeId; include TypeFlags; includeOrigin bool }`
/// (checker.go:237) — the `propertiesTypes map[PropertiesTypesKey]*Type` key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct PropertiesTypesKey {
    pub(crate) type_id: u32,
    pub(crate) include: u32,
    pub(crate) include_origin: bool,
}

/// Go: `type MappedTypeNameTypeKind int32` values (checker.go) — reused from
/// relater.rs's enum where the crate already carries it (None/Filtering/
/// Remapping); re-exported here for the getMappedTypeNameTypeKind consumers.
pub(crate) use crate::relater::MappedTypeNameTypeKind;

impl Checker {
    // ────────────────────────────────────────────────────────────────────────
    // The keyof family (Go checker.go 27214-27459)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getIndexType(t *Type) *Type` (checker.go:27214)
    /// — REAL (M6 S10; replaces relater.rs's neverType stub).
    pub(crate) fn get_index_type_impl(&mut self, t: TypeId) -> TypeId {
        self.get_index_type_ex_impl(t, IndexFlags::NONE)
    }

    /// Go: `func (c *Checker) getIndexTypeEx(t *Type, indexFlags IndexFlags) *Type`
    /// (checker.go:27218) — REAL (M6 S10).
    pub(crate) fn get_index_type_ex_impl(&mut self, t: TypeId, index_flags: IndexFlags) -> TypeId {
        let t = self.get_reduced_type(t);
        let flags = self.type_(t).flags;
        if crate::inference::is_no_infer_type(self, t) {
            // Go: `c.getNoInferType(c.getIndexTypeEx(t.AsSubstitutionType().baseType, indexFlags))`.
            let base = self
                .type_(t)
                .as_substitution_type()
                .expect("SubstitutionType data")
                .base_type
                .expect("substitution baseType");
            let index = self.get_index_type_ex_impl(base, index_flags);
            return self.get_no_infer_type(index);
        }
        if self.should_defer_index_type_impl(t, index_flags) {
            return self.get_index_type_for_generic_type(t, index_flags);
        }
        if flags.intersects(TypeFlags::UNION) {
            let constituents = self.type_(t).types().to_vec();
            let mapped: Vec<TypeId> = constituents
                .into_iter()
                .map(|s| self.get_index_type_ex_impl(s, index_flags))
                .collect();
            return self.get_intersection_type(&mapped);
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self.type_(t).types().to_vec();
            let mapped: Vec<TypeId> = constituents
                .into_iter()
                .map(|s| self.get_index_type_ex_impl(s, index_flags))
                .collect();
            return self.get_union_type(&mapped);
        }
        if self.type_(t).object_flags.intersects(ObjectFlags::MAPPED) {
            return self.get_index_type_for_mapped_type(t, index_flags);
        }
        if t == self.wildcard_type() {
            return self.wildcard_type();
        }
        if flags.intersects(TypeFlags::UNKNOWN) {
            return self.never_type();
        }
        if flags.intersects(TypeFlags::ANY.union(TypeFlags::NEVER)) {
            return self.string_number_symbol_type();
        }
        let include = if index_flags.intersects(IndexFlags::NO_INDEX_SIGNATURES) {
            TypeFlags::STRING_LITERAL
        } else {
            TypeFlags::STRING_LIKE
        }
        .union(if index_flags.intersects(IndexFlags::STRINGS_ONLY) {
            TypeFlags::NONE
        } else {
            TypeFlags::NUMBER_LIKE.union(TypeFlags::ESSYMBOL_LIKE)
        });
        self.get_literal_type_from_properties(t, include, index_flags == IndexFlags::NONE)
    }

    /// Go: `func (c *Checker) getExtractStringType(t *Type) *Type` (checker.go:27240)
    /// — REAL (M6 S10; replaces typeofsymbol.rs's stringType stub). Without
    /// lib.d.ts the global `Extract` type alias is absent (Go's
    /// getGlobalExtractSymbol answers nil), so Go's stringType fallback is
    /// the faithful answer.
    pub(crate) fn get_extract_string_type_impl(&mut self, t: TypeId) -> TypeId {
        let extract_type_alias = self.get_global_type_alias_symbol("Extract", 2, false);
        if let Some(extract_type_alias) = extract_type_alias {
            let string_type = self.string_type();
            return self.get_type_alias_instantiation(extract_type_alias, vec![t, string_type], None);
        }
        self.string_type()
    }

    /// Go: `func (c *Checker) getLiteralTypeFromProperties(t *Type, include TypeFlags, includeOrigin bool) *Type`
    /// (checker.go:27251) — REAL (M6 S10; the propertiesTypes cache is the
    /// checker.rs field added with this wave).
    pub(crate) fn get_literal_type_from_properties(
        &mut self,
        t: TypeId,
        include: TypeFlags,
        include_origin: bool,
    ) -> TypeId {
        let key = PropertiesTypesKey {
            type_id: t.0,
            include: include.0,
            include_origin,
        };
        if let Some(&cached) = self.properties_types.get(&key) {
            return cached;
        }
        let mut origin: Option<TypeId> = None;
        if include_origin
            && self
                .type_(t)
                .object_flags
                .intersects(ObjectFlags::CLASS_OR_INTERFACE.union(ObjectFlags::REFERENCE))
            || self.type_(t).alias.is_some()
        {
            origin = Some(self.new_index_type(t, IndexFlags::NONE));
        }
        let props = self.get_properties_of_type_impl(t);
        let index_infos = self.get_index_infos_of_type_impl(t);
        let mut type_list: Vec<TypeId> = Vec::with_capacity(props.len() + index_infos.len());
        for prop in props {
            let literal = self
                .get_literal_type_from_property_impl(prop, include, false)
                .unwrap_or_else(|| self.never_type());
            type_list.push(literal);
        }
        let enum_number_index_info = self.enum_number_index_info;
        let string_type = self.string_type();
        let string_or_number_type = self.string_or_number_type();
        for info in index_infos {
            if Some(info) != enum_number_index_info {
                let key_type = self.index_info_key_type(info);
                if self.is_key_type_included(key_type, include) {
                    if Some(key_type) == Some(string_type) && include.intersects(TypeFlags::NUMBER) {
                        type_list.push(string_or_number_type);
                    } else {
                        type_list.push(key_type);
                    }
                }
            }
        }
        let result = self.get_union_type_ex(
            type_list,
            crate::todo_shapes::UnionReduction::Literal,
            None,
            origin,
        );
        self.properties_types.insert(key, result);
        result
    }

    /// Go: `func (c *Checker) isKeyTypeIncluded(keyType *Type, include TypeFlags) bool`
    /// (checker.go:27299).
    pub(crate) fn is_key_type_included(&mut self, key_type: TypeId, include: TypeFlags) -> bool {
        let flags = self.type_(key_type).flags;
        if flags.intersects(include) {
            return true;
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self.type_(key_type).types().to_vec();
            return constituents
                .iter()
                .any(|&t| self.is_key_type_included(t, include));
        }
        false
    }

    /// Go: `func (c *Checker) shouldDeferIndexType(t *Type, indexFlags IndexFlags) bool`
    /// (checker.go:27341) — REAL (M6 S10; replaces relater.rs's false stub).
    pub(crate) fn should_defer_index_type_impl(&mut self, t: TypeId, index_flags: IndexFlags) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
            return true;
        }
        if crate::typenodes::is_generic_tuple_type(self, t) {
            return true;
        }
        if relater::is_generic_mapped_type(self, t)
            && self.get_name_type_from_mapped_type_impl(t).is_some()
        {
            return true;
        }
        if flags.intersects(TypeFlags::UNION)
            && !index_flags.intersects(IndexFlags::NO_REDUCIBLE_CHECK)
            && crate::typenodes::is_generic_reducible_type(self, t)
        {
            return true;
        }
        if flags.intersects(TypeFlags::INTERSECTION)
            && relater::maybe_type_of_kind(self, t, TypeFlags::INSTANTIABLE)
            && {
                let constituents = self.type_(t).types().to_vec();
                constituents.iter().any(|&s| self.is_empty_anonymous_object_type(s))
            }
        {
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) getIndexTypeForGenericType(t *Type, indexFlags IndexFlags) *Type`
    /// (checker.go:27383) — the cachedTypes intern.
    pub(crate) fn get_index_type_for_generic_type(&mut self, t: TypeId, index_flags: IndexFlags) -> TypeId {
        let kind = if index_flags.intersects(IndexFlags::STRINGS_ONLY) {
            CachedTypeKind::StringIndexType
        } else {
            CachedTypeKind::IndexType
        };
        let key = CachedTypeKey { kind, type_id: t.0 };
        if let Some(cached) = self.cached_types.get(&key).copied().flatten() {
            return cached;
        }
        let index_type = self.new_index_type(t, {
            let mut flags = IndexFlags::NONE;
            if index_flags.intersects(IndexFlags::STRINGS_ONLY) {
                flags |= IndexFlags::STRINGS_ONLY;
            }
            flags
        });
        self.cached_types.insert(key, Some(index_type));
        index_type
    }

    /// Go: `func (c *Checker) getIndexTypeForMappedType(t *Type, indexFlags IndexFlags) *Type`
    /// (checker.go:27396) — REAL (M6 S10; Go's comment on the keyof-vs-
    /// mapped-properties subtlety preserved).
    pub(crate) fn get_index_type_for_mapped_type(&mut self, t: TypeId, index_flags: IndexFlags) -> TypeId {
        let type_parameter = self.get_type_parameter_from_mapped_type_impl(t);
        let constraint_type = self.get_constraint_type_from_mapped_type_impl(t);
        let mapped_target = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.base.target)
            .unwrap_or(t);
        let name_type = self.get_name_type_from_mapped_type_impl(mapped_target);
        if name_type.is_none() && !index_flags.intersects(IndexFlags::NO_INDEX_SIGNATURES) {
            // no mapping and no filtering required, just quickly bail to
            // returning the constraint in the common case (Go's comment).
            return constraint_type;
        }
        let mut key_types: Vec<TypeId> = Vec::new();
        // Go's `addMemberForKeyType` closure (the `keyof`-vs-`string | number`
        // ternary included).
        let mut add_member_for_key_type = |c: &mut Checker, key_type: TypeId| {
            let prop_name_type = match name_type {
                Some(name_type) => {
                    let mapper = c
                        .type_(t)
                        .as_mapped_type()
                        .expect("MappedType data")
                        .base
                        .mapper;
                    let appended = c.append_type_mapping(mapper, type_parameter, key_type);
                    c.instantiate_type(name_type, appended)
                }
                None => key_type,
            };
            // `keyof` currently always returns `string | number` for concrete
            // `string` index signatures (Go's comment).
            let string_type = c.string_type();
            let string_or_number_type = c.string_or_number_type();
            if Some(prop_name_type) == Some(string_type) {
                key_types.push(string_or_number_type);
            } else {
                key_types.push(prop_name_type);
            }
        };
        // Calling getApparentType on the `T` of a `keyof T` in the constraint
        // type of a generic mapped type can trigger a circularity; we only
        // eagerly manifest the keys if the constraint is non-generic (Go's
        // comment).
        let constraint_is_generic = relater::is_generic_index_type(self, constraint_type);
        if constraint_is_generic {
            if self.is_mapped_type_with_keyof_constraint_declaration_impl(t) {
                // Generic index + homomorphic mapping + key remapping — defer
                // the whole `keyof whatever` (Go's comment).
                return self.get_index_type_for_generic_type(t, index_flags);
            }
            // Include the generic component in the resulting type.
            for_each_type(self, constraint_type, &mut add_member_for_key_type);
        } else if self.is_mapped_type_with_keyof_constraint_declaration_impl(t) {
            let modifiers_type = self.get_modifiers_type_from_mapped_type_impl(t);
            let apparent_modifiers = self.get_apparent_type_impl(modifiers_type);
            let strings_only = index_flags.intersects(IndexFlags::STRINGS_ONLY);
            self.for_each_mapped_type_property_key_type_and_index_signature_key_type(
                apparent_modifiers,
                TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                strings_only,
                &mut add_member_for_key_type,
            );
        } else {
            let lower_bound = self.get_lower_bound_of_key_type(constraint_type);
            for_each_type(self, lower_bound, &mut add_member_for_key_type);
        }
        // Compare the final resulting list with the original constraintType
        // so we can return the union that preserves aliases/origin data
        // (Go's comment).
        let result = if index_flags.intersects(IndexFlags::NO_INDEX_SIGNATURES) {
            let union = self.get_union_type(&key_types);
            let any_or_string = TypeFlags::ANY.union(TypeFlags::STRING);
            self.filter_type(union, |c, u| !c.type_(u).flags.intersects(any_or_string))
        } else {
            self.get_union_type(&key_types)
        };
        if self.type_(result).flags.intersects(TypeFlags::UNION)
            && self.type_(constraint_type).flags.intersects(TypeFlags::UNION)
            && crate::typeofsymbol::get_type_list_key(self.type_(result).types())
                == crate::typeofsymbol::get_type_list_key(self.type_(constraint_type).types())
        {
            return constraint_type;
        }
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // The indexed-access family (Go checker.go 27457-27760)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getIndexedAccessType(objectType, indexType) *Type`
    /// (checker.go:27457).
    pub(crate) fn get_indexed_access_type_impl(&mut self, object_type: TypeId, index_type: TypeId) -> TypeId {
        self.get_indexed_access_type_ex_impl(object_type, index_type, AccessFlags::NONE, None, None)
    }

    /// Go: `func (c *Checker) getIndexedAccessTypeEx(objectType, indexType, accessFlags, accessNode, alias) *Type`
    /// (checker.go:27461) — REAL (M6 S10; replaces relater.rs's errorType stub
    /// and typeofsymbol.rs's S5-era seam carrier).
    pub(crate) fn get_indexed_access_type_ex_impl(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: AccessFlags,
        access_node: Option<NodeId>,
        alias: Option<Box<crate::types::TypeAlias>>,
    ) -> TypeId {
        match self.get_indexed_access_type_or_undefined_impl(
            object_type,
            index_type,
            access_flags,
            access_node,
            alias,
        ) {
            Some(result) => result,
            None => {
                if access_node.is_some() {
                    self.error_type()
                } else {
                    self.unknown_type()
                }
            }
        }
    }

    /// Go: `func (c *Checker) getIndexedAccessTypeOrUndefined(objectType, indexType, accessFlags, accessNode, alias) *Type`
    /// (checker.go:27469) — REAL (M6 S10).
    pub(crate) fn get_indexed_access_type_or_undefined_impl(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: AccessFlags,
        access_node: Option<NodeId>,
        alias: Option<Box<crate::types::TypeAlias>>,
    ) -> Option<TypeId> {
        if object_type == self.wildcard_type() || index_type == self.wildcard_type() {
            return Some(self.wildcard_type());
        }
        let object_type = self.get_reduced_type(object_type);
        let mut index_type = index_type;
        let mut access_flags = access_flags;
        // If the object type has a string index signature and no other
        // members we know that the result will always be the type of that
        // index signature (Go's comment).
        if self.is_string_index_signature_only_type(object_type)
            && !self.type_(index_type).flags.intersects(TypeFlags::NULLABLE)
            && self.is_type_assignable_to_kind(
                index_type,
                TypeFlags::STRING.union(TypeFlags::NUMBER),
            )
        {
            index_type = self.string_type();
        }
        // In noUncheckedIndexedAccess mode, indexed access operations that
        // occur in an expression in a read position and resolve to an index
        // signature have 'undefined' included in their type (Go's comment).
        if self.compiler_options.no_unchecked_indexed_access == tsc_core::tristate::Tristate::TSTrue
            && access_flags.intersects(AccessFlags::EXPRESSION_POSITION)
        {
            access_flags |= AccessFlags::INCLUDE_UNDEFINED;
        }
        // A generic T / K pair defers as an indexed access type (Go's
        // higher-order index access; the eager-resolution compat note
        // preserved).
        if self.should_defer_indexed_access_type(object_type, index_type, access_node) {
            if self.type_(object_type).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                return Some(object_type);
            }
            // Defer the operation by creating an indexed access type.
            let persistent_access_flags = access_flags & AccessFlags::PERSISTENT;
            let key = get_indexed_access_key(object_type, index_type, access_flags, alias.as_deref());
            let mut t = self.indexed_access_types.get(&key).copied();
            if t.is_none() {
                let created = self.new_indexed_access_type(object_type, index_type, persistent_access_flags);
                self.type_mut(created).alias = alias;
                self.indexed_access_types.insert(key, created);
                t = Some(created);
            }
            return t;
        }
        // In the following we resolve T[K] to the type of the property in T
        // selected by K (Go's comment; boolean is treated specially).
        let apparent_object_type = self.get_reduced_apparent_type(object_type);
        let index_flags = self.type_(index_type).flags;
        if index_flags.intersects(TypeFlags::UNION) && !index_flags.intersects(TypeFlags::BOOLEAN) {
            let constituents = self.type_(index_type).types().to_vec();
            let mut prop_types: Vec<TypeId> = Vec::with_capacity(constituents.len());
            let mut was_missing_prop = false;
            for t in constituents {
                let mut flags = access_flags;
                if was_missing_prop {
                    flags |= AccessFlags::SUPPRESS_NO_IMPLICIT_ANY_ERROR;
                }
                let prop_type = self.get_property_type_for_index_type(
                    object_type,
                    apparent_object_type,
                    t,
                    index_type,
                    access_node,
                    flags,
                );
                match prop_type {
                    Some(prop_type) => prop_types.push(prop_type),
                    None => {
                        // If there's no error node, we can immediately stop,
                        // since error reporting is off (Go).
                        access_node?;
                        // Otherwise we set a flag and return at the end of
                        // the loop so we still mark all errors (Go).
                        was_missing_prop = true;
                    }
                }
            }
            if was_missing_prop {
                return None;
            }
            if access_flags.intersects(AccessFlags::WRITING) {
                return Some(self.get_intersection_type_ex(prop_types, crate::relater::IntersectionFlags::NONE, alias));
            }
            return Some(self.get_union_type_ex(
                prop_types,
                crate::todo_shapes::UnionReduction::Literal,
                alias,
                None,
            ));
        }
        let mut flags = access_flags;
        flags |= AccessFlags::CACHE_SYMBOL;
        flags |= AccessFlags::REPORT_DEPRECATED;
        self.get_property_type_for_index_type(
            object_type,
            apparent_object_type,
            index_type,
            index_type,
            access_node,
            flags,
        )
    }

    /// Go: `func (c *Checker) getPropertyTypeForIndexType(originalObjectType, objectType, indexType, fullIndexType, accessNode, accessFlags) *Type`
    /// (checker.go:27535 — the 222-line resolver) — REAL (M6 S10).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn get_property_type_for_index_type(
        &mut self,
        original_object_type: TypeId,
        object_type: TypeId,
        index_type: TypeId,
        full_index_type: TypeId,
        access_node: Option<NodeId>,
        access_flags: AccessFlags,
    ) -> Option<TypeId> {
        let access_expression = access_node.filter(|&node| self.store().node(node).kind == Kind::ElementAccessExpression);
        let mut has_prop_name = false;
        let mut prop_name = tsc_ast::INTERNAL_SYMBOL_NAME_MISSING.to_string();
        let access_node_is_private = access_node.is_some_and(|node| tsc_ast::is_private_identifier(self.store().node(node)));
        if !access_node_is_private {
            prop_name = self.get_property_name_from_index(index_type, access_node);
            has_prop_name = prop_name != tsc_ast::INTERNAL_SYMBOL_NAME_MISSING;
        }
        if has_prop_name {
            if access_flags.intersects(AccessFlags::CONTEXTUAL) {
                let t = self
                    .get_type_of_property_of_contextual_type_impl(object_type, &prop_name)
                    .unwrap_or_else(|| self.any_type());
                return Some(t);
            }
            let prop = self.get_property_of_type_impl(object_type, &prop_name);
            if let Some(prop) = prop {
                // Go: the AccessFlagsReportDeprecated arm (isDeprecatedSymbol
                // + isUncalledFunctionReference + addDeprecatedSuggestion)
                // rides the S6 @deprecated JSDoc stub (permissive false) —
                // TODO(port M6 S6): the deprecated-suggestion attachment.
                if let Some(access_expression) = access_expression {
                    let (is_self_type, object_symbol) = {
                        let expression = crate::ast_shims::expression_of(self.store(), access_expression)
                            .unwrap_or(access_expression);
                        let object_symbol = self.type_(object_type).symbol;
                        (self.is_self_type_access(expression, object_symbol), object_symbol)
                    };
                    let _ = object_symbol;
                    self.mark_property_as_referenced(prop, Some(access_expression), is_self_type);
                    let assignment_kind = u::get_assignment_target_kind(self.store(), access_expression);
                    if self.is_assignment_to_readonly_entity(access_expression, prop, assignment_kind) {
                        let prop_string = relater::symbol_to_string(self, prop);
                        self.error(
                            access_expression,
                            &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_READ_ONLY_PROPERTY,
                            &[prop_string],
                        );
                        return None;
                    }
                    if access_flags.intersects(AccessFlags::CACHE_SYMBOL) {
                        self.symbol_node_links.get(access_node.expect("checked")).resolved_symbol = Some(prop);
                    }
                    if self.is_this_property_access_in_constructor(access_expression, prop) {
                        return Some(self.auto_type());
                    }
                }
                let prop_type = if access_flags.intersects(AccessFlags::WRITING) {
                    self.get_write_type_of_symbol(prop)
                } else {
                    self.get_type_of_symbol(prop)
                };
                if let Some(access_expression) = access_expression {
                    let assignment_kind = u::get_assignment_target_kind(self.store(), access_expression);
                    if assignment_kind != u::AssignmentKind::Definite {
                        // Go: getFlowTypeOfReference — the flow narrowing is
                        // W3's; the checked property type is the permissive
                        // answer (TODO(port M6 W3)).
                        return Some(prop_type);
                    }
                }
                if access_node.is_some_and(|node| self.store().node(node).kind == Kind::IndexedAccessType)
                    && self.contains_missing_type(prop_type)
                {
                    let undefined_type = self.undefined_type();
                    return Some(self.get_union_type(&[prop_type, undefined_type]));
                }
                return Some(prop_type);
            }
            // Tuple out-of-range element access (Go's everyType(objectType,
            // isTupleType) && isNumericLiteralName(propName) arm).
            let all_tuples = {
                let constituents: Vec<TypeId> =
                    if self.type_(object_type).flags.intersects(TypeFlags::UNION) {
                        self.type_(object_type).types().to_vec()
                    } else {
                        vec![object_type]
                    };
                constituents
                    .iter()
                    .all(|&t| crate::typenodes::is_tuple_type(self, t))
            };
            if all_tuples && u::is_numeric_literal_name(&prop_name) {
                let index = parse_numeric_name(&prop_name);
                let no_variable_elements = {
                    let constituents: Vec<TypeId> =
                        if self.type_(object_type).flags.intersects(TypeFlags::UNION) {
                            self.type_(object_type).types().to_vec()
                        } else {
                            vec![object_type]
                        };
                    constituents.iter().all(|&t| {
                        let target = self.type_(t).target_tuple_type();
                        !self
                            .type_(target)
                            .as_tuple_type()
                            .map(|d| d.combined_flags.intersects(types::ElementFlags::VARIABLE))
                            .unwrap_or(false)
                    })
                };
                if access_node.is_some()
                    && no_variable_elements
                    && !access_flags.intersects(AccessFlags::ALLOW_MISSING)
                {
                    let index_node = get_index_node_for_access_expression(self, access_node);
                    if crate::typenodes::is_tuple_type(self, object_type) {
                        if index < 0.0 {
                            self.error(
                                index_node,
                                &A_TUPLE_TYPE_CANNOT_BE_INDEXED_WITH_A_NEGATIVE_VALUE,
                                &[],
                            );
                            return Some(self.undefined_type());
                        }
                        let type_string = self.type_to_string(object_type);
                        let arity = relater::get_type_reference_arity(self, object_type);
                        self.error(
                            index_node,
                            &TUPLE_TYPE_0_OF_LENGTH_1_HAS_NO_ELEMENT_AT_INDEX_2,
                            &[type_string, arity.to_string(), prop_name.clone()],
                        );
                    } else {
                        let type_string = self.type_to_string(object_type);
                        self.error(
                            index_node,
                            &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                            &[prop_name.clone(), type_string],
                        );
                    }
                }
                if index >= 0.0 {
                    let number_type = self.number_type();
                    let index_info = self.get_index_info_of_type_impl(object_type, number_type);
                    self.error_if_writing_to_readonly_index(index_info, object_type, access_expression);
                    let undefined_like = if access_flags.intersects(AccessFlags::INCLUDE_UNDEFINED) {
                        Some(self.missing_type())
                    } else {
                        None
                    };
                    return Some(crate::typenodes::get_tuple_element_type_out_of_start_count(
                        self,
                        object_type,
                        tsc_jsnum::Number::from(index),
                        undefined_like,
                    ));
                }
            }
        }
        if !self.type_(index_type).flags.intersects(TypeFlags::NULLABLE)
            && self.is_type_assignable_to_kind(
                index_type,
                TypeFlags::STRING_LIKE
                    .union(TypeFlags::NUMBER_LIKE)
                    .union(TypeFlags::ESSYMBOL_LIKE),
            )
        {
            if self.type_(object_type).flags.intersects(TypeFlags::ANY.union(TypeFlags::NEVER)) {
                return Some(object_type);
            }
            // If no index signature is applicable, we default to the string
            // index signature (Go's comment).
            let mut index_info = self.get_applicable_index_info_impl(object_type, index_type);
            if index_info.is_none() {
                let string_type = self.string_type();
                index_info = self.get_index_info_of_type_impl(object_type, string_type);
            }
            if let Some(index_info) = index_info {
                let key_type = self.index_info_key_type(index_info);
                let number_type = self.number_type();
                if access_flags.intersects(AccessFlags::NO_INDEX_SIGNATURES)
                    && Some(key_type) != Some(number_type)
                {
                    if let Some(access_expression) = access_expression {
                        if access_flags.intersects(AccessFlags::WRITING) {
                            let type_string = self.type_to_string(original_object_type);
                            self.error(
                                access_expression,
                                &TYPE_0_IS_GENERIC_AND_CAN_ONLY_BE_INDEXED_FOR_READING,
                                &[type_string],
                            );
                        } else {
                            let index_string = self.type_to_string(index_type);
                            let object_string = self.type_to_string(original_object_type);
                            self.error(
                                access_expression,
                                &TYPE_0_CANNOT_BE_USED_TO_INDEX_TYPE_1,
                                &[index_string, object_string],
                            );
                        }
                    }
                    return None;
                }
                if let Some(node) = access_node {
                    let string_type = self.string_type();
                    if Some(key_type) == Some(string_type)
                        && !self.is_type_assignable_to_kind(
                            index_type,
                            TypeFlags::STRING.union(TypeFlags::NUMBER),
                        )
                    {
                        let index_node = get_index_node_for_access_expression(self, Some(node));
                        let index_string = self.type_to_string(index_type);
                        self.error(
                            index_node,
                            &TYPE_0_CANNOT_BE_USED_AS_AN_INDEX_TYPE,
                            &[index_string],
                        );
                        let value_type = self.index_info_value_type(index_info);
                        if access_flags.intersects(AccessFlags::INCLUDE_UNDEFINED) {
                            let missing = self.missing_type();
                            return Some(self.get_union_type(&[value_type, missing]));
                        }
                        return Some(value_type);
                    }
                }
                self.error_if_writing_to_readonly_index(Some(index_info), object_type, access_expression);
                // When accessing an enum object with its own type, undefined
                // shouldn't be included in the result type (Go's comment).
                let enum_self_index = self
                    .type_(object_type)
                    .symbol
                    .is_some_and(|object_symbol| {
                        self.program_symbol(object_symbol)
                            .flags
                            .intersects(SymbolFlags::REGULAR_ENUM.union(SymbolFlags::CONST_ENUM))
                    })
                    && self
                        .type_(index_type)
                        .symbol
                        .is_some_and(|index_symbol| {
                            self.type_(index_type).flags.intersects(TypeFlags::ENUM_LITERAL)
                                && self.program_symbol(index_symbol).parent
                                    == self.type_(object_type).symbol
                        });
                if access_flags.intersects(AccessFlags::INCLUDE_UNDEFINED) && !enum_self_index {
                    let missing = self.missing_type();
                    let value_type = self.index_info_value_type(index_info);
                    return Some(self.get_union_type(&[value_type, missing]));
                }
                return Some(self.index_info_value_type(index_info));
            }
            if self.type_(index_type).flags.intersects(TypeFlags::NEVER) {
                return Some(self.never_type());
            }
            if u::is_js_literal_type(self, object_type) {
                return Some(self.any_type());
            }
            if let Some(access_expression) = access_expression {
                if !crate::spine::is_const_enum_object_type(self, object_type) {
                    if relater::is_object_literal_type(self, object_type) {
                        let index_flags = self.type_(index_type).flags;
                        if self.no_implicit_any
                            && index_flags
                                .intersects(TypeFlags::STRING_LITERAL.union(TypeFlags::NUMBER_LITERAL))
                        {
                            let value_string = literal_value_string(self, index_type);
                            let type_string = self.type_to_string(object_type);
                            let diagnostic = u::new_diagnostic_for_node(
                                self.store(),
                                access_expression,
                                &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                                vec![value_string, type_string],
                            );
                            self.add_diagnostic(diagnostic);
                            return Some(self.undefined_type());
                        } else if index_flags.intersects(TypeFlags::NUMBER.union(TypeFlags::STRING)) {
                            let properties = self
                                .get_properties_of_object_type_impl(object_type);
                            let mut type_list: Vec<TypeId> =
                                properties.into_iter().map(|prop| self.get_type_of_symbol(prop)).collect();
                            let undefined_type = self.undefined_type();
                            type_list.push(undefined_type);
                            return Some(self.get_union_type(&type_list));
                        }
                    }
                    // Go: `objectType.symbol == c.globalThisSymbol &&
                    // hasPropName && Exports[propName] != nil && BlockScoped`
                    // (checker.go:27679) — the whole conjunction gates the
                    // 2339; a globalThis access that fails any leg falls to
                    // the noImplicitAny index-expression ladder. PORT(FCS
                    // tranche 14): the former nesting made a non-block-scoped
                    // globalThis hit terminal (skipping the ladder Go takes).
                    let is_global_this = self
                        .type_(object_type)
                        .symbol
                        .is_some_and(|s| Some(s) == self.global_this_symbol);
                    let export_symbol = self
                        .global_this_symbol
                        .and_then(|s| self.program_symbol(s).exports.get(&prop_name).copied());
                    let blocked = export_symbol.is_some_and(|s| {
                        self.program_symbol(s).flags.intersects(SymbolFlags::BLOCK_SCOPED)
                    });
                    if is_global_this && has_prop_name && blocked {
                        let type_string = self.type_to_string(object_type);
                        self.error(
                            access_expression,
                            &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                            &[prop_name.clone(), type_string],
                        );
                    } else if self.no_implicit_any
                        && !access_flags.intersects(AccessFlags::SUPPRESS_NO_IMPLICIT_ANY_ERROR)
                    {
                        return self.report_index_expression_error(
                            original_object_type,
                            object_type,
                            index_type,
                            full_index_type,
                            has_prop_name,
                            prop_name,
                            access_expression,
                        );
                    }
                    return None;
                }
            }
        }
        if access_flags.intersects(AccessFlags::ALLOW_MISSING)
            && relater::is_object_literal_type(self, object_type)
        {
            return Some(self.undefined_type());
        }
        if u::is_js_literal_type(self, object_type) {
            return Some(self.any_type());
        }
        if let Some(node) = access_node {
            let index_node = get_index_node_for_access_expression(self, Some(node));
            let index_flags = self.type_(index_type).flags;
            if self.store().node(index_node).kind != Kind::BigIntLiteral
                && index_flags
                    .intersects(TypeFlags::STRING_LITERAL.union(TypeFlags::NUMBER_LITERAL))
            {
                let value_string = literal_value_string(self, index_type);
                let type_string = self.type_to_string(object_type);
                self.error(
                    index_node,
                    &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                    &[value_string, type_string],
                );
            } else if index_flags.intersects(TypeFlags::STRING.union(TypeFlags::NUMBER)) {
                let object_string = self.type_to_string(object_type);
                let index_string = self.type_to_string(index_type);
                self.error(
                    index_node,
                    &TYPE_0_HAS_NO_MATCHING_INDEX_SIGNATURE_FOR_TYPE_1,
                    &[object_string, index_string],
                );
            } else {
                let type_string = if self.store().node(index_node).kind == Kind::BigIntLiteral {
                    "bigint".to_string()
                } else {
                    self.type_to_string(index_type)
                };
                self.error(index_node, &TYPE_0_CANNOT_BE_USED_AS_AN_INDEX_TYPE, &[type_string]);
            }
        }
        if u::is_type_any(Some(self.type_(index_type))) {
            return Some(index_type);
        }
        None
    }

    /// Go: the noImplicitAny error ladder at the end of the object-literal/
    /// access-expression arm of getPropertyTypeForIndexType (checker.go
    /// 27695-27730) — surfaced as a helper to keep the parent's flow.
    #[allow(clippy::too_many_arguments)]
    fn report_index_expression_error(
        &mut self,
        original_object_type: TypeId,
        object_type: TypeId,
        index_type: TypeId,
        full_index_type: TypeId,
        has_prop_name: bool,
        prop_name: String,
        access_expression: NodeId,
    ) -> Option<TypeId> {
        if has_prop_name && self.type_has_static_property(&prop_name, object_type) {
            let type_name = self.type_to_string(object_type);
            let access_text = tsc_ast::node_text(
                self.store(),
                self.store()
                    .node(access_expression)
                    .as_element_access_expression()
                    .expect("ElementAccessExpression data")
                    .argument_expression,
            )
            .to_string();
            self.error(
                access_expression,
                &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_TO_ACCESS_THE_STATIC_MEMBER_2_INSTEAD,
                &[prop_name.clone(), type_name.clone(), format!("{type_name}[{access_text}]")],
            );
            return None;
        }
        let number_type = self.number_type();
        if self.get_index_type_of_type_impl(object_type, number_type).is_some() {
            let argument_expression = self
                .store()
                .node(access_expression)
                .as_element_access_expression()
                .expect("ElementAccessExpression data")
                .argument_expression;
            self.error(
                argument_expression,
                &ELEMENT_IMPLICITLY_HAS_AN_ANY_TYPE_BECAUSE_INDEX_EXPRESSION_IS_NOT_OF_TYPE_NUMBER,
                &[],
            );
            return None;
        }
        let mut suggestion = String::new();
        if has_prop_name {
            suggestion = self.get_suggestion_for_nonexistent_property(&prop_name, object_type);
        }
        if !suggestion.is_empty() {
            let type_string = self.type_to_string(object_type);
            let argument_expression = self
                .store()
                .node(access_expression)
                .as_element_access_expression()
                .expect("ElementAccessExpression data")
                .argument_expression;
            self.error(
                argument_expression,
                &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_2,
                &[prop_name.clone(), type_string, suggestion],
            );
            return None;
        }
        suggestion = self.get_suggestion_for_nonexistent_index_signature(object_type, access_expression, index_type);
        if !suggestion.is_empty() {
            let type_string = self.type_to_string(object_type);
            self.error(
                access_expression,
                &ELEMENT_IMPLICITLY_HAS_AN_ANY_TYPE_BECAUSE_TYPE_0_HAS_NO_INDEX_SIGNATURE_DID_YOU_MEAN_TO_CALL_1,
                &[type_string, suggestion],
            );
            return None;
        }
        // The chained diagnostic ladder (checker.go 27716-27728).
        let index_flags = self.type_(index_type).flags;
        let mut diagnostic: Option<tsc_ast::Diagnostic> = None;
        if index_flags.intersects(TypeFlags::ENUM_LITERAL) {
            let index_string = self.type_to_string(index_type);
            let type_string = self.type_to_string(object_type);
            diagnostic = Some(u::new_diagnostic_for_node(
                self.store(),
                access_expression,
                &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                vec![format!("[{index_string}]"), type_string],
            ));
        } else if index_flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
            let symbol = self.type_(index_type).symbol;
            let symbol_name = symbol
                .map(|s| self.get_fully_qualified_name(s))
                .unwrap_or_default();
            let type_string = self.type_to_string(object_type);
            diagnostic = Some(u::new_diagnostic_for_node(
                self.store(),
                access_expression,
                &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                vec![format!("[{symbol_name}]"), type_string],
            ));
        } else if index_flags.intersects(TypeFlags::STRING_LITERAL | TypeFlags::NUMBER_LITERAL) {
            let value_string = literal_value_string(self, index_type);
            let type_string = self.type_to_string(object_type);
            diagnostic = Some(u::new_diagnostic_for_node(
                self.store(),
                access_expression,
                &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                vec![value_string, type_string],
            ));
        } else if index_flags.intersects(TypeFlags::NUMBER.union(TypeFlags::STRING)) {
            let index_string = self.type_to_string(index_type);
            let type_string = self.type_to_string(object_type);
            diagnostic = Some(u::new_diagnostic_for_node(
                self.store(),
                access_expression,
                &NO_INDEX_SIGNATURE_WITH_A_PARAMETER_OF_TYPE_0_WAS_FOUND_ON_TYPE_1,
                vec![index_string, type_string],
            ));
        }
        if let Some(diagnostic) = diagnostic {
            let full_index_string = self.type_to_string(full_index_type);
            let object_string = self.type_to_string(original_object_type);
            let chained = u::new_diagnostic_chain_for_node(
                Some(&diagnostic),
                self.store(),
                access_expression,
                &ELEMENT_IMPLICITLY_HAS_AN_ANY_TYPE_BECAUSE_EXPRESSION_OF_TYPE_0_CAN_T_BE_USED_TO_INDEX_TYPE_1,
                vec![full_index_string, object_string],
            );
            self.add_diagnostic(chained);
        }
        None
    }

    /// Go: `func (c *Checker) typeHasStaticProperty(propName, containingType) bool`
    /// (checker.go:27735).
    fn type_has_static_property(&mut self, prop_name: &str, containing_type: TypeId) -> bool {
        if let Some(symbol) = self.type_(containing_type).symbol {
            let symbol_type = self.get_type_of_symbol(symbol);
            return self.get_property_of_type_impl(symbol_type, prop_name).is_some_and(|prop| {
                let value_declaration = self.program_symbol(prop).value_declaration;
                value_declaration.is_some_and(|d| {
                    self.store().node(d).modifier_flags().intersects(tsc_ast::ModifierFlags::STATIC)
                })
            });
        }
        false
    }

    /// Go: `func (c *Checker) getSuggestionForNonexistentProperty(name, containingType) string`
    /// (checker.go:27743) — REAL over the scanner's Levenshtein engine
    /// (core.GetSpellingSuggestion's string form). PORT: Go's
    /// getSpellingSuggestionKey walks declaration names too; the property-name
    /// key (the common case) matches.
    pub(crate) fn get_suggestion_for_nonexistent_property(
        &mut self,
        name: &str,
        containing_type: TypeId,
    ) -> String {
        let props = self.get_properties_of_type_impl(containing_type);
        let prop_names: Vec<String> = props
            .iter()
            .filter(|prop| self.program_symbol(**prop).flags.intersects(SymbolFlags::VALUE))
            .map(|prop| self.program_symbol(*prop).name.clone())
            .collect();
        tsc_scanner::go_shims::get_spelling_suggestion_for_strings(name, prop_names.iter().map(|s| s.as_str()))
    }

    /// Go: `func (c *Checker) getSuggestionForNonexistentIndexSignature(objectType, expr, keyedType) string`
    /// (checker.go:27750).
    pub(crate) fn get_suggestion_for_nonexistent_index_signature(
        &mut self,
        object_type: TypeId,
        expr: NodeId,
        keyed_type: TypeId,
    ) -> String {
        // check if object type has setter or getter (Go's comment)
        let has_prop = |c: &mut Checker, name: &str| -> bool {
            let Some(prop) = c.get_property_of_object_type_impl(object_type, name) else {
                return false;
            };
            let prop_type = c.get_type_of_symbol(prop);
            let Some(signature) = c.get_single_call_signature(prop_type) else {
                return false;
            };
            c.get_min_argument_count(signature) >= 1 && {
                let first = c.get_type_at_position(signature, 0);
                c.is_type_assignable_to(keyed_type, first)
            }
        };
        let suggested_method = if tsc_binder::ast_shims::is_assignment_target(self.store(), expr) {
            "set"
        } else {
            "get"
        };
        if !has_prop(self, suggested_method) {
            return String::new();
        }
        let expression = crate::ast_shims::expression_of(self.store(), expr);
        let suggestion = expression
            .map(|e| u::try_get_property_access_or_identifier_to_string(self.store(), e))
            .unwrap_or_default();
        if suggestion.is_empty() {
            return suggested_method.to_string();
        }
        format!("{suggestion}.{suggested_method}")
    }

    /// Go: `func (c *Checker) getSuggestedTypeForNonexistentStringLiteralType(source, target) *Type`
    #[allow(dead_code)] // TODO(port M6 W2): relater.go's string-literal
                        // assignability-failure suggestion arm consumes it
    /// (checker.go:27768) — REAL over the scanner's Levenshtein engine. PORT:
    /// Go's GetSpellingSuggestionWithMaxCandidateCount carries the same
    /// (distance, then candidate-identity) selection as the string form; the
    /// port resolves the best string then maps it back to its type (the
    /// 1000-candidate cap rides the slice).
    pub(crate) fn get_suggested_type_for_nonexistent_string_literal_type(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Option<TypeId> {
        let constituents = self.type_(target).types().to_vec();
        let mut candidates: Vec<(String, TypeId)> = Vec::new();
        for t in constituents.iter().take(1000) {
            if self.type_(*t).flags.intersects(TypeFlags::STRING_LITERAL) {
                candidates.push((relater::get_string_literal_value(self, *t), *t));
            }
        }
        let source_value = relater::get_string_literal_value(self, source);
        let best = tsc_scanner::go_shims::get_spelling_suggestion_for_strings(
            &source_value,
            candidates.iter().map(|(s, _)| s.as_str()),
        );
        if best.is_empty() {
            return None;
        }
        candidates
            .into_iter()
            .find(|(s, _)| *s == best)
            .map(|(_, t)| t)
    }

    /// Go: `func (c *Checker) errorIfWritingToReadonlyIndex(indexInfo, objectType, accessExpression)`
    /// (checker.go:27785).
    pub(crate) fn error_if_writing_to_readonly_index(
        &mut self,
        index_info: Option<types::IndexInfoId>,
        object_type: TypeId,
        access_expression: Option<NodeId>,
    ) {
        let Some(info) = index_info else { return };
        let is_readonly = self.index_infos[info.0 as usize].is_readonly;
        if !is_readonly {
            return;
        }
        if let Some(access_expression) = access_expression
            .filter(|&node| {
                tsc_binder::ast_shims::is_assignment_target(self.store(), node) || u::is_delete_target(self.store(), node)
            })
        {
            let type_string = self.type_to_string(object_type);
            self.error(
                access_expression,
                &INDEX_SIGNATURE_IN_TYPE_0_ONLY_PERMITS_READING,
                &[type_string],
            );
        }
    }

    /// Go: `func (c *Checker) getPropertyNameFromIndex(indexType, accessNode) string`
    /// (checker.go:27835) — REAL (M6 S10; replaces expressions.rs's
    /// INTERNAL_SYMBOL_NAME_MISSING stub answer).
    pub(crate) fn get_property_name_from_index(&mut self, index_type: TypeId, access_node: Option<NodeId>) -> String {
        if u::is_type_usable_as_property_name(self.type_(index_type)) {
            return u::get_property_name_from_type(self.type_(index_type));
        }
        if let Some(node) = access_node
            .filter(|&node| crate::utilities::is_property_name(self.store(), node))
        {
            return ast_shims_property_name(self, node);
        }
        tsc_ast::INTERNAL_SYMBOL_NAME_MISSING.to_string()
    }

    /// Go: `func (c *Checker) isStringIndexSignatureOnlyType(t) bool` — the
    /// NewChecker closure form (checker.go:1307's isStringIndexSignatureOnlyType
    /// over isStringIndexSignatureOnlyTypeWorker, checker.go:27838).
    pub(crate) fn is_string_index_signature_only_type(&mut self, t: TypeId) -> bool {
        self.is_string_index_signature_only_type_worker(t)
    }

    /// Go: `func (c *Checker) isStringIndexSignatureOnlyTypeWorker(t) bool` (checker.go:27840).
    fn is_string_index_signature_only_type_worker(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::OBJECT)
            && !relater::is_generic_mapped_type(self, t)
            && self.get_properties_of_type_impl(t).is_empty()
            && self.get_index_infos_of_type_impl(t).len() == 1
        {
            let string_type = self.string_type();
            return self.get_index_info_of_type_impl(t, string_type).is_some();
        }
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let constituents = self.type_(t).types().to_vec();
            return constituents.iter().all(|&s| self.is_string_index_signature_only_type(s));
        }
        false
    }

    /// Go: `func (c *Checker) shouldDeferIndexedAccessType(objectType, indexType, accessNode) bool`
    /// (checker.go:27846).
    pub(crate) fn should_defer_indexed_access_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_node: Option<NodeId>,
    ) -> bool {
        if relater::is_generic_index_type(self, index_type) {
            return true;
        }
        let access_is_index_type_node =
            access_node.is_some_and(|node| self.store().node(node).kind == Kind::IndexedAccessType);
        if access_node.is_some() && !access_is_index_type_node {
            if !crate::typenodes::is_generic_tuple_type(self, object_type) {
                return false;
            }
            let target = self.type_(object_type).target_tuple_type();
            let total_fixed = self
                .type_(target)
                .as_tuple_type()
                .map(crate::typenodes::total_fixed_element_count_of)
                .unwrap_or(0);
            return !index_type_less_than(self, index_type, total_fixed);
        }
        if relater::is_generic_object_type(self, object_type) {
            if crate::typenodes::is_tuple_type(self, object_type) {
                let target = self.type_(object_type).target_tuple_type();
                let total_fixed = self
                    .type_(target)
                    .as_tuple_type()
                    .map(crate::typenodes::total_fixed_element_count_of)
                    .unwrap_or(0);
                if index_type_less_than(self, index_type, total_fixed) {
                    return false;
                }
            }
            return true;
        }
        crate::typenodes::is_generic_reducible_type(self, object_type)
    }

    // ────────────────────────────────────────────────────────────────────────
    // The constraint-of-indexed-access family (Go checker.go 17605-17632)
    // — the Of/From pair lives in typeofsymbol.rs (the declared-constraint
    // family it belongs to); the From body's callees
    // (isMappedTypeGenericIndexedAccess / substituteIndexedMappedType /
    // getSimplifiedTypeOrConstraint / getIndexedAccessTypeOrUndefined) are
    // this module's.
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) isMappedTypeGenericIndexedAccess(t) bool` (checker.go:27539
    /// region) — REAL (M6 S10; replaces relater.rs's false stub).
    pub(crate) fn is_mapped_type_generic_indexed_access(&mut self, t: TypeId) -> bool {
        if !self.type_(t).flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return false;
        }
        let (object_type, index_type) = {
            let d = self.type_(t).as_indexed_access_type().expect("IndexedAccessType data");
            (
                d.object_type.expect("objectType"),
                d.index_type.expect("indexType"),
            )
        };
        self.type_(object_type).object_flags.intersects(ObjectFlags::MAPPED)
            && !relater::is_generic_mapped_type(self, object_type)
            && relater::is_generic_index_type(self, index_type)
            && !relater::get_mapped_type_modifiers(self, object_type)
                .intersects(crate::relater::MappedTypeModifiers::EXCLUDE_OPTIONAL)
            && self
                .type_(object_type)
                .as_mapped_type()
                .and_then(|m| m.declaration)
                .is_some_and(|declaration| {
                    self.store()
                        .node(declaration)
                        .as_mapped_type_node()
                        .expect("MappedTypeNode data")
                        .name_type
                        .is_none()
                })
    }

    // ────────────────────────────────────────────────────────────────────────
    // The substitution family (Go checker.go 29839-29860 + 27357-27360)
    // ────────────────────────────────────────────────────────────────────────

    // Go: `func (c *Checker) getSubstitutionIntersection(t) *Type` (checker.go:27357)
    // — inference.rs owns the REAL body (same name; this module's
    // computeBaseConstraint consumers call the free fn).

    /// Go: `func (c *Checker) substituteIndexedMappedType(objectType, index) *Type`
    /// (checker.go:29839) — REAL (M6 S10; replaces typeofsymbol.rs's banner).
    pub(crate) fn substitute_indexed_mapped_type(&mut self, object_type: TypeId, index: TypeId) -> TypeId {
        let type_parameter = self.get_type_parameter_from_mapped_type_impl(object_type);
        let mapper = self
            .type_(object_type)
            .as_mapped_type()
            .expect("MappedType data")
            .base
            .mapper;
        let simple = self.new_simple_type_mapper(type_parameter, index);
        let template_mapper = self.combine_type_mappers(mapper, Some(simple));
        let mapped_target = self
            .type_(object_type)
            .as_mapped_type()
            .and_then(|m| m.base.target)
            .unwrap_or(object_type);
        let template_type = self.get_template_type_from_mapped_type_impl(mapped_target);
        let instantiated_template_type = self.instantiate_type(template_type, template_mapper.expect("combined mapper"));
        let mut is_optional = self.get_mapped_type_optionality_impl(object_type) > 0;
        if !is_optional {
            if self.is_generic_type(object_type) {
                let modifiers_type = self.get_modifiers_type_from_mapped_type_impl(object_type);
                is_optional = self.get_combined_mapped_type_optionality_impl(modifiers_type) > 0;
            } else {
                is_optional = self.could_access_optional_property(object_type, index);
            }
        }
        self.add_optionality_ex(instantiated_template_type, true /*isProperty*/, is_optional)
    }

    /// Go: `func (c *Checker) couldAccessOptionalProperty(objectType, indexType) bool`
    /// (checker.go:29854).
    pub(crate) fn could_access_optional_property(&mut self, object_type: TypeId, index_type: TypeId) -> bool {
        let index_constraint = relater::get_base_constraint_of_type(self, index_type);
        let Some(index_constraint) = index_constraint else {
            return false;
        };
        let props = self.get_properties_of_type_impl(object_type);
        props.iter().any(|&p| {
            self.program_symbol(p).flags.intersects(SymbolFlags::OPTIONAL) && {
                let literal = self
                    .get_literal_type_from_property_impl(
                        p,
                        TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                        false,
                    )
                    .unwrap_or_else(|| self.never_type());
                self.is_type_assignable_to(literal, index_constraint)
            }
        })
    }

    // ────────────────────────────────────────────────────────────────────────
    // The simplified family (Go checker.go 28436-28562)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getSimplifiedType(t, writing) *Type` (checker.go:28436)
    /// — REAL (M6 S10; replaces relater.rs's identity stub).
    pub(crate) fn get_simplified_type_impl(&mut self, t: TypeId, writing: bool) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return self.get_simplified_indexed_access_type(t, writing);
        }
        if flags.intersects(TypeFlags::CONDITIONAL) {
            return self.get_simplified_conditional_type(t, writing);
        }
        t
    }

    /// Go: `func (c *Checker) getSimplifiedIndexedAccessType(t, writing) *Type`
    /// (checker.go:28453) — the cachedTypes intern (reading/writing axes).
    pub(crate) fn get_simplified_indexed_access_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        let kind = if writing {
            CachedTypeKind::IndexedAccessForWriting
        } else {
            CachedTypeKind::IndexedAccessForReading
        };
        let key = CachedTypeKey { kind, type_id: t.0 };
        if let Some(cached) = self.cached_types.get(&key).copied().flatten() {
            return if cached == self.circular_constraint_type() { t } else { cached };
        }
        // Go seeds the cache with `t` itself before recursing (the
        // circularity marker).
        self.cached_types.insert(key, Some(t));
        let result = self.get_simplified_indexed_access_type_worker(t, writing);
        let result = if result != t {
            // If the simplification is a union type that includes t, remove
            // t from the type (Go's comment).
            self.remove_type(result, t)
        } else {
            result
        };
        self.cached_types.insert(key, Some(result));
        result
    }

    /// Go: `func (c *Checker) getSimplifiedIndexedAccessTypeWorker(t, writing) *Type`
    /// (checker.go:28469).
    pub(crate) fn get_simplified_indexed_access_type_worker(&mut self, t: TypeId, writing: bool) -> TypeId {
        // We recursively simplify the object type as it may in turn be an
        // indexed access type (Go's comment).
        let (object_type, index_type) = {
            let d = self.type_(t).as_indexed_access_type().expect("IndexedAccessType data");
            (
                d.object_type.expect("objectType"),
                d.index_type.expect("indexType"),
            )
        };
        let object_type = self.get_simplified_type_impl(object_type, writing);
        let index_type = self.get_simplified_type_impl(index_type, writing);
        // T[A | B] -> T[A] | T[B] (reading) / T[A] & T[B] (writing).
        if let Some(distributed) = self.distribute_object_over_index_type(object_type, index_type, writing) {
            return distributed;
        }
        // Only do the inner distributions if the index can no longer be
        // instantiated (Go's comment).
        if !self.type_(index_type).flags.intersects(TypeFlags::INSTANTIABLE) {
            if let Some(distributed) = self.distribute_index_over_object_type(object_type, index_type, writing) {
                return distributed;
            }
        }
        // A generic tuple type indexed by a number simplifies to the
        // combined type of the (non-)fixed elements (Go's comment).
        if crate::typenodes::is_generic_tuple_type(self, object_type)
            && self.type_(index_type).flags.intersects(TypeFlags::NUMBER_LIKE)
        {
            let start = if self.type_(index_type).flags.intersects(TypeFlags::NUMBER) {
                0
            } else {
                let target = self.type_(object_type).target_tuple_type();
                self.type_(target)
                    .as_tuple_type()
                    .map(|d| d.fixed_length as usize)
                    .unwrap_or(0)
            };
            if let Some(element_type) = crate::typenodes::get_element_type_of_slice_of_tuple_type(
                self,
                object_type,
                start,
                0, /*endSkipCount*/
                writing,
                false,
            ) {
                return element_type;
            }
        }
        // { [P in K]: E }[X] with generic K (or assignable-remap N)
        // instantiates E with X substituted for P (Go's comment).
        if relater::is_generic_mapped_type(self, object_type)
            && self.get_mapped_type_name_type_kind(object_type) != MappedTypeNameTypeKind::Remapping
        {
            let substituted = self.substitute_indexed_mapped_type(object_type, index_type);
            return self.map_type(
                substituted,
                &mut |c, u| Some(c.get_simplified_type_impl(u, writing)),
            ).unwrap_or(substituted);
        }
        t
    }

    /// Go: `func (c *Checker) distributeObjectOverIndexType(objectType, indexType, writing) *Type`
    /// (checker.go:28517).
    pub(crate) fn distribute_object_over_index_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        writing: bool,
    ) -> Option<TypeId> {
        // T[A | B] -> T[A] | T[B] (reading) / T[A | B] -> T[A] & T[B] (writing)
        if !self.type_(index_type).flags.intersects(TypeFlags::UNION) {
            return None;
        }
        let constituents = self.type_(index_type).types().to_vec();
        let mut types: Vec<TypeId> = Vec::with_capacity(constituents.len());
        for t in constituents {
            let accessed = self.get_indexed_access_type_impl(object_type, t);
            types.push(self.get_simplified_type_impl(accessed, writing));
        }
        if writing {
            return Some(self.get_intersection_type(&types));
        }
        Some(self.get_union_type(&types))
    }

    /// Go: `func (c *Checker) distributeIndexOverObjectType(objectType, indexType, writing) *Type`
    /// (checker.go:28530).
    pub(crate) fn distribute_index_over_object_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        writing: bool,
    ) -> Option<TypeId> {
        let object_flags = self.type_(object_type).flags;
        if !(object_flags.intersects(TypeFlags::UNION)
            || object_flags.intersects(TypeFlags::INTERSECTION)
                && !self.should_defer_index_type_impl(object_type, IndexFlags::NONE))
        {
            return None;
        }
        let constituents = self.type_(object_type).types().to_vec();
        let mut types: Vec<TypeId> = Vec::with_capacity(constituents.len());
        for t in constituents {
            let accessed = self.get_indexed_access_type_impl(t, index_type);
            types.push(self.get_simplified_type_impl(accessed, writing));
        }
        if object_flags.intersects(TypeFlags::INTERSECTION) || writing {
            return Some(self.get_intersection_type(&types));
        }
        Some(self.get_union_type(&types))
    }

    /// Go: `func (c *Checker) getSimplifiedConditionalType(t, writing) *Type`
    /// (checker.go:28547) — the `T extends U ? T : never` simplifications.
    pub(crate) fn get_simplified_conditional_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        let (check_type, extends_type) = {
            let d = self.type_(t).as_conditional_type().expect("ConditionalType data");
            (
                d.check_type.expect("checkType"),
                d.extends_type.expect("extendsType"),
            )
        };
        let true_type = self.get_true_type_from_conditional_type_impl(t);
        let false_type = self.get_false_type_from_conditional_type_impl(t);
        let actual_true = crate::inference::get_actual_type_variable(self, true_type);
        let actual_false = crate::inference::get_actual_type_variable(self, false_type);
        let actual_check = crate::inference::get_actual_type_variable(self, check_type);
        // Simplifications for types of the form `T extends U ? T : never` and
        // `T extends U ? never : T` (Go's comment).
        if self.type_(false_type).flags.intersects(TypeFlags::NEVER) && actual_true == actual_check {
            if self.type_(check_type).flags.intersects(TypeFlags::ANY) || {
                let restricted_check = self.get_restrictive_instantiation(check_type);
                let restricted_extends = self.get_restrictive_instantiation(extends_type);
                self.is_type_assignable_to(restricted_check, restricted_extends)
            } {
                return self.get_simplified_type_impl(true_type, writing);
            } else if self.is_intersection_empty(check_type, extends_type) {
                return self.never_type();
            }
        } else if self.type_(true_type).flags.intersects(TypeFlags::NEVER) && actual_false == actual_check {
            if !self.type_(check_type).flags.intersects(TypeFlags::ANY) && {
                let restricted_check = self.get_restrictive_instantiation(check_type);
                let restricted_extends = self.get_restrictive_instantiation(extends_type);
                self.is_type_assignable_to(restricted_check, restricted_extends)
            } {
                return self.never_type();
            } else if self.type_(check_type).flags.intersects(TypeFlags::ANY)
                || self.is_intersection_empty(check_type, extends_type)
            {
                return self.get_simplified_type_impl(false_type, writing);
            }
        }
        t
    }

    /// Go: `func (c *Checker) isIntersectionEmpty(type1, type2) bool` (checker.go:28571)
    /// — invokes union simplification logic to determine if an intersection
    /// is considered empty as a union constituent.
    pub(crate) fn is_intersection_empty(&mut self, type1: TypeId, type2: TypeId) -> bool {
        let Some(intersected) = self.intersect_types(Some(type1), Some(type2)) else {
            return true;
        };
        let never = self.never_type();
        let union = self.get_union_type(&[intersected, never]);
        self.type_(union).flags.intersects(TypeFlags::NEVER)
    }

    /// Go: `func (c *Checker) getSimplifiedTypeOrConstraint(t) *Type` (checker.go:28575)
    /// — REAL (M6 S10; replaces relater.rs's constraint-only stub).
    pub(crate) fn get_simplified_type_or_constraint_impl(&mut self, t: TypeId) -> Option<TypeId> {
        let simplified = self.get_simplified_type_impl(t, false /*writing*/);
        if simplified != t {
            return Some(simplified);
        }
        relater::get_constraint_of_type(self, t)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Non-undefined + known-keys helpers (Go checker.go 32097-32107, relater.go)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getNonUndefinedType(t) *Type` (checker.go:32097)
    /// — REAL (M6 S10; replaces typeofsymbol.rs's identity stub).
    pub(crate) fn get_non_undefined_type_impl(&mut self, t: TypeId) -> TypeId {
        let mut type_or_constraint = t;
        // Go: `someType(t, isGenericTypeWithUndefinedConstraint)` — PORT: the
        // constituent walk inlined (the closure captures &mut Checker).
        let constituents: Vec<TypeId> = if self.type_(t).flags.intersects(TypeFlags::UNION) {
            self.type_(t).types().to_vec()
        } else {
            vec![t]
        };
        let has_undefined_constraint = constituents.iter().any(|&u| {
            self.type_(u).flags.intersects(TypeFlags::INSTANTIABLE)
                && relater::get_base_constraint_of_type(self, u)
                    .is_some_and(|constraint| {
                        relater::maybe_type_of_kind(self, constraint, TypeFlags::UNDEFINED)
                    })
        });
        if has_undefined_constraint {
            type_or_constraint = self
                .map_type(
                    t,
                    &mut |c, u| {
                        if c.type_(u).flags.intersects(TypeFlags::INSTANTIABLE) {
                            Some(relater::get_base_constraint_or_type(c, u))
                        } else {
                            Some(u)
                        }
                    },
                )
                .unwrap_or(t);
        }
        self.get_type_with_facts_impl(type_or_constraint, TypeFacts::NE_UNDEFINED)
    }

    /// Go: `func (c *Checker) getKnownKeysOfTupleType(t) *Type` (relater.go)
    /// — REAL (M6 S10; replaces relater.rs's neverType stub).
    pub(crate) fn get_known_keys_of_tuple_type_impl(&mut self, t: TypeId) -> TypeId {
        let target = self.type_(t).target_tuple_type();
        let (fixed_length, readonly) = {
            let d = self.type_(target).as_tuple_type().expect("TupleType data");
            (d.fixed_length, d.readonly)
        };
        let mut keys: Vec<TypeId> = Vec::with_capacity(fixed_length as usize + 1);
        for i in 0..fixed_length {
            let literal = self.get_string_literal_type(&i.to_string());
            keys.push(literal);
        }
        let array_type = if readonly {
            self.global_readonly_array_type.unwrap_or_else(|| self.empty_generic_type())
        } else {
            self.global_array_type.unwrap_or_else(|| self.empty_generic_type())
        };
        let index_type = self.get_index_type_impl(array_type);
        keys.push(index_type);
        self.get_union_type(&keys)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers (Go's free functions in the same checker.go slice)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getIndexedAccessKey(objectType, indexType, accessFlags, alias) CacheHashKey`
/// (checker.go:17983).
pub(crate) fn get_indexed_access_key(
    object_type: TypeId,
    index_type: TypeId,
    access_flags: AccessFlags,
    alias: Option<&crate::types::TypeAlias>,
) -> CacheHashKey {
    let mut b = KeyBuilder::new();
    b.write_type(object_type);
    b.write_type(index_type);
    b.write_uint32(access_flags.0);
    b.write_alias(alias);
    b.hash()
}

/// Go: `func getIndexNodeForAccessExpression(accessNode) *ast.Node` (checker.go:27779).
pub(crate) fn get_index_node_for_access_expression(c: &Checker, access_node: Option<NodeId>) -> NodeId {
    let Some(node) = access_node else {
        return NodeId::NONE;
    };
    match c.store().node(node).kind {
        Kind::ElementAccessExpression => c
            .store()
            .node(node)
            .as_element_access_expression()
            .expect("ElementAccessExpression data")
            .argument_expression,
        Kind::IndexedAccessType => c
            .store()
            .node(node)
            .as_indexed_access_type_node()
            .expect("IndexedAccessTypeNode data")
            .index_type,
        Kind::ComputedPropertyName => crate::ast_shims::expression_of(c.store(), node)
            .unwrap_or(node),
        _ => node,
    }
}

/// Go: `func indexTypeLessThan(indexType, limit) bool` (checker.go:27858).
fn index_type_less_than(c: &mut Checker, index_type: TypeId, limit: i32) -> bool {
    // Go: `everyType(indexType, ...)` — PORT: the constituent walk inlined
    // (the closure captures &mut Checker).
    let constituents: Vec<TypeId> = if c.type_(index_type).flags.intersects(TypeFlags::UNION) {
        c.type_(index_type).types().to_vec()
    } else {
        vec![index_type]
    };
    constituents.iter().all(|&t| {
        let flags = c.type_(t).flags;
        if flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
            let prop_name = u::get_property_name_from_type(c.type_(t));
            if u::is_numeric_literal_name(&prop_name) {
                let index = parse_numeric_name(&prop_name);
                return index >= 0.0 && index < limit as f64;
            }
        }
        false
    })
}

/// Go: `jsnum.FromString(propName)` on a name `isNumericLiteralName` already
/// vetted — the canonical decimal forms parse with Rust's f64 parser.
/// PORT: Go's jsnum.FromString accepts the full JS numeric grammar; the
/// names reaching here come from getPropertyNameFromType on number/string
/// literal types (canonical decimals), where the two agree.
fn parse_numeric_name(name: &str) -> f64 {
    name.parse::<f64>().unwrap_or(f64::NAN)
}

/// Go: `forEachType(t, f)` (checker.go:27062) — distributes a union to its
/// constituents, anything else to itself.
pub(crate) fn for_each_type(c: &mut Checker, t: TypeId, f: &mut dyn FnMut(&mut Checker, TypeId)) {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        let constituents = c.type_(t).types().to_vec();
        for u in constituents {
            f(c, u);
        }
    } else {
        f(c, t);
    }
}

/// The diagnostic-arg rendering of `indexType.AsLiteralType().value` (Go
/// passes the `any` value; the port formats the literal payload the way Go's
/// fmt does for the string/number literal cases this slice reaches).
fn literal_value_string(c: &Checker, t: TypeId) -> String {
    match c.type_(t).as_literal_type().and_then(|d| d.value.clone()) {
        Some(types::LiteralValue::String(s)) => s,
        Some(types::LiteralValue::Number(n)) => format!("{n}"),
        _ => String::new(),
    }
}

/// `ast.GetPropertyNameForPropertyNameNode` — the crate's ast_shims carrier
/// (typeofsymbol.rs's getLiteralTypeFromPropertyName uses the same helper).
fn ast_shims_property_name(c: &mut Checker, node: NodeId) -> String {
    crate::ast_shims::get_property_name_for_property_name_node(c.store(), node)
}
