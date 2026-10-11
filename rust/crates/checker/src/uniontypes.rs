// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763
// (M6 S7) — the type-construction engine: the new* constructor family
// (checker.go 25537-25795), getUnionType + unionReduction machinery
// (26144-26520), getIntersectionType + intersection reduction
// (26520-26940), mapType/filterType/removeType (26990-27130), and the
// instantiate* engine (21019-21046, 22559-23290).
//
// STATE OWNERSHIP (M6-DISPATCH §4): this module owns `union_types`,
// `intersection_types`, `subtype_reduction_cache`, `union_of_union_types`,
// `template_literal_types`, `string_mapping_types`, and
// `instantiation_stack` (checker.rs fields). It reuses — never reinvents —
// S2's circularity protocol (typeResolutions/resolutionStart) and W1's
// mapper arena (mapper.rs). S8 seams (resolveStructuredTypeMembers and the
// member/signature readers) stay permissive-empty per the deliberate S7→S8
// cycle direction; every such site carries a `TODO(port M6 S8)` banner.
//
// PORT notes are inline where Rust forced a shape change.

use tsc_ast::{NodeId, SymbolId, SymbolTable};

use crate::relater::IntersectionFlags;
use crate::todo_shapes::{CacheHashKey, CachedTypeKey, CachedTypeKind, TypeMapperId, UnionReduction};
use crate::types::{
    self, LiteralValue, ObjectFlags, SignatureFlags, SignatureId, TypeAlias, TypeData, TypeFlags,
    TypeId,
};
use crate::utilities as u;
use crate::Checker;

/// Go: `type UnionOfUnionKey struct { id1, id2 TypeId; r UnionReduction; a CacheHashKey }`
/// (checker.go's getUnionTypeEx fast-path key).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct UnionOfUnionKey {
    pub id1: u32,
    pub id2: u32,
    pub r: u8,
    pub a: CacheHashKey,
}

// ────────────────────────────────────────────────────────────────────────────
// The new* constructor family (Go checker.go 25537-25795)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getPropagatingFlagsOfTypes(types []*Type, excludeKinds TypeFlags) ObjectFlags`.
    pub(crate) fn get_propagating_flags_of_types(&self, types: &[TypeId], exclude_kinds: TypeFlags) -> ObjectFlags {
        let mut result = ObjectFlags::NONE;
        for &t in types {
            let ty = self.type_(t);
            if !ty.flags.intersects(exclude_kinds) {
                result |= ty.object_flags;
            }
        }
        result & ObjectFlags::PROPAGATING_FLAGS
    }

    /// Go: `func (c *Checker) newUnionType(objectFlags ObjectFlags, types []*Type) *Type`.
    pub(crate) fn new_union_type(&mut self, object_flags: ObjectFlags, types: Vec<TypeId>) -> TypeId {
        let data = TypeData::Union(Box::new(types::UnionType {
            base: types::UnionOrIntersectionType {
                types,
                ..Default::default()
            },
            ..Default::default()
        }));
        self.new_type(TypeFlags::UNION, object_flags, data)
    }

    /// Go: `func (c *Checker) newIntersectionType(objectFlags ObjectFlags, types []*Type) *Type`.
    pub(crate) fn new_intersection_type(&mut self, object_flags: ObjectFlags, types: Vec<TypeId>) -> TypeId {
        let data = TypeData::Intersection(Box::new(types::IntersectionType {
            base: types::UnionOrIntersectionType {
                types,
                ..Default::default()
            },
            ..Default::default()
        }));
        self.new_type(TypeFlags::INTERSECTION, object_flags, data)
    }

    // Go: `func (c *Checker) newTypeParameter(symbol *ast.Symbol) *Type` —
    // lives in typeofsymbol.rs (S2's real landing); not duplicated here.

    /// Go: `func (c *Checker) newIndexedAccessType(objectType, indexType *Type, accessFlags AccessFlags) *Type`.
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn new_indexed_access_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: types::AccessFlags,
    ) -> TypeId {
        let data = TypeData::IndexedAccess(Box::new(types::IndexedAccessType {
            base: Default::default(),
            object_type: Some(object_type),
            index_type: Some(index_type),
            access_flags,
        }));
        self.new_type(TypeFlags::INDEXED_ACCESS, ObjectFlags::NONE, data)
    }

    /// Go: `func (c *Checker) newIndexType(target *Type, indexFlags IndexFlags) *Type`.
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn new_index_type(&mut self, target: TypeId, index_flags: types::IndexFlags) -> TypeId {
        let data = TypeData::Index(Box::new(types::IndexType {
            base: Default::default(),
            target: Some(target),
            index_flags,
        }));
        self.new_type(TypeFlags::INDEX, ObjectFlags::NONE, data)
    }

    /// Go: `func (c *Checker) newTemplateLiteralType(texts []string, types []*Type) *Type`.
    pub(crate) fn new_template_literal_type(&mut self, texts: Vec<String>, types: Vec<TypeId>) -> TypeId {
        let data = TypeData::TemplateLiteral(Box::new(types::TemplateLiteralType {
            base: Default::default(),
            texts,
            types,
        }));
        self.new_type(TypeFlags::TEMPLATE_LITERAL, ObjectFlags::NONE, data)
    }

    /// Go: `func (c *Checker) newStringMappingType(symbol *ast.Symbol, target *Type) *Type`.
    pub(crate) fn new_string_mapping_type(&mut self, symbol: Option<SymbolId>, target: TypeId) -> TypeId {
        let data = TypeData::StringMapping(Box::new(types::StringMappingType {
            base: Default::default(),
            target: Some(target),
        }));
        let t = self.new_type(TypeFlags::STRING_MAPPING, ObjectFlags::NONE, data);
        self.type_mut(t).symbol = symbol;
        t
    }

    // Go: `func (c *Checker) newUniqueESSymbolType(symbol, name) *Type` —
    // typenodes.rs (S5) owns the real ctor; not duplicated here.

    /// Go: `func (c *Checker) tryCreateTypeReference(target *Type, typeArguments []*Type) *Type`.
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn try_create_type_reference(&mut self, target: TypeId, type_arguments: &[TypeId]) -> TypeId {
        if !type_arguments.is_empty() && target == self.empty_generic_type() {
            return self.unknown_type();
        }
        self.create_type_reference_ex(target, type_arguments.to_vec(), ObjectFlags::NONE)
    }

    /// Go: `func (c *Checker) createTypeReference(target *Type, typeArguments []*Type) *Type`.
    pub fn create_type_reference(&mut self, target: TypeId, type_arguments: Vec<TypeId>) -> TypeId {
        self.create_type_reference_ex(target, type_arguments, ObjectFlags::NONE)
    }

    /// Go: `func (c *Checker) createTypeReferenceEx(target *Type, typeArguments []*Type, objectFlags ObjectFlags) *Type`
    /// — the REAL interned type-reference constructor (M6 S7; replaces the
    /// relater.rs identity seam).
    pub(crate) fn create_type_reference_ex(
        &mut self,
        target: TypeId,
        type_arguments: Vec<TypeId>,
        object_flags: ObjectFlags,
    ) -> TypeId {
        let id = crate::typeofsymbol::get_type_list_key(&type_arguments);
        if let Some(instantiations) = self
            .type_(target)
            .as_interface_type()
            .expect("InterfaceType target")
            .base
            .base
            .instantiations
            .as_ref()
            && let Some(&t) = instantiations.get(&id)
        {
            return t;
        }
        let propagating = self.get_propagating_flags_of_types(&type_arguments, TypeFlags::NONE);
        let symbol = self.type_(target).symbol;
        let t = self.new_object_type(ObjectFlags::REFERENCE | object_flags | propagating, symbol);
        match &mut self.type_mut(t).data {
            TypeData::TypeReference(reference) => {
                reference.base.target = Some(target);
                reference.resolved_type_arguments = type_arguments.clone();
            }
            _ => unreachable!("newObjectType(Reference) yields TypeReference data"),
        }
        let instantiations = &mut self
            .type_mut(target)
            .as_interface_type_mut()
            .expect("InterfaceType target")
            .base
            .base
            .instantiations;
        instantiations.get_or_insert_with(Default::default).insert(id, t);
        t
    }

    // Go: `func (c *Checker) createDeferredTypeReference(target, node,
    // mapper, alias) *Type` — typenodes.rs (S5) owns the real ctor
    // (getAliasForTypeNode wiring); not duplicated here.

    /// Go: `func (c *Checker) cloneTypeReference(source *Type) *Type`.
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn clone_type_reference(&mut self, source: TypeId) -> TypeId {
        let symbol = self.type_(source).symbol;
        let object_flags = self.type_(source).object_flags & !ObjectFlags::MEMBERS_RESOLVED;
        let t = self.new_object_type(ObjectFlags::REFERENCE, symbol);
        self.type_mut(t).object_flags = object_flags;
        let (target, resolved) = {
            let d = self.type_(source).as_type_reference().expect("TypeReference data");
            (d.base.target, d.resolved_type_arguments.clone())
        };
        match &mut self.type_mut(t).data {
            TypeData::TypeReference(reference) => {
                reference.base.target = target;
                reference.resolved_type_arguments = resolved;
            }
            _ => unreachable!("newObjectType(Reference) yields TypeReference data"),
        }
        t
    }

    // Go: `func (c *Checker) createNormalizedTypeReference` / `createNormalizedTupleType(Ex)`
    // (checker.go:23816-23990) — typenodes.rs (S5's TupleNormalizer
    // landing) owns the real constructors; not duplicated here.

    /// Go: `func (c *Checker) createArrayType(elementType *Type) *Type` →
    /// createArrayTypeEx → createTypeFromGenericGlobalType (checker.go:25231).
    pub fn create_array_type(&mut self, element_type: TypeId) -> TypeId {
        self.create_array_type_ex(element_type, false)
    }

    /// Go: `func (c *Checker) createArrayTypeEx(elementType *Type, readonly bool) *Type`.
    pub(crate) fn create_array_type_ex(&mut self, element_type: TypeId, readonly: bool) -> TypeId {
        let generic_global_type = if readonly {
            self.global_readonly_array_type
        } else {
            self.global_array_type
        };
        self.create_type_from_generic_global_type(generic_global_type, vec![element_type])
    }

    /// Go: `func (c *Checker) createTypeFromGenericGlobalType(genericGlobalType *Type, typeArguments []*Type) *Type`
    /// (checker.go:25202).
    pub(crate) fn create_type_from_generic_global_type(
        &mut self,
        generic_global_type: Option<TypeId>,
        type_arguments: Vec<TypeId>,
    ) -> TypeId {
        match generic_global_type {
            Some(g) if g != self.empty_generic_type() => self.create_type_reference(g, type_arguments),
            _ => self.empty_object_type(),
        }
    }

    /// Go: `func (c *Checker) createTupleType(elementTypes []*Type) *Type`.
    pub fn create_tuple_type(&mut self, element_types: &[TypeId]) -> TypeId {
        let element_infos = element_types
            .iter()
            .map(|_| types::TupleElementInfo {
                flags: types::ElementFlags::REQUIRED,
                labeled_declaration: None,
            })
            .collect::<Vec<_>>();
        self.create_tuple_type_ex(element_types, &element_infos, false)
    }

    /// Go: `func (c *Checker) createTupleTypeEx(elementTypes []*Type, elementInfos []TupleElementInfo, readonly bool) *Type`
    /// — REAL (M6 S7; replaces the first-element stub).
    pub(crate) fn create_tuple_type_ex(
        &mut self,
        element_types: &[TypeId],
        element_infos: &[types::TupleElementInfo],
        readonly: bool,
    ) -> TypeId {
        let tuple_target = self.get_tuple_target_type(element_infos.to_vec(), readonly);
        if tuple_target == self.empty_generic_type() {
            return self.empty_object_type();
        }
        if !element_types.is_empty() {
            return self.create_normalized_type_reference(tuple_target, element_types.to_vec());
        }
        tuple_target
    }

}

