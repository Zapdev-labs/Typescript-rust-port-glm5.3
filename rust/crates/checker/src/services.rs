// Ported from tsc/internal/checker/services.go @
// ec47d33c23e464a17cdf2475632cba629bee8763 — M6 W3 tranche 3.
//
// The checker-adjacent LS services slice (62 funcs / 1,149 Go lines),
// mirrored function-by-function in Go's declaration order. The family is
// the language-service surface Go keeps beside the checker (the
// Get*/Try*/Is* exported twins + the get* workers they delegate to); the
// checker itself consumes the private workers (getAugmentedPropertiesOf-
// Type, getUninstantiatedSignatures, getTypeArgumentConstraint,
// getTypeOfAssignmentPattern, getResolvedSignatureWorker).
//
// PORT NOTES (the seams this slice carries, same discipline as the landed
// waves — permissive arms name the owning wave, nothing silently lies):
//   - GetContextualType delegates to the S9 contextual worker's permissive
//     None carrier (contextual.rs) until the contextual depth lands.
//   - The jsx family (GetJsxIntrinsicTagNamesAt / GetContextualTypeFor-
//     JsxAttribute) rides the jsx wave's permissive stubs.
//   - runWithoutResolvedSignatureCaching / runWithInferenceBlockedFrom-
//     SourceNode are REAL over the LinkStore pair (Go's resolvedSignature /
//     resolvedType snapshot-restore shape).
//   - The reference-scan family (IsDeclarationUsed / IsSymbolReferenced-
//     InFile / GetReferencesToSymbolInFile) uses get_symbol_at_location_stub
//     (checker.go:32147's S10 worker — permissive None until S10) and
//     getPossibleSymbolReferenceNodes stops at the positions scan (the
//     astnav.GetTouchingPropertyName node-mapping walk is the M7 astnav
//     wave's — the positions themselves are the REAL Go text scan).
//   - GetFirstTypeArgumentFromKnownType resolves through get_global_symbol
//     (REAL since W3 tranche 3 — spine.rs get_global_symbol_real).
//   - IsLibSymbolForHoverVerbosity reads the Program trait's
//     is_source_file_default_library (added with this slice, default
//     false — the twin's lib-less unit programs have no default libs).
//   - IsTypeInvalidDueToUnionDiscriminant's getTypeOfNode(property) probe
//     banners the M7 property-assignment typing worker (dormant arm).
//   - getTypeParameterAtPosition's index-type this-parameter arm and the
//     spreadLinks/mappedSymbolLinks/exportTypeLinks target walks ride the
//     S8 waves (dormant arms; the valueSymbolLinks target walk is real).

use crate::utilities as u;
use crate::Checker;

use tsc_ast::{is_identifier, CheckFlags, Kind, NodeId, SymbolFlags, SymbolId, SymbolTable};

use crate::checker::CheckMode;
use crate::todo_shapes::EvaluatorValue;
use crate::types::{ContextFlags, ObjectFlags, SignatureKind, TypeFlags, TypeId};

/// Go's nil `*ast.Node` for `NodeId` — the Option mapping the walk loops use.
#[inline]
fn nonnone(node: NodeId) -> Option<NodeId> {
    (!node.is_none()).then_some(node)
}

/// Go: `func IsArrayLiteralOrObjectLiteralDestructuringPattern(node) bool`
/// (ast.go:1406) — PORT: the ast crate lacks the twin; local recursive copy
/// (checked against the Go body, incl. the `=`-token and for-of-initializer
/// arms). Takes `&mut Checker` so the caller can't hold a store borrow.
fn is_array_literal_or_object_literal_destructuring_pattern(c: &mut Checker, node: NodeId) -> bool {
    let store = c.store();
    let n = store.node(node);
    if !(tsc_ast::ast_generated::is_array_literal_expression(n)
        || tsc_ast::ast_generated::is_object_literal_expression(n))
    {
        return false;
    }
    let parent = n.parent.get();
    if parent.is_none() {
        return false;
    }
    // [a,b,c] from:
    // [a, b, c] = someExpression;
    if let Some(d) = store.node(parent).as_binary_expression()
        && d.left == node
        && store.node(d.operator_token).kind == Kind::EqualsToken
    {
        return true;
    }
    // [a, b, c] from:
    // for([a, b, c] of expression)
    if tsc_ast::ast_generated::is_for_of_statement(store.node(parent))
        && crate::ast_shims::initializer_of(store, parent) == Some(node)
    {
        return true;
    }
    // {x, a: {a, b, c} } = someExpression
    if store.node(parent).kind == Kind::PropertyAssignment {
        let grandparent = store.node(parent).parent.get();
        if grandparent.is_none() {
            return false;
        }
        return is_array_literal_or_object_literal_destructuring_pattern(c, grandparent);
    }
    // [a, b, c] of
    // [x, [a, b, c] ] = someExpression
    is_array_literal_or_object_literal_destructuring_pattern(c, parent)
}

// ────────────────────────────────────────────────────────────────────────────
// GetSymbolsInScope (Go services.go:16)
// ────────────────────────────────────────────────────────────────────────────
impl Checker {
    /// Go: `func (c *Checker) GetSymbolsInScope(location *ast.Node, meaning ast.SymbolFlags) []*ast.Symbol`.
    pub fn get_symbols_in_scope(&mut self, location: NodeId, meaning: SymbolFlags) -> Vec<SymbolId> {
        if self
            .store()
            .node(location)
            .flags
            .intersects(tsc_ast::NodeFlags::IN_WITH_STATEMENT)
        {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return Vec::new();
        }

        let mut symbols: SymbolTable = SymbolTable::default();
        let mut is_static_symbol = false;

        // Go's `copySymbol`/`copySymbols`/`copyLocallyVisibleExportSymbols`
        // closures — methods below (the loop body inlines their call sites
        // exactly as Go's populateSymbols does).

        let mut location: Option<NodeId> = Some(location);
        let mut last_location: Option<NodeId> = None;
        'walk: while let Some(loc) = location {
            if tsc_ast::ast_generated::is_module_declaration(self.store().node(loc)) {
                let attributes = self
                    .store()
                    .node(loc)
                    .as_module_declaration()
                    .and_then(|d| d.attributes);
                // Module declaration is not in scope inside its attributes.
                if attributes.is_some() && last_location == attributes {
                    last_location = Some(loc);
                    location = nonnone(self.store().node(loc).parent.get());
                    continue 'walk;
                }
            }

            if u::can_have_locals(self.store().node(loc))
                && tsc_binder::node_data::locals_of(self.store(), loc).is_some()
                && !u::is_global_source_file(self.store(), loc)
            {
                let locals = tsc_binder::node_data::locals_of(self.store(), loc)
                    .cloned()
                    .unwrap_or_default();
                self.copy_symbols_into(&mut symbols, &locals, meaning);
            }

            match self.store().node(loc).kind {
                Kind::SourceFile => {
                    if tsc_binder::ast_shims::is_external_module(self.store().file(loc.file_id())) {
                        // fallthrough to the module-declaration arm
                        if let Some(module_symbol) = self.get_symbol_of_declaration(loc) {
                            let exports = self.program_symbol(module_symbol).exports.clone();
                            self.copy_locally_visible_export_symbols_into(
                                &mut symbols,
                                &exports,
                                meaning & SymbolFlags::MODULE_MEMBER,
                            );
                        }
                    }
                }
                Kind::ModuleDeclaration => {
                    if let Some(module_symbol) = self.get_symbol_of_declaration(loc) {
                        let exports = self.program_symbol(module_symbol).exports.clone();
                        self.copy_locally_visible_export_symbols_into(
                            &mut symbols,
                            &exports,
                            meaning & SymbolFlags::MODULE_MEMBER,
                        );
                    }
                }
                Kind::EnumDeclaration => {
                    if let Some(enum_symbol) = self.get_symbol_of_declaration(loc) {
                        let exports = self.program_symbol(enum_symbol).exports.clone();
                        self.copy_symbols_into(
                            &mut symbols,
                            &exports,
                            meaning & SymbolFlags::ENUM_MEMBER,
                        );
                    }
                }
                Kind::ClassExpression => {
                    let class_name = self
                        .store()
                        .node(loc)
                        .as_class_expression()
                        .and_then(|d| d.name);
                    if class_name.is_some() {
                        if let Some(symbol) = tsc_binder::node_data::symbol_of(self.store(), loc) {
                            self.copy_symbol_into(&mut symbols, symbol, meaning);
                        }
                    }
                    // this fall-through is necessary because we would like to handle
                    // type parameter inside class expression similar to how we handle
                    // it in classDeclaration and interface Declaration.
                    if !is_static_symbol {
                        if let Some(class_symbol) = self.get_symbol_of_declaration(loc) {
                            let members = self.get_members_of_symbol_table(class_symbol);
                            self.copy_symbols_into(
                                &mut symbols,
                                &members,
                                meaning & SymbolFlags::TYPE,
                            );
                        }
                    }
                }
                Kind::ClassDeclaration | Kind::InterfaceDeclaration => {
                    // If we didn't come from static member of class or interface,
                    // add the type parameters into the symbol table
                    // (type parameters of classDeclaration/classExpression and interface
                    // are in member property of the symbol. Note: that the memberFlags
                    // come from previous iteration.)
                    if !is_static_symbol {
                        if let Some(symbol) = self.get_symbol_of_declaration(loc) {
                            let members = self.get_members_of_symbol_table(symbol);
                            self.copy_symbols_into(
                                &mut symbols,
                                &members,
                                meaning & SymbolFlags::TYPE,
                            );
                        }
                    }
                }
                Kind::FunctionExpression => {
                    let func_name = self.store().node(loc).name();
                    if func_name.is_some() {
                        if let Some(symbol) = tsc_binder::node_data::symbol_of(self.store(), loc) {
                            self.copy_symbol_into(&mut symbols, symbol, meaning);
                        }
                    }
                }
                _ => {}
            }

            if u::introduces_arguments_exotic_object(self.store().node(loc)) {
                if let Some(arguments_symbol) = self.arguments_symbol {
                    self.copy_symbol_into(&mut symbols, arguments_symbol, meaning);
                }
            }

            is_static_symbol = u::is_static(self.store(), self.store().node(loc));
            last_location = Some(loc);
            location = nonnone(self.store().node(loc).parent.get());
        }

