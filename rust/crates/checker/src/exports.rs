// Ported from tsc/internal/checker/exports.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The checker's exported (capitalized-in-Go) API surface — M6 W1. In Go,
// exports.go is a file of one-line wrappers: `func (c *Checker) GetXxx(...)
// { return c.getXxx(...) }`, delegating to checker.go/relater.go/inference.go
// internals. Two port-specific notes govern this file:
//
// COLLAPSED PAIRS (SPEC §4 naming): Go's exported `GetXxx` and the internal
// `getXxx` snake-case to the SAME Rust name. Where the internal already
// exists in the crate, the pair has collapsed onto it — exports.rs documents
// the collapse instead of redeclaring:
//   * the 13 intrinsic getters (GetStringType/GetNumberType/GetBooleanType/
//     GetVoidType/GetUndefinedType/GetNullType/GetAnyType/GetErrorType/
//     GetNeverType/GetUnknownType/GetBigIntType/GetESSymbolType/
//     GetNonPrimitiveType) → `Checker::string_type()` ... on checker.rs's
//     `intrinsic_accessors!` block (`boolean_type()` is the same collapsed
//     pair with the spike's documented M6 panic).
//   * GetTypeOfSymbol / GetNonMissingTypeOfSymbol → checker.rs
//     `get_type_of_symbol` / `get_non_missing_type_of_symbol` (the S2 slice).
//   * IsTupleType → checker.rs `is_tuple_type` (Go's free `isTupleType`
//     became a Checker method for the arena, §5.4).
//   * GetTypeFromTypeNode → checker.rs `get_type_from_type_node` (the W1
//     callee-stub body — errorType — becomes the real S5 family).
//   * IsArrayType → checker.rs `is_array_type` (the W1 callee-stub body —
//     false — becomes S7's global-array-type check).
//
// TODO-SHAPE BODIES: where the Go callee belongs to a not-yet-ported wave
// (S2–S10, relater, jsx, nodebuilder), the wrapper body is the permissive W1
// stub (obvious error/empty/identity value) with a `TODO(port M6 ...)` banner
// naming the wave that replaces it. When that wave lands its internal
// `get_xxx`, the pair collapses onto this name and the stub body is deleted.

use tsc_ast::{ModifierFlags, NodeId, SymbolFlags, SymbolId};
use tsc_diagnostics::Message;


use crate::types::{
    ContextFlags, IndexInfoId, SignatureId, SignatureKind, TypeId,
};
use crate::utilities as u;
use crate::Checker;

impl Checker {
    /// Go: `func (c *Checker) GetBaseTypeOfLiteralType(t *Type) *Type` →
    /// `c.getBaseTypeOfLiteralType(t)` (checker.go:25968 — S7's
    /// enum/template/union arms) — M6 S7: the REAL body landed in
    /// uniontypes.rs (`&mut self` — the union arm keys the literal-union
    /// base-type cache); this identity stub is retired.
    /// Go: `func (c *Checker) GetUnknownSymbol() *ast.Symbol` — non-nil after
    /// NewChecker; the `expect` mirrors Go's nil-deref contract.
    pub fn get_unknown_symbol(&self) -> SymbolId {
        self.unknown_symbol.expect("unknownSymbol")
    }

    /// Go: `func (c *Checker) GetUndefinedSymbol() *ast.Symbol`.
    pub fn get_undefined_symbol(&self) -> SymbolId {
        self.undefined_symbol.expect("undefinedSymbol")
    }

    /// Go: `func (c *Checker) GetArgumentsSymbol() *ast.Symbol`.
    pub fn get_arguments_symbol(&self) -> SymbolId {
        self.arguments_symbol.expect("argumentsSymbol")
    }

    /// Go: `func (c *Checker) GetUnknownSignature() *Signature` —
    /// `unknownSignature` is constructed in initializeChecker (M6 S7's
    /// newSignature family). Permissive W1 stub: None.
    pub fn get_unknown_signature(&self) -> Option<SignatureId> {
        // REAL (M6 S7): unknownSignature is seeded by new_checker (Go
        // checker.go:1040-1050's newSignature family).
        self.unknown_signature
    }

    // Go: `func (c *Checker) GetUnionType(types []*Type) *Type` →
    // `c.getUnionType(types)` (checker.go — S7's union construction).
    // M6 S7: the REAL interned union constructor landed in uniontypes.rs;
    // this stub is retired (in-slice callers resolve to the real method).


    /// Go: `func (c *Checker) GetNameTypeOfSymbol(symbol *ast.Symbol) *Type` —
    /// `if links := c.valueSymbolLinks.TryGet(symbol); links != nil { return
    /// links.nameType }; return nil` (real body; the link store is ported).
    pub fn get_name_type_of_symbol(&mut self, symbol: SymbolId) -> Option<TypeId> {
        self.value_symbol_links.try_get(symbol).and_then(|links| links.name_type)
    }

    /// Go: `func (c *Checker) GetGlobalSymbol(name string, meaning ast.SymbolFlags, diagnostic *diagnostics.Message) *ast.Symbol`
    /// → `c.getGlobalSymbol(...)` (checker.go — S6's global resolvers). REAL
    /// body landed in spine.rs (`get_global_symbol_real` — the resolveName
    /// walk with a nil location); this carrier delegates (W3 tranche 3).
    pub fn get_global_symbol(
        &mut self,
        name: &str,
        meaning: SymbolFlags,
        diagnostic: Option<&'static Message>,
    ) -> Option<SymbolId> {
        self.get_global_symbol_real(name, meaning, diagnostic)
    }

    /// Go: `func (c *Checker) GetMergedSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// → `c.getMergedSymbol(symbol)` (checker.go — S6's symbol-merging
    /// machinery). REAL body landed in resolution.rs; this carrier delegates.
    pub fn get_merged_symbol(&self, symbol: SymbolId) -> SymbolId {
        self.get_merged_symbol_real(symbol)
    }

    /// Go: `func (c *Checker) GetSymbolOfNode(node *ast.Node) *ast.Symbol` →
    /// `c.getSymbolOfNode(node)` (checker.go:14669 — S6). REAL body landed in
    /// resolution.rs (`get_symbol_of_node_real`); this carrier delegates.
    pub fn get_symbol_of_node(&mut self, node: NodeId) -> Option<SymbolId> {
        self.get_symbol_of_node_real(node)
    }