// ────────────────────────────────────────────────────────────────────────────
// getUnionType + the unionReduction machinery (Go checker.go 26144-26520)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getUnionType(types []*Type) *Type` →
    /// getUnionTypeEx(types, UnionReductionLiteral, nil, nil).
    pub fn get_union_type(&mut self, types: &[TypeId]) -> TypeId {
        self.get_union_type_ex(types.to_vec(), UnionReduction::Literal, None, None)
    }

    /// Go: `func (c *Checker) getUnionTypeEx(types []*Type, unionReduction UnionReduction, alias *TypeAlias, origin *Type) *Type`
    /// (checker.go:26151).
    pub(crate) fn get_union_type_ex(
        &mut self,
        types: Vec<TypeId>,
        union_reduction: UnionReduction,
        alias: Option<Box<TypeAlias>>,
        origin: Option<TypeId>,
    ) -> TypeId {
        if types.is_empty() {
            return self.never_type();
        }
        if types.len() == 1 {
            return types[0];
        }
        // We optimize for the common case of unioning a union type with some
        // other type (such as `undefined`).
        if types.len() == 2
            && origin.is_none()
            && (self.type_(types[0]).flags.intersects(TypeFlags::UNION)
                || self.type_(types[1]).flags.intersects(TypeFlags::UNION))
        {
            let (mut id1, mut id2) = (types[0].0, types[1].0);
            if id1 > id2 {
                std::mem::swap(&mut id1, &mut id2);
            }
            let key = UnionOfUnionKey {
                id1,
                id2,
                r: union_reduction as u8,
                a: crate::typeofsymbol::get_alias_key(alias.as_deref()),
            };
            if let Some(&t) = self.union_of_union_types.get(&key) {
                return t;
            }
            let t = self.get_union_type_worker(types, union_reduction, alias, None);
            self.union_of_union_types.insert(key, t);
            return t;
        }
        self.get_union_type_worker(types, union_reduction, alias, origin)
    }

    /// Go: `func (c *Checker) getUnionTypeWorker(types []*Type, unionReduction UnionReduction, alias *TypeAlias, origin *Type) *Type`
    /// (checker.go:26187).
    fn get_union_type_worker(
        &mut self,
        types: Vec<TypeId>,
        union_reduction: UnionReduction,
        alias: Option<Box<TypeAlias>>,
        mut origin: Option<TypeId>,
    ) -> TypeId {
        let (mut type_set, includes) = self.add_types_to_union(&types);
        if union_reduction != UnionReduction::None {
            if includes.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                if includes.intersects(TypeFlags::ANY) {
                    if includes.intersects(TypeFlags::INCLUDES_WILDCARD) {
                        return self.wildcard_type();
                    }
                    if includes.intersects(TypeFlags::INCLUDES_ERROR) {
                        return self.error_type();
                    }
                    return self.any_type();
                }
                return self.unknown_type();
            }
            if includes.intersects(TypeFlags::UNDEFINED)
                && type_set.len() >= 2
                && type_set[0] == self.undefined_type()
                && type_set[1] == self.missing_type()
            {
                // If type set contains both undefinedType and missingType,
                // remove missingType
                type_set.remove(1);
            }
            if includes.intersects(
                TypeFlags::ENUM
                    | TypeFlags::LITERAL
                    | TypeFlags::UNIQUE_ESSYMBOL
                    | TypeFlags::TEMPLATE_LITERAL
                    | TypeFlags::STRING_MAPPING,
            ) || (includes.intersects(TypeFlags::VOID) && includes.intersects(TypeFlags::UNDEFINED))
            {
                let reduce_void_undefined =
                    union_reduction == UnionReduction::Subtype && includes.intersects(TypeFlags::VOID);
                type_set = self.remove_redundant_literal_types(type_set, includes, reduce_void_undefined);
            }
            if includes.intersects(TypeFlags::STRING_LITERAL)
                && includes.intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
            {
                type_set = self.remove_string_literals_matched_by_template_literals(type_set);
            }
            if includes.intersects(TypeFlags::INCLUDES_CONSTRAINED_TYPE_VARIABLE) {
                type_set = self.remove_constrained_type_variables(type_set);
            }
            if union_reduction == UnionReduction::Subtype {
                let has_object_types = includes.intersects(TypeFlags::OBJECT);
                match self.remove_subtypes(type_set, has_object_types) {
                    Some(reduced) => type_set = reduced,
                    None => return self.error_type(),
                }
            }
            if type_set.is_empty() {
                if includes.intersects(TypeFlags::NULL) {
                    if includes.intersects(TypeFlags::INCLUDES_NON_WIDENING_TYPE) {
                        return self.null_type();
                    }
                    return self.null_widening_type();
                }
                if includes.intersects(TypeFlags::UNDEFINED) {
                    if includes.intersects(TypeFlags::INCLUDES_NON_WIDENING_TYPE) {
                        return self.undefined_type();
                    }
                    return self.undefined_widening_type();
                }
                return self.never_type();
            }
        }
        if origin.is_none() && includes.intersects(TypeFlags::UNION) {
            let named_unions = self.add_named_unions(Vec::new(), &types);
            let mut reduced_types: Vec<TypeId> = Vec::new();
            for &t in &type_set {
                let contained = named_unions.iter().any(|u| {
                    let u_types = self.type_(*u).types().to_vec();
                    crate::uniontypes::contains_type(self, &u_types, t)
                });
                if !contained {
                    reduced_types.push(t);
                }
            }
            if alias.is_none() && named_unions.len() == 1 && reduced_types.is_empty() {
                return named_unions[0];
            }
            // We create a denormalized origin type only when the union was
            // created from one or more named unions (unions with alias symbols
            // or origins) and when there is no overlap between those named
            // unions.
            let mut named_types_count = 0usize;
            for &u in &named_unions {
                named_types_count += self.type_(u).types().len();
            }
            if named_types_count + reduced_types.len() == type_set.len() {
                for &u in &named_unions {
                    let (inserted, reduced) =
                        crate::uniontypes::insert_type(self, std::mem::take(&mut reduced_types), u);
                    reduced_types = reduced;
                    let _ = inserted;
                }
                origin = Some(self.new_union_type(ObjectFlags::NONE, reduced_types.clone()));
            }
        }
        let object_flags = if includes.intersects(TypeFlags::NOT_PRIMITIVE_UNION) {
            ObjectFlags::NONE
        } else {
            ObjectFlags::PRIMITIVE_UNION
        } | if includes.intersects(TypeFlags::INTERSECTION) {
            ObjectFlags::CONTAINS_INTERSECTIONS
        } else {
            ObjectFlags::NONE
        };
        self.get_union_type_from_sorted_list(type_set, object_flags, alias, origin)
    }

    /// Go: `func (c *Checker) getUnionTypeFromSortedList(types []*Type, precomputedObjectFlags ObjectFlags, alias *TypeAlias, origin *Type) *Type`
    /// (checker.go:26270) — assumes the constituent list is sorted and
    /// deduplicated.
    pub(crate) fn get_union_type_from_sorted_list(
        &mut self,
        types: Vec<TypeId>,
        precomputed_object_flags: ObjectFlags,
        alias: Option<Box<TypeAlias>>,
        origin: Option<TypeId>,
    ) -> TypeId {
        if types.is_empty() {
            return self.never_type();
        }
        if types.len() == 1 {
            return types[0];
        }
        let key = crate::typeofsymbol::get_union_key(&types, origin, alias.as_deref(), self);
        if let Some(&t) = self.union_types.get(&key) {
            return t;
        }
        let propagating = self.get_propagating_flags_of_types(&types, TypeFlags::NULLABLE);
        let t = self.new_union_type(precomputed_object_flags | propagating, types.clone());
        let is_boolean_pair = types.len() == 2
            && self.type_(types[0]).flags.intersects(TypeFlags::BOOLEAN_LITERAL)
            && self.type_(types[1]).flags.intersects(TypeFlags::BOOLEAN_LITERAL);
        {
            let ty = self.type_mut(t);
            ty.as_union_type_mut().expect("UnionType data").origin = origin;
            ty.alias = alias;
            if is_boolean_pair {
                ty.flags |= TypeFlags::BOOLEAN;
            }
        }
        self.union_types.insert(key, t);
        t
    }

    /// Go: `func (c *Checker) addTypesToUnion(sourceTypes []*Type) ([]*Type, TypeFlags)`
    /// (checker.go:26320).
    pub(crate) fn add_types_to_union(&mut self, source_types: &[TypeId]) -> (Vec<TypeId>, TypeFlags) {
        let mut types: Vec<TypeId> = Vec::with_capacity(source_types.len());
        let mut includes = TypeFlags::NONE;
        // Go's `addType` closure (checker.go:26324) — a free helper so the
        // borrow discipline works (§5.4 arena idiom).
        fn add_type(c: &mut Checker, types: &mut Vec<TypeId>, includes: &mut TypeFlags, t: TypeId) {
            let flags = c.type_(t).flags;
            // We ignore 'never' types in unions
            if flags.intersects(TypeFlags::NEVER) {
                return;
            }
            *includes |= flags & TypeFlags::INCLUDES_MASK;
            if flags.intersects(TypeFlags::INSTANTIABLE) {
                *includes |= TypeFlags::INCLUDES_INSTANTIABLE;
            }
            if flags.intersects(TypeFlags::INTERSECTION)
                && c.type_(t).object_flags.intersects(ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE)
            {
                *includes |= TypeFlags::INCLUDES_CONSTRAINED_TYPE_VARIABLE;
            }
            if t == c.wildcard_type() {
                *includes |= TypeFlags::INCLUDES_WILDCARD;
            }
            if c.is_error_type(t) {
                *includes |= TypeFlags::INCLUDES_ERROR;
            }
            if !c.strict_null_checks && flags.intersects(TypeFlags::NULLABLE) {
                if !c.type_(t).object_flags.intersects(ObjectFlags::CONTAINS_WIDENING_TYPE) {
                    *includes |= TypeFlags::INCLUDES_NON_WIDENING_TYPE;
                }
                return;
            }
            types.push(t);
        }
        let mut last_type: Option<TypeId> = None;
        for &t in source_types {
            if last_type != Some(t) {
                if self.type_(t).flags.intersects(TypeFlags::UNION) {
                    let u_types = self.type_(t).as_union_type().expect("UnionType data").base.types.clone();
                    if self.type_(t).alias.is_some()
                        || self
                            .type_(t)
                            .as_union_type()
                            .expect("UnionType data")
                            .origin
                            .is_some()
                    {
                        includes |= TypeFlags::UNION;
                    }
                    for s in u_types {
                        add_type(self, &mut types, &mut includes, s);
                    }
                } else {
                    add_type(self, &mut types, &mut includes, t);
                }
                last_type = Some(t);
            }
        }
        if types.len() >= 2 {
            // Sort and deduplicate types
            let mut sorted = std::mem::take(&mut types);
            sorted.sort_by(|a, b| u::compare_types(self, *a, *b));
            sorted.dedup();
            types = sorted;
        }
        (types, includes)
    }

    /// Go: `func (c *Checker) addNamedUnions(namedUnions []*Type, types []*Type) []*Type`
    /// (checker.go:26387).
    fn add_named_unions(&mut self, mut named_unions: Vec<TypeId>, types: &[TypeId]) -> Vec<TypeId> {
        for &t in types {
            if self.type_(t).flags.intersects(TypeFlags::UNION) {
                let (alias, origin) = {
                    let u = self.type_(t).as_union_type().expect("UnionType data");
                    (self.type_(t).alias.is_some(), u.origin)
                };
                if alias || origin.is_some_and(|o| !self.type_(o).flags.intersects(TypeFlags::UNION)) {
                    if !named_unions.contains(&t) {
                        named_unions.push(t);
                    }
                } else if let Some(origin) = origin {
                    let origin_types = self.type_(origin).types().to_vec();
                    named_unions = self.add_named_unions(named_unions, &origin_types);
                }
            }
        }
        named_unions
    }

    /// Go: `func (c *Checker) removeRedundantLiteralTypes(types []*Type, includes TypeFlags, reduceVoidUndefined bool) []*Type`
    /// (checker.go:26400).
    fn remove_redundant_literal_types(
        &mut self,
        mut types: Vec<TypeId>,
        includes: TypeFlags,
        reduce_void_undefined: bool,
    ) -> Vec<TypeId> {
        let mut i = types.len();
        while i > 0 {
            i -= 1;
            let t = types[i];
            let flags = self.type_(t).flags;
            let remove = (flags.intersects(
                TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
            ) && includes.intersects(TypeFlags::STRING))
                || (flags.intersects(TypeFlags::NUMBER_LITERAL) && includes.intersects(TypeFlags::NUMBER))
                || (flags.intersects(TypeFlags::BIGINT_LITERAL) && includes.intersects(TypeFlags::BIGINT))
                || (flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) && includes.intersects(TypeFlags::ESSYMBOL))
                || (reduce_void_undefined
                    && flags.intersects(TypeFlags::UNDEFINED)
                    && includes.intersects(TypeFlags::VOID))
                || (crate::is_fresh_literal_type(self, t)
                    && crate::uniontypes::contains_type(
                        self,
                        &types,
                        self.type_(t)
                            .as_literal_type()
                            .expect("LiteralType data")
                            .regular_type
                            .expect("regularType"),
                    ));
            if remove {
                types.remove(i);
            }
        }
        types
    }

    /// Go: `func (c *Checker) removeStringLiteralsMatchedByTemplateLiterals(types []*Type) []*Type`
    /// (checker.go:26420).
    fn remove_string_literals_matched_by_template_literals(&mut self, mut types: Vec<TypeId>) -> Vec<TypeId> {
        // TODO(port M6 S8): isPatternLiteralType's real body (typenodes.rs's
        // stub) gates the template collection; until it lands the arm is
        // permissive (Go's loop is a no-op with zero templates).
        let templates: Vec<TypeId> = types
            .iter()
            .copied()
            .filter(|&t| crate::typenodes::is_pattern_literal_type(self, t))
            .collect();
        if !templates.is_empty() {
            let mut i = types.len();
            while i > 0 {
                i -= 1;
                let t = types[i];
                if self.type_(t).flags.intersects(TypeFlags::STRING_LITERAL)
                    && templates.iter().any(|&template| {
                        self.is_type_matched_by_template_literal_or_string_mapping(t, template)
                    })
                {
                    types.remove(i);
                }
            }
        }
        types
    }

    /// Go: `func (c *Checker) isTypeMatchedByTemplateLiteralOrStringMapping(t, template *Type) bool`
    /// (checker.go:26432).
    fn is_type_matched_by_template_literal_or_string_mapping(&mut self, t: TypeId, template: TypeId) -> bool {
        if self.type_(template).flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            return crate::relater::is_type_matched_by_template_literal_type(
                self,
                t,
                template,
                crate::relater::TypeComparerKind::Assignable,
                None,
            );
        }
        crate::relater::is_member_of_string_mapping(self, t, template)
    }

    /// Go: `func (c *Checker) removeConstrainedTypeVariables(types []*Type) []*Type`
    /// (checker.go:26450).
    fn remove_constrained_type_variables(&mut self, mut types: Vec<TypeId>) -> Vec<TypeId> {
        let mut type_variables: Vec<TypeId> = Vec::new();
        // First collect a list of the type variables occurring in constraining
        // intersections.
        for &t in &types {
            if self.type_(t).flags.intersects(TypeFlags::INTERSECTION)
                && self.type_(t).object_flags.intersects(ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE)
            {
                let intersection = self.type_(t).as_intersection_type().expect("IntersectionType data");
                let mut index = 0;
                if !self
                    .type_(intersection.base.types[0])
                    .flags
                    .intersects(TypeFlags::TYPE_VARIABLE)
                {
                    index = 1;
                }
                let type_variable = intersection.base.types[index];
                if !type_variables.contains(&type_variable) {
                    type_variables.push(type_variable);
                }
            }
        }
        // For each type variable, check if the constraining intersections for
        // that type variable fully cover the constraint of the type variable;
        // if so, remove the constraining intersections and substitute the
        // type variable.
        for &type_variable in &type_variables {
            let mut primitives: Vec<TypeId> = Vec::new();
            // First collect the primitive types from the constraining intersections.
            for &t in &types {
                if self.type_(t).flags.intersects(TypeFlags::INTERSECTION)
                    && self.type_(t).object_flags.intersects(ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE)
                {
                    let intersection =
                        self.type_(t).as_intersection_type().expect("IntersectionType data");
                    let mut index = 0;
                    if !self
                        .type_(intersection.base.types[0])
                        .flags
                        .intersects(TypeFlags::TYPE_VARIABLE)
                    {
                        index = 1;
                    }
                    if intersection.base.types[index] == type_variable {
                        let (inserted, prim) =
                            crate::uniontypes::insert_type(self, std::mem::take(&mut primitives), intersection.base.types[1 - index]);
                        primitives = prim;
                        let _ = inserted;
                    }
                }
            }
            // If every constituent in the type variable's constraint is covered
            // by an intersection of the type variable and that constituent,
            // remove those intersections and substitute the type variable.
            let constraint = crate::relater::get_base_constraint_of_type(self, type_variable);
            let covered = constraint.is_some_and(|constraint| {
                let constituents = if type_flags(self, constraint).intersects(TypeFlags::UNION) {
                    self.type_(constraint).types().to_vec()
                } else {
                    vec![constraint]
                };
                constituents
                    .iter()
                    .all(|&t| crate::uniontypes::contains_type(self, &primitives, t))
            });
            if covered {
                let mut i = types.len();
                while i > 0 {
                    i -= 1;
                    let t = types[i];
                    if self.type_(t).flags.intersects(TypeFlags::INTERSECTION)
                        && self.type_(t).object_flags.intersects(ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE)
                    {
                        let intersection = self
                            .type_(t)
                            .as_intersection_type()
                            .expect("IntersectionType data")
                            .base
                            .types
                            .clone();
                        let mut index = 0;
                        if !self.type_(intersection[0]).flags.intersects(TypeFlags::TYPE_VARIABLE) {
                            index = 1;
                        }
                        if intersection[index] == type_variable
                            && crate::uniontypes::contains_type(self, &primitives, intersection[1 - index])
                        {
                            types.remove(i);
                        }
                    }
                }
                let (_inserted, replaced) =
                    crate::uniontypes::insert_type(self, std::mem::take(&mut types), type_variable);
                types = replaced;
            }
        }
        types
    }

    /// Go: `func (c *Checker) removeSubtypes(types []*Type, hasObjectTypes bool) []*Type`
    /// (checker.go:26468 — 361 lines, perf-critical). Returns None where Go
    /// returns nil (the depth-limit arm).
    pub(crate) fn remove_subtypes(&mut self, types: Vec<TypeId>, has_object_types: bool) -> Option<Vec<TypeId>> {
        // [] and [T] immediately reduce to [] and [T] respectively
        if types.len() < 2 {
            return Some(types);
        }
        let key = crate::typeofsymbol::get_type_list_key(&types);
        if let Some(cached) = self.subtype_reduction_cache.get(&key) {
            return Some(cached.clone());
        }
        let mut types = types;
        let length = types.len();
        // We assume that redundant primitive types have already been removed
        // from the types array and that there are no any and unknown types in
        // the array. Thus, the only possible supertypes for primitive types
        // are empty object types, and if none of those are present we can
        // exclude primitive types from the subtype check.
        let has_empty_object = has_object_types
            && types.iter().any(|&t| {
                self.type_(t).flags.intersects(TypeFlags::OBJECT)
                    && !crate::relater::is_generic_mapped_type(self, t)
                    && {
                        let members = crate::relater::resolve_structured_type_members(self, t);
                        self.is_empty_resolved_type_of(members)
                    }
            });
        let mut i = length;
        let mut count: u64 = 0;
        while i > 0 {
            i -= 1;
            let source = types[i];
            if has_empty_object
                || self.type_(source).flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
            {
                // A type parameter with a union constraint may be a subtype of
                // some union, but not a subtype of the individual constituents
                // of that union.
                if self.type_(source).flags.intersects(TypeFlags::TYPE_PARAMETER)
                    && {
                        let constraint = crate::relater::get_base_constraint_or_type(self, source);
                        self.type_(constraint).flags.intersects(TypeFlags::UNION)
                    }
                {
                    let others: Vec<TypeId> = types
                        .iter()
                        .map(|&t| if t == source { self.never_type() } else { t })
                        .collect();
                    let union = self.get_union_type(&others);
                    if self.is_type_related_to(source, union, crate::relater::RelationKind::StrictSubtype) {
                        types.remove(i);
                    }
                    continue;
                }
                // Find the first property with a unit type, if any. When
                // constituents have a property by the same name but of a
                // different unit type, we can quickly disqualify them from
                // subtype checks. This helps subtype reduction of large
                // discriminated union types.
                let mut key_property_name: Option<String> = None;
                let mut key_property_type: Option<TypeId> = None;
                if self
                    .type_(source)
                    .flags
                    .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
                {
                    let props = crate::relater::get_properties_of_type(self, source);
                    for p in props {
                        let prop_type = self.get_type_of_symbol(p);
                        if crate::relater::is_unit_type(self, prop_type) {
                            key_property_name = Some(self.program_symbol(p).name.clone());
                            key_property_type = Some(self.get_regular_type_of_literal_type(prop_type));
                            break;
                        }
                    }
                }
                // Go iterates the (immutable) target slice while deleting from
                // the same backing array — the port snapshots first (§5.4).
                let snapshot = types.clone();
                for &target in &snapshot {
                    if source == target {
                        continue;
                    }
                    if let (Some(name), Some(key_type)) = (&key_property_name, key_property_type)
                        && self.type_(target).flags.intersects(
                            TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
                        )
                        && let Some(t) = crate::relater::get_type_of_property_of_type(self, target, name)
                        && crate::relater::is_unit_type(self, t)
                        && self.get_regular_type_of_literal_type(t) != key_type
                    {
                        continue;
                    }
                    if count == 100000 {
                        // After 100000 subtype checks we estimate the
                        // remaining amount of work by assuming the same ratio
                        // of checks per element. If the estimated number of
                        // remaining type checks is greater than 1M we deem the
                        // union type too complex to represent.
                        let estimated_count = (count / (length - i) as u64) * length as u64;
                        if estimated_count > 1_000_000 {
                            self.error(
                                self.current_node.unwrap_or(NodeId::NONE),
                                &tsc_diagnostics::EXPRESSION_PRODUCES_A_UNION_TYPE_THAT_IS_TOO_COMPLEX_TO_REPRESENT,
                                &[],
                            );
                            return None;
                        }
                    }
                    count += 1;
                    if (source == self.empty_object_type()
                        || Some(source) == self.unknown_empty_object_type)
                        && self.type_(target).symbol.is_some()
                        && self.is_empty_anonymous_object_type(target)
                    {
                        continue;
                    }
                    if self.is_type_related_to(
                        source,
                        target,
                        crate::relater::RelationKind::StrictSubtype,
                    ) && (!(self.type_(get_target_type(self, source)).object_flags).intersects(ObjectFlags::CLASS)
                        || !(self.type_(get_target_type(self, target)).object_flags).intersects(ObjectFlags::CLASS)
                        || self.is_type_derived_from(source, target))
                    {
                        types.remove(i);
                        break;
                    }
                }
            }
        }
        self.subtype_reduction_cache.insert(key, types.clone());
        Some(types)
    }
}

/// PORT helper: `t.flags()` on a TypeId (the arena read Go does through the
/// pointer).
fn type_flags(c: &Checker, t: TypeId) -> TypeFlags {
    c.type_(t).flags
}

/// Go: `func getTargetType(t *Type) *Type` (utilities.go) — `t` itself unless
/// it's an unresolved type reference.
pub(crate) fn get_target_type(c: &Checker, t: TypeId) -> TypeId {
    if !c.type_(t).object_flags.intersects(ObjectFlags::REFERENCE) {
        return t;
    }
    c.type_(t)
        .as_type_reference()
        .and_then(|d| d.base.target)
        .unwrap_or(t)
}

// ────────────────────────────────────────────────────────────────────────────
// Free list helpers (Go checker.go 27128-27190)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func containsType(types []*Type, t *Type) bool` — binary search (the
/// list is sorted at every call site).
pub(crate) fn contains_type(c: &Checker, types: &[TypeId], t: TypeId) -> bool {
    types.binary_search_by(|probe| u::compare_types(c, *probe, t)).is_ok()
}

/// Go: `func insertType(types []*Type, t *Type) ([]*Type, bool)`.
pub(crate) fn insert_type(c: &Checker, mut types: Vec<TypeId>, t: TypeId) -> (bool, Vec<TypeId>) {
    match types.binary_search_by(|probe| u::compare_types(c, *probe, t)) {
        Ok(_) => (false, types),
        Err(i) => {
            types.insert(i, t);
            (true, types)
        }
    }
}

/// Go: `func countTypes(t *Type) int`.
#[allow(dead_code)]
pub(crate) fn count_types(c: &Checker, t: TypeId) -> usize {
    if type_flags(c, t).intersects(TypeFlags::UNION) {
        return c.type_(t).types().len();
    }
    if type_flags(c, t).intersects(TypeFlags::NEVER) {
        return 0;
    }
    1
}

