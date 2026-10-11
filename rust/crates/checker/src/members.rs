// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// M6 S8 — THE STRUCTURED-MEMBERS MACHINERY (dispatch row 10; Go source
// checker.go 19253-22154 plus the adjacent pieces the wave owns:
// setStructuredTypeMembers 25679, getNamedMembers 22508, symbolIsValue
// 22550, the mapped-type component readers 23158-23192 + getMappedTypeModifiers
// 29541 + getModifiersTypeFromMappedType 28661, the never-intersection
// reduction family 22242-22337, getReturnTypeOfSignature/getReturnTypeFromBody
// 20210-20800, the union/intersection signature + property machinery
// 20960-22154, and the apparent-type mapping 22154-22337).
//
// PORT notes (structural, Go → Rust):
//   * Go stores `[]*IndexInfo` on StructuredType/InterfaceType; the port
//     stores `Vec<IndexInfoId>` — handles into `Checker::index_infos`, the
//     same §5.4 arena idiom as SignatureId (and the form S7's
//     instantiate_index_info(s) already speak).
//   * Go's `resolveStructuredTypeMembers` returns the live `*StructuredType`;
//     the port resolves into the type's arena payload and hands callers the
//     existing `crate::relater::ResolvedTypeMembers` snapshot carrier
//     (extended with `members` + `call_signature_count` this wave). Re-entrant
//     reads during member resolution observe the partially-resolved payload
//     exactly as they observe Go's partially-filled StructuredType (the
//     deferred-members protocol — resolveObjectTypeMembers and
//     resolveAnonymousTypeMembers install members before signatures are
//     computed, so a signature that circularly references the type sees the
//     empty-but-resolved state Go guarantees).
//   * Go's closure-shaped bodies (resolveMappedTypeMembers'
//     addMemberForKeyTypeWorker, somePropertyReducesToNever's counting walk)
//     flatten into explicit-parameter methods — the closures only captured
//     locals the explicit parameters now carry (same evaluation order).
//   * The typeCache/circularity triangle (S2 × S7 × S8): this wave adds NO new
//     resolution state — base-type resolution rides S2's
//     pushTypeResolution/popTypeResolution (the ResolvedBaseTypes axis) and
//     every interning table is S7's. Cache keys are S7's get_type_list_key
//     family, unchanged.

use tsc_ast::{CheckFlags, Kind, ModifierFlags, NodeId, SymbolFlags, SymbolId, SymbolTable};
use tsc_diagnostics as diag;

use crate::types::{
    IndexInfoId, ObjectFlags, SignatureFlags, SignatureId, SignatureKind, TypeFlags, TypeId,
};
use crate::todo_shapes::{TypeMapperId, UnionReduction};
use crate::utilities as u;
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// Small free helpers (Go checker.go 27061 / 22508 region)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func forEachType(t *Type, f func(*Type))` (checker.go:27061).
pub(crate) fn for_each_type(c: &mut Checker, t: TypeId, f: &mut dyn FnMut(&mut Checker, TypeId)) {
    if c.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
        let types = c.type_(t).types().to_vec();
        for t in types {
            f(c, t);
        }
    } else {
        f(c, t);
    }
}

impl Checker {
    // ────────────────────────────────────────────────────────────────────────
    // Member-name machinery (Go checker.go 22508-22560)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) isNamedMember(symbol, id) bool` (checker.go:22546).
    pub(crate) fn is_named_member(&mut self, symbol: SymbolId, id: &str) -> bool {
        !u::is_reserved_member_name(id) && self.symbol_is_value(symbol)
    }

    /// Go: `func (c *Checker) symbolIsValue(symbol) bool` (checker.go:22550).
    pub(crate) fn symbol_is_value(&mut self, symbol: SymbolId) -> bool {
        self.symbol_is_value_ex(symbol, false /*includeTypeOnlyMembers*/)
    }

    /// Go: `func (c *Checker) symbolIsValueEx(symbol, includeTypeOnlyMembers) bool`
    /// (checker.go:22554).
    pub(crate) fn symbol_is_value_ex(&mut self, symbol: SymbolId, include_type_only_members: bool) -> bool {
        let flags = self.program_symbol(symbol).flags;
        flags.intersects(SymbolFlags::VALUE)
            || flags.intersects(SymbolFlags::ALIAS)
                && self
                    .get_symbol_flags_ex(
                        symbol,
                        !include_type_only_members,
                        false /*excludeLocalMeanings*/,
                    )
                    .intersects(SymbolFlags::VALUE)
    }

    /// Go: `func (c *Checker) isDeclarationContainedBy(symbol, container) bool`
    /// (checker.go:22535).
    fn is_declaration_contained_by(&self, symbol: SymbolId, container: SymbolId) -> bool {
        let Some(declaration) = self.program_symbol(symbol).value_declaration else {
            return false;
        };
        let container_declarations = self.program_symbol(container).declarations.clone();
        let store = self.store();
        let loc = store.node(declaration).loc;
        for d in container_declarations {
            if loc.contained_by(&store.node(d).loc) {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) getNamedMembers(members, container) []*ast.Symbol`
    /// (checker.go:22508). For classes and interfaces, explicitly declared
    /// members sort ahead of inherited members.
    pub(crate) fn get_named_members(
        &mut self,
        members: &SymbolTable,
        container: Option<SymbolId>,
    ) -> Vec<SymbolId> {
        if members.is_empty() {
            return Vec::new();
        }
        let mut result: Vec<SymbolId> = Vec::with_capacity(members.size());
        let mut contained_count = 0usize;
        let container_is_class_or_interface = container.is_some_and(|container| {
            self.program_symbol(container)
                .flags
                .intersects(SymbolFlags::CLASS.union(SymbolFlags::INTERFACE))
        });
        if container_is_class_or_interface {
            let container = container.expect("checked above");
            for (id, &symbol) in members.entries() {
                if self.is_named_member(symbol, id) && self.is_declaration_contained_by(symbol, container) {
                    result.push(symbol);
                }
            }
            contained_count = result.len();
        }
        for (id, &symbol) in members.entries() {
            if self.is_named_member(symbol, id)
                && (!container_is_class_or_interface
                    || !self.is_declaration_contained_by(symbol, container.expect("checked above")))
            {
                result.push(symbol);
            }
        }
        u::sort_symbols(self, &mut result[..contained_count]);
        u::sort_symbols(self, &mut result[contained_count..]);
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // setStructuredTypeMembers + resolveStructuredTypeMembers (Go 25679 + 19430)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) setStructuredTypeMembers(t, members, callSignatures, constructSignatures, indexInfos)`
    /// (checker.go:25679) — the deferred-members protocol's writer.
    pub(crate) fn set_structured_type_members(
        &mut self,
        t: TypeId,
        members: SymbolTable,
        call_signatures: Vec<SignatureId>,
        construct_signatures: Vec<SignatureId>,
        index_infos: Vec<IndexInfoId>,
    ) {
        self.type_mut(t).object_flags |= ObjectFlags::MEMBERS_RESOLVED;
        let symbol = self.type_(t).symbol;
        let properties = self.get_named_members(&members, symbol);
        let call_signature_count = call_signatures.len() as i32;
        let mut signatures = call_signatures;
        signatures.extend(construct_signatures);
        let data = self
            .type_mut(t)
            .as_structured_type_mut()
            .expect("StructuredType data");
        data.members = members;
        data.properties = properties;
        data.signatures = signatures;
        data.call_signature_count = call_signature_count;
        data.index_infos = index_infos;
    }

    /// Go: `func (c *Checker) resolveStructuredTypeMembers(t *Type) *StructuredType`
    /// (checker.go:19430) — the S8 ROOT. The per-kind resolvers run in Go's
    /// dispatch order (Reference before ClassOrInterface: generic class and
    /// interface types carry both flags).
    pub(crate) fn resolve_structured_type_members_impl(
        &mut self,
        t: TypeId,
    ) -> crate::relater::ResolvedTypeMembers {
        if !self.type_(t).object_flags.intersects(ObjectFlags::MEMBERS_RESOLVED) {
            let flags = self.type_(t).flags;
            if flags.intersects(TypeFlags::OBJECT) {
                let object_flags = self.type_(t).object_flags;
                if object_flags.intersects(ObjectFlags::REFERENCE) {
                    self.resolve_type_reference_members(t);
                } else if object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE) {
                    self.resolve_class_or_interface_members(t);
                } else if object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
                    self.resolve_reverse_mapped_type_members(t);
                } else if object_flags.intersects(ObjectFlags::ANONYMOUS) {
                    self.resolve_anonymous_type_members(t);
                } else if object_flags.intersects(ObjectFlags::MAPPED) {
                    self.resolve_mapped_type_members(t);
                } else {
                    panic!("Unhandled case in resolveStructuredTypeMembers");
                }
            } else if flags.intersects(TypeFlags::UNION) {
                self.resolve_union_type_members(t);
            } else if flags.intersects(TypeFlags::INTERSECTION) {
                self.resolve_intersection_type_members(t);
            } else {
                panic!("Unhandled case in resolveStructuredTypeMembers");
            }
        }
        let (members, properties, signatures, call_signature_count, index_infos) = {
            let d = self.type_(t).as_structured_type().expect("StructuredType data");
            (
                d.members.clone(),
                d.properties.clone(),
                d.signatures.clone(),
                d.call_signature_count,
                d.index_infos.clone(),
            )
        };
        crate::relater::ResolvedTypeMembers {
            members,
            properties,
            signatures,
            call_signature_count,
            index_infos,
        }
    }

    /// Go: `func (c *Checker) resolveClassOrInterfaceMembers(t)` (checker.go:19469).
    fn resolve_class_or_interface_members(&mut self, t: TypeId) {
        self.resolve_object_type_members(t, t, &[], &[]);
    }

    /// Go: `func (c *Checker) resolveTypeReferenceMembers(t)` (checker.go:19475).
    fn resolve_type_reference_members(&mut self, t: TypeId) {
        let source = self.type_(t).target().expect("TypeReference target");
        let type_parameters = self
            .type_(source)
            .as_interface_type()
            .expect("InterfaceType data")
            .all_type_parameters
            .clone();
        let type_arguments = crate::relater::get_type_arguments(self, t);
        let mut padded_type_arguments = type_arguments.clone();
        if type_arguments.len() + 1 == type_parameters.len() {
            padded_type_arguments.push(t);
        }
        self.resolve_object_type_members(t, source, &type_parameters, &padded_type_arguments);
    }

    /// Go: `func (c *Checker) resolveObjectTypeMembers(t, source, typeParameters, typeArguments)`
    /// (checker.go:19487) — the heart of the wave: declared members, optional
    /// instantiation, then the inherited-members walk over the base types.
    fn resolve_object_type_members(
        &mut self,
        t: TypeId,
        source: TypeId,
        type_parameters: &[TypeId],
        type_arguments: &[TypeId],
    ) {
        let mut mapper: Option<TypeMapperId> = None;
        let resolved = self.resolve_declared_members(source);
        let (declared_members, declared_call_signatures, declared_construct_signatures, declared_index_infos) = {
            let d = self.type_(source).as_interface_type().expect("InterfaceType data");
            (
                d.declared_members.clone(),
                d.declared_call_signatures.clone(),
                d.declared_construct_signatures.clone(),
                d.declared_index_infos.clone(),
            )
        };
        let _ = resolved;
        let mut members: SymbolTable;
        let mut call_signatures: Vec<SignatureId>;
        let mut construct_signatures: Vec<SignatureId>;
        let mut index_infos: Vec<IndexInfoId>;
        let mut instantiated = false;
        if type_parameters == type_arguments {
            members = declared_members;
            call_signatures = declared_call_signatures;
            construct_signatures = declared_construct_signatures;
            index_infos = declared_index_infos;
        } else {
            instantiated = true;
            let m = self.new_type_mapper(type_parameters, type_arguments);
            mapper = Some(m);
            members = self.instantiate_symbol_table(&declared_members, m);
            call_signatures = self.instantiate_signatures(&declared_call_signatures, Some(m));
            construct_signatures =
                self.instantiate_signatures(&declared_construct_signatures, Some(m));
            index_infos = self.instantiate_index_infos(&declared_index_infos, Some(m));
        }
        let base_types = self.get_base_types_impl(source);
        if !base_types.is_empty() {
            if !instantiated {
                members = members.clone();
            }
            let this_argument = type_arguments.last().copied();
            for base_type in base_types {
                let mut instantiated_base_type = base_type;
                if let Some(this_argument) = this_argument {
                    let instantiated = match mapper {
                        Some(m) => self.instantiate_type(base_type, m),
                        None => base_type,
                    };
                    instantiated_base_type =
                        self.get_type_with_this_argument_impl(instantiated, Some(this_argument), false);
                }
                let base_properties = self.get_properties_of_type_impl(instantiated_base_type);
                members = self.add_inherited_members(members, base_properties);
                call_signatures.extend(self.get_signatures_of_type_impl(instantiated_base_type, SignatureKind::Call));
                construct_signatures.extend(
                    self.get_signatures_of_type_impl(instantiated_base_type, SignatureKind::Construct),
                );
                let inherited_index_infos: Vec<IndexInfoId> =
                    if instantiated_base_type != self.any_type() {
                        self.get_index_infos_of_type_impl(instantiated_base_type)
                    } else {
                        vec![self.any_base_type_index_info.expect("seeded in new_checker")]
                    };
                for info in inherited_index_infos {
                    let key_type = self.index_info_key_type(info);
                    if find_index_info(self, &index_infos, key_type).is_none() {
                        index_infos.push(info);
                    }
                }
            }
        }
        self.set_structured_type_members(t, members, call_signatures, construct_signatures, index_infos);
    }

    /// Go: `func (c *Checker) addInheritedMembers(symbols, baseSymbols) ast.SymbolTable`
    /// (checker.go:20038).
    fn add_inherited_members(&mut self, mut symbols: SymbolTable, base_symbols: Vec<SymbolId>) -> SymbolTable {
        for base in base_symbols {
            if u::is_static_private_identifier_property(self.store(), self.program_symbol(base)) {
                continue;
            }
            let (name, is_value) = {
                let s = self.program_symbol(base);
                (s.name.clone(), s.flags.intersects(SymbolFlags::VALUE))
            };
            let existing = symbols.get(&name).copied();
            match existing {
                Some(_) if is_value => {}
                _ => symbols.set(name, base),
            }
        }
        symbols
    }

    /// Go: `func (c *Checker) createInstantiatedSymbolTable(symbols, m) ast.SymbolTable`
    /// (checker.go:21255).
    fn create_instantiated_symbol_table(
        &mut self,
        symbols: &[SymbolId],
        m: Option<TypeMapperId>,
    ) -> SymbolTable {
        if symbols.is_empty() {
            return SymbolTable::new();
        }
        let mut result = SymbolTable::new();
        for &symbol in symbols {
            let name = self.program_symbol(symbol).name.clone();
            let instantiated = self.instantiate_symbol(symbol, m);
            result.set(name, instantiated);
        }
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // Base types (Go 19513-20080)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getBaseTypes(t) []*Type` (checker.go:19513).
    pub(crate) fn get_base_types_impl(&mut self, t: TypeId) -> Vec<TypeId> {
        if !self
            .type_(t)
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE.union(ObjectFlags::TUPLE))
        {
            return Vec::new();
        }
        let already_resolved = self
            .type_(t)
            .as_interface_type()
            .expect("InterfaceType data")
            .base_types_resolved;
        if !already_resolved {
            if !self.push_type_resolution(
                crate::checker::TypeSystemEntity::Type(t),
                crate::checker::TypeSystemPropertyName::ResolvedBaseTypes,
            ) {
                return self
                    .type_(t)
                    .as_interface_type()
                    .expect("InterfaceType data")
                    .resolved_base_types
                    .clone();
            }
            let symbol = self.type_(t).symbol;
            let symbol_flags = symbol.map(|s| self.program_symbol(s).flags).unwrap_or_default();
            if self.type_(t).object_flags.intersects(ObjectFlags::TUPLE) {
                let tuple_base = self.get_tuple_base_type(t);
                self.type_mut(t)
                    .as_interface_type_mut()
                    .expect("InterfaceType data")
                    .resolved_base_types = vec![tuple_base];
            } else if symbol_flags.intersects(SymbolFlags::CLASS.union(SymbolFlags::INTERFACE)) {
                if symbol_flags.intersects(SymbolFlags::CLASS) {
                    self.resolve_base_types_of_class(t);
                }
                if symbol_flags.intersects(SymbolFlags::INTERFACE) {
                    self.resolve_base_types_of_interface(t);
                }
            } else {
                panic!("Unhandled case in getBaseTypes");
            }
            if !self.pop_type_resolution() {
                if let Some(symbol) = symbol {
                    let declarations = self.program_symbol(symbol).declarations.clone();
                    for declaration in declarations {
                        if matches!(
                            self.store().node(declaration).kind,
                            Kind::ClassDeclaration | Kind::InterfaceDeclaration
                        ) {
                            self.report_circular_base_type(declaration, t);
                        }
                    }
                }
            }
            // In general, base type resolution always precedes member
            // resolution. However, it is possible for resolution of type
            // parameter defaults to cause circularity errors, possibly
            // leaving members partially resolved. Here we ensure any such
            // partial resolution is reset (Go's comment, GH#16861).
            self.type_mut(t).object_flags &= !ObjectFlags::MEMBERS_RESOLVED;
            self.type_mut(t)
                .as_interface_type_mut()
                .expect("InterfaceType data")
                .base_types_resolved = true;
        }
        self.type_(t)
            .as_interface_type()
            .expect("InterfaceType data")
            .resolved_base_types
            .clone()
    }

    /// Go: `func (c *Checker) getTupleBaseType(t) *Type` (checker.go:19557).
    fn get_tuple_base_type(&mut self, t: TypeId) -> TypeId {
        let (type_parameters, element_infos, readonly) = {
            let d = self.type_(t).as_interface_type().expect("InterfaceType data");
            let tuple = self.type_(t).as_tuple_type().expect("TupleType data");
            (
                d.all_type_parameters.clone(),
                tuple.element_infos.clone(),
                tuple.readonly,
            )
        };
        let number_type = self.number_type();
        let mut element_types: Vec<TypeId> = Vec::with_capacity(type_parameters.len());
        for (i, &tp) in type_parameters.iter().enumerate() {
            let is_variadic = element_infos
                .get(i)
                .is_some_and(|info| info.flags.intersects(crate::types::ElementFlags::VARIADIC));
            if is_variadic {
                element_types.push(crate::relater::get_indexed_access_type(self, tp, number_type));
            } else {
                element_types.push(tp);
            }
        }
        let union = self.get_union_type(&element_types);
        self.create_array_type_ex(union, readonly)
    }

    /// Go: `func (c *Checker) resolveBaseTypesOfClass(t)` (checker.go:19624).
    fn resolve_base_types_of_class(&mut self, t: TypeId) {
        let base_ctor = self.get_base_constructor_type_of_class_impl(t);
        let base_constructor_type = self.get_apparent_type_impl(base_ctor);
        let base_flags = self.type_(base_constructor_type).flags;
        if !base_flags
            .intersects(TypeFlags::OBJECT.union(TypeFlags::INTERSECTION).union(TypeFlags::ANY))
        {
            return;
        }
        let base_type_node = self.get_base_type_node_of_class(t);
        
        let mut original_base_type: Option<TypeId> = None;
        let base_constructor_symbol = self.type_(base_constructor_type).symbol;
        if let Some(symbol) = base_constructor_symbol {
            original_base_type = Some(self.get_declared_type_of_symbol(symbol));
        }
        let expression = base_type_node.map(|node| {
            crate::ast_shims::expression_of(self.store(), node)
                .expect("class heritage expression")
        });
        let base_type: Option<TypeId> = if base_constructor_symbol.is_some_and(|s| {
            self.program_symbol(s).flags.intersects(SymbolFlags::CLASS)
                && self.are_all_outer_type_parameters_applied(original_base_type.expect("set above"))
        }) {
            // When base constructor type is a class with no captured type
            // arguments we know that the constructors all have the same type
            // parameters as the class and all return the instance type of the
            // class (Go's comment).
            Some(
                self.get_type_from_class_or_interface_reference(
                    base_type_node.expect("class base type node"),
                    base_constructor_symbol.expect("checked"),
                ),
            )
        } else if base_flags.intersects(TypeFlags::ANY) {
            Some(base_constructor_type)
        } else {
            // The class derives from a "class-like" constructor function.
            let type_argument_nodes = base_type_node
                .map(|node| crate::statements::type_arguments_of(self.store(), node).to_vec())
                .unwrap_or_default();
            let constructors = self.get_instantiated_constructors_for_type_arguments(
                base_constructor_type,
                &type_argument_nodes,
                base_type_node.expect("class base type node"),
            );
            if constructors.is_empty() {
                if let Some(expression) = expression {
                    self.error(
                        expression,
                        &diag::NO_BASE_CONSTRUCTOR_HAS_THE_SPECIFIED_NUMBER_OF_TYPE_ARGUMENTS,
                        &[],
                    );
                }
                return;
            }
            Some(self.get_return_type_of_signature_impl(constructors[0]))
        };
        let Some(base_type) = base_type else { return };
        if self.is_error_type(base_type) {
            return;
        }
        let reduced_base_type = self.get_reduced_type(base_type);
        if !self.is_valid_base_type(reduced_base_type) {
            let error_node = expression.expect("class heritage expression");
            let diagnostic = self.elaborate_never_intersection(None, error_node, base_type);
            let type_string = self.type_to_string(reduced_base_type);
            let diagnostic = u::new_diagnostic_chain_for_node(
                diagnostic.as_ref(),
                self.store(),
                error_node,
                &diag::BASE_CONSTRUCTOR_RETURN_TYPE_0_IS_NOT_AN_OBJECT_TYPE_OR_INTERSECTION_OF_OBJECT_TYPES_WITH_STATICALLY_KNOWN_MEMBERS,
                vec![type_string],
            );
            self.add_diagnostic(diagnostic);
            return;
        }
        if t == reduced_base_type || self.has_base_type_impl(reduced_base_type, t) {
            let type_string = self.type_to_string(t);
            if let Some(value_declaration) = self
                .type_(t)
                .symbol
                .and_then(|s| self.program_symbol(s).value_declaration)
            {
                self.error(
                    value_declaration,
                    &diag::TYPE_0_RECURSIVELY_REFERENCES_ITSELF_AS_A_BASE_TYPE,
                    &[type_string],
                );
            }
            return;
        }
        self.type_mut(t)
            .as_interface_type_mut()
            .expect("InterfaceType data")
            .resolved_base_types = vec![reduced_base_type];
    }

    /// Go: `func getBaseTypeNodeOfClass(t) *ast.Node` (checker.go:19707).
    fn get_base_type_node_of_class(&mut self, t: TypeId) -> Option<NodeId> {
        let symbol = self.type_(t).symbol?;
        let declaration =
            crate::expressions::get_class_like_declaration_of_symbol_for(self, symbol)?;
        crate::statements::get_class_extends_heritage_element(self.store(), declaration)
    }

    /// Go: `func (c *Checker) getInstantiatedConstructorsForTypeArguments(t, typeArgumentNodes, location) []*Signature`
    /// (checker.go:19717) — kills the resolvecall/statements S8 seam.
    pub(crate) fn get_instantiated_constructors_for_type_arguments(
        &mut self,
        t: TypeId,
        type_argument_nodes: &[NodeId],
        location: NodeId,
    ) -> Vec<SignatureId> {
        let signatures = self.get_constructors_for_type_arguments(t, type_argument_nodes, location);
        let mut type_arguments = Vec::with_capacity(type_argument_nodes.len());
        for &node in type_argument_nodes {
            type_arguments.push(self.get_type_from_type_node(node));
        }
        let is_js = tsc_ast::utilities::is_in_js_file(self.store().node(location));
        let mut result = Vec::with_capacity(signatures.len());
        for sig in signatures {
            let type_parameters = self.signatures[sig.0 as usize].type_parameters.clone();
            if !type_parameters.is_empty() {
                result.push(
                    self.get_signature_instantiation_for_members(sig, &type_arguments, is_js, &[]),
                );
            } else {
                result.push(sig);
            }
        }
        result
    }