    /// Go: `func (c *Checker) GetSymbolOfDeclaration(node *ast.Node) *ast.Symbol`
    /// → `c.getSymbolOfDeclaration(node)` (checker.go — S6).
    pub fn get_symbol_of_declaration(&self, node: NodeId) -> Option<SymbolId> {
        // M6 S2: the real payload read (the binder-set declaration symbol);
        // the merged/late-bound composition is S6's (identity permissive).
        crate::typeofsymbol::get_symbol_of_declaration(self, node)
    }

    // Go: `func (c *Checker) GetParentOfSymbol(symbol *ast.Symbol) *ast.Symbol`
    // → `c.getParentOfSymbol(symbol)` (checker.go:14630). The REAL
    // merged+late-bound body landed in resolution.rs as `get_parent_of_symbol`
    // (S6); this twin was removed to keep a single definition.

    /// Go: `func (c *Checker) TryFindAmbientModule(moduleName string) *ast.Symbol`
    /// → `c.tryFindAmbientModule(moduleName, true /*withAugmentations*/)`
    /// (checker.go:15910 — S6's ambient-module machinery). REAL body landed
    /// in resolution.rs; this carrier delegates.
    pub fn try_find_ambient_module(&mut self, module_name: &str) -> Option<SymbolId> {
        self.try_find_ambient_module_real(module_name, true)
    }

    /// Go: `func (c *Checker) GetImmediateAliasedSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// → `c.getImmediateAliasedSymbol(symbol)` (checker.go:2199 — S6's
    /// alias-resolution family). REAL body landed in resolution.rs; this
    /// carrier delegates.
    pub fn get_immediate_aliased_symbol(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        self.get_immediate_aliased_symbol_real(symbol)
    }

    /// Go: `func (c *Checker) GetTargetSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// → `c.getTargetSymbol(symbol)` (checker.go:22087) — S8's REAL body
    /// (members.rs) corrected the S6-era merged+aliased walk to Go's
    /// instantiation-target walk.
    pub fn get_target_symbol(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        Some(self.get_target_symbol_impl(symbol))
    }

    /// Go: `func (c *Checker) GetTypeOnlyAliasDeclaration(symbol *ast.Symbol) *ast.Node`
    /// → `c.getTypeOnlyAliasDeclaration(symbol)` (checker.go — S6). REAL
    /// body landed in resolution.rs; this carrier delegates.
    pub fn get_type_only_alias_declaration(&mut self, symbol: SymbolId) -> Option<NodeId> {
        self.get_type_only_alias_declaration_real(symbol)
    }

    /// Go: `func (c *Checker) ResolveExternalModuleName(moduleSpecifier *ast.Node, importAttributesType *Type) *ast.Symbol`
    /// → `c.resolveExternalModuleName(moduleSpecifier, moduleSpecifier,
    /// true /*ignoreErrors*/, importAttributesType)` (checker.go — S6's
    /// module resolution). REAL body landed in resolution.rs; this carrier
    /// delegates.
    pub fn resolve_external_module_name(
        &mut self,
        module_specifier: NodeId,
        import_attributes_type: Option<TypeId>,
    ) -> Option<SymbolId> {
        self.resolve_external_module_name_ex(
            module_specifier,
            Some(module_specifier),
            true,
            import_attributes_type,
        )
    }

    /// Go: `func (c *Checker) ResolveExternalModuleSymbol(moduleSymbol *ast.Symbol) *ast.Symbol`
    /// → `c.resolveExternalModuleSymbol(moduleSymbol, false /*dontResolveAlias*/)`
    /// (checker.go — S6). REAL body landed in resolution.rs; this carrier
    /// delegates.
    pub fn resolve_external_module_symbol(&mut self, module_symbol: SymbolId) -> Option<SymbolId> {
        self.resolve_external_module_symbol_ex(Some(module_symbol), false)
    }

    /// Go: `func (c *Checker) GetTypeFromTypeNode(node *ast.Node) *Type` →
    /// `c.getTypeFromTypeNode(node)` (checker.go:23295 — S5's type-node
    /// family). COLLAPSED PAIR: the W1 stub body (errorType) lives in
    /// checker.rs's "W1 callee stubs" section — the S5 wave replaces it.
    ///
    /// Go: `func (c *Checker) IsArrayType(t *Type) bool` → `c.isArrayType(t)`
    /// (checker.go:24005 — S7's global array types) is likewise collapsed
    /// onto checker.rs's stub (false until S7).
    ///
    /// Go: `func (c *Checker) IsArrayLikeType(t *Type) bool` →
    /// `c.isArrayLikeType(t)` (checker.go:24049 — needs global array types +
    /// the relater, S7/S10). REAL (FCS): delegates to typenodes's Go-exact
    /// body — the W1-era `false` stub flipped the iteration fallback (the
    /// 2495 "not an array type" family) for every array-like probe in a
    /// lib-less global table ({} stays array-like: assignable to the
    /// missing-ReadonlyArray sentinel, Go's emptyObjectType).
    pub fn is_array_like_type(&mut self, t: TypeId) -> bool {
        crate::typenodes::is_array_like_type(self, t)
    }

    /// Go: `func (c *Checker) GetPropertiesOfType(t *Type) []*ast.Symbol` →
    /// `c.getPropertiesOfType(t)` (checker.go — S8's member resolution).
    pub fn get_properties_of_type(&mut self, t: TypeId) -> Vec<SymbolId> {
        self.get_properties_of_type_impl(t)
    }

    /// Go: `func (c *Checker) GetPropertyOfType(t *Type, name string) *ast.Symbol`
    /// → `c.getPropertyOfType(t, name)` (checker.go — S8).
    pub fn get_property_of_type(&mut self, t: TypeId, name: &str) -> Option<SymbolId> {
        self.get_property_of_type_impl(t, name)
    }

    /// Go: `func (c *Checker) TypeHasCallOrConstructSignatures(t *Type) bool`
    /// → `c.typeHasCallOrConstructSignatures(t)` (checker.go — S8).
    pub fn type_has_call_or_construct_signatures(&mut self, t: TypeId) -> bool {
        crate::relater::type_has_call_or_construct_signatures(self, t)
    }