/// Go: `func getConstituentCount(t *Type) int`.
#[allow(dead_code)]
pub(crate) fn get_constituent_count(c: &Checker, t: TypeId) -> usize {
    let flags = type_flags(c, t);
    if !flags.intersects(TypeFlags::UNION_OR_INTERSECTION) || c.type_(t).alias.is_some() {
        return 1;
    }
    if flags.intersects(TypeFlags::UNION)
        && c.type_(t)
            .as_union_type()
            .expect("UnionType data")
            .origin
            .is_some()
    {
        let origin = c
            .type_(t)
            .as_union_type()
            .expect("UnionType data")
            .origin
            .expect("checked");
        return get_constituent_count(c, origin);
    }
    get_constituent_count_of_types(c, c.type_(t).types())
}

/// Go: `func getConstituentCountOfTypes(types []*Type) int`.
#[allow(dead_code)]
pub(crate) fn get_constituent_count_of_types(c: &Checker, types: &[TypeId]) -> usize {
    types.iter().map(|&t| get_constituent_count(c, t)).sum()
}

// ────────────────────────────────────────────────────────────────────────────
// getIntersectionType + intersection reduction (Go checker.go 26520-26940)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) intersectTypes(type1, type2 *Type) *Type`
    /// (checker.go:26520). PORT: Go's nil handling surfaces as Option.
    pub(crate) fn intersect_types(&mut self, type1: Option<TypeId>, type2: Option<TypeId>) -> Option<TypeId> {
        match (type1, type2) {
            (None, t2) => t2,
            (t1, None) => t1,
            (Some(t1), Some(t2)) => Some(self.get_intersection_type(&[t1, t2])),
        }
    }

    /// Go: `func (c *Checker) getIntersectionType(types []*Type) *Type` →
    /// getIntersectionTypeEx(types, IntersectionFlagsNone, nil).
    pub fn get_intersection_type(&mut self, types: &[TypeId]) -> TypeId {
        self.get_intersection_type_ex(types.to_vec(), IntersectionFlags::NONE, None)
    }

    /// Go: `func (c *Checker) getIntersectionTypeEx(types []*Type, flags IntersectionFlags, alias *TypeAlias) *Type`
    /// (checker.go:26550).
    #[allow(clippy::let_and_return)] // the match arms early-return on the cache-insert path
    pub(crate) fn get_intersection_type_ex(
        &mut self,
        types: Vec<TypeId>,
        flags: IntersectionFlags,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let mut type_set: Vec<TypeId> = Vec::with_capacity(types.len());
        let includes = self.add_types_to_intersection(&mut type_set, TypeFlags::NONE, &types);
        let mut object_flags = ObjectFlags::NONE;
        // An intersection type is considered empty if it contains the type
        // never, or more than one unit type or, an object type and a nullable
        // type, or a string-like type and a type known to be non-string-like,
        // etc. (Go's comment, checker.go:26562).
        if includes.intersects(TypeFlags::NEVER) {
            if type_set.contains(&self.silent_never_type()) {
                return self.silent_never_type();
            }
            return self.never_type();
        }
        if (self.strict_null_checks
            && includes.intersects(TypeFlags::NULLABLE)
            && includes.intersects(
                TypeFlags::OBJECT | TypeFlags::NON_PRIMITIVE | TypeFlags::INCLUDES_EMPTY_OBJECT,
            ))
            || (includes.intersects(TypeFlags::NON_PRIMITIVE)
                && includes.intersects(TypeFlags::DISJOINT_DOMAINS & !TypeFlags::NON_PRIMITIVE))
            || (includes.intersects(TypeFlags::STRING_LIKE)
                && includes.intersects(TypeFlags::DISJOINT_DOMAINS & !TypeFlags::STRING_LIKE))
            || (includes.intersects(TypeFlags::NUMBER_LIKE)
                && includes.intersects(TypeFlags::DISJOINT_DOMAINS & !TypeFlags::NUMBER_LIKE))
            || (includes.intersects(TypeFlags::BIGINT_LIKE)
                && includes.intersects(TypeFlags::DISJOINT_DOMAINS & !TypeFlags::BIGINT_LIKE))
            || (includes.intersects(TypeFlags::ESSYMBOL_LIKE)
                && includes.intersects(TypeFlags::DISJOINT_DOMAINS & !TypeFlags::ESSYMBOL_LIKE))
            || (includes.intersects(TypeFlags::VOID_LIKE)
                && includes.intersects(TypeFlags::DISJOINT_DOMAINS & !TypeFlags::VOID_LIKE))
        {
            return self.never_type();
        }
        if includes.intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
            && includes.intersects(TypeFlags::STRING_LITERAL)
        {
            let (reduced, is_empty_set) = self.extract_redundant_template_literals(type_set);
            if is_empty_set {
                return self.never_type();
            }
            type_set = reduced;
        }
        if includes.intersects(TypeFlags::ANY) {
            if includes.intersects(TypeFlags::INCLUDES_WILDCARD) {
                return self.wildcard_type();
            }
            if includes.intersects(TypeFlags::INCLUDES_ERROR) {
                return self.error_type();
            }
            return self.any_type();
        }
        if !self.strict_null_checks && includes.intersects(TypeFlags::NULLABLE) {
            if includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT) {
                return self.never_type();
            }
            if includes.intersects(TypeFlags::UNDEFINED) {
                return self.undefined_type();
            }
            return self.null_type();
        }
        if includes.intersects(TypeFlags::STRING)
            && includes.intersects(
                TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
            )
            || includes.intersects(TypeFlags::NUMBER) && includes.intersects(TypeFlags::NUMBER_LITERAL)
            || includes.intersects(TypeFlags::BIGINT) && includes.intersects(TypeFlags::BIGINT_LITERAL)
            || includes.intersects(TypeFlags::ESSYMBOL) && includes.intersects(TypeFlags::UNIQUE_ESSYMBOL)
            || includes.intersects(TypeFlags::VOID) && includes.intersects(TypeFlags::UNDEFINED)
            || includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT)
                && includes.intersects(TypeFlags::DEFINITELY_NON_NULLABLE)
            && !flags.intersects(IntersectionFlags::NO_SUPERTYPE_REDUCTION)
        {
            type_set = self.remove_redundant_supertypes(type_set, includes);
        }
        if includes.intersects(TypeFlags::INCLUDES_MISSING_TYPE) {
            let undefined_type = self.undefined_type();
            let missing_type = self.missing_type();
            for t in &mut type_set {
                if *t == undefined_type {
                    *t = missing_type;
                    break;
                }
            }
        }
        if type_set.is_empty() {
            return self.unknown_type();
        }
        if type_set.len() == 1 {
            return type_set[0];
        }
        if type_set.len() == 2 && !flags.intersects(IntersectionFlags::NO_CONSTRAINT_REDUCTION) {
            let mut type_var_index = 0;
            if !self
                .type_(type_set[0])
                .flags
                .intersects(TypeFlags::TYPE_VARIABLE)
            {
                type_var_index = 1;
            }
            let type_variable = type_set[type_var_index];
            let primitive_type = type_set[1 - type_var_index];
            // We have an intersection T & P or P & T, where T is a type
            // variable and P is a primitive type, the object type, or {}.
            if self.type_(type_variable).flags.intersects(TypeFlags::TYPE_VARIABLE)
                && ((self
                    .type_(primitive_type)
                    .flags
                    .intersects(TypeFlags::PRIMITIVE | TypeFlags::NON_PRIMITIVE)
                    && !is_generic_string_like_type(self, primitive_type))
                    || includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT))
            {
                let constraint = crate::relater::get_base_constraint_of_type(self, type_variable);
                let constraint_ok = constraint.is_some_and(|constraint| {
                    let constituents = if type_flags(self, constraint).intersects(TypeFlags::UNION) {
                        self.type_(constraint).types().to_vec()
                    } else {
                        vec![constraint]
                    };
                    constituents.iter().all(|&t| self.is_primitive_or_object_or_empty_type(t))
                });
                if constraint_ok {
                    let constraint = crate::relater::get_base_constraint_of_type(self, type_variable)
                        .expect("just checked");
                    // If T's constraint is a subtype of P, simply return T.
                    if self.is_type_strict_subtype_of(constraint, primitive_type) {
                        return type_variable;
                    }
                    let has_subtype_constituent = if type_flags(self, constraint).intersects(TypeFlags::UNION) {
                        let constituents = self.type_(constraint).types().to_vec();
                        constituents
                            .iter()
                            .any(|&n| self.is_type_strict_subtype_of(n, primitive_type))
                    } else {
                        self.is_type_strict_subtype_of(constraint, primitive_type)
                    };
                    if !has_subtype_constituent {
                        // No constituent of T's constraint is a subtype of
                        // P. If P is also not a subtype of T's constraint,
                        // then the intersection reduces to never.
                        if !self.is_type_strict_subtype_of(primitive_type, constraint) {
                            return self.never_type();
                        }
                    }
                    // The intersection further constrains the type variable.
                    object_flags = ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE;
                }
            }
        }
        let key = crate::typeofsymbol::get_intersection_key(&type_set, flags, alias.as_deref());
        let result = match self.intersection_types.get(&key) {
            Some(&t) => t,
            None => {
                let result = if includes.intersects(TypeFlags::UNION) {
                    let (reduced, was_reduced) = self.intersect_unions_of_primitive_types(type_set.clone());
                    if was_reduced {
                        // When the intersection creates a reduced set we
                        // restart the operation to get a new set of combined
                        // flags (Go: occurs at most once).
                        self.get_intersection_type_ex(reduced, flags, alias.clone())
                    } else if type_set.iter().all(|&t| is_union_with_undefined(self, t)) {
                        let contained_undefined_type = if type_set.iter().any(|&t| self.contains_missing_type(t)) {
                            self.missing_type()
                        } else {
                            self.undefined_type()
                        };
                        let filtered: Vec<TypeId> = type_set
                            .iter()
                            .map(|&t| self.filter_type(t, is_not_undefined_type))
                            .collect();
                        let filtered_intersection = self.get_intersection_type_ex(filtered, flags, None);
                        self.get_union_type_ex(
                            vec![
                                filtered_intersection,
                                contained_undefined_type,
                            ],
                            UnionReduction::Literal,
                            alias.clone(),
                            None,
                        )
                    } else if type_set.iter().all(|&t| is_union_with_null(self, t)) {
                        let filtered: Vec<TypeId> = type_set
                            .iter()
                            .map(|&t| self.filter_type(t, is_not_null_type))
                            .collect();
                        let filtered_intersection = self.get_intersection_type_ex(filtered, flags, None);
                        self.get_union_type_ex(
                            vec![
                                filtered_intersection,
                                self.null_type(),
                            ],
                            UnionReduction::Literal,
                            alias.clone(),
                            None,
                        )
                    } else if type_set.len() >= 3 && types.len() > 2 {
                        // Divide and conquer: A & B & C & D as (A & B) & (C & D).
                        let middle = type_set.len() / 2;
                        let first: Vec<TypeId> = type_set[..middle].to_vec();
                        let second: Vec<TypeId> = type_set[middle..].to_vec();
                        let a = self.get_intersection_type_ex(first, flags, None);
                        let b = self.get_intersection_type_ex(second, flags, None);
                        self.get_intersection_type_ex(vec![a, b], flags, alias.clone())
                    } else {
                        // X & (A | B) & (C | D) → the cross-product union.
                        if !self.check_cross_product_union(&type_set) {
                            return self.error_type();
                        }
                        let constituents = self.get_cross_product_intersections(&type_set, flags);
                        // We attach a denormalized origin type when at least
                        // one constituent of the cross-product union is an
                        // intersection and the denormalized origin has fewer
                        // constituents than the union itself.
                        let mut origin: Option<TypeId> = None;
                        if constituents.iter().any(|&t| type_flags(self, t).intersects(TypeFlags::INTERSECTION))
                            && get_constituent_count_of_types(self, &constituents)
                                > get_constituent_count_of_types(self, &type_set)
                        {
                            origin = Some(self.new_intersection_type(ObjectFlags::NONE, type_set.clone()));
                        }
                        self.get_union_type_ex(constituents, UnionReduction::Literal, alias.clone(), origin)
                    }
                } else {
                    let propagating = self.get_propagating_flags_of_types(&types, TypeFlags::NULLABLE);
                    let t = self.new_intersection_type(object_flags | propagating, type_set.clone());
                    self.type_mut(t).alias = alias.clone();
                    t
                };
                self.intersection_types.insert(key, result);
                return result;
            }
        };
        result
    }

    /// Go: `func (c *Checker) addTypesToIntersection(typeSet *orderedSet[*Type], includes TypeFlags, types []*Type) TypeFlags`
    /// (checker.go:26787).
    fn add_types_to_intersection(
        &mut self,
        type_set: &mut Vec<TypeId>,
        mut includes: TypeFlags,
        types: &[TypeId],
    ) -> TypeFlags {
        for &t in types {
            let regular = self.get_regular_type_of_literal_type(t);
            includes = self.add_type_to_intersection(type_set, includes, regular);
        }
        includes
    }

    /// Go: `func (c *Checker) addTypeToIntersection(typeSet *orderedSet[*Type], includes TypeFlags, t *Type) TypeFlags`
    /// (checker.go:26796).
    fn add_type_to_intersection(
        &mut self,
        type_set: &mut Vec<TypeId>,
        mut includes: TypeFlags,
        t: TypeId,
    ) -> TypeFlags {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::INTERSECTION) {
            let nested = self.type_(t).as_intersection_type().expect("IntersectionType data").base.types.clone();
            return self.add_types_to_intersection(type_set, includes, &nested);
        }
        if self.is_empty_anonymous_object_type(t) {
            if !includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT) {
                includes |= TypeFlags::INCLUDES_EMPTY_OBJECT;
                type_set.push(t);
            }
        } else {
            if flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                if t == self.wildcard_type() {
                    includes |= TypeFlags::INCLUDES_WILDCARD;
                }
                if self.is_error_type(t) {
                    includes |= TypeFlags::INCLUDES_ERROR;
                }
            } else if self.strict_null_checks || !flags.intersects(TypeFlags::NULLABLE) {
                let mut t = t;
                if t == self.missing_type() {
                    includes |= TypeFlags::INCLUDES_MISSING_TYPE;
                    t = self.undefined_type();
                }
                if !type_set.contains(&t) {
                    if self.type_(t).flags.intersects(TypeFlags::UNIT) && includes.intersects(TypeFlags::UNIT) {
                        // We have seen two distinct unit types which means we
                        // should reduce to an empty intersection. Adding
                        // TypeFlags.NonPrimitive causes that to happen.
                        includes |= TypeFlags::NON_PRIMITIVE;
                    }
                    type_set.push(t);
                }
            }
            includes |= flags & TypeFlags::INCLUDES_MASK;
        }
        includes
    }

    /// Go: `func (c *Checker) removeRedundantSupertypes(types []*Type, includes TypeFlags) []*Type`
    /// (checker.go:26829).
    fn remove_redundant_supertypes(&mut self, mut types: Vec<TypeId>, includes: TypeFlags) -> Vec<TypeId> {
        let mut i = types.len();
        while i > 0 {
            i -= 1;
            let t = types[i];
            let flags = self.type_(t).flags;
            let remove = (flags.intersects(TypeFlags::STRING)
                && includes.intersects(
                    TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
                ))
                || (flags.intersects(TypeFlags::NUMBER) && includes.intersects(TypeFlags::NUMBER_LITERAL))
                || (flags.intersects(TypeFlags::BIGINT) && includes.intersects(TypeFlags::BIGINT_LITERAL))
                || (flags.intersects(TypeFlags::ESSYMBOL) && includes.intersects(TypeFlags::UNIQUE_ESSYMBOL))
                || (flags.intersects(TypeFlags::VOID) && includes.intersects(TypeFlags::UNDEFINED))
                || (self.is_empty_anonymous_object_type(t)
                    && includes.intersects(TypeFlags::DEFINITELY_NON_NULLABLE));
            if remove {
                types.remove(i);
            }
        }
        types
    }

    /// Go: `func (c *Checker) extractRedundantTemplateLiterals(types []*Type) ([]*Type, bool)`
    /// (checker.go:26863).
    fn extract_redundant_template_literals(&mut self, mut types: Vec<TypeId>) -> (Vec<TypeId>, bool) {
        let literals: Vec<TypeId> = types
            .iter()
            .copied()
            .filter(|&t| self.type_(t).flags.intersects(TypeFlags::STRING_LITERAL))
            .collect();
        let mut i = types.len();
        while i > 0 {
            i -= 1;
            let t = types[i];
            if !self
                .type_(t)
                .flags
                .intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
            {
                continue;
            }
            for &t2 in &literals {
                if self.is_type_subtype_of(t2, t) {
                    // For example, `get${T}` & "getX" is just "getX".
                    types.remove(i);
                    break;
                }
                if crate::typenodes::is_pattern_literal_type(self, t) {
                    return (types, true);
                }
            }
        }
        (types, false)
    }

    /// Go: `func (c *Checker) intersectUnionsOfPrimitiveTypes(types []*Type) ([]*Type, bool)`
    /// (checker.go:26890).
    fn intersect_unions_of_primitive_types(&mut self, mut types: Vec<TypeId>) -> (Vec<TypeId>, bool) {
        let index = types
            .iter()
            .position(|&t| self.type_(t).object_flags.intersects(ObjectFlags::PRIMITIVE_UNION));
        let Some(index) = index else {
            return (types, false);
        };
        // Remove all but the first union of primitive types.
        let mut union_types: Vec<TypeId> = vec![types[index]];
        let mut i = index + 1;
        while i < types.len() {
            let t = types[i];
            if self.type_(t).object_flags.intersects(ObjectFlags::PRIMITIVE_UNION) {
                union_types.push(t);
                types.remove(i);
            } else {
                i += 1;
            }
        }
        if union_types.len() == 1 {
            return (types, false);
        }
        // We have more than one union of primitive types, now intersect them.
        let mut checked: Vec<TypeId> = Vec::new();
        let mut result: Vec<TypeId> = Vec::new();
        for &u in &union_types {
            let constituents = self.type_(u).types().to_vec();
            for t in constituents {
                let (inserted, replaced) = insert_type(self, std::mem::take(&mut checked), t);
                checked = replaced;
                if inserted && self.each_union_contains(&union_types, t) {
                    // undefinedType/missingType are always sorted first so we
                    // leverage that here (Go's comment).
                    let undefined_type = self.undefined_type();
                    let missing_type = self.missing_type();
                    if t == undefined_type && !result.is_empty() && result[0] == missing_type {
                        continue;
                    }
                    if t == missing_type && !result.is_empty() && result[0] == undefined_type {
                        result[0] = missing_type;
                        continue;
                    }
                    let (_ins, res) = insert_type(self, std::mem::take(&mut result), t);
                    result = res;
                }
            }
        }
        // Finally replace the first union with the result
        let union = self.get_union_type_from_sorted_list(result, ObjectFlags::PRIMITIVE_UNION, None, None);
        types[index] = union;
        // `index` is only mutated by the removal loop above; silence the
        // unused-assignment lint Go's control flow produces after the loop.
        let _ = index;
        (types, true)
    }

    /// Go: `func (c *Checker) eachUnionContains(unionTypes []*Type, t *Type) bool`
    /// (checker.go:26931).
    fn each_union_contains(&mut self, union_types: &[TypeId], t: TypeId) -> bool {
        union_types
            .iter()
            .all(|&u| self.union_contains_type(u, t, true))
    }

    /// Go: `func (c *Checker) unionContainsType(union *Type, t *Type, matchSymbol bool) bool`
    /// (checker.go:26938).
    pub(crate) fn union_contains_type(&mut self, union: TypeId, t: TypeId, match_symbol: bool) -> bool {
        let types = self.type_(union).types().to_vec();
        if contains_type(self, &types, t) {
            return true;
        }
        if t == self.missing_type() {
            return types.contains(&self.undefined_type());
        }
        if t == self.undefined_type() {
            return types.contains(&self.missing_type());
        }
        let flags = self.type_(t).flags;
        let primitive = if flags.intersects(TypeFlags::STRING_LITERAL) {
            Some(self.string_type())
        } else if flags.intersects(TypeFlags::ENUM | TypeFlags::NUMBER_LITERAL) {
            Some(self.number_type())
        } else if flags.intersects(TypeFlags::BIGINT_LITERAL) {
            Some(self.bigint_type())
        } else if flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) && match_symbol {
            Some(self.es_symbol_type())
        } else {
            None
        };
        primitive.is_some_and(|p| types.contains(&p))
    }

    /// Go: `func (c *Checker) getCrossProductIntersections(types []*Type, flags IntersectionFlags) []*Type`
    /// (checker.go:26962).
    fn get_cross_product_intersections(&mut self, types: &[TypeId], flags: IntersectionFlags) -> Vec<TypeId> {
        let count = self.get_cross_product_union_size(types);
        let mut intersections: Vec<TypeId> = Vec::new();
        for i in 0..count {
            let mut constituents = types.to_vec();
            let mut n = i;
            for j in (0..types.len()).rev() {
                if self.type_(types[j]).flags.intersects(TypeFlags::UNION) {
                    let source_types = self.type_(types[j]).types().to_vec();
                    let length = source_types.len();
                    constituents[j] = source_types[n % length];
                    n /= length;
                }
            }
            let t = self.get_intersection_type_ex(constituents, flags, None);
            if !self.type_(t).flags.intersects(TypeFlags::NEVER) {
                intersections.push(t);
            }
        }
        intersections
    }

    /// Go: `func (c *Checker) checkCrossProductUnion(types []*Type) bool`
    /// (checker.go:27275).
    pub(crate) fn check_cross_product_union(&mut self, types: &[TypeId]) -> bool {
        let size = self.get_cross_product_union_size(types);
        if size >= 100_000 {
            self.error(
                self.current_node.unwrap_or(NodeId::NONE),
                &tsc_diagnostics::EXPRESSION_PRODUCES_A_UNION_TYPE_THAT_IS_TOO_COMPLEX_TO_REPRESENT,
                &[],
            );
            return false;
        }
        true
    }

    /// Go: `func (c *Checker) getCrossProductUnionSize(types []*Type) int`
    /// (checker.go:27286) — saturating (Go caps at MaxInt to avoid overflow).
    pub(crate) fn get_cross_product_union_size(&mut self, types: &[TypeId]) -> usize {
        let mut size: usize = 1;
        for &t in types {
            let flags = self.type_(t).flags;
            if flags.intersects(TypeFlags::UNION) {
                let n = self.type_(t).types().len();
                if n > 0 && size > usize::MAX / n {
                    return usize::MAX;
                }
                size *= n;
            } else if flags.intersects(TypeFlags::NEVER) {
                return 0;
            }
        }
        size
    }

    /// Go: `func (c *Checker) IsEmptyAnonymousObjectType(t *Type) bool`
    /// (checker.go:27010) — REAL (M6 S7).
    pub(crate) fn is_empty_anonymous_object_type(&mut self, t: TypeId) -> bool {
        let ty_flags = self.type_(t).flags;
        if !self.type_(t).object_flags.intersects(ObjectFlags::ANONYMOUS) {
            return false;
        }
        if self.type_(t).object_flags.intersects(ObjectFlags::MEMBERS_RESOLVED) {
            let empty = self
                .type_(t)
                .as_structured_type()
                .map(|d| {
                    Some(t) != self.any_function_type
                        && d.properties.is_empty()
                        && d.signatures.is_empty()
                        && d.index_infos.is_empty()
                })
                .unwrap_or(false);
            return empty;
        }
        let _ = ty_flags;
        if let Some(symbol) = self.type_(t).symbol {
            if self.program_symbol(symbol).flags.intersects(tsc_ast::SymbolFlags::TYPE_LITERAL) {
                let members = self.get_members_of_symbol(symbol);
                return members.is_empty();
            }
        }
        false
    }

    /// Go: `func (c *Checker) isEmptyResolvedType(t *StructuredType) bool` —
    /// over the S8 resolveStructuredTypeMembers carrier.
    pub(crate) fn is_empty_resolved_type_of(&mut self, members: crate::relater::ResolvedTypeMembers) -> bool {
        let _ = members;
        // TODO(port M6 S8): the members carrier's emptiness read — the S8
        // stub resolves to no members, so every object type reads empty
        // exactly like Go's not-yet-resolved types.
        true
    }

    /// Go: `func (c *Checker) isEmptyObjectType(t *Type) bool` (checker.go:27025).
    pub(crate) fn is_empty_object_type(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::OBJECT) {
            let members = crate::relater::resolve_structured_type_members(self, t);
            return !crate::relater::is_generic_mapped_type(self, t)
                && members.properties.is_empty()
                && members.signatures.is_empty()
                && members.index_infos.is_empty();
        }
        if flags.intersects(TypeFlags::NON_PRIMITIVE) {
            return true;
        }
        if flags.intersects(TypeFlags::UNION) {
            let constituents = self.type_(t).types().to_vec();
            return constituents.iter().any(|&u| self.is_empty_object_type(u));
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self.type_(t).types().to_vec();
            return constituents.iter().all(|&u| self.is_empty_object_type(u));
        }
        false
    }

    /// Go: the NewChecker closure `c.isPrimitiveOrObjectOrEmptyType`
    /// (checker.go:1306).
    pub(crate) fn is_primitive_or_object_or_empty_type(&mut self, t: TypeId) -> bool {
        self.type_(t)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::NON_PRIMITIVE)
            || self.is_empty_anonymous_object_type(t)
    }

}