        let globals = self.globals.clone();
        self.copy_symbols_into(&mut symbols, &globals, meaning);

        symbols.delete(tsc_ast::INTERNAL_SYMBOL_NAME_THIS); // Not a symbol, a keyword
        u::symbols_to_array(&symbols)
    }

    /// Go: services.go's `copySymbol` closure — copy the symbol into the
    /// table if it carries the meaning and isn't already present.
    fn copy_symbol_into(&mut self, symbols: &mut SymbolTable, symbol: SymbolId, meaning: SymbolFlags) {
        // Go: symbol.CombinedLocalAndExportSymbolFlags() — the export-
        // symbol flag merge (ast/symbol.rs's port takes the arena slice;
        // the two reads keep the Program borrow disjoint).
        let (flags, export_symbol) = {
            let s = self.program_symbol(symbol);
            (s.flags, s.export_symbol)
        };
        let combined = match export_symbol {
            Some(export) => flags | self.program_symbol(export).flags,
            None => flags,
        };
        if combined.intersects(meaning) {
            // We will copy all symbol regardless of its reserved name because
            // symbolsToArray will check whether the key is a reserved name and
            // it will not copy symbol with reserved name to the array
            let id = self.program_symbol(symbol).name.clone();
            if symbols.get(&id).is_none() {
                symbols.set(id, symbol);
            }
        }
    }

    /// Go: services.go's `copySymbols` closure.
    fn copy_symbols_into(
        &mut self,
        symbols: &mut SymbolTable,
        source: &SymbolTable,
        meaning: SymbolFlags,
    ) {
        if !meaning.is_empty() {
            let entries: Vec<SymbolId> = source.values().copied().collect();
            for symbol in entries {
                self.copy_symbol_into(symbols, symbol, meaning);
            }
        }
    }

    /// Go: services.go's `copyLocallyVisibleExportSymbols` closure —
    /// similar condition as in `resolveNameHelper`.
    fn copy_locally_visible_export_symbols_into(
        &mut self,
        symbols: &mut SymbolTable,
        source: &SymbolTable,
        meaning: SymbolFlags,
    ) {
        if !meaning.is_empty() {
            let entries: Vec<SymbolId> = source.values().copied().collect();
            for symbol in entries {
                let locally_visible = !self
                    .program_symbol(symbol)
                    .declarations
                    .iter()
                    .any(|&d| {
                        matches!(
                            self.store().node(d).kind,
                            Kind::ExportSpecifier | Kind::NamespaceExport
                        )
                    })
                    && self.program_symbol(symbol).name != tsc_ast::INTERNAL_SYMBOL_NAME_DEFAULT;
                if locally_visible {
                    self.copy_symbol_into(symbols, symbol, meaning);
                }
            }
        }
    }

    /// Go: `func (c *Checker) GetExportsOfModule(symbol) []*ast.Symbol` (services.go:131).
    pub fn get_exports_of_module_array(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        u::symbols_to_array(&self.get_exports_of_module(symbol))
    }

    /// Go: `func (c *Checker) ForEachExportAndPropertyOfModule(moduleSymbol, cb)` (services.go:135).
    pub fn for_each_export_and_property_of_module(
        &mut self,
        module_symbol: SymbolId,
        mut cb: impl FnMut(&mut Checker, SymbolId, String),
    ) {
        let exports = self.get_exports_of_module(module_symbol);
        for (key, &exported_symbol) in exports.entries() {
            if !u::is_reserved_member_name(key) {
                cb(self, exported_symbol, key.to_string());
            }
        }

        let export_equals = self.resolve_external_module_symbol(module_symbol);
        if export_equals == Some(module_symbol) || export_equals.is_none() {
            return;
        }
        let export_equals = export_equals.expect("checked");

        let type_of_symbol = self.get_type_of_symbol(export_equals);
        if !self.should_treat_properties_of_external_module_as_exports(type_of_symbol) {
            return;
        }

        // forEachPropertyOfType
        let reduced_type = self.get_reduced_apparent_type(type_of_symbol);
        if !self.type_(reduced_type).flags.intersects(TypeFlags::STRUCTURED_TYPE) {
            return;
        }
        let members = self.resolve_structured_type_members_impl(reduced_type);
        let named: Vec<(SymbolId, String)> = members
            .members
            .entries()
            .filter(|(name, symbol)| self.is_named_member(**symbol, name))
            .map(|(name, &symbol)| (symbol, name.to_string()))
            .collect();
        for (symbol, name) in named {
            cb(self, symbol, name);
        }
    }

    /// Go: `func (c *Checker) IsValidPropertyAccess(node, propertyName) bool` (services.go:164).
    pub fn is_valid_property_access(&mut self, node: NodeId, property_name: &str) -> bool {
        match self.store().node(node).kind {
            Kind::PropertyAccessExpression => {
                let expression = crate::ast_shims::expression_of(self.store(), node)
                    .expect("PropertyAccessExpression expression");
                let is_super = self.store().node(expression).kind == Kind::SuperKeyword;
                let checked = self.check_expression(expression);
                let t = self.get_widened_type(checked);
                self.is_valid_property_access_with_type(node, is_super, property_name, t)
            }
            Kind::QualifiedName => {
                let left = self
                    .store()
                    .node(node)
                    .as_qualified_name()
                    .expect("QualifiedName data")
                    .left;
                let checked = self.check_expression(left);
                let t = self.get_widened_type(checked);
                self.is_valid_property_access_with_type(node, false /*isSuper*/, property_name, t)
            }
            Kind::ImportType => {
                let t = self.get_type_from_type_node(node);
                self.is_valid_property_access_with_type(node, false /*isSuper*/, property_name, t)
            }
            _ => panic!("Unexpected node kind in isValidPropertyAccess"),
        }
    }

    /// Go: `func (c *Checker) isValidPropertyAccessWithType(node, isSuper, propertyName, t) bool` (services.go:180).
    fn is_valid_property_access_with_type(
        &mut self,
        node: NodeId,
        is_super: bool,
        property_name: &str,
        t: TypeId,
    ) -> bool {
        // Short-circuiting for improved performance.
        if self.type_(t).flags.intersects(TypeFlags::ANY) {
            return true;
        }
        let Some(prop) = self.get_property_of_type(t, property_name) else {
            return false;
        };
        self.is_property_accessible(node, is_super, false /*isWrite*/, t, prop)
    }

    /// Go: `func (c *Checker) IsValidPropertyAccessForCompletions(node, t, property) bool` (services.go:198).
    pub fn is_valid_property_access_for_completions(
        &mut self,
        node: NodeId,
        t: TypeId,
        property: SymbolId,
    ) -> bool {
        let is_super = self.store().node(node).kind == Kind::PropertyAccessExpression
            && crate::ast_shims::expression_of(self.store(), node)
                .is_some_and(|e| self.store().node(e).kind == Kind::SuperKeyword);
        self.is_property_accessible(node, is_super, false /*isWrite*/, t, property)
        // Previously we validated the 'this' type of methods but this adversely
        // affected performance. See #31377 for more context.
    }

    /// Go: `func (c *Checker) GetAllPossiblePropertiesOfTypes(types) []*ast.Symbol` (services.go:209).
    pub fn get_all_possible_properties_of_types(&mut self, types: &[TypeId]) -> Vec<SymbolId> {
        let union_type = self.get_union_type(types);
        if !self.type_(union_type).flags.intersects(TypeFlags::UNION) {
            return self.get_augmented_properties_of_type(union_type);
        }

        let mut props: SymbolTable = SymbolTable::default();
        for member_type in types {
            let augmented_props = self.get_augmented_properties_of_type(*member_type);
            for p in augmented_props {
                let name = self.program_symbol(p).name.clone();
                if props.get(&name).is_none() {
                    // May be undefined if the property is private
                    if let Some(prop) = self.create_union_or_intersection_property(
                        union_type,
                        &name,
                        false, /*skipObjectFunctionPropertyAugment*/
                    ) {
                        props.set(name, prop);
                    }
                }
            }
        }
        u::symbols_to_array(&props)
    }

    /// Go: `func (c *Checker) IsUnknownSymbol(symbol) bool` (services.go:231).
    pub fn is_unknown_symbol(&self, symbol: Option<SymbolId>) -> bool {
        symbol.is_some_and(|s| s == self.unknown_symbol())
    }

    /// Go: `func (c *Checker) IsUndefinedSymbol(symbol) bool` (services.go:235).
    pub fn is_undefined_symbol(&self, symbol: Option<SymbolId>) -> bool {
        symbol.is_some_and(|s| s == self.undefined_symbol())
    }

    /// Go: `func (c *Checker) IsArgumentsSymbol(symbol) bool` (services.go:239).
    pub fn is_arguments_symbol(&self, symbol: Option<SymbolId>) -> bool {
        symbol.is_some_and(|s| Some(s) == self.arguments_symbol)
    }

    /// Go: `func (c *Checker) GetNonOptionalType(t) *Type` (services.go:244) —
    /// Originally from services.ts.
    pub fn get_non_optional_type(&mut self, t: TypeId) -> TypeId {
        self.remove_optional_type_marker(t)
    }

    /// Go: `func (c *Checker) GetStringIndexType(t) *Type` (services.go:248).
    pub fn get_string_index_type(&mut self, t: TypeId) -> Option<TypeId> {
        let string_type = self.string_type.expect("stringType");
        self.get_index_type_of_type(t, string_type)
    }

    /// Go: `func (c *Checker) GetNumberIndexType(t) *Type` (services.go:252).
    pub fn get_number_index_type(&mut self, t: TypeId) -> Option<TypeId> {
        let number_type = self.number_type.expect("numberType");
        self.get_index_type_of_type(t, number_type)
    }

    /// Go: `func (c *Checker) GetElementTypeOfArrayType(t) *Type` (services.go:256).
    pub fn get_element_type_of_array_type_ex(&mut self, t: TypeId) -> Option<TypeId> {
        self.get_element_type_of_array_type(t)
    }

    /// Go: `func (c *Checker) GetCallSignatures(t) []*Signature` (services.go:260).
    pub fn get_call_signatures(&mut self, t: TypeId) -> Vec<crate::types::SignatureId> {
        self.get_signatures_of_type(t, SignatureKind::Call)
    }

    /// Go: `func (c *Checker) GetConstructSignatures(t) []*Signature` (services.go:264).
    pub fn get_construct_signatures(&mut self, t: TypeId) -> Vec<crate::types::SignatureId> {
        self.get_signatures_of_type(t, SignatureKind::Construct)
    }

    /// Go: `func (c *Checker) GetApparentProperties(t) []*ast.Symbol` (services.go:268).
    pub fn get_apparent_properties(&mut self, t: TypeId) -> Vec<SymbolId> {
        self.get_augmented_properties_of_type(t)
    }

    /// Go: `func (c *Checker) getAugmentedPropertiesOfType(t) []*ast.Symbol` (services.go:272).
    pub(crate) fn get_augmented_properties_of_type(&mut self, t: TypeId) -> Vec<SymbolId> {
        let t = self.get_apparent_type(t);
        let mut props_by_name: SymbolTable = SymbolTable::default();
        for p in self.get_properties_of_type(t) {
            let name = self.program_symbol(p).name.clone();
            props_by_name.set(name, p);
        }
        let mut function_type: Option<TypeId> = None;
        if !self.get_signatures_of_type(t, SignatureKind::Call).is_empty() {
            function_type = self.global_callable_function_type;
        } else if !self.get_signatures_of_type(t, SignatureKind::Construct).is_empty() {
            function_type = self.global_newable_function_type;
        }
        if let Some(function_type) = function_type {
            for p in self.get_properties_of_type(function_type) {
                let name = self.program_symbol(p).name.clone();
                if props_by_name.get(&name).is_none() {
                    props_by_name.set(name, p);
                }
            }
        }
        self.get_named_members(&props_by_name, None)
    }

    /// Go: `func (c *Checker) TryGetMemberInModuleExportsAndProperties(memberName, moduleSymbol) *ast.Symbol` (services.go:295).
    pub fn try_get_member_in_module_exports_and_properties(
        &mut self,
        member_name: &str,
        module_symbol: SymbolId,
    ) -> Option<SymbolId> {
        if let Some(symbol) = self.try_get_member_in_module_exports(member_name, module_symbol) {
            return Some(symbol);
        }
        let export_equals = self.resolve_external_module_symbol(module_symbol)?;
        if export_equals == module_symbol {
            return None;
        }
        let t = self.get_type_of_symbol(export_equals);
        if self.should_treat_properties_of_external_module_as_exports(t) {
            return self.get_property_of_type(t, member_name);
        }
        None
    }

    /// Go: `func (c *Checker) TryGetMemberInModuleExports(memberName, moduleSymbol) *ast.Symbol` (services.go:313).
    pub fn try_get_member_in_module_exports(
        &mut self,
        member_name: &str,
        module_symbol: SymbolId,
    ) -> Option<SymbolId> {
        let symbol_table = self.get_exports_of_module(module_symbol);
        symbol_table.get(member_name).copied()
    }

    /// Go: `func (c *Checker) shouldTreatPropertiesOfExternalModuleAsExports(resolvedExternalModuleType) bool` (services.go:318).
    pub(crate) fn should_treat_properties_of_external_module_as_exports(
        &mut self,
        resolved_external_module_type: TypeId,
    ) -> bool {
        !self
            .type_(resolved_external_module_type)
            .flags
            .intersects(TypeFlags::PRIMITIVE)
            || self
                .type_(resolved_external_module_type)
                .object_flags
                .intersects(ObjectFlags::CLASS)
            // `isArrayOrTupleLikeType` is too expensive to use in this auto-imports hot path.
            || crate::relater::is_array_type(self, resolved_external_module_type)
            || crate::relater::is_tuple_type(self, resolved_external_module_type)
    }

    /// Go: `func (c *Checker) GetContextualType(node, contextFlags) *Type` (services.go:326).
    pub fn get_contextual_type_ex(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        if context_flags.intersects(ContextFlags::IGNORE_NODE_INFERENCES) {
            return self.run_with_inference_blocked_from_source_node(node, |c| {
                c.get_contextual_type_public(node, context_flags)
            });
        }
        self.get_contextual_type_public(node, context_flags)
    }

    /// The inner `getContextualType` read — Go's unexported worker the
    /// exported twin delegates to.
    fn get_contextual_type_public(&mut self, node: NodeId, context_flags: ContextFlags) -> Option<TypeId> {
        // PORT(S9): the checker's getContextualType worker (contextual.rs —
        // the S9 wave's permissive None carrier until the contextual depth
        // lands).
        let _ = (node, context_flags);
        None
    }

    /// Go: `func (c *Checker) runWithInferenceBlockedFromSourceNode[T](node, fn) T` (services.go:333).
    pub fn run_with_inference_blocked_from_source_node<T>(
        &mut self,
        node: NodeId,
        mut f: impl FnMut(&mut Checker) -> T,
    ) -> T {
        let containing_call = crate::ast_shims::find_ancestor(
            self.store(),
            node,
            &mut |n| is_call_like_expression_kind(n.kind),
        );
        if let Some(containing_call) = containing_call {
            let mut to_mark_skip: Option<NodeId> = Some(node);
            loop {
                if let Some(skip) = to_mark_skip {
                    self.skip_direct_inference_nodes.add(skip);
                }
                to_mark_skip = to_mark_skip
                    .and_then(|skip| nonnone(self.store().node(skip).parent.get()));
                match to_mark_skip {
                    None => break,
                    Some(n) if n == containing_call => break,
                    _ => {}
                }
            }
        }

        self.is_inference_partially_blocked = true;
        let result = self.run_without_resolved_signature_caching(node, |c| f(c));
        self.is_inference_partially_blocked = false;

        self.skip_direct_inference_nodes.clear();
        result
    }

    /// Go: `func (c *Checker) runWithoutResolvedSignatureCaching[T](node, fn) T` (services.go:366).
    ///
    /// PORT: Go snapshots the resolvedSignature/resolvedType links of every
    /// enclosing call-like/function-like ancestor, nils them, runs the fn,
    /// and restores. The port's LinkStore is keyed by NodeId — the snapshot
    /// is a Vec of (node, saved) pairs (Go's map order is unspecified; the
    /// restore is per-key idempotent).
    pub fn run_without_resolved_signature_caching<T>(
        &mut self,
        node: NodeId,
        mut f: impl FnMut(&mut Checker) -> T,
    ) -> T {
        let mut ancestor_node = crate::ast_shims::find_ancestor(
            self.store(),
            node,
            &mut |n| is_call_like_or_function_like_expression_kind(n.kind),
        );
        if ancestor_node.is_some() {
            let mut saved_signatures: Vec<(NodeId, Option<crate::types::SignatureId>)> = Vec::new();
            let mut saved_types: Vec<(SymbolId, Option<TypeId>)> = Vec::new();
            while let Some(ancestor) = ancestor_node {
                let resolved = self.signature_node_links.get(ancestor).resolved_signature;
                saved_signatures.push((ancestor, resolved));
                self.signature_node_links.get(ancestor).resolved_signature = None;
                if is_function_expression_or_arrow_function_kind(self.store().node(ancestor).kind) {
                    if let Some(symbol) = self.get_symbol_of_declaration(ancestor) {
                        let resolved_type = self.value_symbol_links.get(symbol).resolved_type;
                        saved_types.push((symbol, resolved_type));
                        self.value_symbol_links.get(symbol).resolved_type = None;
                    }
                }
                let parent = self.store().node(ancestor).parent.get();
                if parent.is_none() {
                    break;
                }
                ancestor_node = crate::ast_shims::find_ancestor(
                    self.store(),
                    parent,
                    &mut |n| is_call_like_or_function_like_expression_kind(n.kind),
                );
            }
            let result = f(self);
            for (node, resolved) in saved_signatures {
                self.signature_node_links.get(node).resolved_signature = resolved;
            }
            for (symbol, resolved) in saved_types {
                self.value_symbol_links.get(symbol).resolved_type = resolved;
            }
            return result;
        }
        f(self)
    }

    /// Go: `func (c *Checker) SkipAlias(symbol) *ast.Symbol` (services.go:395).
    pub fn skip_alias(&mut self, symbol: SymbolId) -> SymbolId {
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::ALIAS) {
            return self.get_aliased_symbol(symbol);
        }
        symbol
    }

    /// Go: `func (c *Checker) GetRootSymbols(symbol) []*ast.Symbol` (services.go:402).
    pub fn get_root_symbols(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        let roots = self.get_immediate_root_symbols(symbol);
        if roots.is_empty() {
            return vec![symbol];
        }
        let mut result = Vec::new();
        for root in roots {
            result.extend(self.get_root_symbols(root));
        }
        result
    }

    /// Go: `func (c *Checker) GetMappedTypeSymbolOfProperty(symbol) *ast.Symbol` (services.go:414).
    pub fn get_mapped_type_symbol_of_property(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        let containing_type = self.value_symbol_links.try_get(symbol)?.containing_type?;
        self.type_(containing_type).symbol
    }

    /// Go: `func (c *Checker) getImmediateRootSymbols(symbol) []*ast.Symbol` (services.go:421).
    fn get_immediate_root_symbols(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        if self.program_symbol(symbol).check_flags.intersects(CheckFlags::SYNTHETIC) {
            let Some(containing_type) = self
                .value_symbol_links
                .try_get(symbol)
                .and_then(|l| l.containing_type)
            else {
                return Vec::new();
            };
            let name = self.program_symbol(symbol).name.clone();
            let types = self.type_(containing_type).types().to_vec();
            let mut result = Vec::new();
            for t in types {
                if let Some(property) = self.get_property_of_type(t, &name) {
                    result.push(property);
                }
            }
            return result;
        }
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::TRANSIENT) {
            // PORT(M6 S8): spreadLinks (leftSpread/rightSpread) and
            // mappedSymbolLinks (syntheticOrigin) — the spread-augmentation
            // and mapped-type property waves own them; dormant arms.
            if let Some(target) = self.try_get_target(symbol) {
                return vec![target];
            }
        }
        Vec::new()
    }

    /// Go: `func (c *Checker) tryGetTarget(symbol) *ast.Symbol` (services.go:452).
    ///
    /// PORT: Go walks valueSymbolLinks/exportTypeLinks target pointers; the
    /// exportTypeLinks half is the S8 export-type wave (dormant arm).
    fn try_get_target(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        let mut target: Option<SymbolId> = None;
        let mut next = Some(symbol);
        while let Some(current) = next {
            let linked = self.value_symbol_links.try_get(current).and_then(|l| l.target);
            next = linked;
            if next.is_none() {
                break;
            }
            target = next;
        }
        target
    }

    /// Go: `func (c *Checker) GetExportSymbolOfSymbol(symbol) *ast.Symbol` (services.go:471).
    pub fn get_export_symbol_of_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        let export_symbol = self.program_symbol(symbol).export_symbol;
        let base = export_symbol.unwrap_or(symbol);
        self.get_merged_symbol(base)
    }

    /// Go: `func (c *Checker) GetExportSpecifierLocalTargetSymbol(node) *ast.Symbol` (services.go:475).
    pub fn get_export_specifier_local_target_symbol(&mut self, node: NodeId) -> Option<SymbolId> {
        // node should be ExportSpecifier | Identifier
        match self.store().node(node).kind {
            Kind::ExportSpecifier => {
                let parent = self.store().node(node).parent.get();
                let grandparent = self.store().node(parent).parent.get();
                if crate::ast_shims::module_specifier_of(self.store(), grandparent).is_some() {
                    return self.get_external_module_member(grandparent, node, false /*dontResolveAlias*/);
                }
                let d = self
                    .store()
                    .node(node)
                    .as_export_specifier()
                    .expect("ExportSpecifier data");
                let name = d.property_name.or(Some(d.name));
                // Go: PropertyNameOrName — property name first, then name.
                if let Some(name) = name
                    && self.store().node(name).kind == Kind::StringLiteral
                {
                    // Skip for invalid syntax like this: export { "x" }
                    return None;
                }
                let name = name?;
                self.resolve_entity_name(
                    name,
                    SymbolFlags::VALUE
                        .union(SymbolFlags::TYPE)
                        .union(SymbolFlags::NAMESPACE)
                        .union(SymbolFlags::ALIAS),
                    true,  /*ignoreErrors*/
                    false, /*dontResolveAlias*/
                    NodeId::NONE,
                )
            }
            Kind::Identifier => self.resolve_entity_name(
                node,
                SymbolFlags::VALUE
                    .union(SymbolFlags::TYPE)
                    .union(SymbolFlags::NAMESPACE)
                    .union(SymbolFlags::ALIAS),
                true,  /*ignoreErrors*/
                false, /*dontResolveAlias*/
                NodeId::NONE,
            ),
            _ => panic!(
                "Unhandled case in getExportSpecifierLocalTargetSymbol, node should be ExportSpecifier | Identifier"
            ),
        }
    }

    /// Go: `func (c *Checker) GetShorthandAssignmentValueSymbol(location) *ast.Symbol` (services.go:494).
    pub fn get_shorthand_assignment_value_symbol(
        &mut self,
        location: Option<NodeId>,
    ) -> Option<SymbolId> {
        if let Some(location) = location
            && self.store().node(location).kind == Kind::ShorthandPropertyAssignment
        {
            let name = self.store().node(location).name()?;
            return self.resolve_entity_name(
                name,
                SymbolFlags::VALUE.union(SymbolFlags::ALIAS),
                true,  /*ignoreErrors*/
                false, /*dontResolveAlias*/
                NodeId::NONE,
            );
        }
        None
    }

    /// Go: `func (c *Checker) GetSymbolsOfParameterPropertyDeclaration(parameter, parameterName) (*ast.Symbol, *ast.Symbol)` (services.go:507).
    ///
    /// Get symbols that represent parameter-property-declaration as parameter
    /// and as property declaration (Go's doc comment).
    pub fn get_symbols_of_parameter_property_declaration(
        &mut self,
        parameter: NodeId,
        parameter_name: &str,
    ) -> Option<(SymbolId, SymbolId)> {
        let constructor_declaration = self.store().node(parameter).parent.get();
        let class_declaration = self.store().node(constructor_declaration).parent.get();

        let constructor_locals = tsc_binder::node_data::locals_of(self.store(), constructor_declaration)
            .cloned()
            .unwrap_or_default();
        let parameter_symbol = self.get_symbol(&constructor_locals, parameter_name, SymbolFlags::VALUE);

        let class_symbol = tsc_binder::node_data::symbol_of(self.store(), class_declaration)?;
        let members = self.get_members_of_symbol_table(class_symbol);
        let property_symbol = self.get_symbol(&members, parameter_name, SymbolFlags::VALUE);

        if let (Some(parameter_symbol), Some(property_symbol)) = (parameter_symbol, property_symbol) {
            return Some((parameter_symbol, property_symbol));
        }

        panic!("There should exist two symbols, one as property declaration and one as parameter declaration")
    }

    /// Go: `func (c *Checker) IsDeclarationUsed(sourceFile, identifier, jsxElementsPresent, jsxModeNeedsExplicitImport) bool` (services.go:523).
    ///
    /// IsDeclarationUsed checks if an import declaration identifier is used
    /// in the source file (Go's doc comment — organizing imports).
    pub fn is_declaration_used(
        &mut self,
        source_file: NodeId,
        identifier: NodeId,
        jsx_elements_present: bool,
        jsx_mode_needs_explicit_import: bool,
    ) -> bool {
        if jsx_elements_present && jsx_mode_needs_explicit_import {
            let jsx_namespace = self.get_jsx_namespace(source_file);
            let jsx_fragment_factory = self.get_jsx_fragment_factory(source_file);
            let identifier_text = tsc_ast::node_text(self.store(), identifier);
            if identifier_text == jsx_namespace {
                return true;
            }
            if !jsx_fragment_factory.is_empty() && identifier_text == jsx_fragment_factory {
                return true;
            }
        }

        let Some(symbol) = self.get_symbol_at_location_stub(identifier, true) else {
            return true;
        };

        self.is_symbol_referenced_in_file(source_file, identifier, symbol)
    }

    /// Go: `func (c *Checker) IsSymbolReferencedInFile(sourceFile, definition, symbol) bool` (services.go:551).
    pub fn is_symbol_referenced_in_file(
        &mut self,
        source_file: NodeId,
        definition: NodeId,
        symbol: SymbolId,
    ) -> bool {
        let identifier_text = tsc_ast::node_text(self.store(), definition).to_string();
        for token in get_possible_symbol_reference_nodes(self, source_file, &identifier_text) {
            if !is_identifier(self.store().node(token)) {
                continue;
            }
            if token == definition || tsc_ast::node_text(self.store(), token) != identifier_text {
                continue;
            }
            let ref_symbol = self.get_symbol_at_location_stub(token, true);
            if ref_symbol == Some(symbol) {
                return true;
            }
            let parent = self.store().node(token).parent.get();
            if !parent.is_none() && self.store().node(parent).kind == Kind::ShorthandPropertyAssignment {
                let shorthand_symbol = self.get_shorthand_assignment_value_symbol(Some(parent));
                if shorthand_symbol == Some(symbol) {
                    return true;
                }
            }
            if !parent.is_none()
                && tsc_ast::ast_generated::is_export_specifier(self.store().node(parent))
            {
                let Some(ref_symbol) = ref_symbol else { continue };
                let local_symbol = self.get_local_symbol_for_export_specifier(token, ref_symbol, parent);
                if local_symbol == Some(symbol) {
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) GetReferencesToSymbolInFile(sourceFile, symbol) []*ast.Node` (services.go:586).
    pub fn get_references_to_symbol_in_file(
        &mut self,
        source_file: NodeId,
        symbol: SymbolId,
    ) -> Vec<NodeId> {
        let identifier_text = self.program_symbol(symbol).name.clone();
        let mut result = Vec::new();
        for token in get_possible_symbol_reference_nodes(self, source_file, &identifier_text) {
            if !is_identifier(self.store().node(token)) {
                continue;
            }
            if tsc_ast::node_text(self.store(), token) != identifier_text {
                continue;
            }
            let ref_symbol = self.get_symbol_at_location_stub(token, true);
            if ref_symbol == Some(symbol) {
                result.push(token);
                continue;
            }
            let parent = self.store().node(token).parent.get();
            if !parent.is_none() && self.store().node(parent).kind == Kind::ShorthandPropertyAssignment {
                let shorthand_symbol = self.get_shorthand_assignment_value_symbol(Some(parent));
                if shorthand_symbol == Some(symbol) {
                    result.push(token);
                    continue;
                }
            }
            if !parent.is_none()
                && tsc_ast::ast_generated::is_export_specifier(self.store().node(parent))
            {
                let Some(ref_symbol) = ref_symbol else { continue };
                let local_symbol = self.get_local_symbol_for_export_specifier(token, ref_symbol, parent);
                if local_symbol == Some(symbol) {
                    result.push(token);
                    continue;
                }
            }
        }
        result
    }

    /// Go: `func (c *Checker) getLocalSymbolForExportSpecifier(referenceLocation, referenceSymbol, exportSpecifier) *ast.Symbol` (services.go:623).
    fn get_local_symbol_for_export_specifier(
        &mut self,
        reference_location: NodeId,
        reference_symbol: SymbolId,
        export_specifier: NodeId,
    ) -> Option<SymbolId> {
        if is_export_specifier_alias(self, reference_location, export_specifier) {
            if let Some(symbol) = self.get_export_specifier_local_target_symbol(export_specifier) {
                return Some(symbol);
            }
        }
        Some(reference_symbol)
    }

    /// Go: `func (c *Checker) GetTypeArgumentConstraint(node) *Type` (services.go:699).
    pub fn get_type_argument_constraint_public(&mut self, node: NodeId) -> Option<TypeId> {
        if !is_type_node(self.store().node(node)) {
            return None;
        }
        self.get_type_argument_constraint(node)
    }

    /// Go: `func (c *Checker) getUninstantiatedSignatures(node) []*Signature` (services.go:707).
    ///
    /// Gets generic signatures from the function's/constructor's type.
    fn get_uninstantiated_signatures(&mut self, node: NodeId) -> Vec<crate::types::SignatureId> {
        match self.store().node(node).kind {
            Kind::CallExpression | Kind::Decorator => {
                let expression = crate::ast_shims::expression_of(self.store(), node)
                    .expect("CallExpression/Decorator expression");
                let t = self.get_type_of_expression(expression);
                self.get_signatures_of_type(t, SignatureKind::Call)
            }
            Kind::NewExpression => {
                let expression = crate::ast_shims::expression_of(self.store(), node)
                    .expect("NewExpression expression");
                let t = self.get_type_of_expression(expression);
                self.get_signatures_of_type(t, SignatureKind::Construct)
            }
            Kind::JsxSelfClosingElement | Kind::JsxOpeningElement => {
                // PORT(jsx wave): isJsxIntrinsicTagName — the jsx wave's
                // predicate; the intrinsic arm stays dormant.
                let tag_name = self
                    .store()
                    .node(node)
                    .as_jsx_self_closing_element()
                    .map(|d| d.tag_name)
                    .or_else(|| {
                        self.store()
                            .node(node)
                            .as_jsx_opening_element()
                            .map(|d| d.tag_name)
                    });
                let Some(tag_name) = tag_name else { return Vec::new() };
                let t = self.get_type_of_expression(tag_name);
                self.get_signatures_of_type(t, SignatureKind::Call)
            }
            Kind::TaggedTemplateExpression => {
                let tag = self
                    .store()
                    .node(node)
                    .as_tagged_template_expression()
                    .expect("TaggedTemplateExpression data")
                    .tag;
                let t = self.get_type_of_expression(tag);
                self.get_signatures_of_type(t, SignatureKind::Call)
            }
            _ => Vec::new(),
        }
    }

    /// Go: `func (c *Checker) getTypeParameterConstraintForPositionAcrossSignatures(signatures, position) *Type` (services.go:726).
    fn get_type_parameter_constraint_for_position_across_signatures(
        &mut self,
        signatures: &[crate::types::SignatureId],
        position: usize,
    ) -> Option<TypeId> {
        let mut relevant_constraints: Vec<TypeId> = Vec::new();
        for &signature in signatures {
            let type_parameters = self.signatures[signature.0 as usize]
                .type_parameters
                .clone();
            if position >= type_parameters.len() {
                continue;
            }
            let relevant_type_parameter = type_parameters[position];
            if let Some(relevant_constraint) =
                self.get_constraint_of_type_parameter(relevant_type_parameter)
            {
                relevant_constraints.push(relevant_constraint);
            }
        }
        if relevant_constraints.is_empty() {
            // Go: getUnionType(nil) — neverType; the Option carries Go's nil.
            return None;
        }
        Some(self.get_union_type(&relevant_constraints))
    }

    /// Go: `func (c *Checker) getTypeArgumentConstraint(node) *Type` (services.go:741).
    pub(crate) fn get_type_argument_constraint(&mut self, node: NodeId) -> Option<TypeId> {
        let mut type_argument_position: i64 = -1;
        let parent = self.store().node(node).parent.get();
        if !parent.is_none() {
            let type_args = crate::statements::type_arguments_of(self.store(), parent);
            for (i, arg) in type_args.iter().enumerate() {
                if *arg == node {
                    type_argument_position = i as i64;
                    break;
                }
            }
        }

        if type_argument_position >= 0 {
            // The node could be a type argument of a call, a `new` expression, a decorator, an
            // instantiation expression, or a generic type instantiation.

            if is_call_like_expression_kind(self.store().node(parent).kind) {
                let signatures = self.get_uninstantiated_signatures(parent);
                return self.get_type_parameter_constraint_for_position_across_signatures(
                    &signatures,
                    type_argument_position as usize,
                );
            }

            let grandparent = self.store().node(parent).parent.get();
            if !grandparent.is_none()
                && self.store().node(grandparent).kind == Kind::Decorator
            {
                let signatures = self.get_uninstantiated_signatures(grandparent);
                return self.get_type_parameter_constraint_for_position_across_signatures(
                    &signatures,
                    type_argument_position as usize,
                );
            }

            if !grandparent.is_none()
                && self.store().node(parent).kind == Kind::ExpressionWithTypeArguments
                && self.store().node(grandparent).kind == Kind::ExpressionStatement
            {
                let expression = crate::ast_shims::expression_of(self.store(), parent)
                    .expect("ExpressionWithTypeArguments expression");
                let uninstantiated_type = self.check_expression(expression);

                let call_signatures = self.get_signatures_of_type(uninstantiated_type, SignatureKind::Call);
                let call_constraint = self.get_type_parameter_constraint_for_position_across_signatures(
                    &call_signatures,
                    type_argument_position as usize,
                );
                let construct_signatures =
                    self.get_signatures_of_type(uninstantiated_type, SignatureKind::Construct);
                let construct_constraint = self
                    .get_type_parameter_constraint_for_position_across_signatures(
                        &construct_signatures,
                        type_argument_position as usize,
                    );

                // An instantiation expression instantiates both call and construct signatures, so
                // if both exist type arguments must be assignable to both constraints.
                if let Some(construct) = construct_constraint
                    && self.type_(construct).flags.intersects(TypeFlags::NEVER)
                {
                    return call_constraint;
                }
                if let Some(call) = call_constraint
                    && self.type_(call).flags.intersects(TypeFlags::NEVER)
                {
                    return construct_constraint;
                }
                if let (Some(call), Some(construct)) = (call_constraint, construct_constraint) {
                    return Some(self.get_intersection_type(&[call, construct]));
                }
                return None;
            }

            if self.store().node(parent).kind == Kind::TypeReference {
                let type_parameters = self.get_type_parameters_for_type_reference_or_import(parent);
                if type_parameters.is_empty() {
                    return None;
                }
                if type_argument_position as usize >= type_parameters.len() {
                    return None;
                }
                let relevant_type_parameter = type_parameters[type_argument_position as usize];
                if let Some(constraint) =
                    self.get_constraint_of_type_parameter(relevant_type_parameter)
                {
                    let effective = self.get_effective_type_arguments(parent, &type_parameters);
                    let mapper = self.new_type_mapper(&type_parameters, &effective);
                    return Some(self.instantiate_type(constraint, mapper));
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) IsTypeInvalidDueToUnionDiscriminant(contextualType, obj) bool` (services.go:815).
    pub fn is_type_invalid_due_to_union_discriminant(
        &mut self,
        contextual_type: TypeId,
        obj: NodeId,
    ) -> bool {
        let properties = self
            .store()
            .node(obj)
            .as_object_literal_expression()
            .map(|d| d.properties.clone())
            .map(|l| l.map(|l| l.nodes.to_vec()).unwrap_or_default())
            .unwrap_or_default();
        for property in properties {
            let mut name_type: Option<TypeId> = None;
            let property_name = self.store().node(property).name();
            if let Some(property_name) = property_name {
                if tsc_ast::ast_generated::is_jsx_namespaced_name(self.store().node(property_name)) {
                    let text = tsc_ast::node_text(self.store(), property_name).to_string();
                    name_type = Some(self.get_string_literal_type(&text));
                } else {
                    name_type = Some(self.get_literal_type_from_property_name(property_name));
                }
            }
            let mut name = String::new();
            if let Some(name_type) = name_type
                && u::is_type_usable_as_property_name(self.type_(name_type))
            {
                name = u::get_property_name_from_type(self.type_(name_type));
            }
            let expected = if !name.is_empty() {
                self.get_type_of_property_of_type(contextual_type, &name)
            } else {
                None
            };
            if let Some(expected) = expected
                && crate::relater::is_literal_type(self, expected)
            {
                // PORT(M7): Go's c.getTypeOfNode(property) — the M7
                // property-assignment typing worker; permissive false keeps
                // the discriminant probe dormant until it lands.
                return false;
            }
        }
        false
    }

    /// Go: `func (c *Checker) GetExportsAndPropertiesOfModule(moduleSymbol) []*ast.Symbol` (services.go:840).
    ///
    /// Unlike `getExportsOfModule`, this includes properties of an `export =` value.
    pub fn get_exports_and_properties_of_module(&mut self, module_symbol: SymbolId) -> Vec<SymbolId> {
        let mut exports = self.get_exports_of_module_as_array(module_symbol);
        let export_equals = self.resolve_external_module_symbol(module_symbol);
        if let Some(export_equals) = export_equals
            && export_equals != module_symbol
        {
            let t = self.get_type_of_symbol(export_equals);
            if self.should_treat_properties_of_external_module_as_exports(t) {
                exports.extend(self.get_properties_of_type(t));
            }
        }
        exports
    }

    /// Go: `func (c *Checker) getExportsOfModuleAsArray(moduleSymbol) []*ast.Symbol` (services.go:852).
    fn get_exports_of_module_as_array(&mut self, module_symbol: SymbolId) -> Vec<SymbolId> {
        u::symbols_to_array(&self.get_exports_of_module(module_symbol))
    }

    /// Go: `func (c *Checker) GetJsxIntrinsicTagNamesAt(location) []*ast.Symbol` (services.go:857).
    ///
    /// Returns all the properties of the Jsx.IntrinsicElements interface.
    pub fn get_jsx_intrinsic_tag_names_at(&mut self, location: NodeId) -> Vec<SymbolId> {
        // PORT(jsx wave): getJsxType(JsxNames.IntrinsicElements, location)
        // — the jsx wave's resolver; permissive empty.
        let _ = location;
        Vec::new()
    }

    /// Go: `func (c *Checker) GetContextualTypeForJsxAttribute(attribute) *Type` (services.go:865).
    pub fn get_contextual_type_for_jsx_attribute(&mut self, attribute: NodeId) -> Option<TypeId> {
        // PORT(jsx wave): getContextualTypeForJsxAttribute — same wave.
        let _ = attribute;
        None
    }

    /// Go: `func (c *Checker) GetConstantValue(node) any` (services.go:869).
    pub fn get_constant_value(&mut self, node: NodeId) -> Option<EvaluatorValue> {
        if self.store().node(node).kind == Kind::EnumMember {
            return self.get_enum_member_value(node).value;
        }

        if self.symbol_node_links.get(node).resolved_symbol.is_none() {
            // ensure cached resolved symbol is set
            let _ = self.check_expression_cached(node);
        }
        let mut symbol = self.symbol_node_links.get(node).resolved_symbol;
        if symbol.is_none() && u::is_entity_name_expression(self.store(), node) {
            symbol = self.resolve_entity_name(
                node,
                SymbolFlags::VALUE,
                true,  /*ignoreErrors*/
                false, /*dontResolveAlias*/
                NodeId::NONE,
            );
        }
        if let Some(symbol) = symbol
            && self
                .program_symbol(symbol)
                .flags
                .intersects(SymbolFlags::ENUM_MEMBER)
        {
            // inline property\index accesses only for const enums
            let member = self.program_symbol(symbol).value_declaration?;
            let parent = self.store().node(member).parent.get();
            if tsc_binder::ast_shims::is_enum_const(self.store(), parent) {
                return self.get_enum_member_value(member).value;
            }
        }

        None
    }

    /// Go: `func (c *Checker) getResolvedSignatureWorker(node, checkMode, argumentCount) (*Signature, []*Signature)` (services.go:898).
    fn get_resolved_signature_worker(
        &mut self,
        node: NodeId,
        check_mode: CheckMode,
        argument_count: i32,
    ) -> (Option<crate::types::SignatureId>, Vec<crate::types::SignatureId>) {
        self.apparent_argument_count = Some(argument_count);
        let mut candidates: Vec<crate::types::SignatureId> = Vec::new();
        let res = if !node.is_none() {
            self.get_resolved_signature_ex(node, Some(&mut candidates), check_mode)
        } else {
            None
        };
        self.apparent_argument_count = None;
        (res, candidates)
    }

    /// Go: `func (c *Checker) GetCandidateSignaturesForStringLiteralCompletions(call, editingArgument) []*Signature` (services.go:909).
    pub fn get_candidate_signatures_for_string_literal_completions(
        &mut self,
        call: NodeId,
        editing_argument: NodeId,
    ) -> Vec<crate::types::SignatureId> {
        // first, get candidates when inference is blocked from the source node.
        let candidates = self.run_with_inference_blocked_from_source_node(editing_argument, |c| {
            let (_, blocked_inference_candidates) =
                c.get_resolved_signature_worker(call, CheckMode::CHECK_MODE_NORMAL, 0);
            blocked_inference_candidates
        });
        let mut candidates_set: std::collections::HashSet<crate::types::SignatureId> =
            candidates.iter().copied().collect();

        // next, get candidates where the source node is considered for inference.
        let other_candidates = self.run_without_resolved_signature_caching(editing_argument, |c| {
            let (_, inference_candidates) =
                c.get_resolved_signature_worker(call, CheckMode::CHECK_MODE_NORMAL, 0);
            inference_candidates
        });

        let mut result = candidates;
        for candidate in other_candidates {
            if candidates_set.contains(&candidate) {
                continue;
            }
            candidates_set.insert(candidate);
            result.push(candidate);
        }

        result
    }

    /// Go: `func (c *Checker) GetTypeAtPosition(s, pos) *Type` (services.go:934).
    ///
    /// GetTypeAtPosition returns the type of a parameter at a given index in
    /// a signature (Go's doc comment).
    pub fn get_type_at_position_public(
        &mut self,
        s: crate::types::SignatureId,
        pos: usize,
    ) -> TypeId {
        self.get_type_at_position(s, pos)
    }

    /// Go: `func (c *Checker) GetTypeParameterAtPosition(s, pos) *Type` (services.go:938).
    pub fn get_type_parameter_at_position(
        &mut self,
        s: crate::types::SignatureId,
        pos: usize,
    ) -> TypeId {
        let t = self.get_type_at_position(s, pos);
        // PORT(M6 S8): Go's `t.IsIndex() && isThisTypeParameter(t.AsIndex-
        // Type().target)` arm — the index-type this-parameter probe rides
        // the S8 structured-members wave; permissive pass-through.
        t
    }

    /// Go: `func (c *Checker) GetContextualTypeForArrayLiteralAtPosition(contextualArrayType, arrayLiteral, position) *Type` (services.go:951).
    ///
    /// GetContextualTypeForArrayLiteralAtPosition returns the contextual
    /// type for an element at the given position in an array with the given
    /// contextual type (Go's doc comment).
    pub fn get_contextual_type_for_array_literal_at_position(
        &mut self,
        contextual_array_type: Option<TypeId>,
        array_literal: NodeId,
        position: i64,
    ) -> Option<TypeId> {
        let contextual_array_type = contextual_array_type?;
        let mut first_spread_index: i32 = -1;
        let mut last_spread_index: i32 = -1;
        let mut element_index = 0usize;
        let elements = self
            .store()
            .node(array_literal)
            .as_array_literal_expression()
            .map(|d| d.elements.clone())
            .map(|l| l.map(|l| l.nodes.to_vec()).unwrap_or_default())
            .unwrap_or_default();
        for (i, elem) in elements.iter().enumerate() {
            if (self.store().node(*elem).pos() as i64) < position {
                element_index += 1;
            }
            if tsc_ast::ast_generated::is_spread_element(self.store().node(*elem)) {
                if first_spread_index == -1 {
                    first_spread_index = i as i32;
                }
                last_spread_index = i as i32;
            }
        }
        // The array may be incomplete, so we don't know its final length.
        self.get_contextual_type_for_element_expression(
            Some(contextual_array_type),
            element_index,
            -1, /*length*/
            first_spread_index,
            last_spread_index,
        )
    }

    /// Go: `func (c *Checker) GetFirstTypeArgumentFromKnownType(t) *Type` (services.go:1007).
    pub fn get_first_type_argument_from_known_type(&mut self, t: TypeId) -> Option<TypeId> {
        // Snapshot the reference arm's reads (Go reads the same fields; the
        // Rust split keeps the Program borrow disjoint from &mut self).
        let (reference_symbol, reference_target) = {
            let data = self.type_(t);
            if data.object_flags.intersects(ObjectFlags::REFERENCE) {
                (
                    data.symbol,
                    data.target().and_then(|target| self.type_(target).symbol),
                )
            } else {
                (None, None)
            }
        };
        if let Some(symbol) = reference_symbol {
            let name = self.program_symbol(symbol).name.clone();
            if is_known_generic_type_name(&name) {
                let resolved = self.get_global_symbol(&name, SymbolFlags::TYPE, None);
                if let Some(symbol) = resolved
                    && Some(symbol) == reference_target
                {
                    let args = self.get_type_arguments(t);
                    return args.into_iter().next();
                }
            }
        }
        let alias = self.type_(t).alias.clone();
        if let Some(alias) = alias {
            if let Some(alias_symbol) = alias.symbol
                && is_known_generic_type_name(&self.program_symbol(alias_symbol).name)
            {
                let name = self.program_symbol(alias_symbol).name.clone();
                let resolved = self.get_global_symbol(&name, SymbolFlags::TYPE, None);
                if let Some(symbol) = resolved
                    && Some(symbol) == alias.symbol
                {
                    return alias.type_arguments.first().copied();
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) GetPropertySymbolsFromContextualType(node, contextualType, unionSymbolOk) []*ast.Symbol` (services.go:1024).
    ///
    /// Gets all symbols for one property. Does not get symbols for every
    /// property (Go's doc comment).
    pub fn get_property_symbols_from_contextual_type(
        &mut self,
        node: NodeId,
        contextual_type: TypeId,
        union_symbol_ok: bool,
    ) -> Vec<SymbolId> {
        // PORT(M7): Go's ast.GetTextOfPropertyName(node.Name()) — the
        // property-name text accessor (computed names) rides the M7
        // utilities wave; the identifier/literal names read directly.
        let name = self
            .store()
            .node(node)
            .name()
            .map(|n| tsc_ast::node_text(self.store(), n))
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return Vec::new();
        }
        if !self.type_(contextual_type).flags.intersects(TypeFlags::UNION) {
            if let Some(symbol) = self.get_property_of_type(contextual_type, &name) {
                return vec![symbol];
            }
            return Vec::new();
        }
        let all_types = self.type_(contextual_type).types().to_vec();
        let mut filtered_types = all_types.clone();
        let parent = self.store().node(node).parent.get();
        if !parent.is_none()
            && (tsc_ast::ast_generated::is_object_literal_expression(self.store().node(parent))
                || self.store().node(parent).kind == Kind::JsxAttributes)
        {
            filtered_types.retain(|&t| !self.is_type_invalid_due_to_union_discriminant(t, parent));
        }
        let mut discriminated_property_symbols: Vec<Option<SymbolId>> = filtered_types
            .iter()
            .map(|&t| self.get_property_of_type(t, &name))
            .collect();
        if union_symbol_ok
            && (discriminated_property_symbols.is_empty()
                || discriminated_property_symbols.len() == all_types.len())
        {
            if let Some(symbol) = self.get_property_of_type(contextual_type, &name) {
                return vec![symbol];
            }
        }
        if filtered_types.is_empty() && discriminated_property_symbols.is_empty() {
            // Bad discriminant -- do again without discriminating
            discriminated_property_symbols = all_types
                .iter()
                .map(|&t| self.get_property_of_type(t, &name))
                .collect();
        }
        // by eliminating duplicates we might even end up with a single symbol
        // that helps with displaying better quick infos on properties of union types
        let mut seen: std::collections::HashSet<SymbolId> = std::collections::HashSet::new();
        let mut result = Vec::new();
        for symbol in discriminated_property_symbols.into_iter().flatten() {
            if seen.insert(symbol) {
                result.push(symbol);
            }
        }
        result
    }

    /// Go: `func (c *Checker) GetPropertySymbolOfDestructuringAssignment(location) *ast.Symbol` (services.go:1069).
    ///
    /// Gets the property symbol corresponding to the property in
    /// destructuring assignment (Go's doc comment).
    pub fn get_property_symbol_of_destructuring_assignment(
        &mut self,
        location: NodeId,
    ) -> Option<SymbolId> {
        let parent = self.store().node(location).parent.get();
        if parent.is_none() {
            return None;
        }
        let grandparent = self.store().node(parent).parent.get();
        if !grandparent.is_none() && is_array_literal_or_object_literal_destructuring_pattern(self, grandparent)
        {
            // Get the type of the object or array literal and then look for property of given name in the type
            if let Some(type_of_object_literal) = self.get_type_of_assignment_pattern(grandparent) {
                let name = tsc_ast::node_text(self.store(), location).to_string();
                return self.get_property_of_type(type_of_object_literal, &name);
            }
        }
        None
    }

    /// Go: `func (c *Checker) getTypeOfAssignmentPattern(expr) *Type` (services.go:1088).
    ///
    /// Gets the type of object literal or array literal of destructuring
    /// assignment (Go's doc comment).
    fn get_type_of_assignment_pattern(&mut self, expr: NodeId) -> Option<TypeId> {
        let parent = self.store().node(expr).parent.get();
        // If this is from "for of"
        //     for ( { a } of elems) {
        //     }
        if tsc_ast::ast_generated::is_for_of_statement(self.store().node(parent)) {
            // PORT: the wrapper answers the errorType where Go's nil
            // iterated type flowed through (typeofsymbol.rs's S2 shape).
            let iterated_type = self.check_right_hand_side_of_for_of(parent);
            return Some(self.check_destructuring_assignment(
                expr,
                iterated_type,
                CheckMode::CHECK_MODE_NORMAL,
                false,
            ));
        }
        // If this is from "for" initializer
        //     for ({a } = elems[0];.....) { }
        if tsc_ast::ast_generated::is_binary_expression(self.store().node(parent)) {
            let right = self
                .store()
                .node(parent)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .right;
            let iterated_type = self.get_type_of_expression(right);
            return Some(self.check_destructuring_assignment(
                expr,
                iterated_type,
                CheckMode::CHECK_MODE_NORMAL,
                false,
            ));
        }
        // If this is from nested object binding pattern
        //     for ({ skills: { primary, secondary } } = multiRobot, i = 0; i < 1; i++) {
        if self.store().node(parent).kind == Kind::PropertyAssignment {
            let node = self.store().node(parent).parent.get();
            let type_of_parent_object_literal = self
                .get_type_of_assignment_pattern(node)
                .unwrap_or_else(|| self.error_type());
            let properties = self
                .store()
                .node(node)
                .as_object_literal_expression()
                .map(|d| d.properties.clone())
                .and_then(|l| l.map(|l| l.nodes.to_vec()))
                .unwrap_or_default();
            let property_index = properties.iter().position(|&p| p == parent).unwrap_or(0);
            return self.check_object_literal_destructuring_property_assignment(
                node,
                type_of_parent_object_literal,
                property_index,
                None,
                false,
            );
        }
        // Array literal assignment - array destructuring pattern
        let node = parent;
        //    [{ property1: p1, property2 }] = elems;
        let type_of_array_literal = self
            .get_type_of_assignment_pattern(node)
            .unwrap_or_else(|| self.error_type());
        let undefined_type = self.undefined_type();
        let element_type = self.check_iterated_type_or_element_type(
            crate::statements::IterationUse::DESTRUCTURING,
            type_of_array_literal,
            undefined_type,
            Some(node),
        );
        let elements = self
            .store()
            .node(node)
            .as_array_literal_expression()
            .map(|d| d.elements.clone())
            .and_then(|l| l.map(|l| l.nodes.to_vec()))
            .unwrap_or_default();
        let index = elements.iter().position(|&e| e == expr).unwrap_or(0);
        self.check_array_literal_destructuring_element_assignment(
            node,
            type_of_array_literal,
            index,
            element_type,
            CheckMode::CHECK_MODE_NORMAL,
        )
    }

    /// Go: `func (c *Checker) GetSignatureFromDeclaration(node) *Signature` (services.go:1118).
    pub fn get_signature_from_declaration_public(
        &mut self,
        node: NodeId,
    ) -> Option<crate::types::SignatureId> {
        self.get_signature_from_declaration(node)
    }

    /// Go: `func (c *Checker) IsLibSymbolForHoverVerbosity(symbol) bool` (services.go:1123).
    ///
    /// IsLibSymbolForHoverVerbosity returns true if a symbol is declared in
    /// a lib file (Go's doc comment).
    pub fn is_lib_symbol_for_hover_verbosity(&self, symbol: Option<SymbolId>) -> bool {
        let Some(symbol) = symbol else { return false };
        for &decl in &self.program_symbol(symbol).declarations {
            let file = tsc_ast::utilities::get_source_file_of_node(self.store(), decl);
            if let Some(file) = file
                && self.program.is_source_file_default_library(file.as_node_id())
            {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) IsLibTypeForHoverVerbosity(t) bool` (services.go:1138).
    ///
    /// IsLibTypeForHoverVerbosity returns true if a type is declared in a
    /// lib file. Don't expand types like Array or Promise, instead treating
    /// them as opaque (Go's doc comment).
    pub fn is_lib_type_for_hover_verbosity(&mut self, t: TypeId) -> bool {
        let symbol = if self.type_(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            self.type_(t)
                .target()
                .and_then(|target| self.type_(target).symbol)
        } else {
            self.type_(t).symbol
        };
        if self.is_lib_symbol_for_hover_verbosity(symbol) {
            return true;
        }
        crate::relater::is_tuple_type(self, t)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free functions (Go's package-level shapes)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetResolvedSignatureForSignatureHelp(node, argumentCount, c) (*Signature, []*Signature)` (services.go:354).
pub fn get_resolved_signature_for_signature_help(
    c: &mut Checker,
    node: NodeId,
    argument_count: i32,
) -> (
    Option<crate::types::SignatureId>,
    Vec<crate::types::SignatureId>,
) {
    c.run_without_resolved_signature_caching(node, |c| {
        c.get_resolved_signature_worker(node, CheckMode::IS_FOR_SIGNATURE_HELP, argument_count)
    })
}

/// Go: `func isExportSpecifierAlias(referenceLocation, exportSpecifier) bool` (services.go:632).
fn is_export_specifier_alias(c: &Checker, reference_location: NodeId, export_specifier: NodeId) -> bool {
    let d = c
        .store()
        .node(export_specifier)
        .as_export_specifier()
        .expect("ExportSpecifier data");
    debug_assert!(
        d.property_name == Some(reference_location)
            || d.name == reference_location,
        "referenceLocation is not export specifier name or property name"
    );
    let Some(property_name) = d.property_name else {
        // `export { foo } from "foo"` is a re-export.
        // `export { foo };` is not a re-export, it creates an alias for the local variable `foo`.
        let parent = c.store().node(export_specifier).parent.get();
        let grandparent = c.store().node(parent).parent.get();
        return crate::ast_shims::module_specifier_of(c.store(), grandparent).is_none();
    };
    // Given `export { foo as bar } [from "someModule"]`: It's an alias at `foo`, but at `bar` it's a new symbol.
    property_name == reference_location
}

/// Go: `func getPossibleSymbolReferenceNodes(sourceFile, symbolName, container) []*ast.Node` (services.go:645).
///
/// PORT(M7 astnav): Go maps each position through
/// astnav.GetTouchingPropertyName(sourceFile, pos) (the touching-node walk);
/// the astnav wave owns it. The positions themselves are exact —
/// `getPossibleSymbolReferenceNodes` returns empty until that wave lands
/// (a wrong-node answer would silently corrupt the reference scan).
fn get_possible_symbol_reference_nodes(
    c: &Checker,
    source_file: NodeId,
    symbol_name: &str,
) -> Vec<NodeId> {
    let positions = get_possible_symbol_reference_positions(c, source_file, symbol_name);
    let _ = positions;
    Vec::new()
}

/// Go: `func getPossibleSymbolReferencePositions(sourceFile, symbolName, container) []int` (services.go:654).
///
/// The text scan is REAL (Go's byte scan — identifier-boundary windows over
/// the container's range, scanner.IsIdentifierPart).
fn get_possible_symbol_reference_positions(
    c: &Checker,
    source_file: NodeId,
    symbol_name: &str,
) -> Vec<i64> {
    let mut positions: Vec<i64> = Vec::new();

    // TODO: Cache symbol existence for files to save text search
    // Also, need to make this work for unicode escapes.

    // Be resilient in the face of a symbol with no name or zero length name
    if symbol_name.is_empty() {
        return positions;
    }

    let file = c.store().file(source_file.file_id());
    let text = file.text();
    let source_length = text.len() as i64;
    let symbol_name_length = symbol_name.len() as i64;

    let container_pos = c.store().node(source_file).pos() as i64;
    let end_pos = c.store().node(source_file).end() as i64;

    let bytes = text.as_bytes();
    let mut position = find_from(bytes, symbol_name.as_bytes(), container_pos as usize).map(|p| p as i64);
    while let Some(pos) = position
        && pos >= 0
        && pos < end_pos
    {
        // We found a match.  Make sure it's not part of a larger word (i.e. the char
        // before and after it have to be a non-identifier char).
        let end_position = pos + symbol_name_length;

        let before_ok = pos == 0 || !tsc_scanner::is_identifier_part(bytes[(pos - 1) as usize] as i32);
        let after_ok = end_position == source_length
            || !tsc_scanner::is_identifier_part(bytes[end_position as usize] as i32);
        if before_ok && after_ok {
            // Found a real match.  Keep searching.
            positions.push(pos);
        }
        let start_index = pos + symbol_name_length + 1;
        if start_index > text.len() as i64 {
            break;
        }
        position = find_from(bytes, symbol_name.as_bytes(), start_index as usize).map(|p| p as i64);
        if position.is_none() {
            break;
        }
    }

    positions
}

fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Go: `var knownGenericTypeNames` + `func isKnownGenericTypeName(name) bool` (services.go:1002).
fn is_known_generic_type_name(name: &str) -> bool {
    const KNOWN_GENERIC_TYPE_NAMES: &[&str] = &[
        "Array",
        "ArrayLike",
        "ReadonlyArray",
        "Promise",
        "PromiseLike",
        "Iterable",
        "IterableIterator",
        "AsyncIterable",
        "Set",
        "WeakSet",
        "ReadonlySet",
        "Map",
        "WeakMap",
        "ReadonlyMap",
        "Partial",
        "Required",
        "Readonly",
        "Pick",
        "Omit",
        "NonNullable",
    ];
    KNOWN_GENERIC_TYPE_NAMES.contains(&name)
}

// ────────────────────────────────────────────────────────────────────────────
// Local ast-predicate ports (Go ast.IsCallLikeExpression / IsCallLikeOr-
// FunctionLikeExpression / IsFunctionExpressionOrArrowFunction — kind sets;
// dedup into the ast/utilities wholesale wave, same pattern as ast_shims.rs)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsCallLikeExpression(node *Node) bool` (ast package) — CallExpression,
/// NewExpression, TaggedTemplateExpression, Decorator.
fn is_call_like_expression_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::CallExpression | Kind::NewExpression | Kind::TaggedTemplateExpression | Kind::Decorator
    )
}

/// Go: `func IsCallLikeOrFunctionLikeExpression(node *Node) bool`.
fn is_call_like_or_function_like_expression_kind(kind: Kind) -> bool {
    is_call_like_expression_kind(kind)
        || matches!(
            kind,
            Kind::FunctionExpression
                | Kind::ArrowFunction
                | Kind::MethodDeclaration
                | Kind::GetAccessor
                | Kind::SetAccessor
                | Kind::Constructor
                | Kind::FunctionDeclaration
        )
}

/// Go: `func IsFunctionExpressionOrArrowFunction(node *Node) bool`.
fn is_function_expression_or_arrow_function_kind(kind: Kind) -> bool {
    matches!(kind, Kind::FunctionExpression | Kind::ArrowFunction)
}

/// Go: `func IsTypeNode(node *Node) bool` — the kind-set predicate
/// (ast/utilities.go's IsTypeNodeKind via scanner go_shims; the ast crate
/// has no node-level twin yet).
fn is_type_node(node: &tsc_ast::Node) -> bool {
    tsc_scanner::go_shims::is_type_node_kind(node.kind)
}