    /// Go: `func (c *Checker) getConstructorsForTypeArguments(t, typeArgumentNodes, location) []*Signature`
    /// (checker.go:19731).
    fn get_constructors_for_type_arguments(
        &mut self,
        t: TypeId,
        type_argument_nodes: &[NodeId],
        _location: NodeId,
    ) -> Vec<SignatureId> {
        let type_arg_count = type_argument_nodes.len();
        self.get_signatures_of_type_impl(t, SignatureKind::Construct)
            .into_iter()
            .filter(|&sig| {
                let type_parameters = self.signatures[sig.0 as usize].type_parameters.clone();
                let min = self.get_min_type_argument_count(&type_parameters);
                type_arg_count >= min && type_arg_count <= type_parameters.len()
            })
            .collect()
    }

    /// Go: `func (c *Checker) resolveBaseTypesOfInterface(t)` (checker.go:19901).
    fn resolve_base_types_of_interface(&mut self, t: TypeId) {
        let Some(symbol) = self.type_(t).symbol else {
            return;
        };
        let declarations = self.program_symbol(symbol).declarations.clone();
        for declaration in declarations {
            if self.store().node(declaration).kind != Kind::InterfaceDeclaration {
                continue;
            }
            for node in crate::statements::get_extends_heritage_clause_elements(self.store(), declaration) {
                let base_type = self.get_type_from_type_node(node);
                let base_type = self.get_reduced_type(base_type);
                if self.is_error_type(base_type) {
                    continue;
                }
                if self.is_valid_base_type(base_type) {
                    if t != base_type && !self.has_base_type_impl(base_type, t) {
                        self.type_mut(t)
                            .as_interface_type_mut()
                            .expect("InterfaceType data")
                            .resolved_base_types
                            .push(base_type);
                    } else {
                        self.report_circular_base_type(declaration, t);
                    }
                } else {
                    self.error(
                        node,
                        &diag::AN_INTERFACE_CAN_ONLY_EXTEND_AN_OBJECT_TYPE_OR_INTERSECTION_OF_OBJECT_TYPES_WITH_STATICALLY_KNOWN_MEMBERS,
                        &[],
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) areAllOuterTypeParametersApplied(t) bool` (checker.go:19911).
    fn are_all_outer_type_parameters_applied(&mut self, t: TypeId) -> bool {
        // An unapplied type parameter has its symbol still the same as the
        // matching argument symbol (Go's comment).
        let d = self.type_(t).as_interface_type().expect("InterfaceType data");
        let outer_type_parameters = d.outer_type_parameters().to_vec();
        if !outer_type_parameters.is_empty() {
            let last = outer_type_parameters.len() - 1;
            let type_arguments = crate::relater::get_type_arguments(self, t);
            let Some(argument_symbol) = type_arguments.get(last).and_then(|&t| self.type_(t).symbol) else {
                return false;
            };
            let Some(parameter_symbol) = self.type_(outer_type_parameters[last]).symbol else {
                return false;
            };
            return parameter_symbol != argument_symbol;
        }
        true
    }

    /// Go: `func (c *Checker) reportCircularBaseType(node, t)` (checker.go:19931).
    fn report_circular_base_type(&mut self, node: NodeId, t: TypeId) {
        let type_string = self.type_to_string(t);
        self.error(
            node,
            &diag::TYPE_0_RECURSIVELY_REFERENCES_ITSELF_AS_A_BASE_TYPE,
            &[type_string],
        );
    }

    /// Go: `func (c *Checker) isValidBaseType(t) bool` (checker.go:19937) — a
    /// valid base type is `any`, an object type or intersection of object types.
    pub(crate) fn is_valid_base_type(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            if let Some(constraint) = crate::relater::get_base_constraint_of_type(self, t) {
                return self.is_valid_base_type(constraint);
            }
        }
        // TODO: Given that we allow type parameters here now, is this
        // `!isGenericMappedType(type)` check really needed? (Go's comment.)
        (flags.intersects(
            TypeFlags::OBJECT.union(TypeFlags::NON_PRIMITIVE).union(TypeFlags::ANY),
        ) && !crate::relater::is_generic_mapped_type(self, t))
            || flags.intersects(TypeFlags::INTERSECTION)
                && {
                    let types = self.type_(t).types().to_vec();
                    types.iter().all(|&t| self.is_valid_base_type(t))
                }
    }

    /// Go: `func (c *Checker) hasBaseType(t, checkBase) bool` (checker.go:19945)
    /// — the recursive closure ports as an explicit worker.
    pub(crate) fn has_base_type_impl(&mut self, t: TypeId, check_base: TypeId) -> bool {
        if self.type_(t)
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE.union(ObjectFlags::REFERENCE))
        {
            let target = crate::uniontypes::get_target_type(self, t);
            if target == check_base {
                return true;
            }
            return self
                .get_base_types_impl(target)
                .into_iter()
                .any(|base| self.has_base_type_impl(base, check_base));
        }
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.type_(t).types().to_vec();
            return types.into_iter().any(|t| self.has_base_type_impl(t, check_base));
        }
        false
    }

    /// Go: `func (c *Checker) getTypeWithThisArgument(t, thisArgument, needApparentType) *Type`
    /// (checker.go:19951).
    pub(crate) fn get_type_with_this_argument_impl(
        &mut self,
        t: TypeId,
        this_argument: Option<TypeId>,
        need_apparent_type: bool,
    ) -> TypeId {
        if self.type_(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            let target = self.type_(t).target().expect("TypeReference target");
            let type_arguments = crate::relater::get_type_arguments(self, t);
            let type_parameter_count = self
                .type_(target)
                .as_interface_type()
                .expect("InterfaceType data")
                .type_parameters()
                .len();
            if type_parameter_count == type_arguments.len() {
                let this_argument = match this_argument {
                    Some(this_argument) => Some(this_argument),
                    None => self
                        .type_(target)
                        .as_interface_type()
                        .expect("InterfaceType data")
                        .this_type,
                };
                if let Some(this_argument) = this_argument {
                    let mut args = type_arguments;
                    args.push(this_argument);
                    return self.create_type_reference(target, args);
                }
                return t;
            }
            return t;
        } else if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.type_(t).types().to_vec();
            let new_types: Vec<TypeId> = types
                .iter()
                .map(|&t| self.get_type_with_this_argument_impl(t, this_argument, need_apparent_type))
                .collect();
            if new_types == types {
                return t;
            }
            return self.get_intersection_type(&new_types);
        }
        if need_apparent_type {
            return self.get_apparent_type_impl(t);
        }
        t
    }

    // ────────────────────────────────────────────────────────────────────────
    // Declared members + index infos (Go 20047-20290)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) resolveDeclaredMembers(t) *InterfaceType` (checker.go:20047).
    fn resolve_declared_members(&mut self, t: TypeId) -> bool {
        let already = self
            .type_(t)
            .as_interface_type()
            .expect("InterfaceType data")
            .declared_members_resolved;
        if !already {
            let Some(symbol) = self.type_(t).symbol else {
                // Go dereferences t.symbol unconditionally here (classes and
                // interfaces always carry one); the defensive arm keeps the
                // port total.
                self.type_mut(t)
                    .as_interface_type_mut()
                    .expect("InterfaceType data")
                    .declared_members_resolved = true;
                return false;
            };
            let members = self.get_members_of_symbol_table(symbol);
            let call = members
                .get(tsc_ast::INTERNAL_SYMBOL_NAME_CALL)
                .copied()
                .map(|s| self.get_signatures_of_symbol(Some(s)))
                .unwrap_or_default();
            let construct = members
                .get(tsc_ast::INTERNAL_SYMBOL_NAME_NEW)
                .copied()
                .map(|s| self.get_signatures_of_symbol(Some(s)))
                .unwrap_or_default();
            let index_infos = self.get_index_infos_of_symbol(Some(symbol));
            let d = self
                .type_mut(t)
                .as_interface_type_mut()
                .expect("InterfaceType data");
            d.declared_members_resolved = true;
            d.declared_members = members;
            d.declared_call_signatures = call;
            d.declared_construct_signatures = construct;
            d.declared_index_infos = index_infos;
        }
        true
    }

    /// Go: `func (c *Checker) getMembersOfSymbol(symbol) ast.SymbolTable` — the
    /// SymbolTable form S8 reads (the Vec snapshot twin lives in
    /// typenodes.rs for the S5 call sites).
    pub(crate) fn get_members_of_symbol_table(&mut self, symbol: SymbolId) -> SymbolTable {
        if self
            .program_symbol(symbol)
            .flags
            .intersects(SymbolFlags::LATE_BINDING_CONTAINER)
        {
            return self.get_resolved_members_or_exports_of_symbol(
                symbol,
                crate::types::MembersOrExportsResolutionKind::ResolvedMembers,
            );
        }
        self.program_symbol(symbol).members.clone()
    }

    /// Go: `func (c *Checker) getIndexInfosOfSymbol(symbol) []*IndexInfo` (checker.go:20070).
    pub(crate) fn get_index_infos_of_symbol(&mut self, symbol: Option<SymbolId>) -> Vec<IndexInfoId> {
        let Some(symbol) = symbol else { return Vec::new() };
        let index_symbol = self.get_index_symbol(symbol);
        if let Some(index_symbol) = index_symbol {
            let sibling_symbols: Vec<SymbolId> = self
                .get_members_of_symbol_table(symbol)
                .values()
                .copied()
                .collect();
            return self.get_index_infos_of_index_symbol(index_symbol, sibling_symbols);
        }
        Vec::new()
    }

    /// Go: `func (c *Checker) getIndexInfosOfIndexSymbol(indexSymbol, siblingSymbols) []*IndexInfo`
    /// (checker.go:20079) — note intentional similarities to index signature
    /// building in `checkObjectLiteral` for parity (Go's comment).
    fn get_index_infos_of_index_symbol(
        &mut self,
        index_symbol: SymbolId,
        sibling_symbols: Vec<SymbolId>,
    ) -> Vec<IndexInfoId> {
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        let has_computed_string_property = false;
        let has_computed_number_property = false;
        let has_computed_symbol_property = false;
        let readonly_computed_string_property = true;
        let readonly_computed_number_property = true;
        let readonly_computed_symbol_property = true;
        let property_symbols: Vec<SymbolId> = Vec::new();
        let declarations = self.program_symbol(index_symbol).declarations.clone();
        for declaration in declarations {
            if self.store().node(declaration).kind == Kind::IndexSignature {
                let parameters = crate::ast_shims::parameters_of(self.store(), declaration).to_vec();
                let return_type_node = crate::ast_shims::type_of_node(self.store(), declaration);
                if parameters.len() == 1 {
                    let Some(type_node) =
                        crate::ast_shims::type_of_node(self.store(), parameters[0])
                    else {
                        continue;
                    };
                    let value_type = match return_type_node {
                        Some(return_type_node) => self.get_type_from_type_node(return_type_node),
                        None => self.any_type(),
                    };
                    let key_types_union = self.get_type_from_type_node(type_node);
                    let mut key_types: Vec<TypeId> = Vec::new();
                    for_each_type(self, key_types_union, &mut |_c, t| key_types.push(t));
                    for key_type in key_types {
                        if self.is_valid_index_key_type(key_type)
                            && find_index_info(self, &index_infos, key_type).is_none()
                        {
                            let is_readonly = crate::ast_shims::has_modifier(
                                self.store().node(declaration),
                                ModifierFlags::READONLY,
                            );
                            let info = IndexInfoId(
                                self.new_index_info(
                                    key_type,
                                    value_type,
                                    is_readonly,
                                    Some(declaration),
                                    Vec::new(),
                                ) as u32,
                            );
                            index_infos.push(info);
                        }
                    }
                }
            } else if self.has_late_bindable_index_signature(declaration) {
                // TODO(port M6 S4): the JS computed-name index signature arm —
                // hasLateBindableIndexSignature only holds in JS files; the
                // S4 JS wave owns checkComputedPropertyName-on-declaration
                // side effects. Permissive skip preserves Go's .ts behavior.
                continue;
            }
        }
        // Go aggregates computed string/number/symbol properties into
        // synthetic index infos — that arm is JS-only (the declarations that
        // reach it are late-bindable JS class members); the permissive skip
        // above means none of the hasComputed* flags ever set in .ts files.
        let _ = (
            has_computed_string_property,
            has_computed_number_property,
            has_computed_symbol_property,
            readonly_computed_string_property,
            readonly_computed_number_property,
            readonly_computed_symbol_property,
            property_symbols,
            sibling_symbols,
        );
        index_infos
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) getObjectLiteralIndexInfo(isReadonly, properties, keyType) *IndexInfo`
    /// (checker.go:20156).
    fn get_object_literal_index_info(
        &mut self,
        is_readonly: bool,
        properties: &[SymbolId],
        key_type: TypeId,
    ) -> IndexInfoId {
        let mut prop_types: Vec<TypeId> = Vec::new();
        let mut components: Vec<NodeId> = Vec::new();
        let string_type = self.string_type();
        let number_type = self.number_type();
        let es_symbol_type = self.es_symbol_type();
        for &prop in properties {
            let matches = (key_type == string_type && !self.is_symbol_with_symbol_name(prop))
                || (key_type == number_type && self.is_symbol_with_numeric_name(prop))
                || (key_type == es_symbol_type && self.is_symbol_with_symbol_name(prop));
            if matches {
                prop_types.push(self.get_type_of_symbol(prop));
                if self.is_symbol_with_computed_name(prop) {
                    let declaration = self.program_symbol(prop).declarations.first().copied();
                    if let Some(declaration) = declaration {
                        components.push(declaration);
                    }
                }
            }
        }
        let union_type = if prop_types.is_empty() {
            self.undefined_type()
        } else {
            self.get_union_type_ex(prop_types, UnionReduction::Subtype, None, None)
        };
        IndexInfoId(
            self.new_index_info(key_type, union_type, is_readonly, None, components) as u32,
        )
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) isSymbolWithSymbolName(symbol) bool` (checker.go:20182).
    fn is_symbol_with_symbol_name(&mut self, symbol: SymbolId) -> bool {
        if u::is_known_symbol(self.program_symbol(symbol)) {
            return true;
        }
        let declarations = self.program_symbol(symbol).declarations.clone();
        if let Some(&first) = declarations.first() {
            let Some(name) = self.store().node(first).name() else {
                return false;
            };
            if self.store().node(name).kind == Kind::ComputedPropertyName {
                let t = self.check_computed_property_name(name);
                return self.is_type_assignable_to_kind(t, TypeFlags::ESSYMBOL);
            }
        }
        false
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) isSymbolWithNumericName(symbol) bool` (checker.go:20192).
    fn is_symbol_with_numeric_name(&mut self, symbol: SymbolId) -> bool {
        if u::is_numeric_literal_name(&self.program_symbol(symbol).name) {
            return true;
        }
        let declarations = self.program_symbol(symbol).declarations.clone();
        if let Some(&first) = declarations.first() {
            let Some(name) = self.store().node(first).name() else {
                return false;
            };
            return self.is_numeric_name(name);
        }
        false
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) isSymbolWithComputedName(symbol) bool` (checker.go:20203).
    fn is_symbol_with_computed_name(&mut self, symbol: SymbolId) -> bool {
        let declarations = self.program_symbol(symbol).declarations.clone();
        if let Some(&first) = declarations.first() {
            let Some(name) = self.store().node(first).name() else {
                return false;
            };
            return self.store().node(name).kind == Kind::ComputedPropertyName;
        }
        false
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) isNumericName(name) bool` (checker.go:21219 region —
    /// checker.go:20213).
    fn is_numeric_name(&mut self, name: NodeId) -> bool {
        match self.store().node(name).kind {
            Kind::ComputedPropertyName => self.is_numeric_computed_name(name),
            Kind::Identifier | Kind::NumericLiteral | Kind::StringLiteral => {
                u::is_numeric_literal_name(tsc_ast::node_text(self.store(), name))
            }
            _ => false,
        }
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) isNumericComputedName(name) bool` (checker.go:20226).
    fn is_numeric_computed_name(&mut self, name: NodeId) -> bool {
        // It seems odd to consider an expression of type Any to result in a
        // numeric name, but this behavior is consistent with checkIndexedAccess
        // (Go's comment).
        let t = self.check_computed_property_name(name);
        self.is_type_assignable_to_kind(t, TypeFlags::NUMBER_LIKE)
    }

    /// Go: `func (c *Checker) isValidIndexKeyType(t) bool` (checker.go:20233).
    fn is_valid_index_key_type(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::STRING.union(TypeFlags::NUMBER).union(TypeFlags::ESSYMBOL)) {
            return true;
        }
        if crate::typenodes::is_pattern_literal_type(self, t) {
            return true;
        }
        if flags.intersects(TypeFlags::INTERSECTION)
            && !self.is_generic_type(t)
            && {
                let types = self.type_(t).types().to_vec();
                types.iter().any(|&t| self.is_valid_index_key_type(t))
            }
        {
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) getIndexSymbol(symbol) *ast.Symbol` (checker.go:20239).
    fn get_index_symbol(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        self.get_members_of_symbol_table(symbol)
            .get(tsc_ast::INTERNAL_SYMBOL_NAME_INDEX)
            .copied()
    }
}

/// Go: `func (c *Checker) findIndexInfo(indexInfos, keyType) *IndexInfo` — the
/// free-fn form (checker.go:19503; the method twin below serves the impl).
pub(crate) fn find_index_info(c: &Checker, index_infos: &[IndexInfoId], key_type: TypeId) -> Option<IndexInfoId> {
    index_infos
        .iter()
        .copied()
        .find(|&info| c.index_infos[info.0 as usize].key_type == Some(key_type))
}

// ────────────────────────────────────────────────────────────────────────────
// Signatures of symbols + signature construction (Go 20246-20528)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getSignaturesOfSymbol(symbol) []*Signature` (checker.go:20246).
    pub(crate) fn get_signatures_of_symbol(&mut self, symbol: Option<SymbolId>) -> Vec<SignatureId> {
        let Some(symbol) = symbol else { return Vec::new() };
        let declarations = self.program_symbol(symbol).declarations.clone();
        let mut result: Vec<SignatureId> = Vec::new();
        for (i, decl) in declarations.iter().copied().enumerate() {
            if !tsc_ast::utilities::is_function_like(self.store().node(decl)) {
                continue;
            }
            // Don't include signature if node is the implementation of an
            // overloaded function (Go's comment).
            let body = crate::statements::function_body_of(self.store(), decl);
            if i > 0 && body.is_some() {
                let previous = declarations[i - 1];
                if self.store().node(decl).parent.get() == self.store().node(previous).parent.get()
                    && self.store().node(decl).kind == self.store().node(previous).kind
                    && (self.store().node(decl).pos() == self.store().node(previous).end()
                        || self.store().node(previous).flags.intersects(tsc_ast::NodeFlags::REPARSED))
                {
                    continue;
                }
            }
            // If this is a function or method declaration, get the signature
            // from the @type tag for the sake of optional parameters (Go's
            // comment) — the JS JSDoc full-signature seam stays bannered
            // until the W3/JS wave.
            // TODO(port M6 W3): getSignatureOfFullSignatureType (JSDoc FullSignature).
            let sig = self.get_signature_from_declaration_impl(decl);
            result.push(sig);
        }
        result
    }

    /// Go: `func (c *Checker) getSignatureFromDeclaration(declaration) *Signature`
    /// (checker.go:20296) — the REAL body (kills the typeofsymbol.rs S8
    /// stub). Go never returns nil; the typeofsymbol.rs twin's Option (None
    /// for non-function-like declarations) preserves the S1 callers'
    /// guards.
    pub(crate) fn get_signature_from_declaration_impl(&mut self, declaration: NodeId) -> SignatureId {
        let cached = self.signature_node_links.get(declaration).resolved_signature;
        if let Some(cached) = cached {
            return cached;
        }
        let mut parameters: Vec<SymbolId> = Vec::new();
        let mut flags = SignatureFlags::NONE;
        let mut this_parameter: Option<SymbolId> = None;
        let mut min_argument_count: i32 = 0;
        let mut has_this_parameter = false;
        let iife = tsc_binder::ast_shims::get_immediately_invoked_function_expression(
            self.store(),
            declaration,
        );
        let is_untyped_signature_in_js_file = iife.is_none()
            && tsc_ast::utilities::is_in_js_file(self.store().node(declaration))
            && matches!(
                self.store().node(declaration).kind,
                Kind::FunctionExpression
                    | Kind::ArrowFunction
                    | Kind::MethodDeclaration
                    | Kind::GetAccessor
                    | Kind::SetAccessor
                    | Kind::FunctionDeclaration
                    | Kind::Constructor
            )
            && crate::ast_shims::parameters_of(self.store(), declaration)
                .iter()
                .all(|&param| crate::ast_shims::type_of_node(self.store(), param).is_none())
            && self
                .get_contextual_type(declaration, crate::types::ContextFlags::SIGNATURE)
                .is_none();
        if is_untyped_signature_in_js_file {
            flags |= SignatureFlags::IS_UNTYPED_SIGNATURE_IN_JS_FILE;
        }
        let param_nodes = crate::ast_shims::parameters_of(self.store(), declaration).to_vec();
        for (i, param) in param_nodes.iter().copied().enumerate() {
            let mut param_symbol = crate::typeofsymbol::get_symbol_of_declaration(self, param);
            let type_node = crate::ast_shims::type_of_node(self.store(), param);
            // Include parameter symbol instead of property symbol in the signature
            if let Some(symbol) = param_symbol
                && self
                    .program_symbol(symbol)
                    .flags
                    .intersects(SymbolFlags::PROPERTY)
                && !crate::statements::is_binding_pattern(self.store(), param)
            {
                let name = self.program_symbol(symbol).name.clone();
                let resolved = self.resolve_name_worker(
                    Some(param),
                    &name,
                    SymbolFlags::VALUE,
                    None,
                    false /*isUse*/,
                    false /*excludeGlobals*/,
                );
                if let Some(resolved) = resolved {
                    param_symbol = Some(resolved);
                }
            }
            let param_symbol_name = param_symbol
                .map(|s| self.program_symbol(s).name.clone())
                .unwrap_or_default();
            if i == 0 && param_symbol_name == tsc_ast::INTERNAL_SYMBOL_NAME_THIS {
                has_this_parameter = true;
                this_parameter = crate::typeofsymbol::get_symbol_of_declaration(self, param);
            } else if let Some(param_symbol) = param_symbol {
                parameters.push(param_symbol);
            }
            if type_node.is_some_and(|tn| self.store().node(tn).kind == Kind::LiteralType) {
                flags |= SignatureFlags::HAS_LITERAL_TYPES;
            }
            // Record a new minimum argument count if this is not an optional
            // parameter (Go's comment).
            let is_optional_parameter = u::is_optional_declaration(self.store(), param)
                || crate::ast_shims::initializer_of(self.store(), param).is_some()
                || u::has_dot_dot_dot_token(self.store(), param)
                || (iife.is_some()
                    && parameters.len()
                        > iife.map(|i| {
                            let arguments =
                                tsc_binder::node_data::arguments_of(self.store(), i);
                            arguments.len()
                        })
                        .unwrap_or(usize::MAX)
                    && type_node.is_none());
            if !is_optional_parameter {
                min_argument_count = parameters.len() as i32;
            }
        }
        // If only one accessor includes a this-type annotation, the other
        // behaves as if it had the same type annotation (Go's comment).
        if matches!(self.store().node(declaration).kind, Kind::GetAccessor | Kind::SetAccessor)
            && self.has_bindable_name(declaration)
            && (!has_this_parameter || this_parameter.is_none())
        {
            let other_kind = if self.store().node(declaration).kind == Kind::GetAccessor {
                Kind::SetAccessor
            } else {
                Kind::GetAccessor
            };
            let own_symbol = crate::typeofsymbol::get_symbol_of_declaration(self, declaration);
            if let Some(own_symbol) = own_symbol {
                let other = self
                    .program_symbol(own_symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| self.store().node(d).kind == other_kind);
                if let Some(other) = other {
                    this_parameter = self.get_annotated_accessor_this_parameter(other);
                }
            }
        }
        let class_type: Option<TypeId> = if self.store().node(declaration).kind == Kind::Constructor {
            let parent = self.store().node(declaration).parent.get();
            let parent_symbol = crate::typeofsymbol::get_symbol_of_declaration(self, parent);
            parent_symbol.map(|s| {
                self.get_declared_type_of_class_or_interface(self.get_merged_symbol(s))
            })
        } else {
            None
        };
        let type_parameters: Vec<TypeId> = match class_type {
            Some(class_type) => self
                .type_(class_type)
                .as_interface_type()
                .expect("InterfaceType data")
                .local_type_parameters()
                .to_vec(),
            None => self.get_type_parameters_from_declaration(declaration),
        };
        if self.has_rest_parameter(declaration) {
            flags |= SignatureFlags::HAS_REST_PARAMETER;
        }
        if matches!(
            self.store().node(declaration).kind,
            Kind::ConstructorType | Kind::Constructor | Kind::ConstructSignature
        ) {
            flags |= SignatureFlags::CONSTRUCT;
        }
        if (self.store().node(declaration).kind == Kind::ConstructorType
            && tsc_ast::has_syntactic_modifier(self.store().node(declaration), ModifierFlags::ABSTRACT))
            || (self.store().node(declaration).kind == Kind::Constructor
                && tsc_ast::has_syntactic_modifier(
                    self.store().node(self.store().node(declaration).parent.get()),
                    ModifierFlags::ABSTRACT,
                ))
        {
            flags |= SignatureFlags::ABSTRACT;
        }
        let result = self.new_signature(
            flags,
            Some(declaration),
            type_parameters,
            this_parameter,
            parameters,
            None /*resolvedReturnType*/,
            min_argument_count,
        );
        self.signature_node_links.get(declaration).resolved_signature = Some(result);
        result
    }

    /// Go: `func (c *Checker) getTypeParametersFromDeclaration(declaration) []*Type`
    /// (checker.go:20397).
    pub(crate) fn get_type_parameters_from_declaration(&mut self, declaration: NodeId) -> Vec<TypeId> {
        // TODO(port M6 W3): getSignatureOfFullSignatureType's JSDoc
        // type-parameter list (JS files only); the declared-node walk is
        // Go's .ts behavior.
        let mut result: Vec<TypeId> = Vec::new();
        for node in crate::statements::type_parameters_of(self.store(), declaration) {
            if let Some(symbol) = crate::typeofsymbol::get_symbol_of_declaration(self, node) {
                let t = self.get_declared_type_of_type_parameter(symbol);
                if !result.contains(&t) {
                    result.push(t);
                }
            }
        }
        result
    }

    /// Go: `func (c *Checker) getAnnotatedAccessorThisParameter(accessor) *ast.Symbol`
    /// (checker.go:20409).
    fn get_annotated_accessor_this_parameter(&mut self, accessor: NodeId) -> Option<SymbolId> {
        let parameter = self.get_accessor_this_parameter(accessor)?;
        crate::typeofsymbol::get_symbol_of_declaration(self, parameter)
    }

    // ── Return types (Go 20462-20800) ──

    /// Go: `func (c *Checker) getReturnTypeOfSignature(sig) *Type` (checker.go:20462).
    pub(crate) fn get_return_type_of_signature_impl(&mut self, sig: SignatureId) -> TypeId {
        if let Some(resolved) = self.signatures[sig.0 as usize].resolved_return_type {
            return resolved;
        }
        if !self.push_type_resolution(
            crate::checker::TypeSystemEntity::Signature(sig),
            crate::checker::TypeSystemPropertyName::ResolvedReturnType,
        ) {
            return self.error_type();
        }
        let (sig_target, sig_mapper, sig_composite, sig_flags, sig_declaration) = {
            let s = &self.signatures[sig.0 as usize];
            (
                s.target,
                s.mapper,
                s.composite.clone(),
                s.flags,
                s.declaration,
            )
        };
        let t: Option<TypeId> = if let Some(target) = sig_target {
            let target_return = self.get_return_type_of_signature_impl(target);
            Some(crate::relater::instantiate_type_ex(self, target_return, sig_mapper))
        } else if let Some(composite) = sig_composite {
            let mut returns: Vec<TypeId> = Vec::with_capacity(composite.signatures.len());
            for &component in &composite.signatures {
                returns.push(self.get_return_type_of_signature_impl(component));
            }
            let union_or_intersection =
                self.get_union_or_intersection_type(&returns, composite.is_union);
            Some(crate::relater::instantiate_type_ex(self, union_or_intersection, sig_mapper))
        } else if let Some(declaration) = sig_declaration {
            let mut t = self.get_return_type_from_annotation_impl(declaration);
            if t.is_none() {
                // Go: NodeIsMissing(body) — the port's function_body_of
                // reports Option; a present body computes, a missing one is
                // anyType (Go's overload-signature answer).
                match crate::statements::function_body_of(self.store(), declaration) {
                    Some(_) => {
                        t = Some(self.get_return_type_from_body(
                            declaration,
                            crate::checker::CheckMode::CHECK_MODE_NORMAL,
                        ));
                    }
                    None => {
                        t = Some(self.any_type());
                    }
                }
            }
            t
        } else {
            // Go: sig.declaration is nil only for synthetic signatures, whose
            // resolvedReturnType is assigned at construction.
            Some(self.any_type())
        };
        let mut t = t.expect("return type computed above");
        if sig_flags.intersects(SignatureFlags::IS_INNER_CALL_CHAIN) {
            t = self.add_optional_type_marker(t);
        } else if sig_flags.intersects(SignatureFlags::IS_OUTER_CALL_CHAIN) {
            t = self.get_optional_type(t, false /*isProperty*/);
        }
        if !self.pop_type_resolution() {
            if let Some(declaration) = sig_declaration {
                let type_node = crate::ast_shims::type_of_node(self.store(), declaration);
                if let Some(type_node) = type_node {
                    self.error(
                        type_node,
                        &diag::RETURN_TYPE_ANNOTATION_CIRCULARLY_REFERENCES_ITSELF,
                        &[],
                    );
                } else if self.no_implicit_any {
                    let name = self.store().node(declaration).name();
                    if let Some(name) = name {
                        // Go: scanner.DeclarationNameToString(name) — the
                        // node text is the common case (the W4 printer owns
                        // the full form).
                        let name_string = tsc_ast::node_text(self.store(), name).to_string();
                        self.error(
                            name,
                            &diag::X_0_IMPLICITLY_HAS_RETURN_TYPE_ANY_BECAUSE_IT_DOES_NOT_HAVE_A_RETURN_TYPE_ANNOTATION_AND_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ONE_OF_ITS_RETURN_EXPRESSIONS,
                            &[name_string],
                        );
                    } else {
                        self.error(
                            declaration,
                            &diag::FUNCTION_IMPLICITLY_HAS_RETURN_TYPE_ANY_BECAUSE_IT_DOES_NOT_HAVE_A_RETURN_TYPE_ANNOTATION_AND_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ONE_OF_ITS_RETURN_EXPRESSIONS,
                            &[],
                        );
                    }
                }
            }
            t = self.any_type();
        }
        if self.signatures[sig.0 as usize].resolved_return_type.is_none() {
            self.signatures[sig.0 as usize].resolved_return_type = Some(t);
        }
        self.signatures[sig.0 as usize]
            .resolved_return_type
            .expect("just cached")
    }

    // Go: `func (c *Checker) getNonCircularReturnTypeOfSignature(sig) *Type`
    // (checker.go:20525) and its `isResolvingReturnTypeOfSignature` probe —
    // the relater.rs versions (composite-signature-aware) are the live forms
    // on this shared Checker type; the members.rs duplicates are deleted.

    /// Go: `func (c *Checker) getReturnTypeFromAnnotation(declaration) *Type`
    /// (checker.go:20503) — the FULL body (the S3 carrier in statements.rs
    /// delegates here).
    pub(crate) fn get_return_type_from_annotation_impl(&mut self, declaration: NodeId) -> Option<TypeId> {
        if self.store().node(declaration).kind == Kind::Constructor {
            let parent = self.store().node(declaration).parent.get();
            let parent_symbol = crate::typeofsymbol::get_symbol_of_declaration(self, parent)?;
            return Some(
                self.get_declared_type_of_class_or_interface(self.get_merged_symbol(parent_symbol)),
            );
        }
        let return_type = crate::ast_shims::type_of_node(self.store(), declaration);
        if let Some(return_type) = return_type {
            return Some(self.get_type_from_type_node(return_type));
        }
        if self.store().node(declaration).kind == Kind::GetAccessor
            && self.has_bindable_name(declaration)
        {
            let own_symbol = crate::typeofsymbol::get_symbol_of_declaration(self, declaration)?;
            let setter = self
                .program_symbol(own_symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| self.store().node(d).kind == Kind::SetAccessor);
            return self.get_annotated_accessor_type(setter);
        }
        // TODO(port M6 W3): getReturnTypeOfFullSignature (the JSDoc
        // full-signature return type — JS files only); permissive None is
        // Go's .ts answer.
        None
    }

    /// Go: `func (c *Checker) getAnnotatedAccessorType(accessor) *Type` (checker.go:20528)
    /// — the members-facing twin (typeofsymbol.rs's S2 stub is superseded).
    pub(crate) fn get_annotated_accessor_type(&mut self, accessor: Option<NodeId>) -> Option<TypeId> {
        let node = self.get_annotated_accessor_type_node(accessor)?;
        Some(self.get_type_from_type_node(node))
    }

    /// Go: `func (c *Checker) getReturnTypeFromBody(fn, checkMode) *Type` (checker.go:20530).
    ///
    /// The expression-body and aggregated-return arms are REAL over the S3/S4
    /// pieces; the S9 contextual-return refinement and the W3 flow type
    /// predicate ride their bannered seams (permissive None/false).
    pub(crate) fn get_return_type_from_body(
        &mut self,
        fn_node: NodeId,
        check_mode: crate::checker::CheckMode,
    ) -> TypeId {
        let Some(body) = crate::statements::function_body_of(self.store(), fn_node) else {
            return self.error_type();
        };
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), fn_node);
        let is_async = function_flags.intersects(tsc_ast::functionflags::FunctionFlags::ASYNC);
        let is_generator = function_flags.intersects(tsc_ast::functionflags::FunctionFlags::GENERATOR);
        let mut return_type: Option<TypeId> = None;
        let mut yield_type: Option<TypeId> = None;
        let mut next_type: Option<TypeId> = None;
        let mut fallback_return_type = self.void_type();
        if self.store().node(body).kind != Kind::Block {
            // Expression-bodied function.
            let check_mode = check_mode & !crate::checker::CheckMode::SKIP_GENERIC_FUNCTIONS;
            let mut t = self.check_expression_cached_ex(body, check_mode);
            if self.is_const_context(body) {
                t = self.get_regular_type_of_literal_type(t);
            }
            if is_async {
                t = {
                    let inner = self.check_awaited_type(
                        t,
                        false, /*withAlias*/
                        fn_node,
                        &diag::THE_RETURN_TYPE_OF_AN_ASYNC_FUNCTION_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
                    );
                    self.unwrap_awaited_type(inner)
                };
            }
            return_type = Some(t);
        } else if is_generator {
            let (return_types, is_never_returning) =
                self.check_and_aggregate_return_expression_types(fn_node, check_mode);
            if is_never_returning {
                fallback_return_type = self.never_type();
            } else if let Some(types) = &return_types
                && !types.is_empty()
            {
                let types = types.clone();
                return_type = Some(self.get_union_type_ex(types, UnionReduction::Subtype, None, None));
            }
            let (yield_types, next_types) = self.check_and_aggregate_yield_operand_types(fn_node, check_mode);
            if !yield_types.is_empty() {
                yield_type = Some(self.get_union_type_ex(yield_types, UnionReduction::Subtype, None, None));
            }
            if !next_types.is_empty() {
                next_type = Some(self.get_intersection_type(&next_types));
            }
        } else {
            let (types, is_never_returning) =
                self.check_and_aggregate_return_expression_types(fn_node, check_mode);
            if is_never_returning {
                // For an async function, the return type will not be never,
                // but rather a Promise for never (Go's comment).
                if is_async {
                    return self.create_promise_return_type(fn_node, self.never_type());
                }
                return self.never_type();
            }
            let Some(types) = types else {
                return self.never_type();
            };
            if types.is_empty() {
                // For an async function, the return type will not be
                // void/undefined, but rather a Promise for void/undefined.
                // TODO(port M6 S9): getContextualReturnType's
                // undefined-vs-void refinement; voidType is the no-context
                // answer (Go's `returnType = c.voidType` default).
                let t = self.void_type();
                if is_async {
                    return self.create_promise_return_type(fn_node, t);
                }
                return t;
            }
            return_type = Some(self.get_union_type_ex(types, UnionReduction::Subtype, None, None));
        }
        if return_type.is_some() || yield_type.is_some() || next_type.is_some() {
            if let Some(yield_type) = yield_type {
                self.report_errors_from_widening(
                    fn_node,
                    yield_type,
                    crate::typeofsymbol::WideningKind::GeneratorYield,
                );
            }
            if let Some(return_type) = return_type {
                self.report_errors_from_widening(
                    fn_node,
                    return_type,
                    crate::typeofsymbol::WideningKind::FunctionReturn,
                );
            }
            if let Some(next_type) = next_type {
                self.report_errors_from_widening(
                    fn_node,
                    next_type,
                    crate::typeofsymbol::WideningKind::GeneratorNext,
                );
            }
            // TODO(port M6 S9): the contextual-signature literal-widening
            // refinement (getWidenedLiteralLikeTypeForContextual*IfNeeded
            // over getContextualSignatureForFunctionLikeDeclaration) — the
            // S9 contextual wave owns the chain; the plain widening below is
            // Go's no-context answer.
            if let Some(yt) = yield_type {
                yield_type = Some(self.get_widened_type(yt));
            }
            if let Some(rt) = return_type {
                return_type = Some(self.get_widened_type(rt));
            }
            if let Some(nt) = next_type {
                next_type = Some(self.get_widened_type(nt));
            }
        }
        let return_type = return_type.unwrap_or(fallback_return_type);
        if is_generator {
            let yield_type = yield_type.unwrap_or_else(|| self.never_type());
            // TODO(port M6 S9): getContextualIterationType(Next) — S9; unknownType
            // is Go's no-context answer.
            let next_type = next_type.unwrap_or_else(|| self.unknown_type());
            return self.create_generator_type(yield_type, return_type, next_type, is_async);
        }
        // From within an async function you can return either a non-promise
        // value or a promise (Go's comment).
        if is_async {
            return self.create_promise_type(return_type);
        }
        return_type
    }

    /// Go: `func (c *Checker) createGeneratorType(yieldType, returnType, nextType, isAsyncGenerator) *Type`
    /// (checker.go:20760) — the resolver-pair composition (Go's body,
    /// verbatim structure).
    pub(crate) fn create_generator_type(
        &mut self,
        yield_type: TypeId,
        return_type: TypeId,
        next_type: TypeId,
        is_async_generator: bool,
    ) -> TypeId {
        let resolver = if is_async_generator {
            crate::statements::IterationTypesResolver {
                kind: crate::statements::IterationTypesResolverKind::Async,
            }
        } else {
            crate::statements::IterationTypesResolver {
                kind: crate::statements::IterationTypesResolverKind::Sync,
            }
        };
        let global_generator_type = self.get_global_generator_type_of_resolver(resolver);
        let yield_type = self
            .resolve_iteration_type(resolver, Some(yield_type), None)
            .unwrap_or_else(|| self.unknown_type());
        let return_type = self
            .resolve_iteration_type(resolver, Some(return_type), None)
            .unwrap_or_else(|| self.unknown_type());
        if global_generator_type == self.empty_generic_type() {
            // Fall back to the global IterableIterator type (Go's comment).
            let global_iterable_iterator_type =
                self.get_global_iterable_iterator_type_of_resolver(resolver);
            if global_iterable_iterator_type != self.empty_generic_type() {
                return self
                    .create_type_from_generic_global_type(Some(global_iterable_iterator_type), vec![yield_type, return_type, next_type]);
            }
            // The global Generator type doesn't exist, so report an error
            // (Go's comment) — the Checked probe reports.
            self.get_global_iterable_iterator_type_checked_of_resolver(resolver);
            return self.empty_object_type();
        }
        self.create_type_from_generic_global_type(
            Some(global_generator_type),
            vec![yield_type, return_type, next_type],
        )
    }

    /// Go: `func (c *Checker) checkAndAggregateYieldOperandTypes(fn, checkMode) (yieldTypes, nextTypes)`
    /// (checker.go:20680).
    fn check_and_aggregate_yield_operand_types(
        &mut self,
        fn_node: NodeId,
        check_mode: crate::checker::CheckMode,
    ) -> (Vec<TypeId>, Vec<TypeId>) {
        let is_async = tsc_ast::functionflags::get_function_flags(self.store(), fn_node)
            .intersects(tsc_ast::functionflags::FunctionFlags::ASYNC);
        let mut yield_types: Vec<TypeId> = Vec::new();
        let mut next_types: Vec<TypeId> = Vec::new();
        let body = crate::statements::function_body_of(self.store(), fn_node);
        if let Some(body) = body {
            u::for_each_yield_expression(self, body, &mut |c, yield_expr| {
                let (expression, has_asterisk) = {
                    let d = c
                        .store()
                        .node(yield_expr)
                        .as_yield_expression()
                        .expect("YieldExpression data");
                    (d.expression, d.asterisk_token.is_some())
                };
                let mut yield_expr_type = c.undefined_widening_type();
                if let Some(expression) = expression {
                    yield_expr_type = c.check_expression_ex(
                        expression,
                        check_mode & !crate::checker::CheckMode::SKIP_GENERIC_FUNCTIONS,
                    );
                    if c.is_const_context(expression) {
                        yield_expr_type = c.get_regular_type_of_literal_type(yield_expr_type);
                    }
                }
                let any_type = c.any_type();
                let yielded = c
                    .get_yielded_type_of_yield_expression(yield_expr, yield_expr_type, any_type, is_async)
                    .unwrap_or_else(|| c.any_type());
                if !yield_types.contains(&yielded) {
                    yield_types.push(yielded);
                }
                let next_type: Option<TypeId> = if has_asterisk {
                    let use_ = if is_async {
                        crate::statements::IterationUse::ASYNC_YIELD_STAR
                    } else {
                        crate::statements::IterationUse::YIELD_STAR
                    };
                    let iteration_types =
                        c.get_iteration_types_of_iterable(yield_expr_type, use_, expression);
                    iteration_types.next_type
                } else {
                    // TODO(port M6 S9): getContextualType(yieldExpr) — the
                    // contextual yield type; permissive None.
                    None
                };
                if let Some(next_type) = next_type
                    && !next_types.contains(&next_type)
                {
                    next_types.push(next_type);
                }
                false
            });
        }
        (yield_types, next_types)
    }

    /// Go: `func (c *Checker) createPromiseReturnType(fn, promisedType) *Type` (checker.go:20737).
    pub(crate) fn create_promise_return_type(&mut self, fn_node: NodeId, promised_type: TypeId) -> TypeId {
        let promise_type = self.create_promise_type(promised_type);
        let unknown_type = self.unknown_type();
        if promise_type == unknown_type {
            self.error(
                fn_node,
                &diag::AN_ASYNC_FUNCTION_OR_METHOD_MUST_RETURN_A_PROMISE_MAKE_SURE_YOU_HAVE_A_DECLARATION_FOR_PROMISE_OR_INCLUDE_ES2015_IN_YOUR_LIB_OPTION,
                &[],
            );
            return self.error_type();
        }
        if self.get_global_promise_constructor_symbol_or_nil().is_none() {
            self.error(
                fn_node,
                &diag::AN_ASYNC_FUNCTION_OR_METHOD_IN_ES5_REQUIRES_THE_PROMISE_CONSTRUCTOR_MAKE_SURE_YOU_HAVE_A_DECLARATION_FOR_THE_PROMISE_CONSTRUCTOR_OR_INCLUDE_ES2015_IN_YOUR_LIB_OPTION,
                &[],
            );
        }
        promise_type
    }

    /// Go: `func (c *Checker) reportErrorsFromWidening(declaration, t, wideningKind)` (checker.go:20798).
    pub(crate) fn report_errors_from_widening(
        &mut self,
        declaration: NodeId,
        t: TypeId,
        widening_kind: crate::typeofsymbol::WideningKind,
    ) {
        if self.no_implicit_any
            && self
                .type_(t)
                .object_flags
                .intersects(ObjectFlags::CONTAINS_WIDENING_TYPE)
            && (widening_kind == crate::typeofsymbol::WideningKind::Normal
                || (tsc_ast::utilities::is_function_like(self.store().node(declaration))
                    && self.should_report_errors_from_widening_with_contextual_signature(
                        declaration,
                        widening_kind,
                    )))
            {
                // Report implicit any error within type if possible, otherwise
                // report error on declaration (Go's comment).
                if !self.report_widening_errors_in_type(t) {
                    self.report_implicit_any(declaration, t, widening_kind);
                }
            }
    }

    /// Go: `func (c *Checker) shouldReportErrorsFromWideningWithContextualSignature(declaration, wideningKind) bool`
    /// (checker.go:20809).
    fn should_report_errors_from_widening_with_contextual_signature(
        &mut self,
        declaration: NodeId,
        widening_kind: crate::typeofsymbol::WideningKind,
    ) -> bool {
        // TODO(port M6 S9): the contextual-signature return-type refinement —
        // the S9 getContextualSignatureForFunctionLikeDeclaration chain; the
        // no-context answer (true) keeps Go's implicit-any reporting for
        // function-like declarations.
        let _ = (declaration, widening_kind);
        true
    }

    /// Go: `func (c *Checker) reportWideningErrorsInType(t) bool` (checker.go:20865).
    fn report_widening_errors_in_type(&mut self, t: TypeId) -> bool {
        let mut error_reported = false;
        if !self
            .type_(t)
            .object_flags
            .intersects(ObjectFlags::CONTAINS_WIDENING_TYPE)
        {
            return false;
        }
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::UNION) {
            let types = self.type_(t).types().to_vec();
            if types.iter().any(|&s| self.is_empty_object_type(s)) {
                error_reported = true;
            } else {
                for s in types {
                    error_reported = self.report_widening_errors_in_type(s) || error_reported;
                }
            }
        } else if crate::relater::is_array_or_tuple_type(self, t) {
            for s in crate::relater::get_type_arguments(self, t) {
                error_reported = self.report_widening_errors_in_type(s) || error_reported;
            }
        } else if crate::relater::is_object_literal_type(self, t) {
            for p in self.get_properties_of_object_type_impl(t) {
                let s = self.get_type_of_symbol(p);
                if self
                    .type_(s)
                    .object_flags
                    .intersects(ObjectFlags::CONTAINS_WIDENING_TYPE)
                {
                    error_reported = self.report_widening_errors_in_type(s);
                    if !error_reported {
                        // We need to account for property types coming from
                        // object literal type normalization in unions (Go's
                        // comment).
                        let declarations = self.program_symbol(p).declarations.clone();
                        let value_declaration = declarations.iter().copied().find(|&d| {
                            let parent = self.store().node(d).parent.get();
                            self.store().node(parent).parent.get()
                                != tsc_ast::NodeId::NONE
                        });
                        let _ = value_declaration;
                        // TODO(port M6 S4): the object-literal property
                        // declaration walk (the valueDeclaration.Parent ==
                        // t.symbol.ValueDeclaration containment check needs
                        // the object-literal normalization state); the
                        // fallback reportImplicitAny fires below via the
                        // caller instead.
                    }
                }
            }
        }
        error_reported
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) getTypePredicateFromBody(fn) *TypePredicate` (checker.go:20922).
    fn get_type_predicate_from_body(
        &mut self,
        _fn_node: NodeId,
    ) -> Option<Box<crate::types::TypePredicate>> {
        // TODO(port M6 W3): getTypePredicateFromBody — the type-predicate
        // inference rides the flow narrowing engine (checkIfExpressionRefines
        // *Parameter needs getFlowTypeOfReferenceEx on synthetic flow nodes).
        None
    }

    /// Go: `func (c *Checker) addOptionalTypeMarker(t) *Type` (checker.go:21012).
    pub(crate) fn add_optional_type_marker(&mut self, t: TypeId) -> TypeId {
        if self.strict_null_checks {
            let optional_type = self.optional_type();
            return self.get_union_type(&[t, optional_type]);
        }
        t
    }

    // ── Isolated-signature + instantiation (Go 19776-19900 region) ──

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) getSingleCallSignature(t) *Signature` (checker.go:19800).
    pub(crate) fn get_single_call_signature(&mut self, t: TypeId) -> Option<SignatureId> {
        self.get_single_signature(t, SignatureKind::Call, false /*allowMembers*/)
    }

    /// Go: `func (c *Checker) getSingleCallOrConstructSignature(t) *Signature` (checker.go:19805).
    pub(crate) fn get_single_call_or_construct_signature(&mut self, t: TypeId) -> Option<SignatureId> {
        if let Some(call_sig) = self.get_single_signature(t, SignatureKind::Call, false) {
            return Some(call_sig);
        }
        self.get_single_signature(t, SignatureKind::Construct, false)
    }

    /// Go: `func (c *Checker) getSingleSignature(t, kind, allowMembers) *Signature` (checker.go:19810).
    pub(crate) fn get_single_signature(
        &mut self,
        t: TypeId,
        kind: SignatureKind,
        allow_members: bool,
    ) -> Option<SignatureId> {
        if !self.type_(t).flags.intersects(TypeFlags::OBJECT) {
            return None;
        }
        let resolved = self.resolve_structured_type_members_impl(t);
        if allow_members || (resolved.properties.is_empty() && resolved.index_infos.is_empty()) {
            let calls = &resolved.signatures[..resolved.call_signature_count as usize];
            let constructs = &resolved.signatures[resolved.call_signature_count as usize..];
            if kind == SignatureKind::Call && calls.len() == 1 && constructs.is_empty() {
                return Some(calls[0]);
            }
            if kind == SignatureKind::Construct && constructs.len() == 1 && calls.is_empty() {
                return Some(constructs[0]);
            }
        }
        None
    }

    /// Go: `func (c *Checker) getOrCreateTypeFromSignature(sig) *Type` (checker.go:19832).
    pub(crate) fn get_or_create_type_from_signature(&mut self, sig: SignatureId) -> TypeId {
        if let Some(isolated) = self.signatures[sig.0 as usize].isolated_signature_type {
            return isolated;
        }
        let (declaration, this_parameter, parameters) = {
            let s = &self.signatures[sig.0 as usize];
            (s.declaration, s.this_parameter, s.parameters.clone())
        };
        let kind = declaration.map(|d| self.store().node(d).kind);
        // If declaration is undefined, it is likely to be the signature of
        // the default constructor (Go's comment).
        let is_constructor = kind.is_none()
            || matches!(
                kind,
                Some(Kind::Unknown) | Some(Kind::Constructor) | Some(Kind::ConstructSignature) | Some(Kind::ConstructorType)
            );
        let symbol = declaration.and_then(|d| crate::typeofsymbol::get_symbol_of_declaration(self, d));
        let t = self.new_object_type(
            ObjectFlags::ANONYMOUS.union(ObjectFlags::SINGLE_SIGNATURE_TYPE),
            symbol,
        );
        let _ = (this_parameter, parameters);
        if is_constructor {
            self.set_structured_type_members(t, SymbolTable::new(), Vec::new(), vec![sig], Vec::new());
        } else {
            self.set_structured_type_members(t, SymbolTable::new(), vec![sig], Vec::new(), Vec::new());
        }
        self.signatures[sig.0 as usize].isolated_signature_type = Some(t);
        t
    }

    /// Go: `func (c *Checker) cloneSignature(sig) *Signature` (checker.go:19789).
    pub(crate) fn clone_signature(&mut self, sig: SignatureId) -> SignatureId {
        let (flags, declaration, type_parameters, this_parameter, parameters, min_argument_count) = {
            let s = &self.signatures[sig.0 as usize];
            (
                s.flags & SignatureFlags::PROPAGATING_FLAGS,
                s.declaration,
                s.type_parameters.clone(),
                s.this_parameter,
                s.parameters.clone(),
                s.min_argument_count,
            )
        };
        let result = self.new_signature(
            flags,
            declaration,
            type_parameters,
            this_parameter,
            parameters,
            None,
            min_argument_count,
        );
        let (target, mapper, composite) = {
            let s = &self.signatures[sig.0 as usize];
            (s.target, s.mapper, s.composite.clone())
        };
        let result_signature = &mut self.signatures[result.0 as usize];
        result_signature.target = target;
        result_signature.mapper = mapper;
        result_signature.composite = composite;
        result
    }

    /// Go: `func (c *Checker) getSignatureInstantiation(sig, typeArguments, isJavaScript, inferredTypeParameters) *Signature`
    /// (checker.go:19745) — the S8 member-facing form; resolvecall.rs's
    /// S7-era method delegates here (the inferredTypeParameters arm is new).
    pub(crate) fn get_signature_instantiation_for_members(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
        is_javascript: bool,
        inferred_type_parameters: &[TypeId],
    ) -> SignatureId {
        let type_parameters = self.signatures[sig.0 as usize].type_parameters.clone();
        let min_count = self.get_min_type_argument_count(&type_parameters);
        let filled =
            self.fill_missing_type_arguments(type_arguments, &type_parameters, min_count, is_javascript);
        let instantiated_signature =
            self.get_signature_instantiation_without_filling_in_type_arguments_for_members(sig, &filled);
        if !inferred_type_parameters.is_empty() {
            let return_signature = self
                .get_return_type_of_signature_impl(instantiated_signature)
                .pipe(|t| self.get_single_call_or_construct_signature(t));
            if let Some(return_signature) = return_signature {
                let new_return_signature = self.clone_signature(return_signature);
                self.signatures[new_return_signature.0 as usize].type_parameters =
                    inferred_type_parameters.to_vec();
                let new_return_type = self.get_or_create_type_from_signature(new_return_signature);
                // Go: newReturnType.AsObjectType().mapper = instantiatedSignature.mapper
                let mapper = self.signatures[instantiated_signature.0 as usize].mapper;
                if let Some(d) = self.type_mut(new_return_type).as_object_type_mut() {
                    d.mapper = mapper;
                }
                let new_instantiated_signature = self.clone_signature(instantiated_signature);
                self.signatures[new_instantiated_signature.0 as usize].resolved_return_type =
                    Some(new_return_type);
                return new_instantiated_signature;
            }
        }
        instantiated_signature
    }

    /// Go: `func (c *Checker) getSignatureInstantiationWithoutFillingInTypeArguments(sig, typeArguments) *Signature`
    /// (checker.go:19772) — the cachedSignatures interning.
    pub(crate) fn get_signature_instantiation_without_filling_in_type_arguments_for_members(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
    ) -> SignatureId {
        let key = crate::typeofsymbol::get_type_list_key(type_arguments);
        if let Some(&cached) = self.cached_signatures.get(&(sig, key.0)) {
            return cached;
        }
        let result = self.create_signature_instantiation_for_members(sig, type_arguments);
        self.cached_signatures.insert((sig, key.0), result);
        result
    }

    /// Go: `func (c *Checker) createSignatureInstantiation(sig, typeArguments) *Signature`
    /// (checker.go:19782).
    pub(crate) fn create_signature_instantiation_for_members(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
    ) -> SignatureId {
        let mapper = self.create_signature_type_mapper_for_members(sig, type_arguments);
        self.instantiate_signature_ex(sig, Some(mapper), true /*eraseTypeParameters*/)
    }

    /// Go: `func (c *Checker) createSignatureTypeMapper(sig, typeArguments) *TypeMapper`
    /// (checker.go:19786).
    pub(crate) fn create_signature_type_mapper_for_members(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
    ) -> TypeMapperId {
        let type_parameters = self.get_type_parameters_for_mapper_for_members(sig);
        self.new_type_mapper(&type_parameters, type_arguments)
    }

    /// Go: `func (c *Checker) getTypeParametersForMapper(sig) []*Type` (checker.go:19788).
    pub(crate) fn get_type_parameters_for_mapper_for_members(&mut self, sig: SignatureId) -> Vec<TypeId> {
        let type_parameters = self.signatures[sig.0 as usize].type_parameters.clone();
        type_parameters
            .iter()
            .map(|&tp| {
                let mapper = self.type_(tp).as_type_parameter().and_then(|d| d.mapper);
                match mapper {
                    Some(mapper) => self.instantiate_type(tp, mapper),
                    None => tp,
                }
            })
            .collect()
    }

    /// Go: `func (c *Checker) getErasedSignature(signature) *Signature` (checker.go:19859).
    pub(crate) fn get_erased_signature(&mut self, signature: SignatureId) -> SignatureId {
        let type_parameters = self.signatures[signature.0 as usize].type_parameters.clone();
        if type_parameters.is_empty() {
            return signature;
        }
        let key = crate::relater::SIGNATURE_KEY_ERASED;
        if let Some(&erased) = self.cached_signatures.get(&(signature, key)) {
            return erased;
        }
        let any_type = self.any_type();
        let mapper = self.new_array_to_single_type_mapper(&type_parameters, any_type);
        let erased = self.instantiate_signature_ex(signature, Some(mapper), true /*eraseTypeParameters*/);
        self.cached_signatures.insert((signature, key), erased);
        erased
    }

    /// Go: `func (c *Checker) getCanonicalSignature(signature) *Signature` (checker.go:19875).
    pub(crate) fn get_canonical_signature(&mut self, signature: SignatureId) -> SignatureId {
        let type_parameters = self.signatures[signature.0 as usize].type_parameters.clone();
        if type_parameters.is_empty() {
            return signature;
        }
        let key = crate::relater::SIGNATURE_KEY_CANONICAL;
        if let Some(&canonical) = self.cached_signatures.get(&(signature, key)) {
            return canonical;
        }
        let canonical = self.create_canonical_signature(signature);
        self.cached_signatures.insert((signature, key), canonical);
        canonical
    }

    /// Go: `func (c *Checker) createCanonicalSignature(signature) *Signature` (checker.go:19886).
    fn create_canonical_signature(&mut self, signature: SignatureId) -> SignatureId {
        // Create an instantiation of the signature where each unconstrained
        // type parameter is replaced with its original (Go's comment).
        let type_parameters = self.signatures[signature.0 as usize].type_parameters.clone();
        let mut type_arguments: Vec<TypeId> = Vec::with_capacity(type_parameters.len());
        for &tp in &type_parameters {
            let target = self.type_(tp).target();
            let unconstrained = target.is_some_and(|target| {
                crate::relater::get_constraint_of_type_parameter(self, target).is_none()
            });
            if unconstrained {
                type_arguments.push(target.expect("checked above"));
            } else {
                type_arguments.push(tp);
            }
        }
        let is_js = self.signatures[signature.0 as usize]
            .declaration
            .is_some_and(|d| tsc_ast::utilities::is_in_js_file(self.store().node(d)));
        self.get_signature_instantiation_for_members(signature, &type_arguments, is_js, &[])
    }

    /// Go: `func (c *Checker) getBaseSignature(signature) *Signature` (checker.go:19900).
    pub(crate) fn get_base_signature(&mut self, signature: SignatureId) -> SignatureId {
        let type_parameters = self.signatures[signature.0 as usize].type_parameters.clone();
        if type_parameters.is_empty() {
            return signature;
        }
        let key = crate::relater::SIGNATURE_KEY_BASE;
        if let Some(&cached) = self.cached_signatures.get(&(signature, key)) {
            return cached;
        }
        let unknown_type = self.unknown_type();
        let base_constraint_targets: Vec<TypeId> = type_parameters
            .iter()
            .map(|&tp| {
                crate::relater::get_constraint_of_type_parameter(self, tp).unwrap_or(unknown_type)
            })
            .collect();
        let base_constraint_mapper = self.new_type_mapper(&type_parameters, &base_constraint_targets);
        let mut base_constraints: Vec<TypeId> = type_parameters
            .iter()
            .map(|&tp| self.instantiate_type(tp, base_constraint_mapper))
            .collect();
        // Run the immediate constraint mapper N-1 times so non-circular
        // interdependent type parameters resolve to their external
        // dependencies (Go's comment).
        for _ in 0..type_parameters.len().saturating_sub(1) {
            base_constraints = self.instantiate_types(&base_constraints, Some(base_constraint_mapper));
        }
        // And then apply a type eraser to remove any remaining circularly
        // dependent type parameters (Go's comment).
        let any_type = self.any_type();
        let eraser = self.new_array_to_single_type_mapper(&type_parameters, any_type);
        base_constraints = self.instantiate_types(&base_constraints, Some(eraser));
        let mapper = self.new_type_mapper(&type_parameters, &base_constraints);
        let result = self.instantiate_signature_ex(signature, Some(mapper), true /*eraseTypeParameters*/);
        self.cached_signatures.insert((signature, key), result);
        result
    }

    /// Go: `func (c *Checker) getDefaultConstructSignatures(classType) []*Signature`
    /// (checker.go:21271).
    pub(crate) fn get_default_construct_signatures(&mut self, class_type: TypeId) -> Vec<SignatureId> {
        let base_constructor_type = self.get_base_constructor_type_of_class_impl(class_type);
        let base_signatures = self.get_signatures_of_type_impl(base_constructor_type, SignatureKind::Construct);
        let symbol = self.type_(class_type).symbol;
        let declaration = symbol
            .and_then(|s| crate::expressions::get_class_like_declaration_of_symbol_for(self, s));
        let is_abstract = declaration.is_some_and(|d| {
            crate::ast_shims::has_modifier(self.store().node(d), ModifierFlags::ABSTRACT)
        });
        if base_signatures.is_empty() {
            let flags = if is_abstract {
                SignatureFlags::CONSTRUCT.union(SignatureFlags::ABSTRACT)
            } else {
                SignatureFlags::CONSTRUCT
            };
            let local_type_parameters = self
                .type_(class_type)
                .as_interface_type()
                .expect("InterfaceType data")
                .local_type_parameters()
                .to_vec();
            return vec![self.new_signature(
                flags,
                None,
                local_type_parameters,
                None,
                Vec::new(),
                Some(class_type),
                0,
            )];
        }
        let base_type_node = self.get_base_type_node_of_class(class_type);
        let is_javascript = declaration
            .is_some_and(|d| tsc_ast::utilities::is_in_js_file(self.store().node(d)));
        let type_arguments = base_type_node
            .map(|node| self.get_type_arguments_from_node(node))
            .unwrap_or_default();
        let type_arg_count = type_arguments.len();
        let mut result: Vec<SignatureId> = Vec::new();
        for &base_sig in &base_signatures {
            let type_parameters = self.signatures[base_sig.0 as usize].type_parameters.clone();
            let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
            let type_param_count = type_parameters.len();
            if is_javascript || (type_arg_count >= min_type_argument_count && type_arg_count <= type_param_count) {
                let sig = if type_param_count != 0 {
                    let filled = self.fill_missing_type_arguments(
                        &type_arguments,
                        &type_parameters,
                        min_type_argument_count,
                        is_javascript,
                    );
                    self.create_signature_instantiation_for_members(base_sig, &filled)
                } else {
                    self.clone_signature(base_sig)
                };
                let local_type_parameters = self
                    .type_(class_type)
                    .as_interface_type()
                    .expect("InterfaceType data")
                    .local_type_parameters()
                    .to_vec();
                {
                    let s = &mut self.signatures[sig.0 as usize];
                    s.type_parameters = local_type_parameters;
                    s.resolved_return_type = Some(class_type);
                    if is_abstract {
                        s.flags |= SignatureFlags::ABSTRACT;
                    } else {
                        s.flags &= !SignatureFlags::ABSTRACT;
                    }
                }
                result.push(sig);
            }
        }
        result
    }

    /// Go: `func hasRestParameter(fn) bool` (utilities.go) — the declaration
    /// probe getSignatureFromDeclaration needs.
    fn has_rest_parameter(&self, node: NodeId) -> bool {
        let parameters = crate::ast_shims::parameters_of(self.store(), node);
        parameters.last().is_some_and(|&param| u::has_dot_dot_dot_token(self.store(), param))
    }
}

/// Go: the `pipe`-free local that keeps get_signature_instantiation's
/// return-signature probe readable (no Go home — inline in Go).
trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

// ────────────────────────────────────────────────────────────────────────────
// The structured-type providers (Go 19253-19340 + 19430)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getPropertiesOfType(t) []*ast.Symbol` (checker.go:19253).
    pub(crate) fn get_properties_of_type_impl(&mut self, t: TypeId) -> Vec<SymbolId> {
        let t = self.get_reduced_apparent_type(t);
        if self.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            return self.get_properties_of_union_or_intersection_type(t);
        }
        self.get_properties_of_object_type_impl(t)
    }

    /// Go: `func (c *Checker) getPropertiesOfObjectType(t) []*ast.Symbol` (checker.go:19263).
    pub(crate) fn get_properties_of_object_type_impl(&mut self, t: TypeId) -> Vec<SymbolId> {
        if self.type_(t).flags.intersects(TypeFlags::OBJECT) {
            return self
                .resolve_structured_type_members_impl(t)
                .properties;
        }
        Vec::new()
    }

    /// Go: `func (c *Checker) getPropertiesOfUnionOrIntersectionType(t) []*ast.Symbol`
    /// (checker.go:19268).
    pub(crate) fn get_properties_of_union_or_intersection_type(&mut self, t: TypeId) -> Vec<SymbolId> {
        let already = self
            .type_(t)
            .as_union_or_intersection_type()
            .expect("UnionOrIntersectionType data")
            .resolved_properties
            .is_empty();
        let is_intersection = self.type_(t).flags.intersects(TypeFlags::INTERSECTION);
        if already {
            let mut checked: rustc_hash::FxHashSet<String> = rustc_hash::FxHashSet::default();
            let mut props: Vec<SymbolId> = Vec::new();
            let constituents = self.type_(t).types().to_vec();
            for current in constituents {
                for prop in self.get_properties_of_type_impl(current) {
                    let name = self.program_symbol(prop).name.clone();
                    if !checked.contains(&name) {
                        checked.insert(name.clone());
                        if let Some(combined_prop) = self.get_property_of_union_or_intersection_type_impl(
                            t,
                            &name,
                            !is_intersection /*skipObjectFunctionPropertyAugment*/,
                        ) {
                            props.push(combined_prop);
                        }
                    }
                }
                // The properties of a union type are those that are present in
                // all constituent types, so we only need to check the
                // properties of the first type without index signature (Go).
                if !is_intersection
                    && self.get_index_infos_of_type_impl(current).is_empty()
                {
                    break;
                }
            }
            if let Some(d) = self.type_mut(t).as_union_or_intersection_type_mut() {
                d.resolved_properties = props;
            }
        }
        self.type_(t)
            .as_union_or_intersection_type()
            .expect("UnionOrIntersectionType data")
            .resolved_properties
            .clone()
    }

    /// Go: `func (c *Checker) getPropertyOfType(t, name) *ast.Symbol` (checker.go:19295).
    pub(crate) fn get_property_of_type_impl(&mut self, t: TypeId, name: &str) -> Option<SymbolId> {
        self.get_property_of_type_ex_impl(t, name, false, false)
    }

    /// Go: `func (c *Checker) getPropertyOfTypeEx(t, name, skipObjectFunctionPropertyAugment, includeTypeOnlyMembers) *ast.Symbol`
    /// (checker.go:19304).
    pub(crate) fn get_property_of_type_ex_impl(
        &mut self,
        t: TypeId,
        name: &str,
        skip_object_function_property_augment: bool,
        include_type_only_members: bool,
    ) -> Option<SymbolId> {
        let t = self.get_reduced_apparent_type(t);
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::OBJECT) {
            let resolved = self.resolve_structured_type_members_impl(t);
            if let Some(&symbol) = resolved.members.get(name) {
                let t_symbol = self.type_(t).symbol;
                if !include_type_only_members
                    && t_symbol.is_some_and(|s| {
                        self.program_symbol(s).flags.intersects(SymbolFlags::VALUE_MODULE)
                    })
                    && self
                        .module_symbol_links
                        .get(t_symbol.expect("checked"))
                        .type_only_export_star_map
                        .as_ref()
                        .is_some_and(|map| map.get(name).is_some())
                {
                    // If this is the type of a module, `resolved.members.get(name)`
                    // might have effectively skipped over an `export type * from
                    // './foo'` (Go's comment).
                    return None;
                }
                if self.symbol_is_value_ex(symbol, include_type_only_members) {
                    return Some(symbol);
                }
            }
            if skip_object_function_property_augment {
                return None;
            }
            let any_function_type = self.any_function_type;
            let function_type: Option<TypeId> = if Some(t) == any_function_type {
                self.global_function_type
            } else if resolved.call_signature_count > 0 {
                self.global_callable_function_type
            } else if resolved.signatures.len() as i32 > resolved.call_signature_count {
                self.global_newable_function_type
            } else {
                None
            };
            if let Some(function_type) = function_type {
                if let Some(symbol) = self.get_property_of_object_type_impl(function_type, name) {
                    return Some(symbol);
                }
            }
            let global_object_type = self.global_object_type?;
            return self.get_property_of_object_type_impl(global_object_type, name);
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            let prop =
                self.get_property_of_union_or_intersection_type_impl(t, name, true /*skipObjectFunctionPropertyAugment*/);
            if prop.is_some() {
                return prop;
            }
            if !skip_object_function_property_augment {
                return self.get_property_of_union_or_intersection_type_impl(
                    t,
                    name,
                    skip_object_function_property_augment,
                );
            }
            return None;
        } else if flags.intersects(TypeFlags::UNION) {
            return self.get_property_of_union_or_intersection_type_impl(
                t,
                name,
                skip_object_function_property_augment,
            );
        }
        None
    }

    /// Go: `func (c *Checker) getTypeOfPropertyOfType(t, name) *Type` (checker.go:19359).
    pub(crate) fn get_type_of_property_of_type_impl(&mut self, t: TypeId, name: &str) -> Option<TypeId> {
        let prop = self.get_property_of_type_impl(t, name)?;
        Some(self.get_type_of_symbol(prop))
    }

    /// Go: `func (c *Checker) getSignaturesOfType(t, kind) []*Signature` (checker.go:19366).
    pub(crate) fn get_signatures_of_type_impl(&mut self, t: TypeId, kind: SignatureKind) -> Vec<SignatureId> {
        let t = self.get_reduced_apparent_type(t);
        self.get_signatures_of_structured_type_impl(t, kind)
    }

    /// Go: `func (c *Checker) getSignaturesOfStructuredType(t, kind) []*Signature` (checker.go:19370).
    pub(crate) fn get_signatures_of_structured_type_impl(&mut self, t: TypeId, kind: SignatureKind) -> Vec<SignatureId> {
        if !self.type_(t).flags.intersects(TypeFlags::STRUCTURED_TYPE) {
            return Vec::new();
        }
        let resolved = self.resolve_structured_type_members_impl(t);
        if kind == SignatureKind::Call {
            resolved.signatures[..resolved.call_signature_count as usize].to_vec()
        } else {
            resolved.signatures[resolved.call_signature_count as usize..].to_vec()
        }
    }

    /// Go: `func (c *Checker) getIndexInfosOfType(t) []*IndexInfo` (checker.go:19377).
    pub(crate) fn get_index_infos_of_type_impl(&mut self, t: TypeId) -> Vec<IndexInfoId> {
        let t = self.get_reduced_apparent_type(t);
        self.get_index_infos_of_structured_type_impl(t)
    }

    /// Go: `func (c *Checker) getIndexInfosOfStructuredType(t) []*IndexInfo` (checker.go:19381).
    pub(crate) fn get_index_infos_of_structured_type_impl(&mut self, t: TypeId) -> Vec<IndexInfoId> {
        if self.type_(t).flags.intersects(TypeFlags::STRUCTURED_TYPE) {
            return self.resolve_structured_type_members_impl(t).index_infos;
        }
        Vec::new()
    }

    /// Go: `func (c *Checker) getIndexInfoOfType(t, keyType) *IndexInfo` (checker.go:19389).
    pub(crate) fn get_index_info_of_type_impl(&mut self, t: TypeId, key_type: TypeId) -> Option<IndexInfoId> {
        let infos = self.get_index_infos_of_type_impl(t);
        find_index_info(self, &infos, key_type)
    }

    /// Go: `func (c *Checker) getIndexTypeOfType(t, keyType) *Type` (checker.go:19397).
    pub(crate) fn get_index_type_of_type_impl(&mut self, t: TypeId, key_type: TypeId) -> Option<TypeId> {
        let info = self.get_index_info_of_type_impl(t, key_type)?;
        Some(self.index_info_value_type(info))
    }

    /// Go: `func (c *Checker) getApplicableIndexInfo(t, keyType) *IndexInfo` (checker.go:19408).
    pub(crate) fn get_applicable_index_info_impl(&mut self, t: TypeId, key_type: TypeId) -> Option<IndexInfoId> {
        let infos = self.get_index_infos_of_type_impl(t);
        self.find_applicable_index_info(&infos, key_type)
    }

    /// Go: `func (c *Checker) getApplicableIndexInfoForName(t, name) *IndexInfo` (checker.go:19412).
    pub(crate) fn get_applicable_index_info_for_name_impl(&mut self, t: TypeId, name: &str) -> Option<IndexInfoId> {
        if u::is_late_bound_name(name) {
            let es_symbol_type = self.es_symbol_type();
            return self.get_applicable_index_info_impl(t, es_symbol_type);
        }
        let key = self.get_string_literal_type(name);
        self.get_applicable_index_info_impl(t, key)
    }

    /// Go: `func (c *Checker) findApplicableIndexInfo(indexInfos, keyType) *IndexInfo`
    /// (checker.go:19417) — index signatures for type 'string' are considered
    /// only when no other index signatures apply.
    pub(crate) fn find_applicable_index_info(
        &mut self,
        index_infos: &[IndexInfoId],
        key_type: TypeId,
    ) -> Option<IndexInfoId> {
        let mut string_index_info: Option<IndexInfoId> = None;
        let mut applicable_infos: Vec<IndexInfoId> = Vec::with_capacity(8);
        let string_type = self.string_type();
        for &info in index_infos {
            let info_key_type = self.index_info_key_type(info);
            if info_key_type == string_type {
                string_index_info = Some(info);
            } else if self.is_applicable_index_type(key_type, info_key_type) {
                applicable_infos.push(info);
            }
        }
        // When more than one index signature is applicable we create a
        // synthetic IndexInfo (Go's comment).
        match applicable_infos.len() {
            0 => {
                if string_index_info.is_some() && self.is_applicable_index_type(key_type, string_type) {
                    return string_index_info;
                }
                None
            }
            1 => Some(applicable_infos[0]),
            _ => {
                let mut is_readonly = true;
                let mut types: Vec<TypeId> = Vec::with_capacity(applicable_infos.len());
                for info in applicable_infos {
                    types.push(self.index_info_value_type(info));
                    if !self.index_info_is_readonly(info) {
                        is_readonly = false;
                    }
                }
                let unknown_type = self.unknown_type();
                let value_type = self.get_intersection_type(&types);
                Some(crate::types::IndexInfoId(
                    self.new_index_info(unknown_type, value_type, is_readonly, None, Vec::new()) as u32,
                ))
            }
        }
    }

    /// Go: `func (c *Checker) getApplicableIndexInfos(t, keyType) []*IndexInfo`
    /// (checker.go:32635) — REAL (M6 S10): the filter form over
    /// getIndexInfosOfType + isApplicableIndexType.
    pub(crate) fn get_applicable_index_infos_impl(
        &mut self,
        t: TypeId,
        key_type: TypeId,
    ) -> Vec<IndexInfoId> {
        let infos = self.get_index_infos_of_type_impl(t);
        let mut result = Vec::new();
        for info in infos {
            let info_key_type = self.index_info_key_type(info);
            if self.is_applicable_index_type(key_type, info_key_type) {
                result.push(info);
            }
        }
        result
    }
    /// Go: `func (c *Checker) isApplicableIndexType(source, target) bool` (checker.go:19448).
    pub(crate) fn is_applicable_index_type(&mut self, source: TypeId, target: TypeId) -> bool {
        // A 'string' index signature applies to types assignable to 'string'
        // or 'number', and a 'number' index signature applies to types
        // assignable to 'number', `${number}` and numeric string literal
        // types (Go's comment).
        let numeric_string_type = self.numeric_string_type();
        let number_type = self.number_type();
        let string_type = self.string_type();
        if source == target {
            return true;
        }
        self.is_type_assignable_to(source, target)
            || (target == string_type && self.is_type_assignable_to(source, number_type))
            || (target == number_type
                && (source == numeric_string_type
                    || (self.type_(source).flags.intersects(TypeFlags::STRING_LITERAL)
                        && u::is_numeric_literal_name(&crate::relater::get_string_literal_value(self, source)))))
    }

    // ────────────────────────────────────────────────────────────────────────
    // Union/intersection property machinery (Go 21666-22154)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getPropertyOfObjectType(t, name) *ast.Symbol` (checker.go:21673).
    pub(crate) fn get_property_of_object_type_impl(&mut self, t: TypeId, name: &str) -> Option<SymbolId> {
        if self.type_(t).flags.intersects(TypeFlags::OBJECT) {
            let resolved = self.resolve_structured_type_members_impl(t);
            if let Some(&symbol) = resolved.members.get(name)
                && self.symbol_is_value(symbol)
            {
                return Some(symbol);
            }
        }
        None
    }

    /// Go: `func (c *Checker) getPropertyOfUnionOrIntersectionType(t, name, skipObjectFunctionPropertyAugment) *ast.Symbol`
    /// (checker.go:21716).
    pub(crate) fn get_property_of_union_or_intersection_type_impl(
        &mut self,
        t: TypeId,
        name: &str,
        skip_object_function_property_augment: bool,
    ) -> Option<SymbolId> {
        let prop = self.get_union_or_intersection_property(t, name, skip_object_function_property_augment)?;
        // We need to filter out partial properties in union types (Go).
        if self
            .program_symbol(prop)
            .check_flags
            .intersects(CheckFlags::READ_PARTIAL)
        {
            return None;
        }
        Some(prop)
    }

    /// Go: `func (c *Checker) getUnionOrIntersectionProperty(t, name, skipObjectFunctionPropertyAugment) *ast.Symbol`
    /// (checker.go:21726) — the per-type property caches.
    pub(crate) fn get_union_or_intersection_property(
        &mut self,
        t: TypeId,
        name: &str,
        skip_object_function_property_augment: bool,
    ) -> Option<SymbolId> {
        let cached = if skip_object_function_property_augment {
            
            self
                .type_(t)
                .as_union_or_intersection_type()
                .expect("UnionOrIntersectionType data")
                .property_cache_without_function_property_augment
                .get(name)
                .copied()
        } else {
            self.type_(t)
                .as_union_or_intersection_type()
                .expect("UnionOrIntersectionType data")
                .property_cache
                .get(name)
                .copied()
        };
        if let Some(prop) = cached {
            return Some(prop);
        }
        let prop = self.create_union_or_intersection_property(t, name, skip_object_function_property_augment);
        if let Some(prop) = prop {
            if skip_object_function_property_augment {
                let check_flags = self.program_symbol(prop).check_flags;
                self.type_mut(t)
                    .as_union_or_intersection_type_mut()
                    .expect("UnionOrIntersectionType data")
                    .property_cache_without_function_property_augment
                    .set(name.to_string(), prop);
                // Propagate an entry from the non-augmented cache to the
                // augmented cache unless the property is partial (Go).
                if !check_flags.intersects(CheckFlags::PARTIAL) {
                    let augmented = self
                        .type_(t)
                        .as_union_or_intersection_type()
                        .expect("UnionOrIntersectionType data")
                        .property_cache
                        .get(name)
                        .copied();
                    if augmented.is_none() {
                        self.type_mut(t)
                            .as_union_or_intersection_type_mut()
                            .expect("UnionOrIntersectionType data")
                            .property_cache
                            .set(name.to_string(), prop);
                    }
                }
            } else {
                self.type_mut(t)
                    .as_union_or_intersection_type_mut()
                    .expect("UnionOrIntersectionType data")
                    .property_cache
                    .set(name.to_string(), prop);
            }
        }
        prop
    }

    /// Go: `func (c *Checker) createUnionOrIntersectionProperty(containingType, name, skipObjectFunctionPropertyAugment) *ast.Symbol`
    /// (checker.go:21751) — the synthetic property combiner.
    fn create_union_or_intersection_property(
        &mut self,
        containing_type: TypeId,
        name: &str,
        skip_object_function_property_augment: bool,
    ) -> Option<SymbolId> {
        let mut prop_flags = SymbolFlags::NONE;
        let mut single_prop: Option<SymbolId> = None;
        let mut prop_set: tsc_collections::OrderedSet<SymbolId> = tsc_collections::OrderedSet::new();
        let mut index_types: Vec<TypeId> = Vec::new();
        let is_union = self.type_(containing_type).flags.intersects(TypeFlags::UNION);
        // Flags we want to propagate to the result if they exist in all source
        // symbols (Go).
        let mut check_flags = if !is_union {
            CheckFlags::READONLY
        } else {
            CheckFlags::NONE
        };
        let mut optional_flag = if !is_union {
            SymbolFlags::OPTIONAL
        } else {
            SymbolFlags::NONE
        };
        let mut synthetic_flag = CheckFlags::SYNTHETIC_METHOD;
        let mut merged_instantiations = false;
        let constituents = self.type_(containing_type).types().to_vec();
        for current in constituents {
            let t = self.get_apparent_type_impl(current);
            if crate::relater::is_error_type(self, t)
                || self.type_(t).flags.intersects(TypeFlags::NEVER)
            {
                continue;
            }
            let prop = self.get_property_of_type_ex_impl(t, name, skip_object_function_property_augment, false);
            if let Some(prop) = prop {
                let modifiers = u::get_declaration_modifier_flags_from_symbol(self, self.store(), prop);
                let write_modifiers =
                    u::get_declaration_modifier_flags_from_symbol_ex(self, self.store(), prop, true /*isWrite*/);
                let prop_symbol_flags = self.program_symbol(prop).flags;
                if prop_symbol_flags.intersects(SymbolFlags::CLASS_MEMBER) {
                    if is_union {
                        optional_flag |= prop_symbol_flags & SymbolFlags::OPTIONAL;
                    } else {
                        optional_flag &= prop_symbol_flags;
                    }
                }
                if let Some(single) = single_prop {
                    if Some(prop) != single_prop {
                        let is_instantiation =
                            self.get_target_symbol_impl(prop) == self.get_target_symbol_impl(single);
                        // If the symbols are instances of one another with
                        // identical types - consider the symbols equivalent (Go).
                        if is_instantiation
                            && crate::relater::compare_properties(
                                self,
                                single,
                                prop,
                                crate::relater::TypeComparerKind::Identical,
                                None,
                            ) == crate::types::Ternary::True
                        {
                            merged_instantiations = self
                                .program_symbol(single)
                                .parent
                                .is_some_and(|parent| {
                                    !self
                                        .get_local_type_parameters_of_class_or_interface_or_type_alias(
                                            parent,
                                        )
                                        .is_empty()
                                });
                        } else {
                            if prop_set.size() == 0 {
                                prop_set.add(single);
                            }
                            prop_set.add(prop);
                        }
                        // The accessor flags are only used when they are the same
                        // in all constituents (Go's comment).
                        if prop_flags.intersects(SymbolFlags::ACCESSOR)
                            && (prop_symbol_flags & SymbolFlags::ACCESSOR)
                                != (prop_flags & SymbolFlags::ACCESSOR)
                        {
                            prop_flags =
                                (prop_flags & !SymbolFlags::ACCESSOR) | SymbolFlags::PROPERTY;
                        }
                    }
                } else {
                    single_prop = Some(prop);
                    prop_flags = if prop_symbol_flags.intersects(SymbolFlags::ACCESSOR) {
                        prop_symbol_flags & SymbolFlags::ACCESSOR
                    } else {
                        SymbolFlags::PROPERTY
                    };
                }
                if is_union && crate::relater::is_readonly_symbol(self, prop) {
                    check_flags |= CheckFlags::READONLY;
                } else if !is_union && !crate::relater::is_readonly_symbol(self, prop) {
                    check_flags &= !CheckFlags::READONLY;
                }
                if modifiers.intersects(ModifierFlags::PROTECTED)
                    && !modifiers.intersects(ModifierFlags::PUBLIC)
                {
                    check_flags |= CheckFlags::CONTAINS_PROTECTED;
                } else if modifiers.intersects(ModifierFlags::PRIVATE)
                    && !modifiers.intersects(ModifierFlags::PUBLIC)
                {
                    check_flags |= CheckFlags::CONTAINS_PRIVATE;
                } else {
                    check_flags |= CheckFlags::CONTAINS_PUBLIC;
                }
                if write_modifiers.intersects(ModifierFlags::PROTECTED)
                    && !write_modifiers.intersects(ModifierFlags::PUBLIC)
                {
                    check_flags |= CheckFlags::CONTAINS_WRITE_PROTECTED;
                } else if write_modifiers.intersects(ModifierFlags::PRIVATE)
                    && !write_modifiers.intersects(ModifierFlags::PUBLIC)
                {
                    check_flags |= CheckFlags::CONTAINS_WRITE_PRIVATE;
                } else {
                    check_flags |= CheckFlags::CONTAINS_WRITE_PUBLIC;
                }
                if modifiers.intersects(ModifierFlags::STATIC) {
                    check_flags |= CheckFlags::CONTAINS_STATIC;
                }
                if !is_prototype_property(self, prop) {
                    synthetic_flag = CheckFlags::SYNTHETIC_PROPERTY;
                }
            } else if is_union {
                let mut index_info: Option<IndexInfoId> = None;
                if !u::is_late_bound_name(name) {
                    index_info = self.get_applicable_index_info_for_name_impl(t, name);
                }
                if let Some(index_info) = index_info {
                    prop_flags = (prop_flags & !SymbolFlags::ACCESSOR) | SymbolFlags::PROPERTY;
                    check_flags |= CheckFlags::WRITE_PARTIAL;
                    if self.index_info_is_readonly(index_info) {
                        check_flags |= CheckFlags::READONLY;
                    }
                    if crate::relater::is_tuple_type(self, t) {
                        // REAL (M6 S10, checker.go:21830): getRestTypeOfTupleType —
                        // the out-of-bounds tuple element type
                        // (getElementTypeOfSliceOfTupleType at fixedLength).
                        let target = self.type_(t).target_tuple_type();
                        let fixed_length = self
                            .type_(target)
                            .as_tuple_type()
                            .map(|d| d.fixed_length as usize)
                            .unwrap_or(0);
                        let rest_type = crate::typenodes::get_element_type_of_slice_of_tuple_type(
                            self, t, fixed_length, 0, false, false,
                        );
                        index_types.push(rest_type.unwrap_or_else(|| self.undefined_type()));
                    } else {
                        index_types.push(self.index_info_value_type(index_info));
                    }
                } else if crate::relater::is_object_literal_type(self, t)
                    && !self
                        .type_(t)
                        .object_flags
                        .intersects(ObjectFlags::CONTAINS_SPREAD)
                {
                    check_flags |= CheckFlags::WRITE_PARTIAL;
                    index_types.push(self.undefined_type());
                } else {
                    check_flags |= CheckFlags::READ_PARTIAL;
                }
            }
        }
        let Some(single) = single_prop else {
            // No property was found (Go).
            return None;
        };
        if is_union
            && (prop_set.size() != 0 || check_flags.intersects(CheckFlags::PARTIAL))
            && check_flags.intersects(
                CheckFlags::CONTAINS_PRIVATE
                    .union(CheckFlags::CONTAINS_PROTECTED)
                    .union(CheckFlags::CONTAINS_WRITE_PRIVATE)
                    .union(CheckFlags::CONTAINS_WRITE_PROTECTED),
            )
            && !(prop_set.size() != 0 && self.has_common_declaration(&prop_set))
        {
            // A property in a union has a private or protected declaration in
            // one constituent, but is missing or has a different declaration
            // in another constituent (Go's comment).
            if check_flags
                .intersects(CheckFlags::CONTAINS_PRIVATE.union(CheckFlags::CONTAINS_PROTECTED))
            {
                return None;
            }
            if check_flags.intersects(CheckFlags::CONTAINS_WRITE_PRIVATE) {
                check_flags &= !(CheckFlags::CONTAINS_WRITE_PUBLIC | CheckFlags::CONTAINS_WRITE_PROTECTED);
            } else if check_flags.intersects(CheckFlags::CONTAINS_WRITE_PROTECTED) {
                check_flags &= !CheckFlags::CONTAINS_WRITE_PUBLIC;
            }
        }
        if prop_set.size() == 0
            && !check_flags.intersects(CheckFlags::READ_PARTIAL)
            && index_types.is_empty()
        {
            if !merged_instantiations {
                return Some(single);
            }
            // Reconstitute the symbol from the "first value declaration"
            // (Go's comment) — the merged-instantiations clone.
            let (single_prop_type, single_prop_mapper) = if self
                .program_symbol(single)
                .flags
                .intersects(SymbolFlags::TRANSIENT)
            {
                let links = self.value_symbol_links.get(single);
                (links.resolved_type, links.mapper)
            } else {
                (None, None)
            };
            let clone = self.create_symbol_with_type(single, single_prop_type.unwrap_or(self.any_type()));
            let value_declaration = self.program_symbol(single).value_declaration;
            if let Some(value_declaration) = value_declaration {
                let parent_symbol = crate::typeofsymbol::get_symbol_of_declaration(self, value_declaration)
                    .and_then(|s| self.program_symbol(s).parent);
                if let Some(parent_symbol) = parent_symbol {
                    self.program_symbol_mut(clone).parent = Some(parent_symbol);
                }
            }
            {
                let links = self.value_symbol_links.get(clone);
                links.containing_type = Some(containing_type);
                links.mapper = single_prop_mapper;
            }
            let write_type = self.get_write_type_of_symbol(single);
            self.value_symbol_links.get(clone).write_type = Some(write_type);
            return Some(clone);
        }
        if prop_set.size() == 0 {
            prop_set.add(single);
        }
        let mut declarations: Vec<NodeId> = Vec::new();
        let mut first_type: Option<TypeId> = None;
        let mut name_type: Option<TypeId> = None;
        let mut prop_types: Vec<TypeId> = Vec::new();
        let mut write_types: Option<Vec<TypeId>> = None;
        let mut first_value_declaration: Option<NodeId> = None;
        let mut has_non_uniform_value_declaration = false;
        for &prop in prop_set.values() {
            let prop_value_declaration = self.program_symbol(prop).value_declaration;
            if first_value_declaration.is_none() {
                first_value_declaration = prop_value_declaration;
            } else if prop_value_declaration.is_some()
                && prop_value_declaration != first_value_declaration
            {
                has_non_uniform_value_declaration = true;
            }
            let prop_declarations = self.program_symbol(prop).declarations.clone();
            for declaration in prop_declarations {
                if !declarations.contains(&declaration) {
                    declarations.push(declaration);
                }
            }
            let t = self.get_type_of_symbol(prop);
            if first_type.is_none() {
                first_type = Some(t);
                name_type = self.value_symbol_links.get(prop).name_type;
            }
            let write_type = self.get_write_type_of_symbol(prop);
            if write_types.is_some() || write_type != t {
                let mut w = write_types.unwrap_or_else(|| prop_types.clone());
                w.push(write_type);
                write_types = Some(w);
            }
            if first_type != Some(t) {
                check_flags |= CheckFlags::HAS_NON_UNIFORM_TYPE;
            }
            if crate::relater::is_literal_type(self, t)
                || crate::typenodes::is_pattern_literal_type(self, t)
            {
                check_flags |= CheckFlags::HAS_LITERAL_TYPE;
            }
            let unique_literal_type = self.unique_literal_type();
            if self.type_(t).flags.intersects(TypeFlags::NEVER) && t != unique_literal_type {
                check_flags |= CheckFlags::HAS_NEVER_TYPE;
            }
            prop_types.push(t);
        }
        prop_types.extend(index_types);
        let result = self.new_symbol_ex(prop_flags | optional_flag, name, check_flags | synthetic_flag);
        {
            let result_symbol = self.program_symbol_mut(result);
            result_symbol.declarations = declarations;
            if !has_non_uniform_value_declaration {
                result_symbol.value_declaration = first_value_declaration;
            }
        }
        if !has_non_uniform_value_declaration
            && let Some(first_value_declaration) = first_value_declaration
        {
            // Inherit information about parent type (Go).
            let parent = crate::typeofsymbol::get_symbol_of_declaration(
                self,
                first_value_declaration,
            )
            .and_then(|s| self.program_symbol(s).parent);
            self.program_symbol_mut(result).parent = parent;
        }
        {
            let links = self.value_symbol_links.get(result);
            links.containing_type = Some(containing_type);
            links.name_type = name_type;
        }
        if prop_types.len() > 2 {
            // When `propTypes` has the potential to explode in size when
            // normalized, defer normalization until absolutely needed (Go).
            self.program_symbol_mut(result).check_flags |= CheckFlags::DEFERRED_TYPE;
            let deferred = self.deferred_symbol_links.get(result);
            deferred.parent = Some(containing_type);
            deferred.constituents = prop_types;
            deferred.write_constituents = write_types.unwrap_or_default();
            return Some(result);
        }
        let resolved_type = if is_union {
            self.get_union_type(&prop_types)
        } else {
            self.get_intersection_type(&prop_types)
        };
        self.value_symbol_links.get(result).resolved_type = Some(resolved_type);
        if let Some(write_types) = write_types {
            let write_type = if is_union {
                self.get_union_type(&write_types)
            } else {
                self.get_intersection_type(&write_types)
            };
            self.value_symbol_links.get(result).write_type = Some(write_type);
        }
        Some(result)
    }

    /// Go: `func (c *Checker) getTargetSymbol(s) *ast.Symbol` (checker.go:22087)
    /// — the instantiation-target walk (`_impl` suffix: the exports.rs
    /// GetTargetSymbol twin is the Option-shaped public carrier).
    pub(crate) fn get_target_symbol_impl(&mut self, s: SymbolId) -> SymbolId {
        // if symbol is instantiated its flags are not copied from the
        // 'target' so we'll need to get back original 'target' symbol to
        // work with correct set of flags (Go's comment).
        if self
            .program_symbol(s)
            .check_flags
            .intersects(CheckFlags::INSTANTIATED)
        {
            let target = self.value_symbol_links.get(s).target;
            if let Some(target) = target {
                return target;
            }
        }
        s
    }

    /// Go: `func (c *Checker) hasCommonDeclaration(symbols) bool` (checker.go:22135).
    fn has_common_declaration(&mut self, symbols: &tsc_collections::OrderedSet<SymbolId>) -> bool {
        let mut common_declarations: rustc_hash::FxHashSet<NodeId> = rustc_hash::FxHashSet::default();
        for &symbol in symbols.values() {
            let declarations = self.program_symbol(symbol).declarations.clone();
            if declarations.is_empty() {
                return false;
            }
            if common_declarations.is_empty() {
                common_declarations.extend(declarations);
                continue;
            }
            common_declarations.retain(|d| declarations.contains(d));
            if common_declarations.is_empty() {
                return false;
            }
        }
        !common_declarations.is_empty()
    }

    // ────────────────────────────────────────────────────────────────────────
    // Apparent types (Go 22154-22337)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getApparentType(t) *Type` (checker.go:22154) —
    /// for a type parameter, the base constraint; for the string, number,
    /// boolean, and symbol primitive types, the corresponding object types;
    /// otherwise the type itself.
    pub(crate) fn get_apparent_type_impl(&mut self, t: TypeId) -> TypeId {
        let original_type = t;
        let mut t = t;
        if self.type_(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            t = crate::relater::get_base_constraint_of_type(self, t)
                .unwrap_or_else(|| self.unknown_type());
        }
        let flags = self.type_(t).flags;
        if self.type_(t).object_flags.intersects(ObjectFlags::MAPPED) {
            return self.get_apparent_type_of_mapped_type(t);
        }
        if self.type_(t).object_flags.intersects(ObjectFlags::REFERENCE) && t != original_type {
            return self.get_type_with_this_argument_impl(t, Some(original_type), false);
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            return self.get_apparent_type_of_intersection_type(t, original_type);
        }
        if flags.intersects(TypeFlags::STRING_LIKE) {
            return self.global_string_type.unwrap_or(t);
        }
        if flags.intersects(TypeFlags::NUMBER_LIKE) {
            return self.global_number_type.unwrap_or(t);
        }
        if flags.intersects(TypeFlags::BIGINT_LIKE) {
            // TODO(port M6 S2): getGlobalBigIntType — the BigInt wrapper
            // global; falls back to the type itself until the globals wave.
            return t;
        }
        if flags.intersects(TypeFlags::BOOLEAN_LIKE) {
            return self.global_boolean_type.unwrap_or(t);
        }
        if flags.intersects(TypeFlags::ESSYMBOL_LIKE) {
            // TODO(port M6 S2): getGlobalESSymbolType — as above.
            return t;
        }
        if flags.intersects(TypeFlags::NON_PRIMITIVE) {
            return self.empty_object_type();
        }
        if flags.intersects(TypeFlags::INDEX) {
            return self.string_number_symbol_type();
        }
        if flags.intersects(TypeFlags::UNKNOWN) && !self.strict_null_checks {
            return self.empty_object_type();
        }
        t
    }

    /// Go: `func (c *Checker) getApparentTypeOfMappedType(t) *Type` (checker.go:22186).
    pub(crate) fn get_apparent_type_of_mapped_type(&mut self, t: TypeId) -> TypeId {
        if let Some(resolved) = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.resolved_apparent_type)
        {
            return resolved;
        }
        let resolved = self.get_resolved_apparent_type_of_mapped_type(t);
        if let Some(d) = self.type_mut(t).as_mapped_type_mut() {
            d.resolved_apparent_type = Some(resolved);
        }
        resolved
    }

    /// Go: `func (c *Checker) getResolvedApparentTypeOfMappedType(t) *Type` (checker.go:22197).
    fn get_resolved_apparent_type_of_mapped_type(&mut self, t: TypeId) -> TypeId {
        let target = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.base.target)
            .unwrap_or(t);
        let type_variable = self.get_homomorphic_type_variable(target);
        let name_type_is_none = self
            .type_(target)
            .as_mapped_type()
            .is_some()
            && self.mapped_type_declaration_name_type_is_none(target);
        if let Some(type_variable) = type_variable
            && name_type_is_none
        {
            // We have a homomorphic mapped type or an instantiation of a
            // homomorphic mapped type (Go's comment).
            let modifiers_type = self.get_modifiers_type_from_mapped_type_impl(t);
            let base_constraint: Option<TypeId> = if crate::relater::is_generic_mapped_type(self, modifiers_type) {
                Some(self.get_apparent_type_of_mapped_type(modifiers_type))
            } else {
                crate::relater::get_base_constraint_of_type(self, modifiers_type)
            };
            if let Some(base_constraint) = base_constraint {
                let all_arrays_or_tuples = if self.type_(base_constraint).flags.intersects(TypeFlags::UNION) {
                    let types = self.type_(base_constraint).types().to_vec();
                    types
                        .iter()
                        .all(|&t| {
                            crate::relater::is_array_or_tuple_type(self, t)
                                || self.is_array_or_tuple_or_intersection(t)
                        })
                } else {
                    crate::relater::is_array_or_tuple_type(self, base_constraint)
                        || self.is_array_or_tuple_or_intersection(base_constraint)
                };
                if all_arrays_or_tuples {
                    let mapper = self
                        .type_(t)
                        .as_mapped_type()
                        .and_then(|m| m.base.mapper);
                    let prepended = self.prepend_type_mapping(type_variable, base_constraint, mapper);
                    return crate::relater::instantiate_type_ex(self, target, Some(prepended));
                }
            }
        }
        t
    }

    /// Go: `func (c *Checker) getApparentTypeOfIntersectionType(t, thisArgument) *Type`
    /// (checker.go:22221).
    fn get_apparent_type_of_intersection_type(&mut self, t: TypeId, this_argument: TypeId) -> TypeId {
        if t == this_argument {
            let resolved = self
                .type_(t)
                .as_intersection_type()
                .and_then(|d| d.resolved_apparent_type);
            if let Some(resolved) = resolved {
                return resolved;
            }
            let resolved = self.get_type_with_this_argument_impl(t, Some(this_argument), true);
            if let Some(d) = self.type_mut(t).as_intersection_type_mut() {
                d.resolved_apparent_type = Some(resolved);
            }
            return resolved;
        }
        let key = crate::todo_shapes::CachedTypeKey {
            kind: crate::todo_shapes::CachedTypeKind::ApparentType,
            type_id: this_argument.0,
        };
        if let Some(cached) = self.cached_types.get(&key) {
            return cached.unwrap_or(t);
        }
        let result = self.get_type_with_this_argument_impl(t, Some(this_argument), true);
        self.cached_types.insert(key, Some(result));
        result
    }

    /// Go: `func (c *Checker) getReducedApparentType(t) *Type` (checker.go:22318) —
    /// type reduction both before and after obtaining the apparent type.
    pub(crate) fn get_reduced_apparent_type(&mut self, t: TypeId) -> TypeId {
        let reduced = self.get_reduced_type(t);
        let apparent = self.get_apparent_type_impl(reduced);
        self.get_reduced_type(apparent)
    }

    /// Go: `func (c *Checker) elaborateNeverIntersection(chain, node, t) *ast.Diagnostic`
    /// (checker.go:22326). PORT: Go can return the nil chain; the Option is
    /// the nil-pointer port.
    pub(crate) fn elaborate_never_intersection(
        &mut self,
        chain: Option<&tsc_ast::Diagnostic>,
        node: NodeId,
        t: TypeId,
    ) -> Option<tsc_ast::Diagnostic> {
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION)
            && self
                .type_(t)
                .object_flags
                .intersects(ObjectFlags::IS_NEVER_INTERSECTION)
        {
            let properties = self.get_properties_of_union_or_intersection_type(t);
            if let Some(never_prop) = properties
                .iter()
                .copied()
                .find(|&prop| self.is_discriminant_with_never_type(prop))
            {
                let type_string = self.type_to_string(t);
                let prop_string = crate::relater::symbol_to_string(self, never_prop);
                return Some(u::new_diagnostic_chain_for_node(
                    chain,
                    self.store(),
                    node,
                    &diag::THE_INTERSECTION_0_WAS_REDUCED_TO_NEVER_BECAUSE_PROPERTY_1_HAS_CONFLICTING_TYPES_IN_SOME_CONSTITUENTS,
                    vec![type_string, prop_string],
                ));
            }
            if let Some(private_prop) = properties
                .iter()
                .copied()
                .find(|&prop| is_conflicting_private_property(self, prop))
            {
                let type_string = self.type_to_string(t);
                let prop_string = crate::relater::symbol_to_string(self, private_prop);
                return Some(u::new_diagnostic_chain_for_node(
                    chain,
                    self.store(),
                    node,
                    &diag::THE_INTERSECTION_0_WAS_REDUCED_TO_NEVER_BECAUSE_PROPERTY_1_EXISTS_IN_MULTIPLE_CONSTITUENTS_AND_IS_PRIVATE_IN_SOME,
                    vec![type_string, prop_string],
                ));
            }
        }
        chain.cloned()
    }

    /// Go: `func (c *Checker) isDiscriminantWithNeverType(prop) bool` (checker.go:22337).
    /// Return true for a synthetic non-optional property with non-uniform
    /// types, where at least one is a literal type and none is never, that
    /// reduces to never (Go's comment).
    pub(crate) fn is_discriminant_with_never_type(&mut self, prop: SymbolId) -> bool {
        let symbol = self.program_symbol(prop);
        if symbol.flags.intersects(SymbolFlags::OPTIONAL) {
            return false;
        }
        let masked = symbol.check_flags
            & (CheckFlags::NON_UNIFORM_AND_LITERAL.union(CheckFlags::HAS_NEVER_TYPE));
        if masked != CheckFlags::NON_UNIFORM_AND_LITERAL {
            return false;
        }
        let t = self.get_type_of_symbol(prop);
        self.type_(t).flags.intersects(TypeFlags::NEVER)
    }

    // ────────────────────────────────────────────────────────────────────────
    // The never-intersection reduction family (Go 22242-22316)
    // ────────────────────────────────────────────────────────────────────────

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) isMappingOfSameObjectType(types) bool` (checker.go:22258).
    fn is_mapping_of_same_object_type(&mut self, types: &[TypeId]) -> bool {
        if !types.is_empty()
            && self.type_(types[0]).object_flags.intersects(ObjectFlags::MAPPED)
        {
            let first_type = self.get_modifiers_type_from_mapped_type_impl(types[0]);
            if self.type_(first_type).flags.intersects(TypeFlags::OBJECT) {
                for &t in &types[1..] {
                    if !self.type_(t).object_flags.intersects(ObjectFlags::MAPPED)
                        || self.get_modifiers_type_from_mapped_type_impl(t) != first_type
                    {
                        return false;
                    }
                }
                return true;
            }
        }
        false
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) somePropertyReducesToNever(t) bool` (checker.go:22271).
    fn some_property_reduces_to_never(&mut self, t: TypeId) -> bool {
        // Collect declaration counts for each property across all constituent
        // types of the intersection (Go's comment).
        let mut counts: tsc_collections::OrderedMap<String, i32> = tsc_collections::OrderedMap::new();
        let constituents = self.type_(t).types().to_vec();
        for t in constituents {
            for prop in self.get_properties_of_type_impl(t) {
                let name = self.program_symbol(prop).name.clone();
                let count = counts.get(&name).copied().unwrap_or(0);
                counts.set(name, count + 1);
            }
        }
        // Check if any property appears in more than one constituent type and
        // reduces to 'never' (Go's comment).
        for (prop_name, count) in counts.entries() {
            if *count > 1 {
                if let Some(prop) = self.get_property_of_union_or_intersection_type_impl(t, prop_name, true) {
                    if self.is_never_reduced_property(prop) {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) isNeverReducedProperty(prop) bool` (checker.go:22308).
    fn is_never_reduced_property(&mut self, prop: SymbolId) -> bool {
        self.is_discriminant_with_never_type(prop) || is_conflicting_private_property(self, prop)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Mapped-type component readers (Go 23158-23192 + 29541 + 28661)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getTypeParameterFromMappedType(t) *Type` (checker.go:23158).
    pub(crate) fn get_type_parameter_from_mapped_type_impl(&mut self, t: TypeId) -> TypeId {
        let already = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.type_parameter);
        if let Some(already) = already {
            return already;
        }
        let declaration = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.declaration)
            .expect("MappedType declaration");
        let type_parameter = self
            .store()
            .node(declaration)
            .as_mapped_type_node()
            .expect("MappedTypeNode data")
            .type_parameter;
        let symbol = crate::typeofsymbol::get_symbol_of_declaration(self, type_parameter)
            .expect("MappedTypeNode type-parameter symbol");
        let declared = self.get_declared_type_of_type_parameter(symbol);
        if let Some(d) = self.type_mut(t).as_mapped_type_mut() {
            d.type_parameter = Some(declared);
        }
        declared
    }

    /// Go: `func (c *Checker) getConstraintTypeFromMappedType(t) *Type` (checker.go:23164).
    pub(crate) fn get_constraint_type_from_mapped_type_impl(&mut self, t: TypeId) -> TypeId {
        let already = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.constraint_type);
        if let Some(already) = already {
            return already;
        }
        let type_parameter = self.get_type_parameter_from_mapped_type_impl(t);
        let constraint = crate::relater::get_constraint_of_type_parameter(self, type_parameter)
            .unwrap_or_else(|| self.error_type());
        if let Some(d) = self.type_mut(t).as_mapped_type_mut() {
            d.constraint_type = Some(constraint);
        }
        constraint
    }

    /// Go: `func (c *Checker) getNameTypeFromMappedType(t) *Type` (checker.go:23170).
    pub(crate) fn get_name_type_from_mapped_type_impl(&mut self, t: TypeId) -> Option<TypeId> {
        let (declaration, mapper, cached) = {
            let m = self.type_(t).as_mapped_type()?;
            (m.declaration?, m.base.mapper, m.name_type)
        };
        let name_type_node = {
            
            self
                .store()
                .node(declaration)
                .as_mapped_type_node()
                .expect("MappedTypeNode data")
                .name_type
        };
        name_type_node?;
        if let Some(cached) = cached {
            return Some(cached);
        }
        let name_type_node = name_type_node.expect("checked above");
        let instantiated = {
            let nt = self.get_type_from_type_node(name_type_node);
            crate::relater::instantiate_type_ex(self, nt, mapper)
        };
        if let Some(d) = self.type_mut(t).as_mapped_type_mut() {
            d.name_type = Some(instantiated);
        }
        Some(instantiated)
    }

    /// Go: `func (c *Checker) getTemplateTypeFromMappedType(t) *Type` (checker.go:23180).
    pub(crate) fn get_template_type_from_mapped_type_impl(&mut self, t: TypeId) -> TypeId {
        let already = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.template_type);
        if let Some(already) = already {
            return already;
        }
        let (declaration, mapper) = {
            let m = self
                .type_(t)
                .as_mapped_type()
                .expect("MappedType data");
            (m.declaration, m.base.mapper)
        };
        let template_type = match declaration {
            Some(declaration) => {
                let type_node = {
                    
                    self
                        .store()
                        .node(declaration)
                        .as_mapped_type_node()
                        .expect("MappedTypeNode data")
                        .type_
                };
                match type_node {
                    Some(type_node) => {
                        let include_optional = self
                            .mapped_type_modifiers_impl(t)
                            .intersects(crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL);
                        let t = self.get_type_from_type_node(type_node);
                        let optional_added = self.add_optionality_ex(t, true /*isProperty*/, include_optional);
                        crate::relater::instantiate_type_ex(self, optional_added, mapper)
                    }
                    None => self.error_type(),
                }
            }
            None => self.error_type(),
        };
        if let Some(d) = self.type_mut(t).as_mapped_type_mut() {
            d.template_type = Some(template_type);
        }
        template_type
    }

    /// Go: `func getMappedTypeModifiers(t) MappedTypeModifiers` (checker.go:29541).
    pub(crate) fn mapped_type_modifiers_impl(&mut self, t: TypeId) -> crate::relater::MappedTypeModifiers {
        let Some(declaration) = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.declaration)
        else {
            return crate::relater::MappedTypeModifiers::NONE;
        };
        let (readonly_token, question_token) = {
            let d = self
                .store()
                .node(declaration)
                .as_mapped_type_node()
                .expect("MappedTypeNode data");
            (
                d.readonly_token.map(|t| self.store().node(t).kind),
                d.question_token.map(|t| self.store().node(t).kind),
            )
        };
        let mut modifiers = crate::relater::MappedTypeModifiers::NONE;
        if let Some(kind) = readonly_token {
            modifiers |= if kind == Kind::MinusToken {
                crate::relater::MappedTypeModifiers::EXCLUDE_READONLY
            } else {
                crate::relater::MappedTypeModifiers::INCLUDE_READONLY
            };
        }
        if let Some(kind) = question_token {
            modifiers |= if kind == Kind::MinusToken {
                crate::relater::MappedTypeModifiers::EXCLUDE_OPTIONAL
            } else {
                crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL
            };
        }
        modifiers
    }

    /// Go: `func getMappedTypeOptionality(t) int` (checker.go:29555).
    pub(crate) fn get_mapped_type_optionality_impl(&mut self, t: TypeId) -> i32 {
        let modifiers = self.mapped_type_modifiers_impl(t);
        if modifiers
            .intersects(crate::relater::MappedTypeModifiers::EXCLUDE_OPTIONAL)
        {
            return -1;
        }
        if modifiers
            .intersects(crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL)
        {
            return 1;
        }
        0
    }

    /// Go: `func (c *Checker) getCombinedMappedTypeOptionality(t) int`
    /// (checker.go:29569) — return -1, 0, or 1, for stripped, unchanged, or
    /// added optionality respectively. When a homomorphic mapped type
    /// doesn't modify optionality, recursively consult the optionality of
    /// the type being mapped over to see if it strips or adds optionality
    /// (Go's comment).
    pub(crate) fn get_combined_mapped_type_optionality_impl(&mut self, t: TypeId) -> i32 {
        if self.type_(t).object_flags.intersects(ObjectFlags::MAPPED) {
            let optionality = self.get_mapped_type_optionality_impl(t);
            if optionality != 0 {
                return optionality;
            }
            let modifiers_type = self.get_modifiers_type_from_mapped_type_impl(t);
            return self.get_combined_mapped_type_optionality_impl(modifiers_type);
        }
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.type_(t).types().to_vec();
            let optionality = self.get_combined_mapped_type_optionality_impl(types[0]);
            for &t in &types[1..] {
                if self.get_combined_mapped_type_optionality_impl(t) != optionality {
                    return 0;
                }
            }
            return optionality;
        }
        0
    }

    /// Go: `func (c *Checker) getConstraintDeclarationForMappedType(t) *ast.Node` (checker.go:23198).
    fn get_constraint_declaration_for_mapped_type(&mut self, t: TypeId) -> Option<NodeId> {
        let type_parameter = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.declaration)?
            .pipe(|declaration| {
                self.store()
                    .node(declaration)
                    .as_mapped_type_node()
                    .expect("MappedTypeNode data")
                    .type_parameter
            });
        self.store()
            .node(type_parameter)
            .as_type_parameter_declaration()
            .expect("TypeParameterDeclaration data")
            .constraint
    }

    /// Go: `func (c *Checker) isMappedTypeWithKeyofConstraintDeclaration(t) bool` (checker.go:23192).
    pub(crate) fn is_mapped_type_with_keyof_constraint_declaration_impl(&mut self, t: TypeId) -> bool {
        let Some(constraint_declaration) = self.get_constraint_declaration_for_mapped_type(t) else {
            return false;
        };
        self.store().node(constraint_declaration).kind == Kind::TypeOperator
            && self
                .store()
                .node(constraint_declaration)
                .as_type_operator_node()
                .is_some_and(|d| d.operator == Kind::KeyOfKeyword)
    }

    /// Go: `func (c *Checker) getModifiersTypeFromMappedType(t) *Type` (checker.go:28661).
    pub(crate) fn get_modifiers_type_from_mapped_type_impl(&mut self, t: TypeId) -> TypeId {
        let already = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.modifiers_type);
        if let Some(already) = already {
            return already;
        }
        let modifiers_type = if self.is_mapped_type_with_keyof_constraint_declaration_impl(t) {
            // If the constraint declaration is a 'keyof T' node, the modifiers
            // type is T (Go's comment).
            let constraint_declaration = self
                .get_constraint_declaration_for_mapped_type(t)
                .expect("keyof constraint declaration");
            let constraint_type_node = {
                let d = self
                    .store()
                    .node(constraint_declaration)
                    .as_type_operator_node()
                    .expect("TypeOperatorNode data");
                d.type_
            };
            let mapper = self
                .type_(t)
                .as_mapped_type()
                .and_then(|m| m.base.mapper);
            let constraint_type = self.get_type_from_type_node(constraint_type_node);
            crate::relater::instantiate_type_ex(self, constraint_type, mapper)
        } else {
            // Otherwise, get the declared constraint type, and if the
            // constraint type is a type parameter, get the constraint of that
            // type parameter. If the resulting type is an indexed type 'keyof
            // T', the modifiers type is T. Otherwise, the modifiers type is
            // unknown (Go's comment).
            self.unknown_type()
        };
        if let Some(d) = self.type_mut(t).as_mapped_type_mut() {
            d.modifiers_type = Some(modifiers_type);
        }
        modifiers_type
    }

    /// Go: `func (c *Checker) getMappedTypeNameTypeKind(t) MappedTypeNameTypeKind` (checker.go:27376).
    pub(crate) fn get_mapped_type_name_type_kind(&mut self, t: TypeId) -> crate::relater::MappedTypeNameTypeKind {
        let Some(name_type) = self.get_name_type_from_mapped_type_impl(t) else {
            return crate::relater::MappedTypeNameTypeKind::None;
        };
        let type_parameter = self.get_type_parameter_from_mapped_type_impl(t);
        if self.is_type_assignable_to(name_type, type_parameter) {
            return crate::relater::MappedTypeNameTypeKind::Filtering;
        }
        crate::relater::MappedTypeNameTypeKind::Remapping
    }

    /// Go: `func (c *Checker) forEachMappedTypePropertyKeyTypeAndIndexSignatureKeyType(t, include, stringsOnly, cb)`
    /// (checker.go:23215).
    pub(crate) fn for_each_mapped_type_property_key_type_and_index_signature_key_type(
        &mut self,
        t: TypeId,
        include: TypeFlags,
        strings_only: bool,
        cb: &mut dyn FnMut(&mut Checker, TypeId),
    ) {
        for prop in self.get_properties_of_type_impl(t) {
            if let Some(literal) = self.get_literal_type_from_property_impl(prop, include, false) {
                cb(self, literal);
            }
        }
        if self.type_(t).flags.intersects(TypeFlags::ANY) {
            let string_type = self.string_type();
            cb(self, string_type);
        } else {
            for info in self.get_index_infos_of_type_impl(t) {
                let key_type = self.index_info_key_type(info);
                if !strings_only
                    || self
                        .type_(key_type)
                        .flags
                        .intersects(TypeFlags::STRING.union(TypeFlags::TEMPLATE_LITERAL))
                {
                    cb(self, key_type);
                }
            }
        }
    }

    /// Go: `func (c *Checker) getLiteralTypeFromProperty(prop, include, includeNonPublic) *Type`
    /// (checker.go:27280).
    pub(crate) fn get_literal_type_from_property_impl(
        &mut self,
        prop: SymbolId,
        include: TypeFlags,
        include_non_public: bool,
    ) -> Option<TypeId> {
        if include_non_public
            || !u::get_declaration_modifier_flags_from_symbol(self, self.store(), prop)
                .intersects(ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER)
        {
            let late_bound = self.get_late_bound_symbol(prop);
            let mut t = self.value_symbol_links.get(late_bound).name_type;
            if t.is_none() {
                let name = self.program_symbol(prop).name.clone();
                if name == tsc_ast::INTERNAL_SYMBOL_NAME_DEFAULT {
                    t = Some(self.get_string_literal_type("default"));
                } else {
                    let value_declaration = self.program_symbol(prop).value_declaration;
                    if let Some(value_declaration) = value_declaration {
                        let name = self.store().node(value_declaration).name();
                        if let Some(name) = name {
                            t = Some(self.get_literal_type_from_property_name(name));
                        }
                    }
                    if t.is_none() && !u::is_known_symbol(self.program_symbol(prop)) {
                        let name = crate::relater::symbol_to_string(self, prop);
                        t = Some(self.get_string_literal_type(&name));
                    }
                }
            }
            if let Some(t) = t
                && self.type_(t).flags.intersects(include)
            {
                return Some(t);
            }
        }
        None
    }

    /// Go: `func (c *Checker) getTypeParameterFromMappedType`'s declaration
    /// NameType nil probe — `mappedType.declaration.NameType == nil`.
    fn mapped_type_declaration_name_type_is_none(&mut self, t: TypeId) -> bool {
        let Some(declaration) = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.declaration)
        else {
            return true;
        };
        self.store()
            .node(declaration)
            .as_mapped_type_node()
            .expect("MappedTypeNode data")
            .name_type
            .is_none()
    }

    // ────────────────────────────────────────────────────────────────────────
    // Constructor accessibility (Go 8867-8920)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `type constructorAccessibilityError struct` — the carrier.
    pub(crate) fn get_constructor_accessibility_error(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
        modifiers_mask: ModifierFlags,
    ) -> Option<(ModifierFlags, TypeId)> {
        for &signature in signatures {
            let Some(declaration) = self.signatures[signature.0 as usize].declaration else {
                continue;
            };
            let modifiers = u::get_selected_modifier_flags(self.store().node(declaration), modifiers_mask);
            // (1) Public constructors and (2) constructor functions are always
            // accessible (Go's comment).
            if modifiers.is_empty() || self.store().node(declaration).kind != Kind::Constructor {
                continue;
            }
            let parent = self.store().node(declaration).parent.get();
            let Some(parent_symbol) = crate::typeofsymbol::get_symbol_of_declaration(self, parent) else {
                continue;
            };
            let declaring_class_declaration =
                crate::expressions::get_class_like_declaration_of_symbol_for(self, parent_symbol);
            // A private or protected constructor can only be instantiated
            // within its own class (or a subclass, for protected) (Go).
            let within = declaring_class_declaration
                .is_some_and(|class_declaration| self.is_node_within_class(node, class_declaration));
            if !within {
                let containing_class = u::get_containing_class(self.store(), node);
                if let Some(containing_class) = containing_class
                    && modifiers.intersects(ModifierFlags::PROTECTED)
                {
                    // PORT: Go reads getTypeOfNode(containingClass) — the
                    // node→type spine reads the class declaration's symbol's
                    // declared (instance) type.
                    let containing_type = crate::typeofsymbol::get_symbol_of_declaration(self, containing_class)
                        .map(|s| self.get_declared_type_of_symbol(self.get_merged_symbol(s)));
                    if let Some(containing_type) = containing_type
                        && self.type_has_protected_accessible_base(parent_symbol, containing_type)
                    {
                        continue;
                    }
                }
                let declaring_class = self.get_declared_type_of_symbol(parent_symbol);
                return Some((modifiers, declaring_class));
            }
        }
        None
    }

    /// Go: `func (c *Checker) typeHasProtectedAccessibleBase(target, t) bool` (checker.go:28901).
    fn type_has_protected_accessible_base(&mut self, target: SymbolId, t: TypeId) -> bool {
        let base_types = self.get_base_types_impl(crate::uniontypes::get_target_type(self, t));
        if base_types.is_empty() {
            return false;
        }
        let first_base = base_types[0];
        if self.type_(first_base).flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.type_(first_base).types().to_vec();
            let (mixin_flags, _) = self.find_mixins(&types);
            for (i, &intersection_member) in types.iter().enumerate() {
                // We want to ignore mixin ctors (Go's comment).
                if !mixin_flags[i]
                    && self
                        .type_(intersection_member)
                        .object_flags
                        .intersects(ObjectFlags::CLASS.union(ObjectFlags::INTERFACE))
                {
                    if self.type_(intersection_member).symbol == Some(target) {
                        return true;
                    }
                    if self.type_has_protected_accessible_base(target, intersection_member) {
                        return true;
                    }
                }
            }
            return false;
        }
        if self.type_(first_base).symbol == Some(target) {
            return true;
        }
        self.type_has_protected_accessible_base(target, first_base)
    }
}