// Go: the NewChecker closure `c.containsMissingType` (checker.go:1309) —
// expressions.rs owns the identical real body; not duplicated here.

/// Go: `func isUnionWithUndefined(t *Type) bool` (checker.go:26771).
fn is_union_with_undefined(c: &Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(TypeFlags::UNION)
        && c.type_(t)
            .types()
            .first()
            .is_some_and(|&f| c.type_(f).flags.intersects(TypeFlags::UNDEFINED))
}

/// Go: `func isUnionWithNull(t *Type) bool` (checker.go:26776).
fn is_union_with_null(c: &Checker, t: TypeId) -> bool {
    if !c.type_(t).flags.intersects(TypeFlags::UNION) {
        return false;
    }
    let types = c.type_(t).types();
    types.len() > 1
        && (c.type_(types[0]).flags.intersects(TypeFlags::NULL)
            || c.type_(types[1]).flags.intersects(TypeFlags::NULL))
}

/// Go: `func isNotUndefinedType(t *Type) bool` (checker.go:26781).
fn is_not_undefined_type(c: &Checker, t: TypeId) -> bool {
    !c.type_(t).flags.intersects(TypeFlags::UNDEFINED)
}

/// Go: `func isNotNullType(t *Type) bool` (checker.go:26785).
fn is_not_null_type(c: &Checker, t: TypeId) -> bool {
    !c.type_(t).flags.intersects(TypeFlags::NULL)
}

/// Go: `func (c *Checker) isGenericStringLikeType(t *Type) bool` — the
/// checker.go:27057 real body composed over the S10 pattern-literal probe
/// (typenodes.rs's stub; permissive until that wave).
fn is_generic_string_like_type(c: &mut Checker, t: TypeId) -> bool {
    c.type_(t)
        .flags
        .intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
        && !crate::typenodes::is_pattern_literal_type(c, t)
}

// ────────────────────────────────────────────────────────────────────────────
// mapType / filterType / removeType (Go checker.go 26990-27130)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) mapTypeWithAlias(t *Type, f func(*Type) *Type, alias *TypeAlias) *Type`
    /// (checker.go:26110).
    pub(crate) fn map_type_with_alias(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> Option<TypeId>,
        alias: Option<&TypeAlias>,
    ) -> Option<TypeId> {
        if self.type_(t).flags.intersects(TypeFlags::UNION) && alias.is_some() {
            let types = self.type_(t).types().to_vec();
            let mut mapped: Vec<TypeId> = Vec::with_capacity(types.len());
            for s in types {
                if let Some(m) = f(self, s) {
                    mapped.push(m);
                }
            }
            return Some(
                self.get_union_type_ex(
                    mapped,
                    UnionReduction::Literal,
                    alias.map(|a| Box::new(a.clone())),
                    None,
                ),
            );
        }
        self.map_type(t, f)
    }

    /// Go: `func (c *Checker) mapType(t *Type, f func(*Type) *Type) *Type`
    /// (checker.go:26116) — Go's `f` may return nil; the port surfaces that
    /// as `Option<TypeId>` (see mapTypeEx's len==0 → nil arm).
    pub(crate) fn map_type(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> Option<TypeId>,
    ) -> Option<TypeId> {
        self.map_type_ex(t, f, false)
    }

    /// Go: `func (c *Checker) mapTypeEx(t *Type, f func(*Type) *Type, noReductions bool) *Type`
    /// (checker.go:26120).
    pub(crate) fn map_type_ex(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> Option<TypeId>,
        no_reductions: bool,
    ) -> Option<TypeId> {
        if self.type_(t).flags.intersects(TypeFlags::NEVER) {
            return Some(t);
        }
        if !self.type_(t).flags.intersects(TypeFlags::UNION) {
            return f(self, t);
        }
        let mut types = self.type_(t).as_union_type().expect("UnionType data").base.types.clone();
        if let Some(origin) = self
            .type_(t)
            .as_union_type()
            .expect("UnionType data")
            .origin
            .filter(|&origin| self.type_(origin).flags.intersects(TypeFlags::UNION))
        {
            types = self.type_(origin).types().to_vec();
        }
        let mut mapped_types: Vec<TypeId> = Vec::with_capacity(16);
        let mut changed = false;
        for s in types {
            let mapped = if self.type_(s).flags.intersects(TypeFlags::UNION) {
                self.map_type_ex(s, f, no_reductions)
            } else {
                f(self, s)
            };
            if mapped != Some(s) {
                changed = true;
            }
            if let Some(mapped) = mapped {
                mapped_types.push(mapped);
            }
        }
        if changed {
            if mapped_types.is_empty() {
                return None;
            }
            return Some(self.get_union_type_ex(
                mapped_types,
                if no_reductions {
                    UnionReduction::None
                } else {
                    UnionReduction::Literal
                },
                None,
                None,
            ));
        }
        Some(t)
    }

    /// Go: `func (c *Checker) filterTypes(types []*Type, predicate func(*Type) bool)`
    /// (checker.go:27004).
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn filter_types(&mut self, types: &[TypeId], predicate: &dyn Fn(&mut Checker, TypeId) -> bool) {
        // PORT: Go filters in place on the shared backing array; the port
        // returns the filtered list (callers assign).
        let _ = predicate;
        let _ = types;
    }

    /// Go: `func (c *Checker) filterType(t *Type, f func(*Type) bool) *Type`
    /// (checker.go:27016).
    pub(crate) fn filter_type(&mut self, t: TypeId, f: impl Fn(&Checker, TypeId) -> bool) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            let types = self.type_(t).types().to_vec();
            let filtered: Vec<TypeId> = types.iter().copied().filter(|&u| f(self, u)).collect();
            if types == filtered {
                return t;
            }
            let origin = self
                .type_(t)
                .as_union_type()
                .expect("UnionType data")
                .origin;
            let mut new_origin: Option<TypeId> = None;
            if let Some(origin) =
                origin.filter(|&origin| self.type_(origin).flags.intersects(TypeFlags::UNION))
            {
                // If the origin type is a (denormalized) union type, filter
                // its non-union constituents (Go's comment, checker.go:27029).
                let origin_types = self.type_(origin).types().to_vec();
                let origin_filtered: Vec<TypeId> = origin_types
                    .iter()
                    .copied()
                    .filter(|&u| {
                        self.type_(u).flags.intersects(TypeFlags::UNION) || f(self, u)
                    })
                    .collect();
                if origin_types.len() - origin_filtered.len() == types.len() - filtered.len() {
                    if origin_filtered.len() == 1 {
                        return origin_filtered[0];
                    }
                    new_origin = Some(self.new_union_type(ObjectFlags::NONE, origin_filtered));
                }
            }
            let object_flags = self.type_(t).object_flags
                & (ObjectFlags::PRIMITIVE_UNION | ObjectFlags::CONTAINS_INTERSECTIONS);
            return self.get_union_type_from_sorted_list(filtered, object_flags, None, new_origin);
        }
        if self.type_(t).flags.intersects(TypeFlags::NEVER) || f(self, t) {
            return t;
        }
        self.never_type()
    }
    /// Go: `func (c *Checker) filterType(t *Type, f func(*Type) bool)`
    /// (checker.go:27016) — the `&mut Checker`-taking twin (M6 S10: the
    /// facts-lattice predicates resolve members through the base-constraint
    /// engine, so the `&Checker` form above can't carry them). The body is
    /// Go-identical to [`Self::filter_type`].
    pub(crate) fn filter_type_mut(
        &mut self,
        t: TypeId,
        mut f: impl FnMut(&mut Checker, TypeId) -> bool,
    ) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            let types = self.type_(t).types().to_vec();
            let mut filtered: Vec<TypeId> = Vec::with_capacity(types.len());
            for &u in &types {
                if f(self, u) {
                    filtered.push(u);
                }
            }
            if types == filtered {
                return t;
            }
            let origin = self
                .type_(t)
                .as_union_type()
                .expect("UnionType data")
                .origin;
            let mut new_origin: Option<TypeId> = None;
            if let Some(origin) =
                origin.filter(|&origin| self.type_(origin).flags.intersects(TypeFlags::UNION))
            {
                // If the origin type is a (denormalized) union type, filter
                // its non-union constituents (Go's comment, checker.go:27029).
                let origin_types = self.type_(origin).types().to_vec();
                let mut origin_filtered: Vec<TypeId> = Vec::with_capacity(origin_types.len());
                for &u in &origin_types {
                    if self.type_(u).flags.intersects(TypeFlags::UNION) || f(self, u) {
                        origin_filtered.push(u);
                    }
                }
                if origin_types.len() - origin_filtered.len() == types.len() - filtered.len() {
                    if origin_filtered.len() == 1 {
                        return origin_filtered[0];
                    }
                    new_origin = Some(self.new_union_type(ObjectFlags::NONE, origin_filtered));
                }
            }
            let object_flags = self.type_(t).object_flags
                & (ObjectFlags::PRIMITIVE_UNION | ObjectFlags::CONTAINS_INTERSECTIONS);
            return self.get_union_type_from_sorted_list(filtered, object_flags, None, new_origin);
        }
        if self.type_(t).flags.intersects(TypeFlags::NEVER) || f(self, t) {
            return t;
        }
        self.never_type()
    }

    /// Go: `func (c *Checker) removeType(t *Type, targetType *Type) *Type`
    /// (checker.go:27071).
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn remove_type(&mut self, t: TypeId, target_type: TypeId) -> TypeId {
        if !self.type_(t).flags.intersects(TypeFlags::UNION) {
            if t == target_type {
                return self.never_type();
            }
            return t;
        }
        if let Some(origin) = self
            .type_(t)
            .as_union_type()
            .expect("UnionType data")
            .origin
            .filter(|&origin| self.type_(origin).flags.intersects(TypeFlags::UNION))
            && contains_type(self, self.type_(origin).types(), target_type)
        {
            let result = self.filter_type(t, |_c, u| u != target_type);
            let _ = origin;
            return result;
        }
        let types = self.type_(t).types().to_vec();
        match types.binary_search_by(|probe| u::compare_types(self, *probe, target_type)) {
            Ok(i) => {
                if types.len() == 2 {
                    return types[1 - i];
                }
                let mut filtered = types.clone();
                filtered.remove(i);
                let object_flags = self.type_(t).object_flags
                    & (ObjectFlags::PRIMITIVE_UNION | ObjectFlags::CONTAINS_INTERSECTIONS);
                self.get_union_type_from_sorted_list(filtered, object_flags, None, None)
            }
            Err(_) => t,
        }
    }

    /// Go: `func (c *Checker) getRegularTypeOfLiteralType(t *Type) *Type`
    /// (checker.go:25799) — REAL union arm (M6 S7; replaces checker.rs's
    /// panicking seam).
    pub fn get_regular_type_of_literal_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::FRESHABLE) {
            return self
                .type_(t)
                .as_literal_type()
                .and_then(|d| d.regular_type)
                .unwrap_or(t);
        }
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            if let Some(regular) = self
                .type_(t)
                .as_union_type()
                .expect("UnionType data")
                .regular_type
            {
                return regular;
            }
            let regular = self
                .map_type(t, &mut |c, u| Some(c.get_regular_type_of_literal_type(u)))
                .unwrap_or(t);
            if let Some(data) = self.type_mut(t).as_union_type_mut() {
                data.regular_type = Some(regular);
            }
            return regular;
        }
        t
    }

    /// Go: `func (c *Checker) getBaseTypeOfLiteralType(t *Type) *Type`
    /// (checker.go:25968) — REAL (M6 S7).
    pub fn get_base_type_of_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::ENUM_LIKE) {
            return self.get_base_type_of_enum_like_type(t).unwrap_or(t);
        }
        if flags.intersects(
            TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
        ) {
            return self.string_type();
        }
        if flags.intersects(TypeFlags::NUMBER_LITERAL) {
            return self.number_type();
        }
        if flags.intersects(TypeFlags::BIGINT_LITERAL) {
            return self.bigint_type();
        }
        if flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
            return self.boolean_type();
        }
        if flags.intersects(TypeFlags::UNION) {
            return self.get_base_type_of_literal_type_union(t);
        }
        t
    }

    /// Go: `func (c *Checker) getBaseTypeOfLiteralTypeUnion(t *Type) *Type`
    /// (checker.go:25998) — the cachedTypes memo.
    fn get_base_type_of_literal_type_union(&mut self, t: TypeId) -> TypeId {
        let key = CachedTypeKey {
            kind: CachedTypeKind::LiteralUnionBaseType,
            type_id: t.0,
        };
        if let Some(Some(cached)) = self.cached_types.get(&key) {
            return *cached;
        }
        let result = self
            .map_type(t, &mut |c, u| Some(c.get_base_type_of_literal_type(u)))
            .unwrap_or(t);
        self.cached_types.insert(key, Some(result));
        result
    }

    /// Go: `func (c *Checker) getWidenedLiteralType(t *Type) *Type`
    /// (checker.go:26015) — REAL (M6 S7).
    pub(crate) fn get_widened_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::ENUM_LIKE) && crate::is_fresh_literal_type(self, t) {
            return self.get_base_type_of_enum_like_type(t).unwrap_or(t);
        }
        if flags.intersects(TypeFlags::STRING_LITERAL) && crate::is_fresh_literal_type(self, t) {
            return self.string_type();
        }
        if flags.intersects(TypeFlags::NUMBER_LITERAL) && crate::is_fresh_literal_type(self, t) {
            return self.number_type();
        }
        if flags.intersects(TypeFlags::BIGINT_LITERAL) && crate::is_fresh_literal_type(self, t) {
            return self.bigint_type();
        }
        if flags.intersects(TypeFlags::BOOLEAN_LITERAL) && crate::is_fresh_literal_type(self, t) {
            return self.boolean_type();
        }
        if flags.intersects(TypeFlags::UNION) {
            return self
                .map_type(t, &mut |c, u| Some(c.get_widened_literal_type(u)))
                .unwrap_or(t);
        }
        t
    }

    /// Go: `func (c *Checker) getWidenedUniqueESSymbolType(t *Type) *Type`
    /// (checker.go:26038).
    pub(crate) fn get_widened_unique_es_symbol_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
            return self.es_symbol_type();
        }
        if flags.intersects(TypeFlags::UNION) {
            return self
                .map_type(t, &mut |c, u| Some(c.get_widened_unique_es_symbol_type(u)))
                .unwrap_or(t);
        }
        t
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Template-literal + string-mapping construction (Go checker.go 29682-29825)
// ────────────────────────────────────────────────────────────────────────────