    // Checks if a property can be accessed in a location.
    // The location is given by the `node` parameter.
    // The node does not need to be a property access.
    // @param node location where to check property accessibility
    // @param isSuper whether to consider this a `super` property access, e.g. `super.foo`.
    // @param isWrite whether this is a write access, e.g. `++foo.x`.
    // @param containingType type where the property comes from.
    // @param property property symbol.
    /// Go: `func (c *Checker) IsPropertyAccessible(node *ast.Node, isSuper bool, isWrite bool, containingType *Type, property *ast.Symbol) bool`
    /// → `c.isPropertyAccessible(...)` (symbolaccessibility.go — S4's wave).
    #[allow(clippy::too_many_arguments)]
    pub fn is_property_accessible(
        &mut self,
        node: NodeId,
        is_super: bool,
        is_write: bool,
        containing_type: TypeId,
        property: SymbolId,
    ) -> bool {
        // TODO(port M6 S4): the symbolaccessibility.go port.
        let _ = (node, is_super, is_write, containing_type, property);
        false
    }

    /// Go: `func (c *Checker) GetTypeOfPropertyOfContextualType(t *Type, name string) *Type`
    /// → `c.getTypeOfPropertyOfContextualType(t, name)` (checker.go — S9's
    /// contextual-typing family).
    pub fn get_type_of_property_of_contextual_type(
        &mut self,
        t: TypeId,
        name: &str,
    ) -> Option<TypeId> {
        self.get_type_of_property_of_contextual_type_impl(t, name)
    }

    /// Go: `func (c *Checker) WasCanceled() bool` (real body — the field).
    pub fn was_canceled(&self) -> bool {
        self.was_canceled
    }

    /// Go: `func (c *Checker) GetSignaturesOfType(t *Type, kind SignatureKind) []*Signature`
    /// → `c.getSignaturesOfType(t, kind)` (checker.go — S8).
    pub fn get_signatures_of_type(&mut self, t: TypeId, kind: SignatureKind) -> Vec<SignatureId> {
        self.get_signatures_of_type_impl(t, kind)
    }