/// Go: `func isPrototypeProperty(symbol) bool` (checker.go:22131).
pub(crate) fn is_prototype_property(c: &Checker, symbol: SymbolId) -> bool {
    c.program_symbol(symbol)
        .flags
        .intersects(SymbolFlags::METHOD)
        || c.program_symbol(symbol)
            .check_flags
            .intersects(CheckFlags::SYNTHETIC_METHOD)
}

/// Go: `func isConflictingPrivateProperty(prop) bool` (checker.go:22341).
pub(crate) fn is_conflicting_private_property(c: &Checker, prop: SymbolId) -> bool {
    // Return true for a synthetic property with multiple declarations, at
    // least one of which is private (Go's comment).
    c.program_symbol(prop).value_declaration.is_none()
        && c.program_symbol(prop)
            .check_flags
            .intersects(CheckFlags::CONTAINS_PRIVATE)
}

// ────────────────────────────────────────────────────────────────────────────
// resolveAnonymousTypeMembers / resolveMappedTypeMembers / union &
// intersection members + the union-signature machinery (Go 21054-21800)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) resolveAnonymousTypeMembers(t)` (checker.go:21054)
    /// — combinations of function, class, enum and module (and the
    /// type-literal + instantiated-reference fast paths).
    fn resolve_anonymous_type_members(&mut self, t: TypeId) {
        let (target, mapper) = {
            let Some(d) = self.type_(t).as_object_type() else {
                // Go's t.AsObjectType() panics on a non-object; the port's
                // dispatcher guarantees ANONYMOUS, so this arm is unreachable.
                unreachable!("resolveAnonymousTypeMembers on a non-object type");
            };
            (d.target, d.mapper)
        };
        if let Some(target) = target {
            self.set_structured_type_members(t, SymbolTable::new(), Vec::new(), Vec::new(), Vec::new());
            let target_properties = self.get_properties_of_object_type_impl(target);
            let target_call_signatures = self.get_signatures_of_type_impl(target, SignatureKind::Call);
            let target_construct_signatures =
                self.get_signatures_of_type_impl(target, SignatureKind::Construct);
            let target_index_infos = self.get_index_infos_of_type_impl(target);
            let members = self.create_instantiated_symbol_table(&target_properties, mapper);
            let call_signatures = self.instantiate_signatures(&target_call_signatures, mapper);
            let construct_signatures =
                self.instantiate_signatures(&target_construct_signatures, mapper);
            let index_infos = self.instantiate_index_infos(&target_index_infos, mapper);
            self.set_structured_type_members(t, members, call_signatures, construct_signatures, index_infos);
            return;
        }
        let raw_symbol = self.type_(t).symbol;
        let Some(raw_symbol) = raw_symbol else {
            // Go's `t.symbol` is non-nil on every type that reaches here.
            unreachable!("resolveAnonymousTypeMembers without a symbol");
        };
        let symbol = self.get_merged_symbol(raw_symbol);
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::TYPE_LITERAL) {
            self.set_structured_type_members(t, SymbolTable::new(), Vec::new(), Vec::new(), Vec::new());
            let members = self.get_members_of_symbol_table(symbol);
            let call_signatures = members
                .get(tsc_ast::INTERNAL_SYMBOL_NAME_CALL)
                .copied()
                .map(|s| self.get_signatures_of_symbol(Some(s)))
                .unwrap_or_default();
            let construct_signatures = members
                .get(tsc_ast::INTERNAL_SYMBOL_NAME_NEW)
                .copied()
                .map(|s| self.get_signatures_of_symbol(Some(s)))
                .unwrap_or_default();
            let index_infos = self.get_index_infos_of_symbol(Some(symbol));
            self.set_structured_type_members(t, members, call_signatures, construct_signatures, index_infos);
            return;
        }
        // Combinations of function, class, enum and module (Go's comment).
        let mut members = self.get_exports_of_symbol(symbol);
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        if Some(symbol) == self.global_this_symbol {
            // Go: keep only plain variables in the globalThis object (and
            // ambient value modules are kept).
            let mut vars_only = SymbolTable::new();
            let entries: Vec<(String, SymbolId)> = members
                .entries()
                .map(|(name, &symbol)| (name.clone(), symbol))
                .collect();
            for (name, p) in entries {
                let flags = self.program_symbol(p).flags;
                let keep = !flags.intersects(SymbolFlags::BLOCK_SCOPED)
                    && !(flags.intersects(SymbolFlags::VALUE_MODULE) && {
                        let declarations = self.program_symbol(p).declarations.clone();
                        !declarations.is_empty()
                            && declarations.iter().all(|&d| {
                                tsc_binder::ast_shims::is_ambient_module(self.store(), d)
                            })
                    });
                if keep {
                    vars_only.set(name, p);
                }
            }
            members = vars_only;
        }
        let mut base_constructor_index_info: Option<IndexInfoId> = None;
        self.set_structured_type_members(t, members.clone(), Vec::new(), Vec::new(), Vec::new());
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::CLASS) {
            let class_type = self.get_declared_type_of_class_or_interface(symbol);
            let base_constructor_type = self.get_base_constructor_type_of_class_impl(class_type);
            let base_flags = self.type_(base_constructor_type).flags;
            if base_flags.intersects(
                TypeFlags::OBJECT
                    .union(TypeFlags::INTERSECTION)
                    .union(TypeFlags::TYPE_VARIABLE),
            ) {
                // Go: maps.Clone then addInheritedMembers — the clone already
                // happened above (`members.clone()`), so add in place.
                let base_properties = self.get_properties_of_type_impl(base_constructor_type);
                members = self.add_inherited_members(members, base_properties);
                self.set_structured_type_members(t, members.clone(), Vec::new(), Vec::new(), Vec::new());
            } else if base_constructor_type == self.any_type() {
                base_constructor_index_info = self.any_base_type_index_info;
            }
        }
        let index_symbol = members.get(tsc_ast::INTERNAL_SYMBOL_NAME_INDEX).copied();
        match index_symbol {
            Some(index_symbol) => {
                let sibling_symbols: Vec<SymbolId> =
                    members.values().copied().collect();
                index_infos =
                    self.get_index_infos_of_index_symbol(index_symbol, sibling_symbols);
            }
            None => {
                if let Some(info) = base_constructor_index_info {
                    index_infos.push(info);
                }
                if self.program_symbol(symbol).flags.intersects(SymbolFlags::ENUM)
                    && ({
                        let declared = self.get_declared_type_of_symbol(symbol);
                        self.type_(declared).flags.intersects(TypeFlags::ENUM)
                            || {
                                let props = self
                                    .type_(t)
                                    .as_structured_type()
                                    .expect("StructuredType data")
                                    .properties
                                    .clone();
                                props.iter().any(|&prop| {
                                    let prop_type = self.get_type_of_symbol(prop);
                                    self.type_(prop_type)
                                        .flags
                                        .intersects(TypeFlags::NUMBER_LIKE)
                                })
                            }
                    })
                {
                    if let Some(info) = self.enum_number_index_info {
                        index_infos.push(info);
                    }
                }
            }
        }
        // We resolve the members before computing the signatures because a
        // signature may use typeof with a qualified name expression that
        // circularly references the type we are in the process of resolving
        // (see issue #6072). The temporarily empty signature list will never
        // be observed because a qualified name can't reference signatures
        // (Go's comment).
        let mut call_signature_count = 0i32;
        let mut signatures: Vec<SignatureId> = Vec::new();
        if self
            .program_symbol(symbol)
            .flags
            .intersects(SymbolFlags::FUNCTION.union(SymbolFlags::METHOD))
        {
            signatures = self.get_signatures_of_symbol(Some(symbol));
            call_signature_count = signatures.len() as i32;
        }
        // And likewise for construct signatures for classes (Go's comment).
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::CLASS) {
            let class_type = self.get_declared_type_of_class_or_interface(symbol);
            let constructor_symbol = self
                .program_symbol(symbol)
                .members
                .get(tsc_ast::INTERNAL_SYMBOL_NAME_CONSTRUCTOR)
                .copied();
            let mut construct_signatures = self
                .get_signatures_of_symbol(constructor_symbol);
            if construct_signatures.is_empty() {
                construct_signatures = self.get_default_construct_signatures(class_type);
            }
            signatures.extend(construct_signatures);
        }
        let data = self
            .type_mut(t)
            .as_structured_type_mut()
            .expect("StructuredType data");
        data.index_infos = index_infos;
        data.signatures = signatures;
        data.call_signature_count = call_signature_count;
    }

    /// Go: `func (c *Checker) resolveMappedTypeMembers(t)` (checker.go:21298).
    fn resolve_mapped_type_members(&mut self, t: TypeId) {
        let mut members = SymbolTable::new();
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        // Resolve upfront such that recursive references see an empty object
        // type (Go's comment).
        self.set_structured_type_members(t, SymbolTable::new(), Vec::new(), Vec::new(), Vec::new());
        // In { [P in K]: T }, we refer to P as the type parameter type, K as
        // the constraint type, and T as the template type (Go's comment).
        let type_parameter = self.get_type_parameter_from_mapped_type_impl(t);
        let constraint_type = self.get_constraint_type_from_mapped_type_impl(t);
        let mapped_type = self
            .type_(t)
            .as_mapped_type()
            .and_then(|m| m.base.target)
            .unwrap_or(t);
        let name_type = self.get_name_type_from_mapped_type_impl(mapped_type);
        let should_link_prop_declarations = self.get_mapped_type_name_type_kind(mapped_type)
            != crate::relater::MappedTypeNameTypeKind::Remapping;
        let template_type = self.get_template_type_from_mapped_type_impl(mapped_type);
        let modifiers_source = self.get_modifiers_type_from_mapped_type_impl(t);
        let modifiers_type = self.get_apparent_type_impl(modifiers_source);
        // The 'T' in 'keyof T' (Go's comment).
        let template_modifiers = self.mapped_type_modifiers_impl(t);
        let include = TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE;
        if self.is_mapped_type_with_keyof_constraint_declaration_impl(t) {
            // We have a { [P in keyof T]: X } (Go's comment).
            let mut keys: Vec<TypeId> = Vec::new();
            self.for_each_mapped_type_property_key_type_and_index_signature_key_type(
                modifiers_type,
                include,
                false, /*stringsOnly*/
                &mut |_c, key_type| keys.push(key_type),
            );
            for key_type in keys {
                self.add_member_for_key_type(
                    t,
                    type_parameter,
                    name_type,
                    template_type,
                    modifiers_type,
                    template_modifiers,
                    should_link_prop_declarations,
                    &mut members,
                    &mut index_infos,
                    key_type,
                );
            }
        } else {
            let mut keys: Vec<TypeId> = Vec::new();
            let lower_bound = self.get_lower_bound_of_key_type(constraint_type);
            for_each_type(self, lower_bound, &mut |_c, key| keys.push(key));
            for key_type in keys {
                self.add_member_for_key_type(
                    t,
                    type_parameter,
                    name_type,
                    template_type,
                    modifiers_type,
                    template_modifiers,
                    should_link_prop_declarations,
                    &mut members,
                    &mut index_infos,
                    key_type,
                );
            }
        }
        let index_infos = index_infos;
        self.set_structured_type_members(t, members, Vec::new(), Vec::new(), index_infos);
    }

    /// Go: `resolveMappedTypeMembers`'s `addMemberForKeyType` closure (the
    /// `addMemberForKeyTypeWorker` inlined; checker.go:21298) — flattened to
    /// an explicit-parameter method per the module PORT note.
    #[allow(clippy::too_many_arguments)]
    fn add_member_for_key_type(
        &mut self,
        t: TypeId,
        type_parameter: TypeId,
        name_type: Option<TypeId>,
        template_type: TypeId,
        modifiers_type: TypeId,
        template_modifiers: crate::relater::MappedTypeModifiers,
        should_link_prop_declarations: bool,
        members: &mut SymbolTable,
        index_infos: &mut Vec<IndexInfoId>,
        key_type: TypeId,
    ) {
        // The outer closure instantiated the name type per key first.
        let prop_name_type = match name_type {
            None => key_type,
            Some(name_type) => {
                let mapper = self.type_(t).as_mapped_type().and_then(|m| m.base.mapper);
                let appended = self.append_type_mapping(mapper, type_parameter, key_type);
                self.instantiate_type(name_type, appended)
            }
        };
        let mut prop_name_types: Vec<TypeId> = Vec::new();
        for_each_type(self, prop_name_type, &mut |_c, p| prop_name_types.push(p));
        for prop_name_type in prop_name_types {
            // If the current iteration type constituent is a string literal
            // type, create a property. Otherwise, for type string create a
            // string index signature (Go's comment).
            if crate::exports::is_type_usable_as_property_name(self.type_(prop_name_type)) {
                let prop_name = crate::exports::get_property_name_from_type(self.type_(prop_name_type));
                if let Some(&existing_prop) = members.get(&prop_name) {
                    // String enum members from separate enums with identical
                    // values are distinct types with the same property name.
                    // Make the resulting property symbol's name type be the
                    // union of those enum member types (Go's comment).
                    let existing_name_type = self.value_symbol_links.get(existing_prop).name_type;
                    let merged_name_type = self.get_union_type(&[existing_name_type.unwrap_or(self.never_type()), prop_name_type]);
                    self.value_symbol_links.get(existing_prop).name_type = Some(merged_name_type);
                    let existing_key_type = self.mapped_symbol_links.get(existing_prop).key_type;
                    let merged_key_type = self.get_union_type(&[existing_key_type.unwrap_or(self.never_type()), key_type]);
                    self.mapped_symbol_links.get(existing_prop).key_type = Some(merged_key_type);
                } else {
                    let mut modifiers_prop: Option<SymbolId> = None;
                    if crate::exports::is_type_usable_as_property_name(self.type_(key_type)) {
                        modifiers_prop = self.get_property_of_type_impl(
                            modifiers_type,
                            &crate::exports::get_property_name_from_type(self.type_(key_type)),
                        );
                    }
                    let modifiers_prop_flags = modifiers_prop
                        .map(|p| self.program_symbol(p).flags)
                        .unwrap_or_default();
                    let is_optional = template_modifiers
                        .intersects(crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL)
                        || (!template_modifiers
                            .intersects(crate::relater::MappedTypeModifiers::EXCLUDE_OPTIONAL)
                            && modifiers_prop_flags.intersects(SymbolFlags::OPTIONAL));
                    let is_readonly = template_modifiers
                        .intersects(crate::relater::MappedTypeModifiers::INCLUDE_READONLY)
                        || (!template_modifiers
                            .intersects(crate::relater::MappedTypeModifiers::EXCLUDE_READONLY)
                            && modifiers_prop
                                .map(|p| self.is_readonly_symbol(p))
                                .unwrap_or(false));
                    let strip_optional = self.strict_null_checks
                        && !is_optional
                        && modifiers_prop_flags.intersects(SymbolFlags::OPTIONAL);
                    let late_flag = modifiers_prop
                        .map(|p| self.program_symbol(p).check_flags & CheckFlags::LATE)
                        .unwrap_or_default();
                    let prop_flags =
                        SymbolFlags::PROPERTY | if is_optional { SymbolFlags::OPTIONAL } else { SymbolFlags::NONE };
                    let prop = self.new_symbol(prop_flags, &prop_name);
                    let mut check_flags = late_flag | CheckFlags::MAPPED;
                    if is_readonly {
                        check_flags |= CheckFlags::READONLY;
                    }
                    if strip_optional {
                        check_flags |= CheckFlags::STRIP_OPTIONAL;
                    }
                    self.program_symbol_mut(prop).check_flags = check_flags;
                    self.value_symbol_links.get(prop).containing_type = Some(t);
                    self.value_symbol_links.get(prop).name_type = Some(prop_name_type);
                    self.mapped_symbol_links.get(prop).key_type = Some(key_type);
                    if let Some(modifiers_prop) = modifiers_prop {
                        self.mapped_symbol_links.get(prop).synthetic_origin = Some(modifiers_prop);
                        if should_link_prop_declarations {
                            let declarations = self.program_symbol(modifiers_prop).declarations.clone();
                            self.program_symbol_mut(prop).declarations = declarations;
                        }
                    }
                    members.set(prop_name, prop);
                }
            } else if self.is_valid_index_key_type(prop_name_type)
                || self
                    .type_(prop_name_type)
                    .flags
                    .intersects(TypeFlags::ANY.union(TypeFlags::ENUM))
            {
                let mut index_key_type = prop_name_type;
                let prop_name_flags = self.type_(prop_name_type).flags;
                if prop_name_flags.intersects(TypeFlags::ANY.union(TypeFlags::STRING)) {
                    index_key_type = self.string_type();
                } else if prop_name_flags.intersects(TypeFlags::NUMBER.union(TypeFlags::ENUM)) {
                    index_key_type = self.number_type();
                }
                let mapper = self.type_(t).as_mapped_type().and_then(|m| m.base.mapper);
                let appended = self.append_type_mapping(mapper, type_parameter, key_type);
                let prop_type = self.instantiate_type(template_type, appended);
                let modifiers_index_info =
                    self.get_applicable_index_info_impl(modifiers_type, prop_name_type);
                let is_readonly = template_modifiers
                    .intersects(crate::relater::MappedTypeModifiers::INCLUDE_READONLY)
                    || (!template_modifiers
                        .intersects(crate::relater::MappedTypeModifiers::EXCLUDE_READONLY)
                        && modifiers_index_info
                            .map(|info| self.index_infos[info.0 as usize].is_readonly)
                            .unwrap_or(false));
                let info = IndexInfoId(
                    self.new_index_info(index_key_type, prop_type, is_readonly, None, Vec::new())
                        as u32,
                );
                *index_infos = self.append_index_info(std::mem::take(index_infos), info, true);
            }
        }
    }

    /// Go: `func (c *Checker) getLowerBoundOfKeyType(t) *Type` (checker.go:21370)
    /// — return the lower bound of the key type in a mapped type. Intuitively,
    /// the lower bound includes those keys that are known to always be present,
    /// for example because of constraints on type parameters (e.g. 'keyof T'
    /// for a constrained T).
    pub(crate) fn get_lower_bound_of_key_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::INDEX) {
            let target = self
                .type_(t)
                .as_index_type()
                .expect("IndexType data")
                .target
                .expect("IndexType target");
            let target = self.get_apparent_type_impl(target);
            if crate::relater::is_generic_tuple_type(self, target) {
                return crate::relater::get_known_keys_of_tuple_type(self, target);
            }
            return crate::relater::get_index_type(self, t);
        }
        if flags.intersects(TypeFlags::CONDITIONAL) {
            let (is_distributive, check_type, mapper) = {
                let d = self.type_(t).as_conditional_type().expect("ConditionalType data");
                let root = d.root.as_ref().expect("conditional root");
                (
                    root.is_distributive,
                    d.check_type.expect("conditional check type"),
                    d.mapper,
                )
            };
            if is_distributive {
                let constraint = self.get_lower_bound_of_key_type(check_type);
                if constraint != check_type {
                    let prepended = self.prepend_type_mapping(check_type, constraint, mapper);
                    return self.get_conditional_type_instantiation(t, Some(prepended), false, None);
                }
            }
            return t;
        }
        if flags.intersects(TypeFlags::UNION) {
            return self
                .map_type_ex(t, &mut |c, t| Some(c.get_lower_bound_of_key_type(t)), true)
                .unwrap_or(t);
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            // Similarly to getTypeFromIntersectionTypeNode, we preserve the
            // special string & {}, number & {}, and bigint & {} intersections
            // that are used to prevent subtype reduction in union types
            // (Go's comment).
            let types = self.type_(t).types().to_vec();
            if types.len() == 2
                && self
                    .type_(types[0])
                    .flags
                    .intersects(TypeFlags::STRING.union(TypeFlags::NUMBER).union(TypeFlags::BIGINT))
                && types[1] == self.empty_object_type()
            {
                return t;
            }
            let lowered: Vec<TypeId> = types
                .iter()
                .copied()
                .map(|t| self.get_lower_bound_of_key_type(t))
                .collect();
            return self.get_intersection_type(&lowered);
        }
        t
    }

    /// Go: `func (c *Checker) resolveUnionTypeMembers(t)` (checker.go:21456)
    /// — the members and properties collections are empty for union types.
    /// To get all properties of a union type use getPropertiesOfType (only the
    /// language service uses this) (Go's comment).
    fn resolve_union_type_members(&mut self, t: TypeId) {
        let constituents = self.type_(t).types().to_vec();
        let mut signature_lists: Vec<Vec<SignatureId>> = Vec::with_capacity(constituents.len());
        for &t in &constituents {
            if Some(t) == self.global_function_type {
                signature_lists.push(vec![self.unknown_signature.expect("seeded")]);
            } else {
                signature_lists.push(self.get_signatures_of_type_impl(t, SignatureKind::Call));
            }
        }
        let mut call_signatures = self.get_union_signatures(&signature_lists);
        if call_signatures.is_empty() {
            call_signatures = self.get_array_member_call_signatures(t);
        }
        let mut construct_signature_lists: Vec<Vec<SignatureId>> = Vec::with_capacity(constituents.len());
        for &t in &constituents {
            construct_signature_lists.push(self.get_signatures_of_type_impl(t, SignatureKind::Construct));
        }
        let construct_signatures = self.get_union_signatures(&construct_signature_lists);
        let index_infos = self.get_union_index_infos_impl(&constituents);
        self.set_structured_type_members(t, SymbolTable::new(), call_signatures, construct_signatures, index_infos);
    }

    /// Go: `func (c *Checker) getUnionIndexInfos(types) []*IndexInfo` (checker.go:13764).
    pub(crate) fn get_union_index_infos_impl(&mut self, types: &[TypeId]) -> Vec<IndexInfoId> {
        let Some((&first, rest)) = types.split_first() else {
            return Vec::new();
        };
        let source_infos = self.get_index_infos_of_type_impl(first);
        let mut result: Vec<IndexInfoId> = Vec::new();
        for info in source_infos {
            let index_type = self.index_info_key_type(info);
            let all_have = types.iter().all(|&t| {
                self.get_index_info_of_type_impl(t, index_type).is_some()
            });
            if all_have {
                let value_types: Vec<TypeId> = types
                    .iter()
                    .map(|&t| {
                        self.get_index_type_of_type_impl(t, index_type)
                            .unwrap_or_else(|| self.error_type())
                    })
                    .collect();
                let value_type = self.get_union_type(&value_types);
                let is_readonly = types.iter().any(|&t| {
                    self.get_index_info_of_type_impl(t, index_type)
                        .map(|info| self.index_infos[info.0 as usize].is_readonly)
                        .unwrap_or(false)
                });
                let _ = rest;
                result.push(IndexInfoId(
                    self.new_index_info(index_type, value_type, is_readonly, None, Vec::new())
                        as u32,
                ));
            }
        }
        result
    }

    /// Go: `func (c *Checker) getArrayMemberCallSignatures(t) []*Signature` (checker.go:21475).
    fn get_array_member_call_signatures(&mut self, t: TypeId) -> Vec<SignatureId> {
        // Check if union is exclusively instantiations of a member of the
        // global Array or ReadonlyArray type (Go's comment).
        let constituents = self.type_(t).types().to_vec();
        let mut member_name: Option<String> = None;
        for (i, &t) in constituents.iter().enumerate() {
            let symbol = self.type_(t).symbol;
            let parent = symbol.and_then(|s| self.program_symbol(s).parent);
            let not_candidate = !self
                .type_(t)
                .object_flags
                .intersects(ObjectFlags::INSTANTIATED)
                || symbol.is_none()
                || parent.is_none()
                || !self.is_array_or_tuple_symbol(parent);
            if not_candidate {
                return Vec::new();
            }
            let name = self.program_symbol(symbol.expect("checked")).name.clone();
            if i == 0 {
                member_name = Some(name);
            } else if member_name.as_deref() != Some(&name) {
                return Vec::new();
            }
        }
        let Some(member_name) = member_name else {
            return Vec::new();
        };
        // Transform the type from `(A[] | B[])["member"]` to
        // `(A | B)[]["member"]` (since we pretend array is covariant anyway)
        // (Go's comment).
        let array_arg = self
            .map_type(t, &mut |c, t| {
                let parent = c
                    .type_(t)
                    .symbol
                    .and_then(|s| c.program_symbol(s).parent)?;
                let readonly_array = c.is_readonly_array_symbol(Some(parent));
                let global_type = if readonly_array {
                    c.global_readonly_array_type?
                } else {
                    c.global_array_type?
                };
                let type_parameters = c
                    .type_(global_type)
                    .as_interface_type()
                    .map(|d| d.all_type_parameters.clone())?;
                let mapper = c.type_(t).mapper();
                Some(crate::mapper::get_mapped_type(c, type_parameters[0], mapper?))
            })
            .unwrap_or_else(|| self.never_type());
        let any_readonly = constituents
            .iter()
            .any(|&t| {
                let parent = self
                    .type_(t)
                    .symbol
                    .and_then(|s| self.program_symbol(s).parent);
                self.is_readonly_array_symbol(parent)
            });
        let array_type = self.create_array_type_ex(array_arg, any_readonly);
        let property_type = self.get_type_of_property_of_type_impl(array_type, &member_name);
        property_type
            .map(|property_type| self.get_signatures_of_type_impl(property_type, SignatureKind::Call))
            .unwrap_or_default()
    }

    /// Go: `func (c *Checker) isArrayOrTupleSymbol(symbol) bool`.
    fn is_array_or_tuple_symbol(&mut self, symbol: Option<SymbolId>) -> bool {
        let global_array_symbol = self.global_array_type.and_then(|t| self.type_(t).symbol);
        let global_readonly_array_symbol = self
            .global_readonly_array_type
            .and_then(|t| self.type_(t).symbol);
        match symbol {
            None => false,
            Some(symbol) => {
                global_array_symbol.is_some_and(|g| self.get_symbol_if_same_reference(symbol, g).is_some())
                    || global_readonly_array_symbol
                        .is_some_and(|g| self.get_symbol_if_same_reference(symbol, g).is_some())
            }
        }
    }

    /// Go: `func (c *Checker) isReadonlyArraySymbol(symbol) bool`.
    fn is_readonly_array_symbol(&mut self, symbol: Option<SymbolId>) -> bool {
        match symbol {
            None => false,
            Some(symbol) => {
                self.global_readonly_array_type
                    .and_then(|t| self.type_(t).symbol)
                    .is_some_and(|g| self.get_symbol_if_same_reference(symbol, g).is_some())
            }
        }
    }

    /// Go: `func (c *Checker) getUnionSignatures(signatureLists) []*Signature`
    /// (checker.go:21516) — the signatures of a union type are those
    /// signatures that are present in each of the constituent types.
    pub(crate) fn get_union_signatures(
        &mut self,
        signature_lists: &[Vec<SignatureId>],
    ) -> Vec<SignatureId> {
        let mut result: Vec<SignatureId> = Vec::new();
        let mut index_with_length_over_one = 0usize;
        let mut count_length_over_one = 0usize;
        for (i, list) in signature_lists.iter().enumerate() {
            if list.is_empty() {
                return Vec::new();
            }
            if list.len() > 1 {
                index_with_length_over_one = i;
                count_length_over_one += 1;
            }
            for &signature in list {
                // Only process signatures with parameter lists that aren't
                // already in the result list (Go's comment).
                if result.is_empty()
                    || self
                        .find_matching_signature(&result, signature, false, false, true)
                        .is_none()
                {
                    if let Some(union_signatures) =
                        self.find_matching_signatures(signature_lists, signature, i)
                    {
                        let mut s = signature;
                        // Union the result types when more than one
                        // signature matches (Go's comment).
                        if union_signatures.len() > 1 {
                            let mut this_parameter =
                                self.signatures[signature.0 as usize].this_parameter;
                            let first_this_parameter_of_union_signatures =
                                union_signatures.iter().copied().find_map(|sig| {
                                    self.signatures[sig.0 as usize].this_parameter
                                });
                            if let Some(first_this) = first_this_parameter_of_union_signatures {
                                let this_types: Vec<TypeId> = union_signatures
                                    .iter()
                                    .filter_map(|&sig| {
                                        self.signatures[sig.0 as usize]
                                            .this_parameter
                                            .map(|p| self.get_type_of_symbol(p))
                                    })
                                    .collect();
                                let this_type = self.get_intersection_type(&this_types);
                                this_parameter =
                                    Some(self.create_symbol_with_type(first_this, this_type));
                            }
                            s = self.create_union_signature(signature, union_signatures);
                            self.signatures[s.0 as usize].this_parameter = this_parameter;
                        }
                        result.push(s);
                    }
                }
            }
        }
        if result.is_empty() && count_length_over_one <= 1 {
            // No sufficiently similar signature existed to subsume all the
            // other signatures in the union - time to see if we can make a
            // single signature that handles all of them. We only do this when
            // there are overloads in only one constituent. (Overloads are
            // conditional in nature and having overloads in multiple
            // constituents would necessitate making a power set of signatures
            // from the type, whose ordering would be non-obvious) (Go's
            // comment).
            let master_list = &signature_lists[index_with_length_over_one];
            let mut results: Option<Vec<SignatureId>> = Some(master_list.clone());
            for signatures in signature_lists {
                if signatures == master_list {
                    continue;
                }
                let signature = signatures[0];
                let results_ref = results.clone().unwrap_or_default();
                let generic_conflict = !self.signatures[signature.0 as usize]
                    .type_parameters
                    .is_empty()
                    && results_ref.iter().any(|&s| {
                        !self.signatures[s.0 as usize].type_parameters.is_empty()
                            && {
                                let sig_params = self.signatures[signature.0 as usize].type_parameters.clone();
                                let s_params = self.signatures[s.0 as usize].type_parameters.clone();
                                !self.compare_type_parameters_identical(&sig_params, &s_params)
                            }
                    });
                if generic_conflict {
                    results = None;
                } else {
                    results = results.map(|results| {
                        results
                            .into_iter()
                            .map(|sig| {
                                self.combine_union_or_intersection_member_signatures(
                                    sig, signature, true, /*isUnion*/
                                )
                            })
                            .collect()
                    });
                }
                if results.is_none() {
                    break;
                }
            }
            result = results.unwrap_or_default();
        }
        result
    }

    /// Go: `func (c *Checker) createUnionSignature(sig, unionSignatures) *Signature`.
    pub(crate) fn create_union_signature(
        &mut self,
        sig: SignatureId,
        union_signatures: Vec<SignatureId>,
    ) -> SignatureId {
        let result = self.clone_signature(sig);
        let s = &mut self.signatures[result.0 as usize];
        s.composite = Some(Box::new(crate::types::CompositeSignature {
            is_union: true,
            signatures: union_signatures,
        }));
        s.target = None;
        s.mapper = None;
        result
    }

    /// Go: `func (c *Checker) combineUnionOrIntersectionMemberSignatures(left, right, isUnion) *Signature`
    /// (checker.go:21587).
    pub(crate) fn combine_union_or_intersection_member_signatures(
        &mut self,
        left: SignatureId,
        right: SignatureId,
        is_union: bool,
    ) -> SignatureId {
        let left_type_parameters = self.signatures[left.0 as usize].type_parameters.clone();
        let right_type_parameters = self.signatures[right.0 as usize].type_parameters.clone();
        let mut type_params = left_type_parameters.clone();
        if type_params.is_empty() {
            type_params = right_type_parameters.clone();
        }
        let mut param_mapper: Option<TypeMapperId> = None;
        if !left_type_parameters.is_empty() && !right_type_parameters.is_empty() {
            // We just use the type parameter defaults from the first
            // signature (Go's comment).
            param_mapper = Some(
                self.new_type_mapper(&right_type_parameters, &left_type_parameters),
            );
        }
        let mut flags = (self.signatures[left.0 as usize].flags
            | self.signatures[right.0 as usize].flags)
            & (SignatureFlags::PROPAGATING_FLAGS & !SignatureFlags::HAS_REST_PARAMETER);
        let declaration = self.signatures[left.0 as usize].declaration;
        let params = self.combine_union_or_intersection_parameters(left, right, param_mapper, is_union);
        if let Some(&last_param) = params.last() {
            if self.program_symbol(last_param)
                .check_flags
                .intersects(CheckFlags::REST_PARAMETER)
            {
                flags |= SignatureFlags::HAS_REST_PARAMETER;
            }
        }
        let left_this = self.signatures[left.0 as usize].this_parameter;
        let right_this = self.signatures[right.0 as usize].this_parameter;
        let this_param =
            self.combine_union_or_intersection_this_param(left_this, right_this, param_mapper, is_union);
        let left_min = self.signatures[left.0 as usize].min_argument_count;
        let right_min = self.signatures[right.0 as usize].min_argument_count;
        let min_arg_count = left_min.max(right_min);
        let result = self.new_signature(
            flags,
            declaration,
            type_params,
            this_param,
            params,
            None,
            min_arg_count,
        );
        let left_composite = self.signatures[left.0 as usize].composite.clone();
        let left_composite_is_union = left_composite
            .as_ref()
            .map(|c| c.is_union)
            .unwrap_or(false);
        let left_signatures: Vec<SignatureId> = match left_composite {
            Some(composite) if composite.is_union => composite.signatures,
            _ => vec![left],
        };
        let mut combined = left_signatures;
        combined.push(right);
        let result_mapper = self.signatures[result.0 as usize].mapper;
        if param_mapper.is_some() {
            if left_composite_is_union == is_union && result_mapper.is_some() {
                // Go: result.mapper = combineTypeMappers(left.mapper, paramMapper)
                // — left.mapper is the clone's source mapper (clone_signature
                // propagates it).
                let left_mapper = self.signatures[left.0 as usize].mapper;
                let combined_mapper = match left_mapper {
                    Some(left_mapper) => {
                        self.combine_type_mappers(Some(left_mapper), param_mapper)
                    }
                    None => param_mapper,
                };
                self.signatures[result.0 as usize].mapper = combined_mapper;
            } else {
                self.signatures[result.0 as usize].mapper = param_mapper;
            }
        } else if left_composite_is_union == is_union {
            self.signatures[result.0 as usize].mapper = self.signatures[left.0 as usize].mapper;
        }
        self.signatures[result.0 as usize].composite = Some(Box::new(crate::types::CompositeSignature {
            is_union,
            signatures: combined,
        }));
        result
    }

    /// Go: `func (c *Checker) combineUnionOrIntersectionParameters(left, right, mapper, isUnion) []*ast.Symbol`
    /// (checker.go:21634).
    fn combine_union_or_intersection_parameters(
        &mut self,
        left: SignatureId,
        right: SignatureId,
        mapper: Option<TypeMapperId>,
        is_union: bool,
    ) -> Vec<SymbolId> {
        let left_count = self.get_parameter_count(left);
        let right_count = self.get_parameter_count(right);
        let (longest_count, longest, shorter) = if left_count >= right_count {
            (left_count, left, right)
        } else {
            (right_count, right, left)
        };
        let either_has_effective_rest =
            self.has_effective_rest_parameter(left) || self.has_effective_rest_parameter(right);
        let needs_extra_rest_element =
            either_has_effective_rest && !self.has_effective_rest_parameter(longest);
        let mut params: Vec<SymbolId> = Vec::with_capacity(longest_count as usize + needs_extra_rest_element as usize);
        for i in 0..longest_count as usize {
            let mut longest_param_type = self
                .try_get_type_at_position(longest, i)
                .unwrap_or_else(|| self.unknown_type());
            if longest == right {
                longest_param_type = match mapper {
                    Some(mapper) => self.instantiate_type(longest_param_type, mapper),
                    None => longest_param_type,
                };
            }
            let mut shorter_param_type = self
                .try_get_type_at_position(shorter, i)
                .unwrap_or_else(|| self.unknown_type());
            if shorter == right {
                shorter_param_type = match mapper {
                    Some(mapper) => self.instantiate_type(shorter_param_type, mapper),
                    None => shorter_param_type,
                };
            }
            let combined_param_type = self.get_union_or_intersection_type(
                &[longest_param_type, shorter_param_type],
                !is_union,
            );
            let is_rest_param = either_has_effective_rest
                && !needs_extra_rest_element
                && i == longest_count as usize - 1;
            let is_optional = i as i32 >= self.get_min_argument_count(longest)
                && i as i32 >= self.get_min_argument_count(shorter);
            let left_name = if i < left_count as usize {
                Some(self.get_parameter_name_at_position(left, i))
            } else {
                None
            };
            let right_name = if i < right_count as usize {
                Some(self.get_parameter_name_at_position(right, i))
            } else {
                None
            };
            let mut param_name = match (&left_name, &right_name) {
                (Some(l), Some(r)) if l == r => left_name.clone(),
                (None, Some(r)) => Some(r.clone()),
                (Some(l), None) => Some(l.clone()),
                _ => None,
            };
            if param_name.as_deref().unwrap_or("").is_empty() {
                param_name = Some(format!("arg{i}"));
            }
            let mut flags = SymbolFlags::FUNCTION_SCOPED_VARIABLE;
            if is_optional && !is_rest_param {
                flags |= SymbolFlags::OPTIONAL;
            }
            let check_flags = if is_rest_param {
                CheckFlags::REST_PARAMETER
            } else if is_optional {
                CheckFlags::OPTIONAL_PARAMETER
            } else {
                CheckFlags::NONE
            };
            let param_symbol = self.new_symbol_ex(flags, param_name.as_deref().unwrap_or(""), check_flags);
            if is_rest_param {
                let array = self.create_array_type(combined_param_type);
                self.value_symbol_links.get(param_symbol).resolved_type = Some(array);
            } else {
                self.value_symbol_links.get(param_symbol).resolved_type = Some(combined_param_type);
            }
            params.push(param_symbol);
        }
        if needs_extra_rest_element {
            let rest_param_symbol = self.new_symbol_ex(
                SymbolFlags::FUNCTION_SCOPED_VARIABLE,
                "args",
                CheckFlags::REST_PARAMETER,
            );
            let element_type = self.get_type_at_position(shorter, longest_count as usize);
            let mut resolved = self.create_array_type(element_type);
            if shorter == right {
                if let Some(mapper) = mapper {
                    resolved = self.instantiate_type(resolved, mapper);
                }
            }
            self.value_symbol_links.get(rest_param_symbol).resolved_type = Some(resolved);
            params.push(rest_param_symbol);
        }
        params
    }

    /// Go: `func (c *Checker) combineUnionOrIntersectionThisParam(left, right, mapper, isUnion) *ast.Symbol`
    /// (checker.go:21700).
    fn combine_union_or_intersection_this_param(
        &mut self,
        left: Option<SymbolId>,
        right: Option<SymbolId>,
        mapper: Option<TypeMapperId>,
        is_union: bool,
    ) -> Option<SymbolId> {
        let (Some(left), Some(right)) = (left, right) else {
            return left.or(right);
        };
        // A signature `this` type might be a read or a write position... It's
        // very possible that it should be invariant and we should refuse to
        // merge signatures if there are `this` types and they do not match.
        // However, so as to be permissive when calling, for now, we'll
        // intersect the `this` types just like we do for param types in union
        // signatures (Go's comment).
        let left_type = self.get_type_of_symbol(left);
        let right_type = match mapper {
            Some(mapper) => {
                let rt = self.get_type_of_symbol(right);
                self.instantiate_type(rt, mapper)
            }
            None => self.get_type_of_symbol(right),
        };
        let this_type = self.get_union_or_intersection_type(&[left_type, right_type], !is_union);
        Some(self.create_symbol_with_type(left, this_type))
    }

    /// Go: `func (c *Checker) resolveIntersectionTypeMembers(t)` (checker.go:21706)
    /// — the members and properties collections are empty for intersection
    /// types. To get all properties of an intersection type use
    /// getPropertiesOfType (only the language service uses this) (Go's
    /// comment).
    fn resolve_intersection_type_members(&mut self, t: TypeId) {
        let mut call_signatures: Vec<SignatureId> = Vec::new();
        let mut construct_signatures: Vec<SignatureId> = Vec::new();
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        let types = self.type_(t).types().to_vec();
        let (mixin_flags, mixin_count) = self.find_mixins(&types);
        for (i, &t) in types.iter().enumerate() {
            // When an intersection type contains mixin constructor types, the
            // construct signatures from those types are discarded and their
            // return types are mixed into the return types of all other
            // construct signatures in the intersection type. For example,
            // the intersection type '{ new(...args: any[]) => A } & { new(s:
            // string) => B }' has a single construct signature 'new(s:
            // string) => A & B' (Go's comment).
            if !mixin_flags[i] {
                let mut signatures = self.get_signatures_of_type_impl(t, SignatureKind::Construct);
                if !signatures.is_empty() && mixin_count > 0 {
                    signatures = signatures
                        .into_iter()
                        .map(|s| {
                            let clone = self.clone_signature(s);
                            let return_type = self.get_return_type_of_signature(s);
                            let mixed = self.include_mixin_type(return_type, &types, &mixin_flags, i);
                            self.signatures[clone.0 as usize].resolved_return_type = Some(mixed);
                            clone
                        })
                        .collect();
                }
                construct_signatures = self.append_signatures(construct_signatures, signatures);
            }
            let constituent_call_signatures = self.get_signatures_of_type_impl(t, SignatureKind::Call);
            call_signatures =
                self.append_signatures(call_signatures, constituent_call_signatures);
            for info in self.get_index_infos_of_type_impl(t) {
                index_infos = self.append_index_info(index_infos, info, false);
            }
        }
        self.set_structured_type_members(t, SymbolTable::new(), call_signatures, construct_signatures, index_infos);
    }

    /// Go: `func (c *Checker) appendSignatures(signatures, newSignatures) []*Signature`
    /// (checker.go:21739).
    fn append_signatures(
        &mut self,
        mut signatures: Vec<SignatureId>,
        new_signatures: Vec<SignatureId>,
    ) -> Vec<SignatureId> {
        for sig in new_signatures {
            if signatures.is_empty()
                || signatures.iter().all(|&s| {
                    self.compare_signatures_identical(
                        s,
                        sig,
                        false, /*partialMatch*/
                        false, /*ignoreThisTypes*/
                        false, /*ignoreReturnTypes*/
                        crate::relater::TypeComparerKind::Identical,
                    ) == crate::types::Ternary::False
                })
            {
                signatures.push(sig);
            }
        }
        signatures
    }

    /// Go: `func (c *Checker) appendIndexInfo(indexInfos, newInfo, union) []*IndexInfo`
    /// (checker.go:21750).
    fn append_index_info(
        &mut self,
        mut index_infos: Vec<IndexInfoId>,
        new_info: IndexInfoId,
        union: bool,
    ) -> Vec<IndexInfoId> {
        let new_key_type = self.index_info_key_type(new_info);
        for (i, &info) in index_infos.iter().enumerate() {
            let key_type = self.index_info_key_type(info);
            if key_type == new_key_type {
                let value_type = self.index_info_value_type(info);
                let new_value_type = self.index_info_value_type(new_info);
                let (value_type, is_readonly) = if union {
                    (
                        self.get_union_type(&[value_type, new_value_type]),
                        self.index_infos[info.0 as usize].is_readonly
                            || self.index_infos[new_info.0 as usize].is_readonly,
                    )
                } else {
                    (
                        self.get_intersection_type(&[value_type, new_value_type]),
                        self.index_infos[info.0 as usize].is_readonly
                            && self.index_infos[new_info.0 as usize].is_readonly,
                    )
                };
                index_infos[i] = IndexInfoId(
                    self.new_index_info(key_type, value_type, is_readonly, None, Vec::new())
                        as u32,
                );
                return index_infos;
            }
        }
        index_infos.push(new_info);
        index_infos
    }

    /// Go: `func (c *Checker) findMixins(types) ([]bool, int)` (checker.go:21769).
    pub(crate) fn find_mixins(&mut self, types: &[TypeId]) -> (Vec<bool>, usize) {
        let mixin_flags: Vec<bool> = types
            .iter()
            .copied()
            .map(|t| self.is_mixin_constructor_type(t))
            .collect();
        let mut constructor_type_count = 0usize;
        let mut mixin_count = 0usize;
        let mut first_mixin_index: isize = -1;
        for (i, &t) in types.iter().enumerate() {
            if !self.get_signatures_of_type_impl(t, SignatureKind::Construct).is_empty() {
                constructor_type_count += 1;
            }
            if mixin_flags[i] {
                if first_mixin_index < 0 {
                    first_mixin_index = i as isize;
                }
                mixin_count += 1;
            }
        }
        let mut mixin_flags = mixin_flags;
        if constructor_type_count > 0 && constructor_type_count == mixin_count {
            mixin_flags[first_mixin_index as usize] = false;
            mixin_count -= 1;
        }
        (mixin_flags, mixin_count)
    }

    /// Go: `func (c *Checker) includeMixinType(t, types, mixinFlags, index) *Type`
    /// (checker.go:21791).
    fn include_mixin_type(
        &mut self,
        t: TypeId,
        types: &[TypeId],
        mixin_flags: &[bool],
        index: usize,
    ) -> TypeId {
        let mut mixed_types: Vec<TypeId> = Vec::new();
        for i in 0..types.len() {
            if i == index {
                mixed_types.push(t);
            } else if mixin_flags[i] {
                let first_construct = self
                    .get_signatures_of_type_impl(types[i], SignatureKind::Construct)
                    .into_iter()
                    .next();
                if let Some(first_construct) = first_construct {
                    let return_type = self.get_return_type_of_signature(first_construct);
                    mixed_types.push(return_type);
                }
            }
        }
        self.get_intersection_type(&mixed_types)
    }
}

// __S8_APPEND2__