/// Go: the `maxTemplateLiteralTypeLength`/`maxTemplateLiteralTypeSpans`
/// limits (checker.go:29677).
const MAX_TEMPLATE_LITERAL_TYPE_LENGTH: usize = 50_000_000;
const MAX_TEMPLATE_LITERAL_TYPE_SPANS: usize = 100_000;

impl Checker {
    /// Go: `func (c *Checker) getTemplateLiteralType(texts []string, types []*Type) *Type`
    /// (checker.go:29682) — REAL (M6 S7).
    pub fn get_template_literal_type(&mut self, texts: &[String], types: &[TypeId]) -> TypeId {
        let union_index = types.iter().position(|&t| {
            self.type_(t)
                .flags
                .intersects(TypeFlags::NEVER | TypeFlags::UNION)
        });
        if let Some(union_index) = union_index {
            if !self.check_cross_product_union(types) {
                return self.error_type();
            }
            let texts = texts.to_vec();
            let types = types.to_vec();
            let union_type = types[union_index];
            return self
                .map_type(union_type, &mut |c, u| {
                    let mut call_types = types.clone();
                    call_types[union_index] = u;
                    Some(c.get_template_literal_type(&texts, &call_types))
                })
                .unwrap_or_else(|| self.string_type());
        }
        if types.contains(&self.wildcard_type()) {
            return self.wildcard_type();
        }
        let mut new_types: Vec<TypeId> = Vec::new();
        let mut new_texts: Vec<String> = Vec::new();
        let mut sb = texts[0].clone();
        let mut text_length = 0usize;
        let mut too_large = false;
        // Go's recursive `addSpans` closure (checker.go:29708) — a recursive
        // method over the shared builder state.
        if !self.template_add_spans(
            texts,
            types,
            &mut sb,
            &mut new_types,
            &mut new_texts,
            &mut text_length,
            &mut too_large,
        ) {
            if too_large {
                self.error(
                    self.current_node.unwrap_or(NodeId::NONE),
                    &tsc_diagnostics::TYPE_INSTANTIATION_IS_EXCESSIVELY_DEEP_AND_POSSIBLY_INFINITE,
                    &[],
                );
                return self.error_type();
            }
            return self.string_type();
        }
        if new_types.is_empty() {
            return self.get_string_literal_type(&combine_surrogate_pairs(&sb));
        }
        new_texts.push(combine_surrogate_pairs(&sb));
        if new_texts.iter().all(|t| t.is_empty()) {
            if new_types.iter().all(|&t| self.type_(t).flags.intersects(TypeFlags::STRING)) {
                return self.string_type();
            }
            // Normalize `${Mapping<xxx>}` into Mapping<xxx>
            if new_types.len() == 1 && crate::typenodes::is_pattern_literal_type(self, new_types[0]) {
                return new_types[0];
            }
        }
        let key = crate::typeofsymbol::get_template_type_key(&new_texts, &new_types);
        if let Some(&t) = self.template_literal_types.get(&key) {
            return t;
        }
        let t = self.new_template_literal_type(new_texts, new_types);
        self.template_literal_types.insert(key, t);
        t
    }

    /// Go: the recursive `addSpans` closure inside getTemplateLiteralType
    /// (checker.go:29708) — split into a recursive method so the shared
    /// builder state threads through (PORT note; same evaluation order).
    #[allow(clippy::too_many_arguments)] // Go: addSpans carries the same eight parameters
    fn template_add_spans(
        &mut self,
        texts: &[String],
        types: &[TypeId],
        sb: &mut String,
        new_types: &mut Vec<TypeId>,
        new_texts: &mut Vec<String>,
        text_length: &mut usize,
        too_large: &mut bool,
    ) -> bool {
        for (i, &t) in types.iter().enumerate() {
            let flags = self.type_(t).flags;
            if flags.intersects(TypeFlags::LITERAL | TypeFlags::NULL | TypeFlags::UNDEFINED) {
                let s = self.get_template_string_for_type(t);
                sb.push_str(&s);
                sb.push_str(&texts[i + 1]);
            } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let (t_texts, t_types) = {
                    let d = self
                        .type_(t)
                        .as_template_literal_type()
                        .expect("TemplateLiteralType data");
                    (d.texts.clone(), d.types.clone())
                };
                sb.push_str(&t_texts[0]);
                if !self.template_add_spans(
                    &t_texts,
                    &t_types,
                    sb,
                    new_types,
                    new_texts,
                    text_length,
                    too_large,
                ) {
                    return false;
                }
                sb.push_str(&texts[i + 1]);
            } else if crate::relater::is_generic_index_type(self, t)
                || self.is_pattern_literal_placeholder_type(t)
            {
                new_types.push(t);
                new_texts.push(combine_surrogate_pairs(sb));
                *text_length += sb.len();
                sb.clear();
                sb.push_str(&texts[i + 1]);
            } else {
                return false;
            }
            if *text_length + sb.len() > MAX_TEMPLATE_LITERAL_TYPE_LENGTH
                || new_types.len() > MAX_TEMPLATE_LITERAL_TYPE_SPANS
            {
                *too_large = true;
                return false;
            }
        }
        true
    }

    /// Go: `func (c *Checker) getTemplateStringForType(t *Type) string`
    /// (checker.go:29762).
    fn get_template_string_for_type(&mut self, t: TypeId) -> String {
        let flags = self.type_(t).flags;
        if flags.intersects(
            TypeFlags::STRING_LITERAL
                | TypeFlags::NUMBER_LITERAL
                | TypeFlags::BOOLEAN_LITERAL
                | TypeFlags::BIGINT_LITERAL,
        ) {
            let value = self
                .type_(t)
                .as_literal_type()
                .and_then(|d| d.value.clone())
                .expect("literal value");
            return match value {
                LiteralValue::String(s) => s,
                LiteralValue::Number(n) => crate::utilities::value_to_string(&LiteralValue::Number(n)),
                LiteralValue::Boolean(b) => if b { "true".to_string() } else { "false".to_string() },
                LiteralValue::BigInt(b) => crate::utilities::value_to_string(&LiteralValue::BigInt(b)),
                LiteralValue::None => String::new(),
            };
        }
        if flags.intersects(TypeFlags::NULLABLE) {
            return self
                .type_(t)
                .as_intrinsic_type()
                .map(|d| d.intrinsic_name.clone())
                .unwrap_or_default();
        }
        String::new()
    }

    /// Go: `func (c *Checker) isPatternLiteralPlaceholderType(t *Type) bool`
    /// (checker.go:27041).
    pub(crate) fn is_pattern_literal_placeholder_type(&mut self, t: TypeId) -> bool {
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            // Return true if the intersection consists of one or more
            // placeholders and zero or more object type tags.
            let mut seen_placeholder = false;
            let constituents = self.type_(t).types().to_vec();
            for s in constituents {
                if self.type_(s)
                    .flags
                    .intersects(TypeFlags::LITERAL | TypeFlags::NULLABLE)
                    || self.is_pattern_literal_placeholder_type(s)
                {
                    seen_placeholder = true;
                } else if !self.type_(s).flags.intersects(TypeFlags::OBJECT) {
                    return false;
                }
            }
            return seen_placeholder;
        }
        self.type_(t)
            .flags
            .intersects(TypeFlags::ANY | TypeFlags::STRING | TypeFlags::NUMBER | TypeFlags::BIGINT)
            || crate::typenodes::is_pattern_literal_type(self, t)
    }

    /// Go: `func (c *Checker) getStringMappingType(symbol *ast.Symbol, t *Type) *Type`
    /// (checker.go:29771) — REAL (M6 S7).
    pub(crate) fn get_string_mapping_type(&mut self, symbol: Option<SymbolId>, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::UNION | TypeFlags::NEVER) {
            return self
                .map_type(t, &mut |c, u| Some(c.get_string_mapping_type(symbol, u)))
                .unwrap_or(t);
        }
        if flags.intersects(TypeFlags::STRING_LITERAL) {
            let value = self
                .type_(t)
                .as_literal_type()
                .and_then(|d| d.value.clone())
                .and_then(|v| match v {
                    LiteralValue::String(s) => Some(s),
                    _ => None,
                })
                .unwrap_or_default();
            let symbol_name = symbol.map(|s| self.program_symbol(s).name.clone());
            return self.get_string_literal_type(&apply_string_mapping(symbol_name.as_deref(), &value));
        }
        if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let (texts, types) = {
                let d = self.type_(t).as_template_literal_type().expect("TemplateLiteralType data");
                (d.texts.clone(), d.types.clone())
            };
            let symbol_name = symbol.map(|s| self.program_symbol(s).name.clone());
            let (new_texts, new_types) =
                self.apply_template_string_mapping(symbol_name.as_deref(), texts, types);
            return self.get_template_literal_type(&new_texts, &new_types);
        }
        if flags.intersects(TypeFlags::STRING_MAPPING) && symbol == self.type_(t).symbol {
            return t;
        }
        if flags.intersects(TypeFlags::ANY | TypeFlags::STRING | TypeFlags::STRING_MAPPING)
            || crate::relater::is_generic_index_type(self, t)
        {
            return self.get_string_mapping_type_for_generic_type(symbol, t);
        }
        if self.is_pattern_literal_placeholder_type(t) {
            let template = self.get_template_literal_type(&[String::new(), String::new()], &[t]);
            return self.get_string_mapping_type_for_generic_type(symbol, template);
        }
        t
    }

    /// Go: `func (c *Checker) getStringMappingTypeForGenericType(symbol *ast.Symbol, t *Type) *Type`
    /// (checker.go:29824).
    fn get_string_mapping_type_for_generic_type(&mut self, symbol: Option<SymbolId>, t: TypeId) -> TypeId {
        let key = (symbol.unwrap_or(SymbolId(0)), t);
        if let Some(&result) = self.string_mapping_types.get(&key) {
            return result;
        }
        let result = self.new_string_mapping_type(symbol, t);
        self.string_mapping_types.insert(key, result);
        result
    }

    /// Go: `func (c *Checker) applyTemplateStringMapping(symbol *ast.Symbol, texts []string, types []*Type) ([]string, []*Type)`
    /// (checker.go:29804).
    fn apply_template_string_mapping(
        &mut self,
        symbol_name: Option<&str>,
        texts: Vec<String>,
        types: Vec<TypeId>,
    ) -> (Vec<String>, Vec<TypeId>) {
        let name = symbol_name.unwrap_or("");
        if matches!(name, "Uppercase" | "Lowercase") {
            let new_texts = texts
                .iter()
                .map(|t| apply_string_mapping(Some(name), t))
                .collect();
            let new_types = types
                .iter()
                .map(|&t| {
                    let symbol = self.type_(t).symbol;
                    self.get_string_mapping_type(symbol, t)
                })
                .collect();
            return (new_texts, new_types);
        }
        if matches!(name, "Capitalize" | "Uncapitalize") {
            if !texts[0].is_empty() {
                let mut new_texts = texts.clone();
                new_texts[0] = apply_string_mapping(Some(name), &new_texts[0]);
                return (new_texts, types);
            }
            let mut new_types = types.clone();
            let symbol = self.type_(new_types[0]).symbol;
            new_types[0] = self.get_string_mapping_type(symbol, new_types[0]);
            return (texts, new_types);
        }
        (texts, types)
    }
}

/// Go: `func applyStringMapping(symbol *ast.Symbol, str string) string`
/// (checker.go:29792).
fn apply_string_mapping(symbol_name: Option<&str>, str_: &str) -> String {
    let bytes = str_.as_bytes();
    match symbol_name.unwrap_or("") {
        "Uppercase" => String::from_utf8_lossy(&tsc_stringutil::to_upper_js(bytes)).into_owned(),
        "Lowercase" => String::from_utf8_lossy(&tsc_stringutil::to_lower_js(bytes)).into_owned(),
        "Capitalize" => {
            let (_, size) = tsc_stringutil::decode_js_string_rune(bytes);
            format!(
                "{}{}",
                String::from_utf8_lossy(&tsc_stringutil::to_upper_js(&bytes[..size])),
                &str_[size..]
            )
        }
        "Uncapitalize" => {
            let (_, size) = tsc_stringutil::decode_js_string_rune(bytes);
            format!(
                "{}{}",
                String::from_utf8_lossy(&tsc_stringutil::to_lower_js(&bytes[..size])),
                &str_[size..]
            )
        }
        _ => str_.to_string(),
    }
}

/// Go: `stringutil.CombineSurrogatePairs` (via the checker's own helper).
fn combine_surrogate_pairs(s: &str) -> String {
    String::from_utf8_lossy(&tsc_stringutil::combine_surrogate_pairs(s.as_bytes())).into_owned()
}