    /// Go: `func (c *Checker) GetDeclaredTypeOfSymbol(symbol *ast.Symbol) *Type`
    /// → `c.getDeclaredTypeOfSymbol(symbol)` (checker.go — S2's declared-type
    /// family). PORT (M6 S2): the W1 error-type stub flipped to the real
    /// family (typeofsymbol.rs's tryGetDeclaredTypeOfSymbol chain).
    pub fn get_declared_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        self.try_get_declared_type_of_symbol(symbol)
            .unwrap_or_else(|| self.error_type())
    }

    /// Go: `func (c *Checker) GetConstraintOfTypeParameter(typeParameter *Type) *Type`
    /// → `c.getConstraintOfTypeParameter(typeParameter)` (checker.go — S2).
    /// PORT (M6 S2): the W1 nil stub flipped to the real constraint family
    /// (typeofsymbol.rs — the noConstraintType sentinel reads as None, Go's
    /// Go-API answer for an unconstrained parameter).
    pub fn get_constraint_of_type_parameter(&mut self, type_parameter: TypeId) -> Option<TypeId> {
        self.get_constraint_of_type_parameter_impl(type_parameter)
    }

    /// Go: `func (c *Checker) GetTrueTypeOfConditionalType(t *Type) *Type` →
    /// `c.getTrueTypeFromConditionalType(t)` (checker.go — S9; nil when the
    /// conditional has no true branch yet). PORT (M6 S9): flipped to the
    /// REAL lazily-resolving body (typeofsymbol.rs, checker.go:25073). The
    /// family is only ever called on conditional types by contract (Go
    /// nil-derefs otherwise); the port's flags guard keeps the
    /// non-conditional answer None.
    pub fn get_true_type_of_conditional_type(&mut self, t: TypeId) -> Option<TypeId> {
        if !self
            .type_(t)
            .flags
            .intersects(crate::types::TypeFlags::CONDITIONAL)
        {
            return None;
        }
        Some(self.get_true_type_from_conditional_type_impl(t))
    }

    /// Go: `func (c *Checker) GetFalseTypeOfConditionalType(t *Type) *Type` →
    /// `c.getFalseTypeFromConditionalType(t)` (checker.go — S5). PORT (M6 S2 →
    /// S9): the payload read flipped to the REAL lazily-resolving body
    /// (typeofsymbol.rs, checker.go:25081); same flags guard as the true
    /// branch twin.
    pub fn get_false_type_of_conditional_type(&mut self, t: TypeId) -> Option<TypeId> {
        if !self
            .type_(t)
            .flags
            .intersects(crate::types::TypeFlags::CONDITIONAL)
        {
            return None;
        }
        Some(self.get_false_type_from_conditional_type_impl(t))
    }

    // (M6 S5): `GetDefaultFromTypeParameter` MOVED — the real body
    // (checker.go:22446) lives in typenodes.rs as
    // `get_default_from_type_parameter` (the resolved-default machinery
    // over the resolvingDefaultType/circularConstraintType sentinels).

    /// Go: `func (c *Checker) GetEffectiveDeclarationFlags(n *ast.Node, flagsToCheck ast.ModifierFlags) ast.ModifierFlags`
    /// → `c.getEffectiveDeclarationFlags(n, flagsToCheck)` (checker.go — S8).
    pub fn get_effective_declaration_flags(
        &self,
        n: NodeId,
        flags_to_check: ModifierFlags,
    ) -> ModifierFlags {
        // PORT (M6 S3): routes to the real body (checker.go:3736, ambient
        // + JS-file handling) that landed with statements.rs.
        self.get_effective_declaration_flags_real(n, flags_to_check)
    }

    /// Go: `func (c *Checker) GetBaseConstraintOfType(t *Type) *Type` →
    /// `c.getBaseConstraintOfType(t)` (checker.go — S9's constraint
    /// resolution; nil for unconstrained types). PORT (M6 S9): flipped to
    /// the REAL resolved-base-constraint engine (typeofsymbol.rs).
    pub fn get_base_constraint_of_type(&mut self, t: TypeId) -> Option<TypeId> {
        self.get_base_constraint_of_type_impl(t)
    }

    /// Go: `func (c *Checker) IsTupleTypeTarget(t *Type) bool` →
    /// `isTupleType(t) && t.Target() == t` (real body — `is_tuple_type` is the
    /// collapsed Go pair; the target check needs the arena payload).
    pub fn is_tuple_type_target(&self, t: TypeId) -> bool {
        self.is_tuple_type(t) && self.type_(t).target() == Some(t)
    }

    /// Go: `func (c *Checker) IsReadonlySymbol(symbol *ast.Symbol) bool` →
    /// `c.isReadonlySymbol(symbol)` (checker.go — M6 S4 tranche 2: the real
    /// body lives in relater.rs as the crate carrier).
    pub fn is_readonly_symbol(&mut self, symbol: SymbolId) -> bool {
        crate::relater::is_readonly_symbol(self, symbol)
    }

    /// Go: `func (c *Checker) GetReturnTypeOfSignature(sig *Signature) *Type`
    /// → `c.getReturnTypeOfSignature(sig)` (checker.go — S8; unresolved
    /// bodies resolve to errorType in Go).
    pub fn get_return_type_of_signature(&mut self, sig: SignatureId) -> TypeId {
        self.get_return_type_of_signature_impl(sig)
    }

    /// Go: `func (c *Checker) HasEffectiveRestParameter(signature *Signature) bool`
    /// → `c.hasEffectiveRestParameter(signature)` (checker.go — S8) —
    /// W2: the real body landed with relater.rs.
    ///
    /// Go: `func (c *Checker) IsTypeAssignableTo(source *Type, target *Type) bool`
    /// → `c.isTypeAssignableTo(source, target)` (relater.go — M6 wave 2) —
    /// W2: the real body landed with relater.rs.
    /// Go: `func (c *Checker) GetLocalTypeParametersOfClassOrInterfaceOrTypeAlias(symbol *ast.Symbol) []*Type`
    /// → `c.getLocalTypeParametersOfClassOrInterfaceOrTypeAlias(symbol)`
    /// (checker.go — S2/S8). PORT (M6 S2): the W1 empty-vec stub flipped to
    /// the real declared-type read (typeofsymbol.rs).
    pub fn get_local_type_parameters_of_class_or_interface_or_type_alias(
        &mut self,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        self.get_local_type_parameters_of_class_or_interface_or_type_alias_impl(symbol)
    }

    /// Go: `func (c *Checker) GetContextualTypeForObjectLiteralElement(element *ast.Node, contextFlags ContextFlags) *Type`
    /// → `c.getContextualTypeForObjectLiteralElement(element, contextFlags)`
    /// (checker.go — S9's contextual typing).
    pub fn get_contextual_type_for_object_literal_element(
        &mut self,
        element: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        self.get_contextual_type_for_object_literal_element_impl(element, context_flags)
    }

    /// Go: `func (c *Checker) TypePredicateToString(t *TypePredicate) string`
    /// → `c.typePredicateToString(t)` (nodebuilderimpl.go — M6 W4 printer).
    pub fn type_predicate_to_string(&mut self, t: &crate::types::TypePredicate) -> String {
        crate::nodebuilder::type_predicate_to_string(self, t)
    }

    /// Go: `func (c *Checker) GetExpandedParameters(signature *Signature, skipUnionExpanding bool) [][]*ast.Symbol`
    /// → `c.getExpandedParameters(signature, skipUnionExpanding)`
    /// (checker.go — S8's parameter union expansion).
    pub fn get_expanded_parameters(
        &mut self,
        signature: SignatureId,
        skip_union_expanding: bool,
    ) -> Vec<Vec<SymbolId>> {
        // TODO(port M6 W4): getExpandedParameters (nodebuilderimpl.go:1985)
        // — the union-parameter cross product.
        let _ = (signature, skip_union_expanding);
        Vec::new()
    }

    /// Go: `func (c *Checker) GetResolvedSignature(node *ast.Node) *Signature`
    /// → `c.getResolvedSignature(node, nil, CheckModeNormal)` (checker.go —
    /// S6's overload resolution).
    pub fn get_resolved_signature(&mut self, node: NodeId) -> Option<SignatureId> {
        // Go: GetResolvedSignature → getResolvedSignature(node, nil,
        // CheckModeNormal) — resolvecall.rs's real body.
        self.get_resolved_signature_ex(node, None, crate::checker::CheckMode::CHECK_MODE_NORMAL)
    }

    // Return the type of the given property in the given type, or nil if no such property exists
    /// Go: `func (c *Checker) GetTypeOfPropertyOfType(t *Type, name string) *Type`
    /// → `c.getTypeOfPropertyOfType(t, name)` (checker.go — S8).
    pub fn get_type_of_property_of_type(&mut self, t: TypeId, name: &str) -> Option<TypeId> {
        self.get_type_of_property_of_type_impl(t, name)
    }

    /// Go: `func (c *Checker) GetContextualTypeForArgumentAtIndex(node *ast.Node, argIndex int) *Type`
    /// → `c.getContextualTypeForArgumentAtIndex(node, argIndex)` (checker.go — S9).
    pub fn get_contextual_type_for_argument_at_index(
        &mut self,
        node: NodeId,
        arg_index: usize,
    ) -> Option<TypeId> {
        self.get_contextual_type_for_argument_at_index_impl(node, arg_index)
    }

    /// Go: `func (c *Checker) GetAwaitedType(t *Type) *Type` →
    /// `c.getAwaitedType(t)` (checker.go — S9/S10's awaited-type caches).
    /// PORT (M6 S9): the identity stub flipped to the REAL awaited family
    /// (statements.rs, checker.go 31802+ — identity is the answer for
    /// non-thenables, which the lib-less scaffold always produces).
    pub fn get_awaited_type(&mut self, t: TypeId) -> TypeId {
        self.get_awaited_type_impl(t)
    }

    /// Go: `func (c *Checker) GetIndexSignaturesAtLocation(node *ast.Node) []*ast.Node`
    /// → `c.getIndexSignaturesAtLocation(node)` (checker.go:32317) — REAL
    /// (M6 S10): the location's apparent-type index signatures (the
    /// property-access name arm over getApplicableIndexInfos).
    pub fn get_index_signatures_at_location(&mut self, node: NodeId) -> Vec<NodeId> {
        let mut signatures: Vec<NodeId> = Vec::new();
        let is_property_access_name = {
            let store = self.store();
            if store.node(node).kind == tsc_ast::Kind::Identifier {
                let parent = store.node(node).parent.get();
                store.node(parent).kind == tsc_ast::Kind::PropertyAccessExpression
                    && store.node(parent).name() == Some(node)
            } else {
                false
            }
        };
        if is_property_access_name {
            let key_type = self.get_literal_type_from_property_name(node);
            let (object_type,) = {
                let store = self.store();
                let parent = store.node(node).parent.get();
                let expression = store
                    .node(parent)
                    .as_property_access_expression()
                    .expect("PropertyAccessExpression data")
                    .expression;
                let object_type = self.get_type_of_expression(expression);
                (object_type,)
            };
            // Go: `objectType.Distributed()`.
            let constituents: Vec<TypeId> =
                if self.type_(object_type).flags.intersects(crate::types::TypeFlags::UNION) {
                    self.type_(object_type).types().to_vec()
                } else {
                    vec![object_type]
                };
            for t in constituents {
                for info in self.get_applicable_index_infos_impl(t, key_type) {
                    let declaration = self.index_infos[info.0 as usize].declaration;
                    if let Some(declaration) = declaration
                        && !signatures.contains(&declaration)
                    {
                        signatures.push(declaration);
                    }
                }
            }
        }
        signatures
    }

    /// Go: `func (c *Checker) getResolvedSymbol(node *ast.Node) *ast.Symbol`
    /// (checker.go:14161). REAL caching body over the S6 resolveName seam:
    /// the name resolution itself (resolveName's scope walk) is S6's, so the
    /// seam answers None (Go's unknownSymbol). PORT: the crate's Option
    /// convention keeps None as the unknownSymbol answer (the Go callers
    /// compare against `c.unknownSymbol`).
    pub fn get_resolved_symbol(&mut self, node: NodeId) -> Option<SymbolId> {
        if self.symbol_node_links.get(node).resolved_symbol.is_none() {
            let mut symbol = None;
            if !tsc_ast::node_is_missing(self.store().node(node)) {
                // M6 S6: the REAL resolveName scope walk with Go's exact
                // arguments — the cannot-find-name diagnostic (TS2304/TS2769)
                // and the isUse probe.
                let name_not_found_message =
                    self.get_cannot_find_name_diagnostic_for_name(node);
                let is_use = !crate::expressions::is_write_only_access(self.store(), node);
                let name = tsc_ast::node_text(self.store(), node).to_string();
                symbol = self.resolve_name_worker(
                    Some(node),
                    &name,
                    SymbolFlags::VALUE.union(SymbolFlags::EXPORT_VALUE),
                    Some(name_not_found_message),
                    is_use,
                    false, /*excludeGlobals*/
                );
            }
            self.symbol_node_links.get(node).resolved_symbol = symbol;
        }
        self.symbol_node_links.get(node).resolved_symbol
    }

    /// Go: `func (c *Checker) GetJsxNamespace(location *ast.Node) string` →
    /// `c.getJsxNamespace(location)` (jsx.go:1404) — REAL (M6 W5).
    pub fn get_jsx_namespace(&mut self, location: NodeId) -> String {
        self.get_jsx_namespace_string(Some(location))
    }

    /// Go: `func (c *Checker) GetJsxFragmentFactory(location *ast.Node) string` —
    /// `entity := c.getJsxFragmentFactoryEntity(location);
    /// if entity != nil { return ast.GetFirstIdentifier(entity).Text() }; return ""`
    /// (jsx.go:1476) — REAL (M6 W5).
    pub fn get_jsx_fragment_factory(&mut self, location: NodeId) -> String {
        match self.get_jsx_fragment_factory_entity(Some(location)) {
            Some(entity) => {
                let mut node = entity;
                loop {
                    let n = self.store().node(node);
                    if n.kind == tsc_ast::Kind::QualifiedName {
                        node = n.as_qualified_name().expect("data").left;
                    } else {
                        return tsc_ast::node_text(self.store(), node).to_string();
                    }
                }
            }
            None => String::new(),
        }
    }

    /// Go: `func (c *Checker) ResolveName(name string, location *ast.Node, meaning ast.SymbolFlags, excludeGlobals bool) *ast.Symbol`
    /// → `c.resolveName(location, name, meaning, nil, true, excludeGlobals)`
    /// (checker.go — S6's name resolution). REAL scope walk landed in
    /// resolution.rs (`resolve_name_worker`); this carrier delegates.
    pub fn resolve_name(
        &mut self,
        name: &str,
        location: NodeId,
        meaning: SymbolFlags,
        exclude_globals: bool,
    ) -> Option<SymbolId> {
        self.resolve_name_worker(Some(location), name, meaning, None, true, exclude_globals)
    }

    /// Go: `func (c *Checker) GetSymbolFlags(symbol *ast.Symbol) ast.SymbolFlags` →
    /// `c.getSymbolFlags(symbol)` (checker.go:16749 — S6's alias-walking
    /// flag computation). REAL body: typeofsymbol.rs's getSymbolFlagsEx
    /// with both excludes off.
    pub fn get_symbol_flags(&mut self, symbol: SymbolId) -> SymbolFlags {
        self.get_symbol_flags_ex(symbol, false, false)
    }

    /// Go: `func (c *Checker) GetBaseTypes(t *Type) []*Type` →
    /// `c.getBaseTypes(t)` (checker.go — S8's base-type resolution).
    pub fn get_base_types(&mut self, t: TypeId) -> Vec<TypeId> {
        self.get_base_types_impl(t)
    }

    /// Go: `func (c *Checker) GetApparentType(t *Type) *Type` →
    /// `c.getApparentType(t)` (checker.go — S8/S9). Identity is the
    /// permissive value (Go returns `t` for primitive/already-apparent types).
    pub fn get_apparent_type(&mut self, t: TypeId) -> TypeId {
        self.get_apparent_type_impl(t)
    }

    // Go: `func (c *Checker) GetReducedType(t *Type) *Type` →
    // `c.getReducedType(t)` — M6 S7: the REAL body landed in uniontypes.rs
    // (the reducible-intersection reduction; the S9 conditional arm is
    // bannered there); this identity stub is retired.

    // GetFullyQualifiedName returns the fully qualified name of a symbol, walking up
    // its parent chain (e.g. `"/path/to/module".Namespace.Name`).
    /// Go: `func (c *Checker) GetFullyQualifiedName(symbol *ast.Symbol) string` →
    /// `c.getFullyQualifiedName(symbol, nil /*containingLocation*/)`
    /// (checker.go — S6). REAL parent-chain walk; the SymbolFormatFlags of
    /// Go's symbolToStringEx ride the relater's plain symbolToString.
    pub fn get_fully_qualified_name(&mut self, symbol: SymbolId) -> String {
        if let Some(parent) = self.program_symbol(symbol).parent {
            let parent_name = self.get_fully_qualified_name(parent);
            let own = crate::relater::symbol_to_string(self, symbol);
            return format!("{parent_name}.{own}");
        }
        crate::relater::symbol_to_string(self, symbol)
    }

    /// Go: `func (c *Checker) GetBaseConstructorTypeOfClass(t *Type) *Type` →
    /// `c.getBaseConstructorTypeOfClass(t)` (checker.go — S2). PORT (M6 S2):
    /// the W1 error-type stub flipped to the real base-constructor resolution
    /// (typeofsymbol.rs — the ResolvedBaseConstructorType resolution-stack
    /// axis + the circularity diagnostics).
    pub fn get_base_constructor_type_of_class(&mut self, t: TypeId) -> TypeId {
        self.get_base_constructor_type_of_class_impl(t)
    }

    /// Go: `func (c *Checker) GetMemberOverrideModifierStatus(...)` — the
    /// real body landed with M6 S3's class-member checks
    /// (statements.rs::get_member_override_modifier_status).
    ///
    /// Go: `func (c *Checker) GetRestTypeOfSignature(sig *Signature) *Type` →
    /// `c.getRestTypeOfSignature(sig)` (checker.go — S8).
    pub fn get_rest_type_of_signature(&mut self, sig: SignatureId) -> Option<TypeId> {
        // TODO(port M6 S8): the instantiated rest parameter's type.
        let _ = sig;
        None
    }

    /// Go: `func (c *Checker) GetTypeArguments(t *Type) []*Type` →
    /// `c.getTypeArguments(t)` (checker.go — S7/S8).
    pub fn get_type_arguments(&mut self, t: TypeId) -> Vec<TypeId> {
        // PORT(FCS tranche 3): Go's getTypeArguments is the RESOLVING form
        // (checker.go:22352) — relater.rs's resolve_type_arguments.
        crate::relater::resolve_type_arguments(self, t)
    }

    /// Go: `func (c *Checker) GetIndexInfoOfType(t *Type, keyType *Type) *IndexInfo`
    /// → `c.getIndexInfoOfType(t, keyType)` (checker.go — S10).
    pub fn get_index_info_of_type(&mut self, t: TypeId, key_type: TypeId) -> Option<IndexInfoId> {
        self.get_index_info_of_type_impl(t, key_type)
    }

    /// Go: `func (c *Checker) GetIndexTypeOfType(t *Type, keyType *Type) *Type` →
    /// `c.getIndexTypeOfType(t, keyType)` (checker.go — S10).
    pub fn get_index_type_of_type(&mut self, t: TypeId, key_type: TypeId) -> Option<TypeId> {
        self.get_index_type_of_type_impl(t, key_type)
    }

    /// Go: `func (c *Checker) GetIndexInfosOfType(t *Type) []*IndexInfo` →
    /// `c.getIndexInfosOfType(t)` (checker.go — S10).
    pub fn get_index_infos_of_type(&mut self, t: TypeId) -> Vec<IndexInfoId> {
        self.get_index_infos_of_type_impl(t)
    }

    /// Go: `func (c *Checker) IsContextSensitive(node *ast.Node) bool` →
    /// `c.isContextSensitive(node)` (checker.go — S9).
    pub fn is_context_sensitive(&mut self, node: NodeId) -> bool {
        self.is_context_sensitive_impl(node)
    }

    // (M6 S5): `FillMissingTypeArguments`/`GetMinTypeArgumentCount` MOVED —
    // the REAL bodies (the defaults-aware machinery, checker.go:22392–22510)
    // live in typenodes.rs as `fill_missing_type_arguments` /
    // `get_min_type_argument_count` (same signatures; the pub API surface is
    // preserved through them). `GetDefaultFromTypeParameter` likewise MOVED
    // (typenodes.rs `get_default_from_type_parameter` — the
    // resolvingDefaultType/circularConstraintType sentinel machinery).

    // Go: `func (c *Checker) GetWidenedLiteralType(t *Type) *Type` →
    // `c.getWidenedLiteralType(t)` (checker.go — S9). COLLAPSED PAIR: the
    // spike already ported the literal-widening slice as checker.rs's
    // `get_widened_literal_type` — that is this wrapper's target; the S9
    // wave completes it.
    //
    // Go: `func (c *Checker) GetWidenedType(t *Type) *Type` →
    // `c.getWidenedType(t)` (checker.go — S9) is likewise collapsed onto
    // checker.rs's `get_widened_type`.
    //
    // Go: `func (c *Checker) IsTypeAssignableTo(source *Type, target *Type) bool`
    // → `c.isTypeAssignableTo(source, target)` (relater.go — M6 wave 2) —
    // W2: the real body landed with relater.rs.
    // Go: `func (c *Checker) GetUnionTypeEx(types []*Type, unionReduction UnionReduction) *Type`
    // → `c.getUnionTypeEx(types, unionReduction, nil, nil)` (checker.go — S7).
    // M6 S7: the REAL constructor landed in uniontypes.rs; this stub is
    // retired (callers use the 4-arg `get_union_type_ex` form).


    /// Go: `func (c *Checker) RequiresAddingImplicitUndefined(node *ast.Node) bool` —
    /// `enclosingDeclaration := ast.FindAncestor(node, ast.IsDeclaration);
    /// if enclosingDeclaration == nil { enclosingDeclaration =
    /// ast.GetSourceFileOfNode(node).AsNode() }; symbol := node.Symbol();
    /// if symbol == nil { return false }; return c.requiresAddingImplicitUndefined(...)`
    /// (checker.go — S9). False is the permissive value.
    pub fn requires_adding_implicit_undefined(&mut self, node: NodeId) -> bool {
        // TODO(port M6 S9): the optional-undefined declaration analysis.
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) RemoveMissingOrUndefinedType(t *Type) *Type` →
    /// `c.removeMissingOrUndefinedType(t)` (checker.go:29616) — REAL (M6 S10):
    /// delegates to facts.rs's body (the missing-type filter under
    /// exactOptionalPropertyTypes, the NEUndefined facts filter otherwise).
    pub fn remove_missing_or_undefined_type(&mut self, t: TypeId) -> TypeId {
        self.remove_missing_or_undefined_type_impl(t)
    }

    /// Go: `func (c *Checker) GetWidenedType(t *Type) *Type` →
    /// `c.getWidenedType(t)` (checker.go — S9). COLLAPSED PAIR: the spike
    /// already ported the entry as checker.rs's `get_widened_type` (the
    /// requires-widening arm panics with its M6 banner until S9); that is
    /// this wrapper's target.
    ///
    /// Go: `func (c *Checker) CompareSymbols(s1, s2 *ast.Symbol) int` →
    /// `c.compareSymbols(s1, s2)` — the `compareSymbols` STRUCT FIELD Go
    /// assigns in NewChecker (`c.compareSymbols = c.compareSymbolsWorker`,
    /// the "Closure optimization"); the port calls the worker directly.
    pub fn compare_symbols(&self, s1: SymbolId, s2: SymbolId) -> std::cmp::Ordering {
        let store = self.program.node_store();
        u::compare_symbols_worker(self, store, s1, s2)
    }
}

/// Go: `func IsTypeUsableAsPropertyName(t *Type) bool` → free
/// `isTypeUsableAsPropertyName` (utilities.go — real, ported).
pub fn is_type_usable_as_property_name(t: &crate::types::Type) -> bool {
    u::is_type_usable_as_property_name(t)
}

/// Go: `func GetPropertyNameFromType(t *Type) string` → free
/// `getPropertyNameFromType` (utilities.go — real, ported).
pub fn get_property_name_from_type(t: &crate::types::Type) -> String {
    u::get_property_name_from_type(t)
}

/// Go: `func GetDeclarationModifierFlagsFromSymbol(s *ast.Symbol) ast.ModifierFlags`
/// → free `getDeclarationModifierFlagsFromSymbol` (utilities.go — real,
/// ported).
pub fn get_declaration_modifier_flags_from_symbol(c: &Checker, s: SymbolId) -> ModifierFlags {
    u::get_declaration_modifier_flags_from_symbol(c, c.program.node_store(), s)
}

/// Go: `func IsDistributedTypeParameter(t *Type) bool` →
/// `t.flags&TypeFlagsTypeParameter != 0 && t.AsTypeParameter().isDistributed`
/// (real body; the arena makes it take the checker, §5.4).
pub fn is_distributed_type_parameter(c: &Checker, t: TypeId) -> bool {
    let ty = c.type_(t);
    ty.flags.intersects(crate::types::TypeFlags::TYPE_PARAMETER)
        && ty
            .as_type_parameter()
            .expect("TypeParameter data")
            .is_distributed
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (Go has no exports_test.go; these pin the W1 surface: the real
// bodies against the checker's own state, and the stub bodies' permissive
// contracts so later waves notice when they flip a wrapper to its real callee).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn new_checker() -> Checker {
        crate::mapper::tests_support::new_empty_checker()
    }

    #[test]
    fn symbol_and_cancellation_accessors_read_the_checker_fields() {
        let c = new_checker();
        // The four NewChecker-seeded symbols (Go: non-nil after NewChecker).
        assert_eq!(c.get_unknown_symbol(), c.unknown_symbol.expect("seeded"));
        assert_eq!(c.get_undefined_symbol(), c.undefined_symbol.expect("seeded"));
        assert_eq!(c.get_arguments_symbol(), c.arguments_symbol.expect("seeded"));
        // WasCanceled reads the field (Go: set by the cancellation context).
        assert!(!c.was_canceled());
        // unknownSignature is REAL (M6 S7): seeded by new_checker's
        // newSignature family; the accessor reads the field.
        assert_eq!(c.get_unknown_signature(), c.unknown_signature);
    }

    #[test]
    fn name_type_of_symbol_reads_the_value_links() {
        let mut c = new_checker();
        let symbol = c.get_unknown_symbol();
        // No links yet: TryGet misses, Go returns nil.
        assert_eq!(c.get_name_type_of_symbol(symbol), None);
    }

    #[test]
    fn permissive_stub_bodies_return_the_documented_values() {
        let mut c = new_checker();
        let t = c.string_type();
        let sym = c.get_unknown_symbol();
        let node = NodeId::new(0, 1); // never dereferenced by the stubs
        // Identity stubs.
        assert_eq!(c.get_base_type_of_literal_type(t), t);
        assert_eq!(c.get_awaited_type(t), t);
        // M6 S8 flipped getApparentType to the REAL body (members.rs): the
        // string-like arm maps to the global String wrapper type (Go's
        // apparentType), which the empty checker seeds as errorType (the
        // getGlobalType report-errors answer) — the identity pin is retired.
        assert_eq!(c.get_apparent_type(t), c.global_string_type.expect("global resolver seeded"));
        assert_eq!(c.get_reduced_type(t), t);
        assert_eq!(c.get_widened_literal_type(t), t);
        assert_eq!(c.get_widened_type(t), t);
        assert_eq!(c.remove_missing_or_undefined_type(t), t);
        // M6 S7 flipped the union pair to the REAL constructors
        // (uniontypes.rs): a single-constituent list returns the constituent
        // (Go's `len(types) == 1` arm), and a two-constituent list returns
        // an interned union with the UNION flag set (no longer errorType).
        assert_eq!(c.get_union_type(&[t]), t);
        let string_t = c.string_type();
        let number_t = c.number_type();
        let union = c.get_union_type(&[string_t, number_t]);
        assert!(c.type_(union).flags.intersects(crate::types::TypeFlags::UNION));
        assert_eq!(c.get_union_type_ex(vec![t], crate::todo_shapes::UnionReduction::Literal, None, None), t);
        // M6 S2 flipped the declared-type twin to the real family
        // (typeofsymbol.rs): the unknown symbol has no declared-type meaning,
        // so GetDeclaredTypeOfSymbol still resolves to errorType (Go's nil
        // arm) — the pin survives the flip.
        assert_eq!(c.get_declared_type_of_symbol(sym), c.error_type());
        // M6 S8 flipped getReturnTypeOfSignature to the REAL body
        // (members.rs): the seeded signature 0 (NewChecker's anySignature,
        // whose resolvedReturnType is anyType — see spine.rs's own seed
        // asserts) now resolves its return type instead of the stub-era
        // errorType — the any-signature pin.
        assert_eq!(c.get_return_type_of_signature(SignatureId(0)), c.any_type());
        // The W1 `get_base_constructor_type_of_class == errorType` pin is
        // SUPERSEDED (M6 S2): the real base-constructor resolution asserts
        // InterfaceType data — Go's `t.AsInterfaceType()` nil-deref on a
        // non-class — so the pin now asserts the panic (contract parity with
        // Go's debug assert rather than the stub's errorType).
        {
            let mut c2 = new_checker();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                c2.get_base_constructor_type_of_class(t)
            }));
            assert!(result.is_err());
        }
        // Empty/None/false stubs.
        assert_eq!(c.get_type_from_type_node(node), c.error_type());
        assert!(c.get_properties_of_type(t).is_empty());
        assert_eq!(c.get_property_of_type(t, "x"), None);
        assert!(!c.type_has_call_or_construct_signatures(t));
        // The W1 `is_array_like_type == false` pin is SUPERSEDED (FCS):
        // the real body delegates to typenodes (Go checker.go:24049) —
        // in the lib-less global table the missing-ReadonlyArray
        // sentinel (emptyObjectType) accepts the synthetic t, so the
        // predicate reads true (Go's exact shape in the same
        // environment).
        assert!(c.is_array_like_type(t));
        assert!(!c.is_array_type(t));
        assert!(c.get_type_predicate_of_signature(SignatureId(0)).is_none());
        assert_eq!(c.get_true_type_of_conditional_type(t), None);
        assert_eq!(c.get_false_type_of_conditional_type(t), None);
        // M6 S2 flipped the constraint twin to the real family
        // (typeofsymbol.rs): the family is only ever called on type
        // parameters by contract (Go nil-derefs otherwise); the port's
        // flags guard keeps the non-type-parameter answer None.
        assert_eq!(c.get_constraint_of_type_parameter(t), None);
        assert_eq!(c.get_base_constraint_of_type(t), None);
        assert_eq!(c.get_symbol_of_node(node), None);
        assert_eq!(c.get_merged_symbol(sym), sym);
        assert_eq!(c.get_parent_of_symbol(sym), c.program.symbol(sym).parent);
        assert_eq!(c.get_immediate_aliased_symbol(sym), None);
        // M6 S8 flipped getTargetSymbol to the REAL instantiation-target
        // walk (members.rs, Go checker.go:22087): a non-instantiated symbol
        // resolves to itself (Go returns the symbol, never nil here) — the
        // stub-era None pin is retired.
        assert_eq!(c.get_target_symbol(sym), Some(sym));
        // M6 S6 flipped the resolved-signature twin to the REAL family
        // (resolvecall.rs): resolveSignature is only ever called on
        // call-like nodes by contract (Go panics "Unhandled case in
        // resolveSignature" otherwise); the pin now asserts the panic.
        {
            let mut c2 = new_checker();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                c2.get_resolved_signature(node)
            }));
            assert!(result.is_err());
        }
        // M6 S6 flipped the fully-qualified-name twin to the REAL parent-chain
        // walk: the unknown symbol's own name is "unknown" (Go's
        // symbolToString(unknownSymbol) answers the same).
        assert_eq!(c.get_fully_qualified_name(sym), "unknown");
        // M6 W5 flipped the JSX namespace/fragment-factory twins to the REAL
        // family (jsx.rs): get_jsx_namespace answers the default "React"
        // namespace (Go getJsxNamespace with no pragma/factory options and
        // no file pragmas — jsxNamespace default), and
        // get_jsx_fragment_factory still answers "" (Go: no fragment
        // factory without jsxFragmentFactory/pragma).
        assert_eq!(c.get_jsx_namespace(node), "React");
        assert_eq!(c.get_jsx_fragment_factory(node), "");
        assert_eq!(c.resolve_name("x", node, SymbolFlags::VALUE, false), None);
        // M6 W2 wired the real relater: identity comparability of a type
        // with itself now holds (Go: isTypeAssignableTo(string, string)).
        assert!(c.is_type_assignable_to(t, t));
        assert!(!c.is_context_sensitive(node));
        assert!(!c.requires_adding_implicit_undefined(node));
        assert!(!c.is_readonly_symbol(sym));
        assert_eq!(c.get_effective_declaration_flags(node, ModifierFlags::PUBLIC), ModifierFlags::NONE);
        assert_eq!(c.get_symbol_flags(sym), c.program.symbol(sym).flags);
        // Type arguments pass through the no-filling stub.
        assert_eq!(c.fill_missing_type_arguments(&[t], &[t], 1, false), vec![t]);
        assert_eq!(c.get_min_type_argument_count(&[t, t]), 2);
    }

    #[test]
    fn tuple_type_target_requires_the_tuple_target_identity() {
        let c = new_checker();
        // Intrinsic types are not tuple references — is_tuple_type is false,
        // so IsTupleTypeTarget is false (Go: isTupleType(t) && t.Target() == t).
        let t = c.string_type();
        assert!(!c.is_tuple_type_target(t));
    }

    #[test]
    fn compare_symbols_delegates_to_the_worker() {
        let c = new_checker();
        let (s1, s2) = (c.get_unknown_symbol(), c.get_undefined_symbol());
        // Same symbol → Equal (the worker's first check).
        assert_eq!(c.compare_symbols(s1, s1), std::cmp::Ordering::Equal);
        // Distinct symbols order by declaration/name/id (worker fallback).
        let _ = c.compare_symbols(s1, s2);
    }

    #[test]
    fn free_function_wrappers_delegate_to_utilities() {
        let mut c = new_checker();
        let t = c.string_type();
        // isTypeUsableAsPropertyName: string literal types are; string is not.
        let literal = c.get_string_literal_type("a");
        assert!(is_type_usable_as_property_name(c.type_(literal)));
        assert!(!is_type_usable_as_property_name(c.type_(t)));
        assert_eq!(get_property_name_from_type(c.type_(literal)), "a");
        // The modifier-flags wrapper over the utilities family.
        assert_eq!(
            get_declaration_modifier_flags_from_symbol(&c, c.get_unknown_symbol()),
            ModifierFlags::NONE
        );
        // IsDistributedTypeParameter: intrinsics are not type parameters.
        assert!(!is_distributed_type_parameter(&c, t));
        let (tp, _) = c.alloc_test_distributed_type_parameter();
        assert!(is_distributed_type_parameter(&c, tp));
    }
}
