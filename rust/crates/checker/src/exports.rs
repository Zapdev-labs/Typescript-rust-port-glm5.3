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

use crate::todo_shapes::UnionReduction;
use crate::types::{
    ContextFlags, IndexInfoId, MemberOverrideStatus, SignatureId, SignatureKind, TypeId,
};
use crate::utilities as u;
use crate::Checker;

impl Checker {
    /// Go: `func (c *Checker) GetBaseTypeOfLiteralType(t *Type) *Type` →
    /// `c.getBaseTypeOfLiteralType(t)` (checker.go:25968 — S7's
    /// enum/template/union arms).
    pub fn get_base_type_of_literal_type(&self, t: TypeId) -> TypeId {
        // TODO(port M6 S7): the string/number/bigint/boolean arms + the
        // enum-like and union arms (getBaseTypeOfEnumLikeType /
        // getBaseTypeOfLiteralTypeUnion). Identity is the permissive value
        // (Go's own default arm returns t).
        t
    }

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
        // TODO(port M6 S7): the anySignature family seeds unknownSignature.
        None
    }

    /// Go: `func (c *Checker) GetUnionType(types []*Type) *Type` →
    /// `c.getUnionType(types)` (checker.go — S7's union construction).
    ///
    /// TODO(port M6 S7): the real getUnionType (interned union construction)
    /// replaces this stub body — the exported/internal pair collapses onto
    /// this name.
    pub fn get_union_type(&mut self, types: &[TypeId]) -> TypeId {
        let _ = types;
        self.error_type()
    }

    /// Go: `func (c *Checker) GetNameTypeOfSymbol(symbol *ast.Symbol) *Type` —
    /// `if links := c.valueSymbolLinks.TryGet(symbol); links != nil { return
    /// links.nameType }; return nil` (real body; the link store is ported).
    pub fn get_name_type_of_symbol(&mut self, symbol: SymbolId) -> Option<TypeId> {
        self.value_symbol_links.try_get(symbol).and_then(|links| links.name_type)
    }

    /// Go: `func (c *Checker) GetGlobalSymbol(name string, meaning ast.SymbolFlags, diagnostic *diagnostics.Message) *ast.Symbol`
    /// → `c.getGlobalSymbol(...)` (checker.go — S6's global resolvers).
    pub fn get_global_symbol(
        &mut self,
        name: &str,
        meaning: SymbolFlags,
        diagnostic: Option<&'static Message>,
    ) -> Option<SymbolId> {
        // TODO(port M6 S6): the globalSymbol resolution family.
        let _ = (name, meaning, diagnostic);
        None
    }

    /// Go: `func (c *Checker) GetMergedSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// → `c.getMergedSymbol(symbol)` (checker.go — S6's symbol-merging
    /// machinery). Identity is the permissive value: symbols with no merge
    /// record resolve to themselves, and no merges exist until S6.
    pub fn get_merged_symbol(&self, symbol: SymbolId) -> SymbolId {
        // TODO(port M6 S6): mergedSymbols table + recordMergedSymbol.
        symbol
    }

    /// Go: `func (c *Checker) GetSymbolOfNode(node *ast.Node) *ast.Symbol` →
    /// `c.getSymbolOfNode(node)` (checker.go:14669 — S6).
    pub fn get_symbol_of_node(&self, node: NodeId) -> Option<SymbolId> {
        // TODO(port M6 S6): node→symbol resolution through the binder links.
        let _ = node;
        None
    }

    /// Go: `func (c *Checker) GetSymbolOfDeclaration(node *ast.Node) *ast.Symbol`
    /// → `c.getSymbolOfDeclaration(node)` (checker.go — S6).
    pub fn get_symbol_of_declaration(&self, node: NodeId) -> Option<SymbolId> {
        // TODO(port M6 S6): declaration→symbol resolution.
        let _ = node;
        None
    }

    /// Go: `func (c *Checker) GetParentOfSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// → `c.getParentOfSymbol(symbol)` (checker.go:14630 —
    /// `if symbol.Parent != nil { return c.getMergedSymbol(
    /// c.getLateBoundSymbol(symbol.Parent)) }; return nil`). The late-bound
    /// and merged machinery is S6; the raw parent is the permissive value.
    pub fn get_parent_of_symbol(&self, symbol: SymbolId) -> Option<SymbolId> {
        // TODO(port M6 S6): getLateBoundSymbol + getMergedSymbol composition.
        self.program.symbol(symbol).parent
    }

    /// Go: `func (c *Checker) TryFindAmbientModule(moduleName string) *ast.Symbol`
    /// → `c.tryFindAmbientModule(moduleName, true /*withAugmentations*/)`
    /// (checker.go:15910 — S6's ambient-module machinery).
    pub fn try_find_ambient_module(&mut self, module_name: &str) -> Option<SymbolId> {
        // TODO(port M6 S6): the ambientModulesOnce scan + augmentations.
        let _ = module_name;
        None
    }

    /// Go: `func (c *Checker) GetImmediateAliasedSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// → `c.getImmediateAliasedSymbol(symbol)` (checker.go:2199 — S6's
    /// alias-resolution family).
    pub fn get_immediate_aliased_symbol(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        // TODO(port M6 S6): resolveAlias machinery + the JS paths.
        let _ = symbol;
        None
    }

    /// Go: `func (c *Checker) GetTargetSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// → `c.getTargetSymbol(symbol)` (checker.go — S6).
    pub fn get_target_symbol(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        // TODO(port M6 S6): getMergedSymbol + getImmediateAliasedSymbol walk.
        let _ = symbol;
        None
    }

    /// Go: `func (c *Checker) GetTypeOnlyAliasDeclaration(symbol *ast.Symbol) *ast.Node`
    /// → `c.getTypeOnlyAliasDeclaration(symbol)` (checker.go — S6).
    pub fn get_type_only_alias_declaration(&mut self, symbol: SymbolId) -> Option<NodeId> {
        // TODO(port M6 S6): the typeOnlyDeclaration scan.
        let _ = symbol;
        None
    }

    /// Go: `func (c *Checker) ResolveExternalModuleName(moduleSpecifier *ast.Node, importAttributesType *Type) *ast.Symbol`
    /// → `c.resolveExternalModuleName(moduleSpecifier, moduleSpecifier,
    /// true /*ignoreErrors*/, importAttributesType)` (checker.go — S6's
    /// module resolution).
    pub fn resolve_external_module_name(
        &mut self,
        module_specifier: NodeId,
        import_attributes_type: Option<TypeId>,
    ) -> Option<SymbolId> {
        // TODO(port M6 S6): resolveExternalModule + the program seam.
        let _ = (module_specifier, import_attributes_type);
        None
    }

    /// Go: `func (c *Checker) ResolveExternalModuleSymbol(moduleSymbol *ast.Symbol) *ast.Symbol`
    /// → `c.resolveExternalModuleSymbol(moduleSymbol, false /*dontResolveAlias*/)`
    /// (checker.go — S6).
    pub fn resolve_external_module_symbol(&mut self, module_symbol: SymbolId) -> Option<SymbolId> {
        // TODO(port M6 S6): the export=`.` resolution.
        let _ = module_symbol;
        None
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
    /// the relater, S7/S10).
    pub fn is_array_like_type(&self, t: TypeId) -> bool {
        // TODO(port M6 S7/S10): isArrayType || assignability to ReadonlyArray<any>.
        let _ = t;
        false
    }

    /// Go: `func (c *Checker) GetPropertiesOfType(t *Type) []*ast.Symbol` →
    /// `c.getPropertiesOfType(t)` (checker.go — S8's member resolution).
    pub fn get_properties_of_type(&mut self, t: TypeId) -> Vec<SymbolId> {
        // TODO(port M6 S8): resolveStructuredTypeMembers plumbing.
        let _ = t;
        Vec::new()
    }

    /// Go: `func (c *Checker) GetPropertyOfType(t *Type, name string) *ast.Symbol`
    /// → `c.getPropertyOfType(t, name)` (checker.go — S8).
    pub fn get_property_of_type(&mut self, t: TypeId, name: &str) -> Option<SymbolId> {
        // TODO(port M6 S8): getPropertyOfObjectType/getPropertyOfUnionOrIntersectionType.
        let _ = (t, name);
        None
    }

    /// Go: `func (c *Checker) TypeHasCallOrConstructSignatures(t *Type) bool`
    /// → `c.typeHasCallOrConstructSignatures(t)` (checker.go — S8).
    pub fn type_has_call_or_construct_signatures(&mut self, t: TypeId) -> bool {
        // TODO(port M6 S8): the structured-type signature probes.
        let _ = t;
        false
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
        // TODO(port M6 S9): the contextual property-type resolution.
        let _ = (t, name);
        None
    }

    /// Go: `func (c *Checker) WasCanceled() bool` (real body — the field).
    pub fn was_canceled(&self) -> bool {
        self.was_canceled
    }

    /// Go: `func (c *Checker) GetSignaturesOfType(t *Type, kind SignatureKind) []*Signature`
    /// → `c.getSignaturesOfType(t, kind)` (checker.go — S8).
    pub fn get_signatures_of_type(&mut self, t: TypeId, kind: SignatureKind) -> Vec<SignatureId> {
        // TODO(port M6 S8): getSignaturesOfStructuredType.
        let _ = (t, kind);
        Vec::new()
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
    /// `c.getTrueTypeFromConditionalType(t)` (checker.go — S5; nil when the
    /// conditional has no true branch yet).
    pub fn get_true_type_of_conditional_type(&self, t: TypeId) -> Option<TypeId> {
        // TODO(port M6 S5): getTrueTypeFromConditionalType.
        let _ = t;
        None
    }

    /// Go: `func (c *Checker) GetFalseTypeOfConditionalType(t *Type) *Type` →
    /// `c.getFalseTypeFromConditionalType(t)` (checker.go — S5). PORT (M6 S2):
    /// the payload read (Go returns the false branch or nil; S5 owns the
    /// branch resolution that populates it).
    pub fn get_false_type_of_conditional_type(&self, t: TypeId) -> Option<TypeId> {
        self.type_(t)
            .as_conditional_type()
            .and_then(|d| d.resolved_false_type)
    }

    /// Go: `func (c *Checker) GetDefaultFromTypeParameter(typeParameter *Type) *Type`
    /// → `c.getDefaultFromTypeParameter(typeParameter)` (checker.go — S2).
    pub fn get_default_from_type_parameter(&self, type_parameter: TypeId) -> Option<TypeId> {
        // TODO(port M6 S2): resolvedDefaultType resolution.
        let _ = type_parameter;
        None
    }

    /// Go: `func (c *Checker) GetEffectiveDeclarationFlags(n *ast.Node, flagsToCheck ast.ModifierFlags) ast.ModifierFlags`
    /// → `c.getEffectiveDeclarationFlags(n, flagsToCheck)` (checker.go — S8).
    pub fn get_effective_declaration_flags(
        &self,
        n: NodeId,
        flags_to_check: ModifierFlags,
    ) -> ModifierFlags {
        // TODO(port M6 S8): the effective-modifiers computation
        // (ambient/JS-file handling).
        let _ = n;
        let _ = flags_to_check;
        ModifierFlags::NONE
    }

    /// Go: `func (c *Checker) GetBaseConstraintOfType(t *Type) *Type` →
    /// `c.getBaseConstraintOfType(t)` (checker.go — S9's constraint
    /// resolution; nil for unconstrained types).
    pub fn get_base_constraint_of_type(&mut self, t: TypeId) -> Option<TypeId> {
        // TODO(port M6 S9): getResolvedBaseConstraint/getNextBaseConstraint.
        let _ = t;
        None
    }

    /// Go: `func (c *Checker) IsTupleTypeTarget(t *Type) bool` →
    /// `isTupleType(t) && t.Target() == t` (real body — `is_tuple_type` is the
    /// collapsed Go pair; the target check needs the arena payload).
    pub fn is_tuple_type_target(&self, t: TypeId) -> bool {
        self.is_tuple_type(t) && self.type_(t).target() == Some(t)
    }

    /// Go: `func (c *Checker) IsReadonlySymbol(symbol *ast.Symbol) bool` →
    /// `c.isReadonlySymbol(symbol)` (checker.go — S8).
    pub fn is_readonly_symbol(&mut self, symbol: SymbolId) -> bool {
        // TODO(port M6 S8): the readonly-modifier walk.
        let _ = symbol;
        false
    }

    /// Go: `func (c *Checker) GetReturnTypeOfSignature(sig *Signature) *Type`
    /// → `c.getReturnTypeOfSignature(sig)` (checker.go — S8; unresolved
    /// bodies resolve to errorType in Go).
    pub fn get_return_type_of_signature(&mut self, sig: SignatureId) -> TypeId {
        // TODO(port M6 S8): resolvedReturnType + getReturnTypeFromBody.
        let _ = sig;
        self.error_type()
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
        // TODO(port M6 S9): the object-literal contextual chain.
        let _ = (element, context_flags);
        None
    }

    /// Go: `func (c *Checker) TypePredicateToString(t *TypePredicate) string`
    /// → `c.typePredicateToString(t)` (nodebuilderimpl.go — M6 wave 4).
    pub fn type_predicate_to_string(&self, t: &crate::types::TypePredicate) -> String {
        // TODO(port M6 nodebuilder wave): the printer-facing formatting.
        let _ = t;
        String::new()
    }

    /// Go: `func (c *Checker) GetExpandedParameters(signature *Signature, skipUnionExpanding bool) [][]*ast.Symbol`
    /// → `c.getExpandedParameters(signature, skipUnionExpanding)`
    /// (checker.go — S8's parameter union expansion).
    pub fn get_expanded_parameters(
        &mut self,
        signature: SignatureId,
        skip_union_expanding: bool,
    ) -> Vec<Vec<SymbolId>> {
        // TODO(port M6 S8): the union-parameter cross product.
        let _ = (signature, skip_union_expanding);
        Vec::new()
    }

    /// Go: `func (c *Checker) GetResolvedSignature(node *ast.Node) *Signature`
    /// → `c.getResolvedSignature(node, nil, CheckModeNormal)` (checker.go —
    /// S6's overload resolution).
    pub fn get_resolved_signature(&mut self, node: NodeId) -> Option<SignatureId> {
        // TODO(port M6 S6): resolveSignature/resolveCall.
        let _ = node;
        None
    }

    // Return the type of the given property in the given type, or nil if no such property exists
    /// Go: `func (c *Checker) GetTypeOfPropertyOfType(t *Type, name string) *Type`
    /// → `c.getTypeOfPropertyOfType(t, name)` (checker.go — S8).
    pub fn get_type_of_property_of_type(&mut self, t: TypeId, name: &str) -> Option<TypeId> {
        // TODO(port M6 S8): getPropertyOfType + its type links.
        let _ = (t, name);
        None
    }

    /// Go: `func (c *Checker) GetContextualTypeForArgumentAtIndex(node *ast.Node, argIndex int) *Type`
    /// → `c.getContextualTypeForArgumentAtIndex(node, argIndex)` (checker.go — S9).
    pub fn get_contextual_type_for_argument_at_index(
        &mut self,
        node: NodeId,
        arg_index: usize,
    ) -> Option<TypeId> {
        // TODO(port M6 S9): the call-argument contextual chain.
        let _ = (node, arg_index);
        None
    }

    /// Go: `func (c *Checker) GetAwaitedType(t *Type) *Type` →
    /// `c.getAwaitedType(t)` (checker.go — S9/S10's awaited-type caches).
    /// Identity is the permissive value (Go returns `t` for non-thenables).
    pub fn get_awaited_type(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S9/S10): unwrapAwaitedType/thenable resolution.
        t
    }

    /// Go: `func (c *Checker) GetIndexSignaturesAtLocation(node *ast.Node) []*ast.Node`
    /// → `c.getIndexSignaturesAtLocation(node)` (checker.go — S8).
    pub fn get_index_signatures_at_location(&mut self, node: NodeId) -> Vec<NodeId> {
        // TODO(port M6 S8): the location's apparent-type index signatures.
        let _ = node;
        Vec::new()
    }

    /// Go: `func (c *Checker) GetResolvedSymbol(node *ast.Node) *ast.Symbol` →
    /// `c.getResolvedSymbol(node)` (checker.go:14161 — S6).
    pub fn get_resolved_symbol(&mut self, node: NodeId) -> Option<SymbolId> {
        // TODO(port M6 S6): the alias-resolving symbol resolution.
        let _ = node;
        None
    }

    /// Go: `func (c *Checker) GetJsxNamespace(location *ast.Node) string` →
    /// `c.getJsxNamespace(location)` (jsx.go — the jsx wave).
    pub fn get_jsx_namespace(&mut self, location: NodeId) -> String {
        // TODO(port M6 jsx wave): the JSX namespace resolution.
        let _ = location;
        String::new()
    }

    /// Go: `func (c *Checker) GetJsxFragmentFactory(location *ast.Node) string` —
    /// `entity := c.getJsxFragmentFactoryEntity(location);
    /// if entity != nil { return ast.GetFirstIdentifier(entity).Text() }; return ""`
    /// (jsx.go — the jsx wave).
    pub fn get_jsx_fragment_factory(&mut self, location: NodeId) -> String {
        // TODO(port M6 jsx wave): getJsxFragmentFactoryEntity.
        let _ = location;
        String::new()
    }

    /// Go: `func (c *Checker) ResolveName(name string, location *ast.Node, meaning ast.SymbolFlags, excludeGlobals bool) *ast.Symbol`
    /// → `c.resolveName(location, name, meaning, nil, true, excludeGlobals)`
    /// (checker.go — S6's name resolution).
    pub fn resolve_name(
        &mut self,
        name: &str,
        location: NodeId,
        meaning: SymbolFlags,
        exclude_globals: bool,
    ) -> Option<SymbolId> {
        // TODO(port M6 S6): resolveName's scope walk.
        let _ = (name, location, meaning, exclude_globals);
        None
    }

    /// Go: `func (c *Checker) GetSymbolFlags(symbol *ast.Symbol) ast.SymbolFlags` →
    /// `c.getSymbolFlags(symbol)` (checker.go:16749 — S6's alias-walking
    /// flag computation). The raw flags are the permissive value.
    pub fn get_symbol_flags(&self, symbol: SymbolId) -> SymbolFlags {
        // TODO(port M6 S6): getSymbolFlagsEx's alias-flag accumulation.
        self.program.symbol(symbol).flags
    }

    /// Go: `func (c *Checker) GetBaseTypes(t *Type) []*Type` →
    /// `c.getBaseTypes(t)` (checker.go — S8's base-type resolution).
    pub fn get_base_types(&mut self, t: TypeId) -> Vec<TypeId> {
        // TODO(port M6 S8): resolveBaseTypesOfClass/OfInterface.
        let _ = t;
        Vec::new()
    }

    /// Go: `func (c *Checker) GetApparentType(t *Type) *Type` →
    /// `c.getApparentType(t)` (checker.go — S8/S9). Identity is the
    /// permissive value (Go returns `t` for primitive/already-apparent types).
    pub fn get_apparent_type(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S8/S9): the apparent-type mapping table.
        t
    }

    /// Go: `func (c *Checker) GetReducedType(t *Type) *Type` →
    /// `c.getReducedType(t)` (checker.go — S9). Identity is the permissive
    /// value (reduction only rewrites discriminant-conditioned unions).
    pub fn get_reduced_type(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S9): the literal-reduction engine.
        t
    }

    // GetFullyQualifiedName returns the fully qualified name of a symbol, walking up
    // its parent chain (e.g. `"/path/to/module".Namespace.Name`).
    /// Go: `func (c *Checker) GetFullyQualifiedName(symbol *ast.Symbol) string` →
    /// `c.getFullyQualifiedName(symbol, nil /*containingLocation*/)`
    /// (checker.go — S6).
    pub fn get_fully_qualified_name(&mut self, symbol: SymbolId) -> String {
        // TODO(port M6 S6): the parent-chain name walk.
        let _ = symbol;
        String::new()
    }

    /// Go: `func (c *Checker) GetBaseConstructorTypeOfClass(t *Type) *Type` →
    /// `c.getBaseConstructorTypeOfClass(t)` (checker.go — S2). PORT (M6 S2):
    /// the W1 error-type stub flipped to the real base-constructor resolution
    /// (typeofsymbol.rs — the ResolvedBaseConstructorType resolution-stack
    /// axis + the circularity diagnostics).
    pub fn get_base_constructor_type_of_class(&mut self, t: TypeId) -> TypeId {
        self.get_base_constructor_type_of_class_impl(t)
    }

    /// Go: `func (c *Checker) GetMemberOverrideModifierStatus(node *ast.Node, member *ast.Node, memberSymbol *ast.Symbol) MemberOverrideStatus`
    /// → `c.getMemberOverrideModifierStatus(...)` (checker.go — S3's class
    /// member checks).
    #[allow(clippy::too_many_arguments)]
    pub fn get_member_override_modifier_status(
        &mut self,
        node: NodeId,
        member: NodeId,
        member_symbol: SymbolId,
    ) -> MemberOverrideStatus {
        // TODO(port M6 S3): the override-modifier checks.
        let _ = (node, member, member_symbol);
        MemberOverrideStatus::None
    }

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
        // TODO(port M6 S7/S8): the type-argument resolution.
        let _ = t;
        Vec::new()
    }

    /// Go: `func (c *Checker) GetIndexInfoOfType(t *Type, keyType *Type) *IndexInfo`
    /// → `c.getIndexInfoOfType(t, keyType)` (checker.go — S10).
    pub fn get_index_info_of_type(&mut self, t: TypeId, key_type: TypeId) -> Option<IndexInfoId> {
        // TODO(port M6 S10): getIndexInfosOfType plumbing.
        let _ = (t, key_type);
        None
    }

    /// Go: `func (c *Checker) GetIndexTypeOfType(t *Type, keyType *Type) *Type` →
    /// `c.getIndexTypeOfType(t, keyType)` (checker.go — S10).
    pub fn get_index_type_of_type(&mut self, t: TypeId, key_type: TypeId) -> Option<TypeId> {
        // TODO(port M6 S10): getIndexType over the index info.
        let _ = (t, key_type);
        None
    }

    /// Go: `func (c *Checker) GetIndexInfosOfType(t *Type) []*IndexInfo` →
    /// `c.getIndexInfosOfType(t)` (checker.go — S10).
    pub fn get_index_infos_of_type(&mut self, t: TypeId) -> Vec<IndexInfoId> {
        // TODO(port M6 S10): the structured-type index-info collection.
        let _ = t;
        Vec::new()
    }

    /// Go: `func (c *Checker) IsContextSensitive(node *ast.Node) bool` →
    /// `c.isContextSensitive(node)` (checker.go — S9).
    pub fn is_context_sensitive(&mut self, node: NodeId) -> bool {
        // TODO(port M6 S9): the context-sensitivity scan.
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) FillMissingTypeArguments(typeArguments []*Type, typeParameters []*Type, minTypeArgumentCount int, isJavaScriptImplicitAny bool) []*Type`
    /// → `c.fillMissingTypeArguments(...)` (checker.go — S6). The unchanged
    /// argument list is the permissive value (no filling).
    #[allow(clippy::too_many_arguments)]
    pub fn fill_missing_type_arguments(
        &mut self,
        type_arguments: &[TypeId],
        type_parameters: &[TypeId],
        min_type_argument_count: usize,
        is_java_script_implicit_any: bool,
    ) -> Vec<TypeId> {
        // TODO(port M6 S6): the wildcard/any padding.
        let _ = (type_parameters, min_type_argument_count, is_java_script_implicit_any);
        type_arguments.to_vec()
    }

    /// Go: `func (c *Checker) GetMinTypeArgumentCount(typeParameters []*Type) int`
    /// → `c.getMinTypeArgumentCount(typeParameters)` (checker.go:22397 — S7).
    /// The full count is the permissive value (every argument required).
    pub fn get_min_type_argument_count(&self, type_parameters: &[TypeId]) -> usize {
        // TODO(port M6 S7): the optional/rest prefix count.
        type_parameters.len()
    }

    /// Go: `func (c *Checker) GetWidenedLiteralType(t *Type) *Type` →
    /// `c.getWidenedLiteralType(t)` (checker.go — S9). COLLAPSED PAIR: the
    /// spike already ported the literal-widening slice as checker.rs's
    /// `get_widened_literal_type` — that is this wrapper's target; the S9
    /// wave completes it.
    ///
    /// Go: `func (c *Checker) GetWidenedType(t *Type) *Type` →
    /// `c.getWidenedType(t)` (checker.go — S9) is likewise collapsed onto
    /// checker.rs's `get_widened_type`.
    ///
    /// Go: `func (c *Checker) IsTypeAssignableTo(source *Type, target *Type) bool`
    /// → `c.isTypeAssignableTo(source, target)` (relater.go — M6 wave 2) —
    /// W2: the real body landed with relater.rs.
    /// Go: `func (c *Checker) GetUnionTypeEx(types []*Type, unionReduction UnionReduction) *Type`
    /// → `c.getUnionTypeEx(types, unionReduction, nil, nil)` (checker.go — S7).
    ///
    /// TODO(port M6 S7): the real getUnionTypeEx replaces this stub body
    /// (the exported/internal pair collapses onto this name).
    pub fn get_union_type_ex(&mut self, types: &[TypeId], union_reduction: UnionReduction) -> TypeId {
        let _ = (types, union_reduction);
        self.error_type()
    }

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
    /// `c.removeMissingOrUndefinedType(t)` (checker.go — S9). Identity is
    /// the permissive value (nothing removed).
    pub fn remove_missing_or_undefined_type(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S9): the union constituent filtering.
        t
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
        // unknownSignature is M6-constructed: the stub's None contract.
        assert!(c.get_unknown_signature().is_none());
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
        assert_eq!(c.get_apparent_type(t), t);
        assert_eq!(c.get_reduced_type(t), t);
        assert_eq!(c.get_widened_literal_type(t), t);
        assert_eq!(c.get_widened_type(t), t);
        assert_eq!(c.remove_missing_or_undefined_type(t), t);
        // Error-type stubs.
        assert_eq!(c.get_union_type(&[t]), c.error_type());
        assert_eq!(c.get_union_type_ex(&[t], UnionReduction::Literal), c.error_type());
        // M6 S2 flipped the declared-type twin to the real family
        // (typeofsymbol.rs): the unknown symbol has no declared-type meaning,
        // so GetDeclaredTypeOfSymbol still resolves to errorType (Go's nil
        // arm) — the pin survives the flip.
        assert_eq!(c.get_declared_type_of_symbol(sym), c.error_type());
        assert_eq!(c.get_return_type_of_signature(SignatureId(0)), c.error_type());
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
        assert!(!c.is_array_like_type(t));
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
        assert_eq!(c.get_target_symbol(sym), None);
        assert_eq!(c.get_resolved_signature(node), None);
        assert_eq!(c.get_fully_qualified_name(sym), "");
        assert_eq!(c.get_jsx_namespace(node), "");
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