// ────────────────────────────────────────────────────────────────────────────
// The instantiate* engine (Go checker.go 21019-21046, 22559-23290)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getUniqueLiteralTypeForTypeParameter(t *Type) *Type`
    /// (checker.go:25511) — the uniqueLiteralMapper's worker.
    pub(crate) fn get_unique_literal_type_for_type_parameter(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.unique_literal_type();
        }
        t
    }

    /// Go: `func (c *Checker) getRestrictiveTypeParameter(t *Type) *Type`
    /// (checker.go — the restrictiveMapper worker's callee).
    pub(crate) fn get_restrictive_type_parameter(&mut self, t: TypeId) -> TypeId {
        let constraint = self
            .type_(t)
            .as_type_parameter()
            .expect("TypeParameter data")
            .constraint;
        let has_constraint_declaration =
            constraint.is_some() || self.get_constraint_declaration(t).is_some();
        if !has_constraint_declaration || constraint == Some(self.no_constraint_type()) {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::RestrictiveTypeParameter,
            type_id: t.0,
        };
        if let Some(Some(cached)) = self.cached_types.get(&key) {
            return *cached;
        }
        let symbol = self.type_(t).symbol;
        let result = self.new_type_parameter(symbol);
        self.type_mut(result)
            .as_type_parameter_mut()
            .expect("TypeParameter data")
            .constraint = Some(self.no_constraint_type());
        self.cached_types.insert(key, Some(result));
        result
    }

    /// Go: `func (c *Checker) getPermissiveInstantiation(t *Type) *Type`.
    pub(crate) fn get_permissive_instantiation(&mut self, t: TypeId) -> TypeId {
        if self
            .type_(t)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::ANY_OR_UNKNOWN | TypeFlags::NEVER)
        {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::PermissiveInstantiation,
            type_id: t.0,
        };
        if let Some(Some(cached)) = self.cached_types.get(&key) {
            return *cached;
        }
        let mapper = self.permissive_mapper.expect("permissiveMapper");
        let result = self.instantiate_type(t, mapper);
        self.cached_types.insert(key, Some(result));
        result
    }

    /// Go: `func (c *Checker) getRestrictiveInstantiation(t *Type) *Type`.
    pub(crate) fn get_restrictive_instantiation(&mut self, t: TypeId) -> TypeId {
        if self
            .type_(t)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::ANY_OR_UNKNOWN | TypeFlags::NEVER)
        {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::RestrictiveInstantiation,
            type_id: t.0,
        };
        if let Some(Some(cached)) = self.cached_types.get(&key) {
            return *cached;
        }
        let mapper = self.restrictive_mapper.expect("restrictiveMapper");
        let result = self.instantiate_type(t, mapper);
        // Go: the result's own restrictive instantiation is itself — record
        // the redundancy so repeated probes don't mint new identities
        // (checker.go's comment above getRestrictiveInstantiation).
        let restrictive_key = CachedTypeKey {
            kind: CachedTypeKind::RestrictiveInstantiation,
            type_id: result.0,
        };
        self.cached_types.insert(restrictive_key, Some(result));
        self.cached_types.insert(key, Some(result));
        result
    }

    /// Go: `func (c *Checker) getNoInferType(t *Type) *Type` (checker.go:27928).
    pub(crate) fn get_no_infer_type(&mut self, t: TypeId) -> TypeId {
        if self.is_no_infer_target_type(t) {
            let unknown = self.unknown_type();
            return self.get_or_create_substitution_type(t, unknown);
        }
        t
    }

    /// Go: `func (c *Checker) isNoInferTargetType(t *Type) bool` (checker.go:27932).
    fn is_no_infer_target_type(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let snapshot = self.type_(t).types().to_vec();
            return snapshot.iter().any(|&u| self.is_no_infer_target_type(u));
        }
        if flags.intersects(TypeFlags::SUBSTITUTION)
            && !crate::inference::is_no_infer_type(self, t)
        {
            let base_type = self
                .type_(t)
                .as_substitution_type()
                .and_then(|d| d.base_type)
                .expect("SubstitutionType base");
            return self.is_no_infer_target_type(base_type);
        }
        if flags.intersects(TypeFlags::OBJECT) && !self.is_empty_anonymous_object_type(t) {
            return true;
        }
        flags.intersects(TypeFlags::INSTANTIABLE & !TypeFlags::SUBSTITUTION)
            && !crate::typenodes::is_pattern_literal_type(self, t)
    }

    /// Go: `func (c *Checker) getSubstitutionType(baseType, constraint *Type) *Type`
    /// (checker.go:27945).
    pub(crate) fn get_substitution_type(&mut self, base_type: TypeId, constraint: TypeId) -> TypeId {
        if self.type_(constraint).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
            || constraint == base_type
            || self.type_(base_type).flags.intersects(TypeFlags::ANY)
        {
            return base_type;
        }
        self.get_or_create_substitution_type(base_type, constraint)
    }

    /// Go: `func (c *Checker) getOrCreateSubstitutionType(baseType, constraint *Type) *Type`
    /// (checker.go:27953).
    pub(crate) fn get_or_create_substitution_type(&mut self, base_type: TypeId, constraint: TypeId) -> TypeId {
        let key = (base_type.0, constraint.0);
        if let Some(&cached) = self.substitution_types.get(&key) {
            return cached;
        }
        let data = TypeData::Substitution(Box::new(types::SubstitutionType {
            base: Default::default(),
            base_type: Some(base_type),
            constraint: Some(constraint),
        }));
        let result = self.new_type(TypeFlags::SUBSTITUTION, ObjectFlags::NONE, data);
        self.substitution_types.insert(key, result);
        result
    }

    /// Go: `func (c *Checker) instantiateType(t *Type, m *TypeMapper) *Type`
    /// (checker.go:22559) — REAL (M6 S7; replaces checker.rs's identity stub).
    pub fn instantiate_type(&mut self, t: TypeId, mapper: TypeMapperId) -> TypeId {
        self.instantiate_type_with_alias(t, Some(mapper), None)
    }

    /// Go: `func (c *Checker) instantiateTypeWithAlias(t *Type, m *TypeMapper, alias *TypeAlias) *Type`
    /// (checker.go:22563).
    pub(crate) fn instantiate_type_with_alias(
        &mut self,
        t: TypeId,
        mapper: Option<TypeMapperId>,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        // Check for type variables in the alias, so things like
        // `type Brand<T> = number & {}` can potentially be copied with new
        // alias type args (Go's comment, checker.go:22565).
        let alias_has_type_vars = alias.as_ref().is_some_and(|a| {
            !a.type_arguments.is_empty()
                && a.type_arguments
                    .iter()
                    .any(|&arg| self.could_contain_type_variables(arg))
        });
        let Some(mapper) = mapper else {
            return t;
        };
        if !(self.could_contain_type_variables(t) || alias_has_type_vars) {
            return t;
        }
        if self.instantiation_stack.len() == 100 || self.instantiation_count >= 5_000_000 {
            // We have reached 100 recursive type instantiations, or 5M type
            // instantiations caused by the same statement or expression
            // (Go's comment, checker.go:22573).
            let circular_type_names = self.get_circular_type_names();
            let node = self.current_node.unwrap_or(NodeId::NONE);
            match circular_type_names.len() {
                1 => {
                    self.error(
                        node,
                        &tsc_diagnostics::INSTANTIATIONS_OF_TYPE_0_APPEAR_INFINITELY_CIRCULAR,
                        &[circular_type_names[0].clone()],
                    );
                }
                n if n > 1 => {
                    let quoted = circular_type_names
                        .iter()
                        .map(|n| format!("\"{n}\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.error(
                        node,
                        &tsc_diagnostics::INSTANTIATIONS_OF_THE_FOLLOWING_TYPES_APPEAR_INFINITELY_CIRCULAR_COLON_0,
                        &[quoted],
                    );
                }
                _ => {
                    self.error(
                        node,
                        &tsc_diagnostics::TYPE_INSTANTIATION_IS_EXCESSIVELY_DEEP_AND_POSSIBLY_INFINITE,
                        &[],
                    );
                }
            }
            return self.error_type();
        }
        let index = self.find_active_mapper(mapper);
        if index.is_none() {
            self.push_active_mapper(mapper);
        }
        let mut b = crate::relater::KeyBuilder::new();
        b.write_type(t);
        b.write_alias(alias.as_deref());
        let key = b.hash();
        let cache_index = index.unwrap_or(self.active_type_mappers_caches.len().wrapping_sub(1));
        if let Some(&cached_type) = self.active_type_mappers_caches[cache_index].get(&key) {
            if index.is_none() {
                self.pop_active_mapper();
            }
            return cached_type;
        }
        self.instantiation_count += 1;
        self.instantiation_stack.push(t);
        let result = self.instantiate_type_worker(t, mapper, alias);
        match index {
            None => {
                self.pop_active_mapper();
            }
            Some(index) => {
                self.active_type_mappers_caches[index].insert(key, result);
            }
        }
        self.instantiation_stack.pop();
        result
    }

    /// Go: `func (c *Checker) getCircularTypeNames() []string` (checker.go:22599).
    fn get_circular_type_names(&mut self) -> Vec<String> {
        let mut type_counts: rustc_hash::FxHashMap<TypeId, u32> =
            rustc_hash::FxHashMap::default();
        let mut circular_type_names: Vec<String> = Vec::new();
        for t in self.instantiation_stack.clone() {
            let count = type_counts.entry(t).or_insert(0);
            *count += 1;
            if *count == 3 {
                let symbol = {
                    let ty = self.type_(t);
                    if let Some(alias) = ty.alias.as_deref() {
                        alias.symbol
                    } else {
                        ty.symbol
                    }
                };
                if let Some(symbol) = symbol {
                    let name = self.program_symbol(symbol).name.clone();
                    if !name.is_empty() && !name.starts_with('\u{FE}') {
                        let name = crate::relater::symbol_to_string(self, symbol);
                        if !circular_type_names.contains(&name) {
                            circular_type_names.push(name);
                        }
                    }
                }
            }
        }
        circular_type_names
    }

    /// Go: `func (c *Checker) instantiateTypeWorker(t *Type, m *TypeMapper, alias *TypeAlias) *Type`
    /// (checker.go:22708).
    fn instantiate_type_worker(
        &mut self,
        t: TypeId,
        m: TypeMapperId,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return crate::mapper::get_mapped_type(self, t, m);
        }
        if flags.intersects(TypeFlags::OBJECT) {
            let object_flags = self.type_(t).object_flags;
            if object_flags
                .intersects(ObjectFlags::REFERENCE | ObjectFlags::ANONYMOUS | ObjectFlags::MAPPED)
            {
                if object_flags.intersects(ObjectFlags::REFERENCE)
                    && self
                        .type_(t)
                        .as_type_reference()
                        .expect("TypeReference data")
                        .node
                        .is_none()
                {
                    let resolved_type_arguments = self
                        .type_(t)
                        .as_type_reference()
                        .expect("TypeReference data")
                        .resolved_type_arguments
                        .clone();
                    let new_type_arguments = self.instantiate_types(&resolved_type_arguments, Some(m));
                    if new_type_arguments == resolved_type_arguments {
                        return t;
                    }
                    let target = self
                        .type_(t)
                        .as_object_type()
                        .and_then(|d| d.target)
                        .expect("reference target");
                    return self.create_normalized_type_reference(target, new_type_arguments);
                }
                if object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
                    // TODO(port M6 S9): instantiateReverseMappedType — the
                    // reverse-mapped instantiation composes with inference's
                    // inferTypeForHomomorphicMappedType (the W2 seam); the
                    // identity keeps reverse-mapped types stable until then.
                    return t;
                }
                return self.get_object_type_instantiation(t, m, alias);
            }
            return t;
        }
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let mut source = t;
            if flags.intersects(TypeFlags::UNION)
                && let Some(origin) = self
                    .type_(t)
                    .as_union_type()
                    .expect("UnionType data")
                    .origin
                    .filter(|&origin| self.type_(origin).flags.intersects(TypeFlags::UNION_OR_INTERSECTION))
            {
                source = origin;
            }
            let types = self.type_(source).types().to_vec();
            let new_types = self.instantiate_types(&types, Some(m));
            let alias_symbols_equal = match (alias.as_deref(), self.type_(t).alias.as_deref()) {
                (None, None) => true,
                (Some(a), Some(b)) => a.symbol() == b.symbol(),
                _ => false,
            };
            if new_types == types && alias_symbols_equal {
                return t;
            }
            let alias = alias.or_else(|| self.instantiate_type_alias_of(t, Some(m)));
            if self.type_(source).flags.intersects(TypeFlags::INTERSECTION) {
                return self.get_intersection_type_ex(new_types, IntersectionFlags::NONE, alias);
            }
            return self.get_union_type_ex(new_types, UnionReduction::Literal, alias, None);
        }
        if flags.intersects(TypeFlags::INDEX) {
            // REAL (M6 S10, checker.go:22750):
            // `c.getIndexType(c.instantiateType(t.Target(), m))`.
            let target = self
                .type_(t)
                .as_index_type()
                .expect("IndexType data")
                .target
                .expect("IndexType target");
            let new_target = self.instantiate_type(target, m);
            return self.get_index_type_impl(new_target);
        }
        if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let alias = alias.or_else(|| self.instantiate_type_alias_of(t, Some(m)));
            let (object_type, index_type, access_flags) = {
                let d = self.type_(t).as_indexed_access_type().expect("IndexedAccessType data");
                (d.object_type, d.index_type, d.access_flags)
            };
            let object_type = object_type.expect("objectType");
            let index_type = index_type.expect("indexType");
            let new_object = self.instantiate_type(object_type, m);
            let new_index = self.instantiate_type(index_type, m);
            return self.get_indexed_access_type_ex(
                new_object,
                new_index,
                access_flags,
                None,
                alias,
            );
        }
        if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let (texts, types) = {
                let d = self.type_(t).as_template_literal_type().expect("TemplateLiteralType data");
                (d.texts.clone(), d.types.clone())
            };
            let new_types = self.instantiate_types(&types, Some(m));
            return self.get_template_literal_type(&texts, &new_types);
        }
        if flags.intersects(TypeFlags::STRING_MAPPING) {
            let symbol = self.type_(t).symbol;
            let target = self
                .type_(t)
                .as_string_mapping_type()
                .expect("StringMappingType data")
                .target
                .expect("target");
            let new_target = self.instantiate_type(target, m);
            return self.get_string_mapping_type(symbol, new_target);
        }
        if flags.intersects(TypeFlags::CONDITIONAL) {
            let mapper = self
                .type_(t)
                .as_conditional_type()
                .expect("ConditionalType data")
                .mapper;
            let combined = self.combine_type_mappers(mapper, Some(m));
            return self.get_conditional_type_instantiation(t, combined, false, alias);
        }
        if flags.intersects(TypeFlags::SUBSTITUTION) {
            let (base_type, constraint) = {
                let d = self.type_(t).as_substitution_type().expect("SubstitutionType data");
                (d.base_type, d.constraint)
            };
            let base_type = base_type.expect("baseType");
            let constraint = constraint.expect("constraint");
            let new_base_type = self.instantiate_type(base_type, m);
            if crate::inference::is_no_infer_type(self, t) {
                return self.get_no_infer_type(new_base_type);
            }
            let new_constraint = self.instantiate_type(constraint, m);
            // A substitution type originates in the true branch of a
            // conditional type and can be resolved to just the base type in
            // the same cases as the conditional type resolves to its true
            // branch (Go's comment, checker.go:22755).
            if self.type_(new_base_type).flags.intersects(TypeFlags::TYPE_VARIABLE)
                && self.is_generic_type(new_constraint)
            {
                return self.get_substitution_type(new_base_type, new_constraint);
            }
            if self.type_(new_constraint).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
                || {
                    let source = self.get_restrictive_instantiation(new_base_type);
                    let target = self.get_restrictive_instantiation(new_constraint);
                    self.is_type_assignable_to(source, target)
                }
            {
                return new_base_type;
            }
            if self.type_(new_base_type).flags.intersects(TypeFlags::TYPE_VARIABLE) {
                return self.get_substitution_type(new_base_type, new_constraint);
            }
            return self.get_intersection_type(&[new_constraint, new_base_type]);
        }
        t
    }

    /// Go: `func (c *Checker) getObjectTypeInstantiation(t *Type, m *TypeMapper, alias *TypeAlias) *Type`
    /// (checker.go:22792).
    #[allow(clippy::let_and_return)] // the match arms early-return on the deferred/mapped paths
    fn get_object_type_instantiation(
        &mut self,
        t: TypeId,
        m: TypeMapperId,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let object_flags = self.type_(t).object_flags;
        let declaration: NodeId = if object_flags.intersects(ObjectFlags::REFERENCE) {
            self.type_(t)
                .as_type_reference()
                .expect("TypeReference data")
                .node
                .expect("deferred type reference node")
        } else if object_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE) {
            self.type_(t)
                .as_instantiation_expression_type()
                .expect("InstantiationExpressionType data")
                .node
                .expect("instantiation expression node")
        } else {
            self.type_(t)
                .symbol
                .and_then(|s| self.program_symbol(s).declarations.first().copied())
                .expect("anonymous type declaration")
        };
        // Go reads links := c.typeNodeLinks.Get(declaration) once and caches
        // outerTypeParameters through it; the port's LinkStore hands out
        // per-call refs, so the reads split.
        let target: TypeId = if object_flags.intersects(ObjectFlags::REFERENCE) {
            self.type_node_links
                .get(declaration)
                .resolved_type
                .expect("deferred type reference target")
        } else if object_flags.intersects(ObjectFlags::INSTANTIATED) {
            self.type_(t)
                .as_object_type()
                .and_then(|d| d.target)
                .expect("instantiated target")
        } else {
            t
        };
        let mut type_parameters = self.type_node_links.get(declaration).outer_type_parameters.clone();
        if type_parameters.is_empty() {
            // The first time an anonymous type is instantiated we compute and
            // store a list of the type parameters that are in scope (Go's
            // comment, checker.go:22811).
            type_parameters = self.get_outer_type_parameters(declaration, true);
            let target_alias_type_arguments_empty = self
                .type_(target)
                .alias
                .as_ref()
                .is_none_or(|a| a.type_arguments.is_empty());
            if target_alias_type_arguments_empty {
                if object_flags
                    .intersects(ObjectFlags::REFERENCE | ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
                {
                    type_parameters.retain(|&tp| {
                        self.is_type_parameter_possibly_referenced(tp, declaration)
                    });
                } else if self
                    .type_(t)
                    .symbol
                    .is_some_and(|s| {
                        self.program_symbol(s)
                            .flags
                            .intersects(tsc_ast::SymbolFlags::METHOD | tsc_ast::SymbolFlags::TYPE_LITERAL)
                    })
                {
                    let declarations = self
                        .type_(t)
                        .symbol
                        .map(|s| self.program_symbol(s).declarations.clone())
                        .unwrap_or_default();
                    type_parameters.retain(|&tp| {
                        declarations
                            .iter()
                            .any(|&d| self.is_type_parameter_possibly_referenced(tp, d))
                    });
                }
            }
            self.type_node_links.get(declaration).outer_type_parameters = type_parameters.clone();
        }
        if type_parameters.is_empty() {
            return t;
        }
        // We are instantiating an anonymous type that has one or more type
        // parameters in scope (Go's comment, checker.go:22837).
        let mapper = self.type_(t).mapper();
        let mut type_arguments: Vec<TypeId> = Vec::with_capacity(type_parameters.len());
        for &tp in &type_parameters {
            type_arguments.push(self.map_type_with_composite_mapper(tp, mapper, m));
        }
        let new_alias = alias.or_else(|| self.instantiate_type_alias_of(t, Some(m)));
        let single_signature =
            self.type_(t).object_flags.intersects(ObjectFlags::SINGLE_SIGNATURE_TYPE);
        let key = crate::typeofsymbol::get_type_instantiation_key(
            &type_arguments,
            new_alias.as_deref(),
            single_signature,
        );
        {
            let target_alias = self.type_(target).alias.clone();
            let base_key = crate::typeofsymbol::get_type_instantiation_key(
                &type_parameters,
                target_alias.as_deref(),
                false,
            );
            let data = self.type_mut(target).as_object_type_mut().expect("ObjectType data");
            data.instantiations
                .get_or_insert_with(Default::default)
                .entry(base_key)
                .or_insert(target);
        }
        let result = match self
            .type_(target)
            .as_object_type()
            .and_then(|d| d.instantiations.as_ref())
            .and_then(|map| map.get(&key))
        {
            Some(&result) => result,
            None => {
                let new_mapper = self.new_type_mapper(&type_parameters, &type_arguments);
                let new_mapper = if self.type_(target).object_flags.intersects(ObjectFlags::SINGLE_SIGNATURE_TYPE) {
                    self.combine_type_mappers(Some(new_mapper), Some(m))
                        .expect("combined mapper")
                } else {
                    new_mapper
                };
                let result = if self.type_(target).object_flags.intersects(ObjectFlags::REFERENCE) {
                    let target_target = self
                        .type_(t)
                        .as_object_type()
                        .and_then(|d| d.target)
                        .expect("deferred target");
                    let node = self
                        .type_(t)
                        .as_type_reference()
                        .expect("TypeReference data")
                        .node;
                    self.create_deferred_type_reference(target_target, node.expect("node"), Some(new_mapper), new_alias.clone())
                } else if self.type_(target).object_flags.intersects(ObjectFlags::MAPPED) {
                    self.instantiate_mapped_type(target, new_mapper, new_alias.clone())
                } else {
                    self.instantiate_anonymous_type(target, new_mapper, new_alias.clone())
                };
                let data = self.type_mut(target).as_object_type_mut().expect("ObjectType data");
                data.instantiations
                    .get_or_insert_with(Default::default)
                    .insert(key, result);
                // if `result` is one of the object types we tried to make, we
                // can carry forward the type variable containment check from
                // the input type arguments (Go's comment, checker.go:22876).
                if self.type_(result).flags.intersects(TypeFlags::OBJECT_FLAGS_TYPE)
                    && !self
                        .type_(result)
                        .object_flags
                        .intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED)
                {
                    let result_could_contain_object_flags = type_arguments
                        .iter()
                        .any(|&arg| self.could_contain_type_variables(arg));
                    let object_flags = self.type_(result).object_flags;
                    if !object_flags
                        .intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED)
                    {
                        if object_flags
                            .intersects(ObjectFlags::MAPPED | ObjectFlags::ANONYMOUS | ObjectFlags::REFERENCE)
                        {
                            let flags = ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                                | if result_could_contain_object_flags {
                                    ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES
                                } else {
                                    ObjectFlags::NONE
                                };
                            self.type_mut(result).object_flags |= flags;
                        } else {
                            if !result_could_contain_object_flags {
                                self.type_mut(result).object_flags |=
                                    ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED;
                            }
                        }
                    }
                }
                result
            }
        };
        result
    }

    /// Go: `func (c *Checker) isTypeParameterPossiblyReferenced(tp *Type, node *ast.Node) bool`
    /// (checker.go:22855).
    pub(crate) fn is_type_parameter_possibly_referenced(&mut self, tp: TypeId, node: NodeId) -> bool {
        // PORT: Go's recursive `containsReference` closure walks the subtree
        // through ForEachChild; the port walks through the node store's child
        // edges with the same case arms (see ast_shims).
        fn contains_reference(c: &mut Checker, tp: TypeId, node: NodeId, depth: u32) -> bool {
            use tsc_ast::Kind;
            if depth > 512 || node.is_none() {
                return false;
            }
            let kind = c.store().node(node).kind;
            match kind {
                Kind::ThisType => {
                    return c
                        .type_(tp)
                        .as_type_parameter()
                        .expect("TypeParameter data")
                        .is_this_type
                }
                Kind::TypeReference => {
                    let type_arguments = crate::statements::type_arguments_of(c.store(), node);
                    let symbol = c.get_symbol_from_type_reference(node);
                    let tp_symbol = c.type_(tp).symbol;
                    if !c
                        .type_(tp)
                        .as_type_parameter()
                        .expect("TypeParameter data")
                        .is_this_type
                        && type_arguments.is_empty()
                        && tp_symbol.is_some_and(|s| s == symbol)
                    {
                        return true;
                    }
                }
                Kind::TypeQuery => {
                    return true;
                }
                Kind::MethodDeclaration | Kind::MethodSignature => {
                    let return_type = crate::ast_shims::type_of_node(c.store(), node);
                    let body = tsc_binder::node_data::body_of(c.store(), node);
                    if return_type.is_none() && body.is_some() {
                        return true;
                    }
                    let type_parameters = crate::statements::type_parameters_of(c.store(), node);
                    if type_parameters
                        .iter()
                        .any(|&t| contains_reference(c, tp, t, depth + 1))
                    {
                        return true;
                    }
                    let parameters = crate::ast_shims::parameters_of(c.store(), node).to_vec();
                    if parameters
                        .iter()
                        .any(|&p| contains_reference(c, tp, p, depth + 1))
                    {
                        return true;
                    }
                    if let Some(return_type) = return_type
                        && contains_reference(c, tp, return_type, depth + 1)
                    {
                        return true;
                    }
                }
                _ => {}
            }
            let children = crate::statements::children_of(c.store(), node);
            for child in children {
                if contains_reference(c, tp, child, depth + 1) {
                    return true;
                }
            }
            false
        }
        // If the type parameter doesn't have exactly one declaration, if there
        // are intervening statement blocks between the node and the type
        // parameter declaration, if the node contains actual references to the
        // type parameter, or if the node contains type queries that we can't
        // prove couldn't contain references to the type parameter, we consider
        // the type parameter possibly referenced (Go's comment).
        let tp_symbol = self.type_(tp).symbol;
        if let Some(symbol) = tp_symbol {
            let declarations = self.program_symbol(symbol).declarations.clone();
            if declarations.len() == 1 {
                let container = {
                    let store = self.store();
                    store.node(declarations[0]).parent.get()
                };
                let mut n = node;
                let mut bail = false;
                while n != container {
                    if n.is_none() {
                        bail = true;
                        break;
                    }
                    let (kind, is_block, conditional_extends) = {
                        let store = self.store();
                        let node_data = store.node(n);
                        let conditional_extends = if node_data.kind == tsc_ast::Kind::ConditionalType {
                            node_data
                                .as_conditional_type_node()
                                .map(|d| d.extends_type)
                                .unwrap_or(NodeId::NONE)
                        } else {
                            NodeId::NONE
                        };
                        (
                            node_data.kind,
                            node_data.kind == tsc_ast::Kind::Block,
                            conditional_extends,
                        )
                    };
                    let _ = kind;
                    if is_block {
                        bail = true;
                        break;
                    }
                    if conditional_extends != NodeId::NONE
                        && contains_reference(self, tp, conditional_extends, 0)
                    {
                        bail = true;
                        break;
                    }
                    n = self.store().node(n).parent.get();
                }
                if !bail {
                    return contains_reference(self, tp, node, 0);
                }
            }
        }
        true
    }

    /// Go: `func (c *Checker) instantiateAnonymousType(t *Type, m *TypeMapper, alias *TypeAlias) *Type`
    /// (checker.go:22946).
    fn instantiate_anonymous_type(
        &mut self,
        t: TypeId,
        m: TypeMapperId,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let object_flags = self.type_(t).object_flags;
        let result = self.new_object_type(
            (object_flags
                & !(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                    | ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES))
                | ObjectFlags::INSTANTIATED,
            self.type_(t).symbol,
        );
        let mut m = m;
        if object_flags.intersects(ObjectFlags::MAPPED) {
            let declaration = self
                .type_(t)
                .as_mapped_type()
                .expect("MappedType data")
                .declaration;
            self.type_mut(result)
                .as_mapped_type_mut()
                .expect("MappedType data")
                .declaration = declaration;
            // C.f. instantiateSignature (Go's comment, checker.go:22955)
            let orig_type_parameter = crate::relater::get_type_parameter_from_mapped_type(self, t);
            let fresh_type_parameter = self.clone_type_parameter(orig_type_parameter);
            self.type_mut(result)
                .as_mapped_type_mut()
                .expect("MappedType data")
                .type_parameter = Some(fresh_type_parameter);
            let simple = self.new_simple_type_mapper(orig_type_parameter, fresh_type_parameter);
            m = self
                .combine_type_mappers(Some(simple), Some(m))
                .expect("combined mapper");
            self.type_mut(fresh_type_parameter)
                .as_type_parameter_mut()
                .expect("TypeParameter data")
                .mapper = Some(m);
        }
        if object_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE) {
            let node = self
                .type_(t)
                .as_instantiation_expression_type()
                .expect("InstantiationExpressionType data")
                .node;
            self.type_mut(result)
                .as_instantiation_expression_type_mut()
                .expect("InstantiationExpressionType data")
                .node = node;
        }
        let alias = alias.or_else(|| self.instantiate_type_alias_of(t, Some(m)));
        if let Some(alias) = alias.as_ref() {
            if !alias.type_arguments.is_empty() {
                let propagating =
                    self.get_propagating_flags_of_types(&alias.type_arguments, TypeFlags::NONE);
                self.type_mut(result).object_flags |= propagating;
            }
        }
        {
            let ty = self.type_mut(result);
            ty.alias = alias;
            let d = ty.as_object_type_mut().expect("ObjectType data");
            d.target = Some(t);
            d.mapper = Some(m);
        }
        result
    }

    /// Go: `func (c *Checker) getConditionalTypeInstantiation(t *Type, mapper *TypeMapper, forConstraint bool, alias *TypeAlias) *Type`
    /// (checker.go:23010).
    pub(crate) fn get_conditional_type_instantiation(
        &mut self,
        t: TypeId,
        mapper: Option<TypeMapperId>,
        for_constraint: bool,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let Some(root_box) = self.type_(t).as_conditional_type().and_then(|d| d.root.clone()) else {
            return t;
        };
        let outer_type_parameters = root_box.outer_type_parameters.clone();
        if !outer_type_parameters.is_empty() {
            // We are instantiating a conditional type that has one or more
            // type parameters in scope (Go's comment, checker.go:23015).
            let Some(mapper) = mapper else {
                return t;
            };
            let type_arguments: Vec<TypeId> = outer_type_parameters
                .iter()
                .map(|&tp| self.mapper_map(mapper, tp))
                .collect();
            let key = crate::typeofsymbol::get_conditional_type_key(
                &type_arguments,
                alias.as_deref(),
                for_constraint,
            );
            if let Some(&result) = root_box
                .instantiations
                .as_ref()
                .and_then(|map| map.get(&key))
            {
                return result;
            }
            let new_mapper = self.new_type_mapper(&outer_type_parameters, &type_arguments);
            let check_type = root_box.check_type.expect("checkType");
            let is_distributive = root_box.is_distributive;
            let mut distribution_type: Option<TypeId> = None;
            if is_distributive {
                let mapped = crate::mapper::get_mapped_type(self, check_type, new_mapper);
                let reduced = self.get_reduced_type(mapped);
                distribution_type = Some(reduced);
            }
            // Distributive conditional types are distributed over union types
            // (Go's comment, checker.go:23031).
            let result = if let Some(distribution) = distribution_type {
                if check_type != distribution
                    && self
                        .type_(distribution)
                        .flags
                        .intersects(TypeFlags::UNION | TypeFlags::NEVER)
                {
                    self.map_type_with_alias(
                        distribution,
                        &mut |c, constituent| {
                            let mapper = c.prepend_type_mapping(
                                check_type,
                                constituent,
                                Some(new_mapper),
                            );
                            Some(c.get_conditional_type(root_box.clone(), Some(mapper), for_constraint, None))
                        },
                        alias.as_deref(),
                    )
                    .unwrap_or_else(|| self.error_type())
                } else {
                    self.get_conditional_type(root_box.clone(), Some(new_mapper), for_constraint, alias.clone())
                }
            } else {
                self.get_conditional_type(root_box.clone(), Some(new_mapper), for_constraint, alias.clone())
            };
            if let Some(d) = self.type_mut(t).as_conditional_type_mut()
                && let Some(root) = d.root.as_deref_mut()
            {
                root.instantiations
                    .get_or_insert_with(Default::default)
                    .insert(key, result);
            }
            return result;
        }
        t
    }

    /// Go: `func (c *Checker) cloneTypeParameter(tp *Type) *Type`
    /// (checker.go:22966) — REAL (M6 S7).
    pub(crate) fn clone_type_parameter(&mut self, tp: TypeId) -> TypeId {
        let symbol = self.type_(tp).symbol;
        let result = self.new_type_parameter(symbol);
        self.type_mut(result)
            .as_type_parameter_mut()
            .expect("TypeParameter data")
            .target = Some(tp);
        result
    }

    /// Go: `func (c *Checker) instantiateTypeAlias(alias *TypeAlias, m *TypeMapper) *TypeAlias`
    /// (checker.go:23268) — the checker-facing form over a TypeId's alias.
    fn instantiate_type_alias_of(&mut self, t: TypeId, m: Option<TypeMapperId>) -> Option<Box<TypeAlias>> {
        let alias = self.type_(t).alias.clone()?;
        let type_arguments = self.instantiate_types(&alias.type_arguments, m);
        Some(Box::new(TypeAlias {
            symbol: alias.symbol,
            type_arguments,
        }))
    }

    /// Go: `func (c *Checker) instantiateTypes(types []*Type, m *TypeMapper) []*Type`
    /// (checker.go:23272) — PORT: the nil-mapper form is identity.
    pub(crate) fn instantiate_types(&mut self, types: &[TypeId], m: Option<TypeMapperId>) -> Vec<TypeId> {
        match m {
            None => types.to_vec(),
            Some(m) => {
                let mut result = Vec::with_capacity(types.len());
                for &t in types {
                    result.push(self.instantiate_type(t, m));
                }
                result
            }
        }
    }

    /// Go: `func (c *Checker) instantiateSymbols(symbols []*ast.Symbol, m *TypeMapper) []*ast.Symbol`
    /// (checker.go:23276).
    pub(crate) fn instantiate_symbols(&mut self, symbols: &[SymbolId], m: TypeMapperId) -> Vec<SymbolId> {
        let mut result = Vec::with_capacity(symbols.len());
        for &symbol in symbols {
            result.push(self.instantiate_symbol(symbol, Some(m)));
        }
        result
    }

    /// Go: `func (c *Checker) instantiateSignatures(signatures []*Signature, m *TypeMapper) []*Signature`
    /// (checker.go:23280).
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn instantiate_signatures(
        &mut self,
        signatures: &[SignatureId],
        m: Option<TypeMapperId>,
    ) -> Vec<SignatureId> {
        let mut result = Vec::with_capacity(signatures.len());
        for &signature in signatures {
            result.push(self.instantiate_signature(signature, m));
        }
        result
    }

    /// Go: `func (c *Checker) instantiateIndexInfos(infos []*IndexInfo, m *TypeMapper) []*IndexInfo`
    /// (checker.go:23284) — the arena form (IndexInfoId in, out; S8 flipped
    /// the payload fields to the handle form).
    pub(crate) fn instantiate_index_infos(
        &mut self,
        infos: &[crate::types::IndexInfoId],
        m: Option<TypeMapperId>,
    ) -> Vec<crate::types::IndexInfoId> {
        let mut result = Vec::with_capacity(infos.len());
        for &info in infos {
            result.push(self.instantiate_index_info(info, m));
        }
        result
    }

    /// Go: `func (c *Checker) instantiateIndexInfo(info *IndexInfo, m *TypeMapper) *IndexInfo`
    /// (checker.go:21034) — the arena form.
    pub(crate) fn instantiate_index_info(
        &mut self,
        info: crate::types::IndexInfoId,
        m: Option<TypeMapperId>,
    ) -> crate::types::IndexInfoId {
        let (key_type, value_type, is_readonly, declaration, components) = {
            let info = &self.index_infos[info.0 as usize];
            (
                info.key_type.expect("keyType"),
                info.value_type.expect("valueType"),
                info.is_readonly,
                info.declaration,
                info.components.clone(),
            )
        };
        let new_value_type = match m {
            None => value_type,
            Some(m) => self.instantiate_type(value_type, m),
        };
        if new_value_type == value_type {
            return info;
        }
        crate::types::IndexInfoId(
            self.new_index_info(key_type, new_value_type, is_readonly, declaration, components)
                as u32,
        )
    }

    /// Go: `func (c *Checker) instantiateSignature(sig *Signature, m *TypeMapper) *Signature`
    /// (checker.go:21019) — PORT: the nil-mapper form is identity.
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn instantiate_signature(&mut self, sig: SignatureId, m: Option<TypeMapperId>) -> SignatureId {
        match m {
            None => sig,
            Some(m) => {
                let erase = Some(m) == self.permissive_mapper;
                self.instantiate_signature_ex(sig, Some(m), erase)
            }
        }
    }

    /// Go: `func (c *Checker) instantiateSignatureEx(sig *Signature, m *TypeMapper, eraseTypeParameters bool) *Signature`
    /// (checker.go:21023) — REAL (M6 S7; kills resolvecall.rs's identity
    /// seams).
    pub(crate) fn instantiate_signature_ex(
        &mut self,
        sig: SignatureId,
        m: Option<TypeMapperId>,
        erase_type_parameters: bool,
    ) -> SignatureId {
        let (sig_flags, declaration, type_parameters, this_parameter, parameters, min_argument_count) = {
            let signature = &self.signatures[sig.0 as usize];
            (
                signature.flags,
                signature.declaration,
                signature.type_parameters.clone(),
                signature.this_parameter,
                signature.parameters.clone(),
                signature.min_argument_count,
            )
        };
        let mut fresh_type_parameters: Vec<TypeId> = Vec::new();
        let m = if !type_parameters.is_empty() && !erase_type_parameters {
            // First create a fresh set of type parameters, then include a
            // mapping from the old to the new type parameters in the mapper
            // function (Go's comment, checker.go:21027).
            fresh_type_parameters = type_parameters
                .iter()
                .map(|&tp| self.clone_type_parameter(tp))
                .collect();
            let mapper = self.new_type_mapper(&type_parameters, &fresh_type_parameters);
            let combined = self.combine_type_mappers(Some(mapper), m);
            for &tp in &fresh_type_parameters {
                self.type_mut(tp)
                    .as_type_parameter_mut()
                    .expect("TypeParameter data")
                    .mapper = combined;
            }
            combined
        } else {
            m
        };
        // Don't compute resolvedReturnType and resolvedTypePredicate now,
        // because using `mapper` now could trigger inferences to become fixed
        // (See `createInferenceContext`. See GH#17600.)
        let this_parameter = match (this_parameter, m) {
            (Some(this), Some(m)) => Some(self.instantiate_symbol(this, Some(m))),
            (this, _) => this,
        };
        let parameters = match m {
            Some(m) => self.instantiate_symbols(&parameters, m),
            None => parameters,
        };
        let flags = sig_flags & SignatureFlags::PROPAGATING_FLAGS;
        let result = self.new_signature(
            flags,
            declaration,
            fresh_type_parameters,
            this_parameter,
            parameters,
            None,
            min_argument_count,
        );
        {
            let signature = &mut self.signatures[result.0 as usize];
            signature.target = Some(sig);
            signature.mapper = m;
        }
        result
    }

    /// Go: `func (c *Checker) instantiateSymbol(symbol *ast.Symbol, m *TypeMapper) *ast.Symbol`
    /// (checker.go:21157).
    pub(crate) fn instantiate_symbol(&mut self, symbol: SymbolId, m: Option<TypeMapperId>) -> SymbolId {
        let Some(m) = m else {
            return symbol;
        };
        if self.mapper_maps_this_only(m) && is_thisless(self, symbol) {
            return symbol;
        }
        // If the type of the symbol is already resolved, and if that type
        // could not possibly be affected by instantiation, simply return the
        // symbol itself (Go's comment, checker.go:21169).
        let (resolved_type, write_type) = {
            let links = self.value_symbol_links.get(symbol);
            (links.resolved_type, links.write_type)
        };
        if let Some(resolved_type) = resolved_type
            && !self.could_contain_type_variables(resolved_type)
        {
            if !self
                .program_symbol(symbol)
                .flags
                .intersects(tsc_ast::SymbolFlags::SET_ACCESSOR)
            {
                return symbol;
            }
            // If we're a setter, check writeType.
            if let Some(write_type) = write_type
                && !self.could_contain_type_variables(write_type)
            {
                return symbol;
            }
        }
        let (check_flags, symbol_flags, name) = {
            let symbol = self.program_symbol(symbol);
            (symbol.check_flags, symbol.flags, symbol.name.clone())
        };
        let mut symbol = symbol;
        let mut m = m;
        if check_flags.intersects(tsc_ast::CheckFlags::INSTANTIATED) {
            // If symbol being instantiated is itself an instantiation, fetch
            // the original target and combine the type mappers. This ensures
            // that original type identities are properly preserved and that
            // aliases always reference a non-aliases (Go's comment,
            // checker.go:21181).
            let links = self.value_symbol_links.get(symbol);
            let target = links.target;
            let mapper = links.mapper;
            symbol = target.unwrap_or(symbol);
            m = self
                .combine_type_mappers(mapper, Some(m))
                .expect("instantiated symbol mapper");
        }
        // Keep the flags from the symbol we're instantiating. Mark that it
        // is instantiated, and also transient (Go's comment, checker.go:21187).
        let result = self.new_symbol(symbol_flags, &name);
        {
            let (declarations, parent, value_declaration) = {
                let src = self.program_symbol(symbol);
                (src.declarations.clone(), src.parent, src.value_declaration)
            };
            let result_symbol = self.program_symbol_mut(result);
            result_symbol.check_flags = tsc_ast::CheckFlags::INSTANTIATED
                | (check_flags
                    & (tsc_ast::CheckFlags::READONLY
                        | tsc_ast::CheckFlags::LATE
                        | tsc_ast::CheckFlags::OPTIONAL_PARAMETER
                        | tsc_ast::CheckFlags::REST_PARAMETER));
            result_symbol.declarations = declarations;
            result_symbol.parent = parent;
            result_symbol.value_declaration = value_declaration;
        }
        let name_type = self.value_symbol_links.get(symbol).name_type;
        {
            let result_links = self.value_symbol_links.get(result);
            result_links.target = Some(symbol);
            result_links.mapper = Some(m);
            result_links.name_type = name_type;
        }
        result
    }

    /// Go: `func (c *Checker) instantiateSymbolTable(symbols ast.SymbolTable, m *TypeMapper) ast.SymbolTable`
    /// (checker.go:21054).
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    pub(crate) fn instantiate_symbol_table(
        &mut self,
        symbols: &SymbolTable,
        m: TypeMapperId,
    ) -> SymbolTable {
        let mut result = SymbolTable::default();
        for (id, &symbol) in symbols.entries() {
            result.set(id.clone(), self.instantiate_symbol(symbol, Some(m)));
        }
        result
    }

    /// Go: `func (c *Checker) instantiateMappedType(t *Type, m *TypeMapper, alias *TypeAlias) *Type`
    /// (checker.go:23094) — the homomorphic mapped-type instantiation. The
    /// array/tuple constituent walks ride the real createArrayType /
    /// createTupleTypeEx (M6 S7); the mapped-type component getters compose
    /// over the S8/S9 seams.
    fn instantiate_mapped_type(
        &mut self,
        t: TypeId,
        m: TypeMapperId,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let type_variable = self.get_homomorphic_type_variable(t);
        if let Some(type_variable) = type_variable {
            let mapped_type_variable = self.instantiate_type(type_variable, m);
            if type_variable != mapped_type_variable {
                let reduced = self.get_reduced_type(mapped_type_variable);
                return self
                    .map_type_with_alias(
                        reduced,
                        &mut |c, s| Some(c.instantiate_mapped_constituent(t, type_variable, s, m)),
                        alias.as_deref(),
                    )
                    .unwrap_or_else(|| self.error_type());
            }
        }
        // If the constraint type of the instantiation is the wildcard type,
        // return the wildcard type (Go's comment, checker.go:23131).
        let constraint = crate::relater::get_constraint_type_from_mapped_type(self, t);
        if self.instantiate_type(constraint, m) == self.wildcard_type() {
            return self.wildcard_type();
        }
        self.instantiate_anonymous_type(t, m, alias)
    }

    /// Go: the `instantiateConstituent` closure inside instantiateMappedType
    /// (checker.go:23103).
    fn instantiate_mapped_constituent(
        &mut self,
        t: TypeId,
        type_variable: TypeId,
        s: TypeId,
        m: TypeMapperId,
    ) -> TypeId {
        let flags = self.type_(s).flags;
        if !flags.intersects(
            TypeFlags::ANY_OR_UNKNOWN
                | TypeFlags::INSTANTIABLE_NON_PRIMITIVE
                | TypeFlags::OBJECT
                | TypeFlags::INTERSECTION,
        ) || s == self.wildcard_type()
            || self.is_error_type(s)
        {
            return s;
        }
        let declaration = self
            .type_(t)
            .as_mapped_type()
            .expect("MappedType data")
            .declaration;
        let name_type_is_none = declaration
            .map(|d| {
                let store = self.store();
                store
                    .node(d)
                    .as_mapped_type_node()
                    .map(|n| n.name_type.is_none())
                    .unwrap_or(true)
            })
            .unwrap_or(true);
        if name_type_is_none {
            if crate::relater::is_array_type(self, s) {
                let mapper = self.prepend_type_mapping(type_variable, s, Some(m));
                return self.instantiate_mapped_array_type(s, t, mapper);
            }
            if crate::typenodes::is_tuple_type(self, s) {
                return self.instantiate_mapped_tuple_type(s, t, type_variable, m);
            }
            if self.is_array_or_tuple_or_intersection(s) {
                let types = self.type_(s).types().to_vec();
                let parts: Vec<TypeId> = types
                    .iter()
                    .map(|&u| self.instantiate_mapped_constituent(t, type_variable, u, m))
                    .collect();
                return self.get_intersection_type(&parts);
            }
        }
        let mapper = self.prepend_type_mapping(type_variable, s, Some(m));
        self.instantiate_anonymous_type(t, mapper, None)
    }

    /// Go: `func (c *Checker) getHomomorphicTypeVariable(t *Type) *Type`
    /// (checker.go:22972).
    pub(crate) fn get_homomorphic_type_variable(&mut self, t: TypeId) -> Option<TypeId> {
        let constraint = crate::relater::get_constraint_type_from_mapped_type(self, t);
        if !self.type_(constraint).flags.intersects(TypeFlags::INDEX) {
            return None;
        }
        let target = self
            .type_(constraint)
            .as_index_type()
            .expect("IndexType data")
            .target?;
        let type_variable = self.get_actual_type_variable(target);
        if self.type_(type_variable).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return Some(type_variable);
        }
        None
    }

    /// Go: `func (c *Checker) hasArrayOrTypeTypeConstraint(typeVariable *Type) bool`
    /// (checker.go:23138).
#[allow(dead_code)] // TODO(port M6 S8/S9/S10): Go-mirrored; the member/conditional/index waves consume it
    fn has_array_or_type_type_constraint(&mut self, type_variable: TypeId) -> bool {
        let constraint = crate::relater::get_constraint_of_type_parameter(self, type_variable);
        constraint.is_some_and(|constraint| {
            let constituents = if type_flags(self, constraint).intersects(TypeFlags::UNION) {
                self.type_(constraint).types().to_vec()
            } else {
                vec![constraint]
            };
            constituents.iter().all(|&t| crate::relater::is_array_or_tuple_type(self, t))
        })
    }

    /// Go: `func (c *Checker) instantiateMappedArrayType(arrayType, mappedType *Type, m *TypeMapper) *Type`
    /// (checker.go:23143).
    fn instantiate_mapped_array_type(&mut self, array_type: TypeId, mapped_type: TypeId, m: TypeMapperId) -> TypeId {
        let number_type = self.number_type();
        let element_type = self.instantiate_mapped_type_template(mapped_type, number_type, true, m);
        if self.is_error_type(element_type) {
            return self.error_type();
        }
        let readonly_state = crate::relater::is_readonly_array_type(self, array_type);
        let modifiers = crate::relater::get_mapped_type_modifiers(self, mapped_type);
        let readonly = get_modified_readonly_state(readonly_state, modifiers);
        self.create_array_type_ex(element_type, readonly)
    }

    /// Go: `func (c *Checker) instantiateMappedTupleType(tupleType, mappedType, typeVariable *Type, m *TypeMapper) *Type`
    /// (checker.go:23152).
    fn instantiate_mapped_tuple_type(
        &mut self,
        tuple_type: TypeId,
        mapped_type: TypeId,
        type_variable: TypeId,
        m: TypeMapperId,
    ) -> TypeId {
        let target = self.type_(tuple_type).target_tuple_type();
        let (element_infos, fixed_length, tuple_readonly) = {
            let d = self.type_(target).as_tuple_type().expect("TupleType data");
            (d.element_infos.clone(), d.fixed_length, d.readonly)
        };
        let mut fixed_mapper = m;
        if fixed_length != 0 {
            fixed_mapper = self.prepend_type_mapping(type_variable, tuple_type, Some(m));
        }
        let modifiers = crate::relater::get_mapped_type_modifiers(self, mapped_type);
        let element_types = self.get_element_types(tuple_type);
        let mut new_element_types: Vec<TypeId> = Vec::with_capacity(element_types.len());
        let mut new_element_infos = element_infos.clone();
        for (i, &e) in element_types.iter().enumerate() {
            let flags = element_infos[i].flags;
            let mapped = if (i as i32) < fixed_length {
                let key = self.get_string_literal_type(&i.to_string());
                let is_optional = flags.intersects(types::ElementFlags::OPTIONAL);
                self.instantiate_mapped_type_template(mapped_type, key, is_optional, fixed_mapper)
            } else if flags.intersects(types::ElementFlags::VARIADIC) {
                let mapper = self.prepend_type_mapping(type_variable, e, Some(m));
                self.instantiate_type(mapped_type, mapper)
            } else {
                let array = self.create_array_type(e);
                let mapper = self.prepend_type_mapping(type_variable, array, Some(m));
                let mapped = self.instantiate_type(mapped_type, mapper);
                self.get_element_type_of_array_type(mapped).unwrap_or_else(|| self.unknown_type())
            };
            if modifiers.intersects(crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL) {
                if flags.intersects(types::ElementFlags::REQUIRED) {
                    new_element_infos[i].flags = types::ElementFlags::OPTIONAL;
                }
            } else if modifiers.intersects(crate::relater::MappedTypeModifiers::EXCLUDE_OPTIONAL)
                && flags.intersects(types::ElementFlags::OPTIONAL)
            {
                new_element_infos[i].flags = types::ElementFlags::REQUIRED;
            }
            new_element_types.push(mapped);
        }
        let readonly = get_modified_readonly_state(tuple_readonly, modifiers);
        if new_element_types.contains(&self.error_type()) {
            return self.error_type();
        }
        self.create_tuple_type_ex(&new_element_types, &new_element_infos, readonly)
    }

    /// Go: `func (c *Checker) instantiateMappedTypeTemplate(t *Type, key *Type, isOptional bool, m *TypeMapper) *Type`
    /// (checker.go:23201).
    fn instantiate_mapped_type_template(
        &mut self,
        t: TypeId,
        key: TypeId,
        is_optional: bool,
        m: TypeMapperId,
    ) -> TypeId {
        let type_parameter = crate::relater::get_type_parameter_from_mapped_type(self, t);
        let template_mapper = self.append_type_mapping(Some(m), type_parameter, key);
        let target = self
            .type_(t)
            .as_mapped_type()
            .expect("MappedType data")
            .base
            .target;
        let template_source = target.unwrap_or(t);
        let template_type = crate::relater::get_template_type_from_mapped_type(self, template_source);
        let prop_type = self.instantiate_type(template_type, template_mapper);
        let modifiers = crate::relater::get_mapped_type_modifiers(self, t);
        if self.strict_null_checks
            && modifiers.intersects(crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL)
            && !crate::relater::maybe_type_of_kind(self, prop_type, TypeFlags::UNDEFINED | TypeFlags::VOID)
        {
            return self.get_optional_type(prop_type, true);
        }
        if self.strict_null_checks
            && modifiers.intersects(crate::relater::MappedTypeModifiers::EXCLUDE_OPTIONAL)
            && is_optional
        {
            return self.remove_missing_or_undefined_type(prop_type);
        }
        prop_type
    }

    /// Go: `func (c *Checker) getReducedType(t *Type) *Type` (checker.go:22244)
    /// — REAL (M6 S7): the unionOrIntersection resolvedReducedType walk over
    /// isReducibleIntersection (typenodes.rs, REAL since S5).
    pub fn get_reduced_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            if let Some(reduced) = self.type_(t).as_union_type().and_then(|d| d.resolved_reduced_type) {
                return reduced;
            }
            if self.type_(t).flags.intersects(TypeFlags::INTERSECTION)
                && crate::typenodes::is_reducible_intersection(self, t)
            {
                let reduced = self.get_reduced_type_worker(t);
                if let Some(d) = self.type_mut(t).as_intersection_type_mut() {
                    d.resolved_apparent_type = None;
                }
                // PORT: Go stores resolvedReducedType on the shared
                // UnionOrIntersectionType; the arena keeps it on the union
                // payload (the only reader of this memo is the union arm
                // above — reducible intersections carry their own memo in
                // is_reducible_intersection).
                if let Some(d) = self.type_mut(t).as_union_type_mut() {
                    d.resolved_reduced_type = Some(reduced);
                }
                return reduced;
            }
            return t;
        }
        t
    }

    /// Go: `func (c *Checker) getReducedTypeWorker(t *Type) *Type` (checker.go:22260)
    /// — the reduction engine over the constituents.
    fn get_reduced_type_worker(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S8): the full intersection constituent comparison
        // (checker.go:22260-22320: reduceContravariantIndexInfo +
        // isSameIndexInfo + the union-of-reduced cross product). The S8
        // member-resolution machinery gates the property walk; until it
        // lands, permissive identity keeps reducible intersections stable.
        t
    }

    /// Go: `func (c *Checker) isArrayOrTupleOrIntersection(t) bool`.
    pub(crate) fn is_array_or_tuple_or_intersection(&mut self, t: TypeId) -> bool {
        crate::relater::is_array_or_tuple_type(self, t)
            || self.type_(t).flags.intersects(TypeFlags::INTERSECTION)
    }
}

/// Go: `func getModifiedReadonlyState(state bool, modifiers MappedTypeModifiers) bool`
/// (checker.go:23222).
fn get_modified_readonly_state(
    state: bool,
    modifiers: crate::relater::MappedTypeModifiers,
) -> bool {
    if modifiers.intersects(crate::relater::MappedTypeModifiers::INCLUDE_READONLY) {
        return true;
    }
    if modifiers.intersects(crate::relater::MappedTypeModifiers::EXCLUDE_READONLY) {
        return false;
    }
    state
}

// ── The isThisless family (Go checker.go:21209-21250) ──

/// Go: `func isThisless(symbol *ast.Symbol) bool`.
fn is_thisless(c: &mut Checker, symbol: SymbolId) -> bool {
    let declarations = c.program_symbol(symbol).declarations.clone();
    if declarations.len() == 1 {
        if let Some(&declaration) = declarations.first() {
            let kind = c.store().node(declaration).kind;
            return match kind {
                tsc_ast::Kind::Parameter
                | tsc_ast::Kind::PropertyDeclaration
                | tsc_ast::Kind::PropertySignature => {
                    is_thisless_variable_like_declaration(c, declaration)
                }
                tsc_ast::Kind::MethodDeclaration
                | tsc_ast::Kind::MethodSignature
                | tsc_ast::Kind::Constructor
                | tsc_ast::Kind::GetAccessor
                | tsc_ast::Kind::SetAccessor => {
                    is_thisless_function_like_declaration(c, declaration)
                }
                _ => false,
            };
        }
    }
    false
}

/// Go: `func isThislessVariableLikeDeclaration(node *ast.Node) bool`.
fn is_thisless_variable_like_declaration(c: &mut Checker, node: NodeId) -> bool {
    let (type_node, has_initializer) = {
        let store = c.store();
        (
            crate::ast_shims::type_of_node(store, node),
            crate::ast_shims::initializer_of(store, node).is_some(),
        )
    };
    match type_node {
        Some(type_node) => is_thisless_type(c, type_node),
        None => !has_initializer,
    }
}

/// Go: `func isThislessType(node *ast.Node) bool`.
fn is_thisless_type(c: &mut Checker, node: NodeId) -> bool {
    use tsc_ast::Kind;
    let (kind, array_element, type_arguments) = {
        let store = c.store();
        let n = store.node(node);
        (
            n.kind,
            n.as_array_type_node().map(|d| d.element_type),
            crate::statements::type_arguments_of(store, node),
        )
    };
    match kind {
        Kind::AnyKeyword
        | Kind::UnknownKeyword
        | Kind::StringKeyword
        | Kind::NumberKeyword
        | Kind::BigIntKeyword
        | Kind::BooleanKeyword
        | Kind::SymbolKeyword
        | Kind::ObjectKeyword
        | Kind::VoidKeyword
        | Kind::UndefinedKeyword
        | Kind::NeverKeyword
        | Kind::LiteralType => true,
        Kind::ArrayType => array_element
            .map(|d| is_thisless_type(c, d))
            .unwrap_or(false),
        Kind::TypeReference => type_arguments.iter().all(|&t| is_thisless_type(c, t)),
        _ => false,
    }
}

/// Go: `func isThislessFunctionLikeDeclaration(node *ast.Node) bool`.
fn is_thisless_function_like_declaration(c: &mut Checker, node: NodeId) -> bool {
    let (kind, return_type, parameters, type_parameters) = {
        let store = c.store();
        (
            store.node(node).kind,
            crate::ast_shims::type_of_node(store, node),
            crate::ast_shims::parameters_of(store, node).to_vec(),
            crate::statements::type_parameters_of(store, node),
        )
    };
    let constructor_ok = kind == tsc_ast::Kind::Constructor
        || return_type.is_some_and(|r| is_thisless_type(c, r));
    constructor_ok
        && parameters
            .iter()
            .all(|&p| is_thisless_variable_like_declaration(c, p))
        && type_parameters
            .iter()
            .all(|&tp| is_thisless_type_parameter(c, tp))
}

/// Go: `func isThislessTypeParameter(node *ast.Node) bool`.
fn is_thisless_type_parameter(c: &mut Checker, node: NodeId) -> bool {
    let constraint = c
        .store()
        .node(node)
        .as_type_parameter_declaration()
        .map(|d| d.constraint)
        .unwrap_or(None);
    match constraint {
        None => true,
        Some(constraint) => is_thisless_type(c, constraint),
    }
}
