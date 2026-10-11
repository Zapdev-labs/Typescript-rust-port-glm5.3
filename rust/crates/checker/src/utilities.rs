// Ported from tsc/internal/checker/utilities.go @ ec47d33c23e464a17cdf2475632cba629bee8763
// (complete pass; checker/program-entangled arms carry TODO(port) banners).
//
// Conventions:
// * Pure kind/flag predicates keep Go's `node *Node` shape as `&Node`.
// * Functions reading node payloads take `(store: &dyn NodeStore, node: NodeId)`
//   (the §5.1 node-arena seam — Go's pointer deref becomes the explicit store).
// * Functions Go hangs off `t.checker` take `&Checker` explicitly (SPEC §5.4).

use std::sync::OnceLock;

use rustc_hash::FxHashMap;
use tsc_ast::{
    is_access_expression, is_binary_expression, is_element_access_expression,
    is_identifier, is_parenthesized_expression, is_property_access_expression, is_qualified_name,
    is_type_reference_node, Kind, ModifierFlags, Node, NodeId, NodeList, NodeStore, Symbol,
    SymbolFlags, SymbolId, SymbolTable, has_syntactic_modifier, INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
    INTERNAL_SYMBOL_NAME_INDEX,
};
use tsc_ast::nodeflags::NodeFlags;
use tsc_ast::utilities::{
    is_in_js_file,
};
use tsc_binder::ast_shims as binder_shims;
use tsc_core::text::{TextPos, TextRange};
use tsc_diagnostics::Message;
use tsc_jsnum::{Number, PseudoBigInt};
use tsc_jsnum as jsnum;

use crate::ast_shims::{
    self, find_ancestor, find_ancestor_or_quit, get_property_name_for_property_name_node,
    walk_up_parenthesized_expressions, FindAncestorResult,
};
use crate::checker::Checker;
use crate::types::{
    LiteralValue, ObjectFlags, Signature, Type, TypeFlags, TypeId,
};

// ────────────────────────────────────────────────────────────────────────────
// Diagnostics (Go top of file)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func NewDiagnosticForNode(node *ast.Node, message, args...) *ast.Diagnostic`.
///
/// PORT: the checker's diagnostic creation flows through `ast.NewDiagnostic`;
/// the localized-message path is the parser/binder-shaped constructor. The
/// `scanner.GetErrorRangeForNode` span table is REAL as of W3T6 — the
/// verbatim port lives in `tsc_binder::scanner_shims::get_error_range_for_node`
/// (the binder wave's copy; dedup candidate when the scanner crate takes
/// it back) and `get_file_and_range` below routes through it, exactly like
/// Go's NewDiagnosticForNode.
pub fn new_diagnostic_for_node(
    store: &dyn NodeStore,
    node: NodeId,
    message: &'static Message,
    args: Vec<String>,
) -> tsc_ast::Diagnostic {
    let (file, loc) = match get_file_and_range(store, node) {
        Some((file, pos, end)) => (Some(file), TextRange::new(pos, end)),
        None => (None, TextRange::new(0, 0)),
    };
    let code = message.code();
    let category = message.category();
    tsc_ast::Diagnostic::new(
        file,
        loc,
        code,
        category,
        "" /*source*/,
        Some(message),
        "" /*messageText — resolved at format time, like Go's localized chain*/,
        message.key(),
        args,
    )
}

fn get_file_and_range(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<(NodeId, TextPos, TextPos)> {
    // Go: `if node != nil { file = ...; loc = ... }` (utilities.go
    // NewDiagnosticForNode) — the NodeId::NONE sentinel is Go's nil
    // `*ast.Node`: the diagnostic has no file and a zero range (a global
    // diagnostic in the collection's classification; e.g. the
    // getGlobalSymbol family reports its errors with no location).
    if node.is_none() {
        return None;
    }
    let file = tsc_ast::utilities::get_source_file_of_node(store, node)?;
    // Go: `loc = scanner.GetErrorRangeForNode(file, node)` (utilities.go's
    // NewDiagnosticForNode) — REAL as of W3T6 (was the M7-deferred
    // skip-trivia'd node range; the verbatim port landed with the binder
    // wave in tsc_binder::scanner_shims). This is Go's error-node choice
    // discipline — the span clamp for ClassDeclaration/FunctionDeclaration/
    // VariableDeclaration/... names, the arrow first-line rule, the
    // constructor/return/yield token rules.
    let loc = tsc_binder::scanner_shims::get_error_range_for_node(file, node);
    Some((file.as_node_id(), loc.pos(), loc.end()))
}

/// Go: `func NewDiagnosticChainForNode(chain, node, message, args...) *ast.Diagnostic`.
///
/// PORT: chains are appended via the diagnostic's message-chain plumbing,
/// which lands with the diagnostics wave; the spike asserts the shape only.
pub fn new_diagnostic_chain_for_node(
    _chain: Option<&tsc_ast::Diagnostic>,
    store: &dyn NodeStore,
    node: NodeId,
    message: &'static Message,
    args: Vec<String>,
) -> tsc_ast::Diagnostic {
    // TODO(port M7 diagnostics): Go appends to `chain` when present.
    new_diagnostic_for_node(store, node, message, args)
}

// ────────────────────────────────────────────────────────────────────────────
// Small helpers (Go order)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func tokenIsIdentifierOrKeyword(token ast.Kind) bool` — Kind preserves
/// Go's numeric ordering, so the `>=` survives as a discriminant compare.
pub fn token_is_identifier_or_keyword(token: Kind) -> bool {
    (token as u32) >= (Kind::Identifier as u32)
}

/// Go: `func tokenIsIdentifierOrKeywordOrGreaterThan(token ast.Kind) bool`.
pub fn token_is_identifier_or_keyword_or_greater_than(token: Kind) -> bool {
    token == Kind::GreaterThanToken || token_is_identifier_or_keyword(token)
}

/// Go: `func hasOverrideModifier(node *ast.Node) bool`.
pub fn has_override_modifier(node: &Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::OVERRIDE)
}

/// Go: `func hasAsyncModifier(node *ast.Node) bool`.
pub fn has_async_modifier(node: &Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::ASYNC)
}

/// Go: `func getSelectedModifierFlags(node *ast.Node, flags ast.ModifierFlags) ast.ModifierFlags`.
pub fn get_selected_modifier_flags(node: &Node, flags: ModifierFlags) -> ModifierFlags {
    node.modifier_flags() & flags
}

/// Go: `func hasReadonlyModifier(node *ast.Node) bool`.
pub fn has_readonly_modifier(node: &Node) -> bool {
    crate::ast_shims::has_modifier(node, ModifierFlags::READONLY)
}

/// Go: `func isStaticPrivateIdentifierProperty(s *ast.Symbol) bool`.
pub fn is_static_private_identifier_property(
    store: &dyn NodeStore,
    s: &Symbol,
) -> bool {
    s.value_declaration.is_some_and(|decl| {
        tsc_ast::utilities::is_private_identifier_class_element_declaration(store, decl)
            && is_static(store, store.node(decl))
    })
}

/// Go: `ast.IsStatic(node)` — `HasSyntacticModifier(node, ModifierFlagsStatic)`.
pub fn is_static(store: &dyn NodeStore, node: &Node) -> bool {
    let _ = store;
    has_syntactic_modifier(node, ModifierFlags::STATIC)
}

/// Go: `func isEmptyObjectLiteral(expression *ast.Node) bool`.
pub fn is_empty_object_literal(store: &dyn NodeStore, expression: NodeId) -> bool {
    let n = store.node(expression);
    if n.kind != Kind::ObjectLiteralExpression {
        return false;
    }
    n.as_object_literal_expression()
        .map(|d| d.properties.as_ref().is_none_or(|p| p.nodes.is_empty()))
        .unwrap_or(true)
}

/// Go: `type AssignmentKind int32` + constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignmentKind {
    None,
    Definite,
    Compound,
}

/// Go: `func getAssignmentTargetKind(node *ast.Node) AssignmentKind`.
pub fn get_assignment_target_kind(store: &dyn NodeStore, node: NodeId) -> AssignmentKind {
    let target = binder_shims::get_assignment_target(store, node);
    let Some(target) = target else { return AssignmentKind::None };
    let n = store.node(target);
    match n.kind {
        Kind::BinaryExpression => {
            let d = n.as_binary_expression().expect("BinaryExpression data");
            let binary_operator = store.node(d.operator_token).kind;
            if binary_operator == Kind::EqualsToken
                || tsc_ast::is_logical_or_coalescing_assignment_operator(binary_operator)
            {
                AssignmentKind::Definite
            } else {
                AssignmentKind::Compound
            }
        }
        Kind::PrefixUnaryExpression | Kind::PostfixUnaryExpression => AssignmentKind::Compound,
        Kind::ForInStatement | Kind::ForOfStatement => AssignmentKind::Definite,
        _ => panic!("Unhandled case in getAssignmentTargetKind"),
    }
}

/// Go: `func isDeleteTarget(node *ast.Node) bool`.
pub fn is_delete_target(store: &dyn NodeStore, node: NodeId) -> bool {
    if !is_access_expression(store.node(node)) {
        return false;
    }
    let parent = walk_up_parenthesized_expressions(store, store.node(node).parent.get());
    parent.is_some_and(|p| store.node(p).kind == Kind::DeleteExpression)
}

/// Go: `func isInCompoundLikeAssignment(node *ast.Node) bool`.
pub fn is_in_compound_like_assignment(store: &dyn NodeStore, node: NodeId) -> bool {
    let Some(target) = binder_shims::get_assignment_target(store, node) else {
        return false;
    };
    tsc_ast::utilities::is_assignment_expression(store, target, true)
        && is_compound_like_assignment(store, target)
}

/// Go: `func isCompoundLikeAssignment(assignment *ast.Node) bool`.
pub fn is_compound_like_assignment(store: &dyn NodeStore, assignment: NodeId) -> bool {
    let d = store
        .node(assignment)
        .as_binary_expression()
        .expect("BinaryExpression data");
    let right = tsc_ast::utilities::skip_parentheses(store, d.right);
    let rn = store.node(right);
    rn.kind == Kind::BinaryExpression
        && is_shift_operator_or_higher(
            store
                .node(rn.as_binary_expression().expect("BinaryExpression data").operator_token)
                .kind,
        )
}

/// Go: `func isConstTypeReference(node *ast.Node) bool`.
pub fn is_const_type_reference(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if !is_type_reference_node(n) {
        return false;
    }
    let d = n.as_type_reference_node().expect("TypeReferenceNode data");
    d.type_arguments.as_ref().is_none_or(|args| args.nodes.is_empty())
        && is_identifier(store.node(d.type_name))
        && tsc_ast::node_text(store, d.type_name) == "const"
}

/// Go: `func isConstTypeReferenceName(node *ast.Node) bool`.
pub fn is_const_type_reference_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if !is_identifier(n) {
        return false;
    }
    let parent = n.parent.get();
    if parent.is_none() || !is_const_type_reference(store, parent) {
        return false;
    }
    let grandparent = store.node(parent).parent.get();
    !grandparent.is_none() && is_assertion_expression(store, grandparent)
}

/// Go: `ast.IsAssertionExpression`.
fn is_assertion_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::TypeAssertionExpression | Kind::AsExpression
    )
}

/// Go: `func isExportAssignmentExpressionName(node *ast.Node) bool`.
pub fn is_export_assignment_expression_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut current = node;
    loop {
        let parent = store.node(current).parent.get();
        if parent.is_none() || !is_property_access_or_qualified_name(store.node(parent)) {
            break;
        }
        current = parent;
    }
    let parent = store.node(current).parent.get();
    !parent.is_none() && {
        let p = parent;
        store.node(p).kind == Kind::ExportAssignment
            && ast_shims::expression_of(store, p).is_some_and(|e| e == current)
    }
}

/// Go: `ast.IsPropertyAccessOrQualifiedName`.
fn is_property_access_or_qualified_name(node: &Node) -> bool {
    is_property_access_expression(node) || is_qualified_name(node)
}

/// Go: `func GetSingleVariableOfVariableStatement(node *ast.Node) *ast.Node`.
pub fn get_single_variable_of_variable_statement(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    let n = store.node(node);
    if n.kind != Kind::VariableStatement {
        return None;
    }
    let list = n
        .as_variable_statement()
        .expect("VariableStatement data")
        .declaration_list;
    let declarations = store
        .node(list)
        .as_variable_declaration_list()
        .expect("VariableDeclarationList data")
        .declarations.as_ref()?;
    declarations.nodes.first().copied()
}

/// Go: `func isTypeReferenceIdentifier(node *ast.Node) bool`.
pub fn is_type_reference_identifier(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut node = node;
    while store.node(store.node(node).parent.get()).kind == Kind::QualifiedName {
        node = store.node(node).parent.get();
    }
    is_type_reference_node(store.node(store.node(node).parent.get()))
}

/// Go: `func IsInTypeQuery(node *ast.Node) bool`.
pub fn is_in_type_query(store: &dyn NodeStore, node: NodeId) -> bool {
    find_ancestor_or_quit(
        store,
        node,
        &mut |n| match n.kind {
            Kind::TypeQuery => FindAncestorResult::True,
            Kind::Identifier | Kind::QualifiedName => FindAncestorResult::False,
            _ => FindAncestorResult::Quit,
        },
    )
    .is_some()
}

/// Go: `func canHaveLocals(node *ast.Node) bool`.
pub fn can_have_locals(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::ArrowFunction
            | Kind::Block
            | Kind::CallSignature
            | Kind::CaseBlock
            | Kind::CatchClause
            | Kind::ClassStaticBlockDeclaration
            | Kind::ConditionalType
            | Kind::Constructor
            | Kind::ConstructorType
            | Kind::ConstructSignature
            | Kind::ForStatement
            | Kind::ForInStatement
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::FunctionType
            | Kind::GetAccessor
            | Kind::IndexSignature
            | Kind::JSDocSignature
            | Kind::MappedType
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::ModuleDeclaration
            | Kind::SetAccessor
            | Kind::SourceFile
            | Kind::TypeAliasDeclaration
    )
}

/// Go: `func isShorthandAmbientModuleSymbol(moduleSymbol *ast.Symbol) bool`.
pub fn is_shorthand_ambient_module_symbol(
    c: &Checker,
    store: &dyn NodeStore,
    module_symbol: SymbolId,
) -> bool {
    is_shorthand_ambient_module(store, c.program.symbol(module_symbol).value_declaration)
}

/// Go: `func isShorthandAmbientModule(node *ast.Node) bool` — the only kind of
/// module that can be missing a body is a shorthand ambient module.
pub fn is_shorthand_ambient_module(store: &dyn NodeStore, node: Option<NodeId>) -> bool {
    node.is_some_and(|n| {
        store.node(n).kind == Kind::ModuleDeclaration
            && tsc_binder::node_data::body_of(store, n).is_none()
    })
}

/// Go: `func getAliasDeclarationFromName(node *ast.Node) *ast.Node`.
pub fn get_alias_declaration_from_name(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let parent = store.node(node).parent.get();
    match store.node(parent).kind {
        Kind::ImportClause
        | Kind::ImportSpecifier
        | Kind::NamespaceImport
        | Kind::ExportSpecifier
        | Kind::ExportAssignment
        | Kind::ImportEqualsDeclaration
        | Kind::NamespaceExport => Some(parent),
        Kind::QualifiedName => get_alias_declaration_from_name(store, parent),
        _ => None,
    }
}

/// Go: `func entityNameToString(name *ast.Node) string`.
pub fn entity_name_to_string(store: &dyn NodeStore, name: NodeId) -> String {
    ast_shims::entity_name_to_string(store, name)
}

/// Go: `func getContainingQualifiedNameNode(node *ast.Node) *ast.Node`.
pub fn get_containing_qualified_name_node(store: &dyn NodeStore, node: NodeId) -> NodeId {
    let mut node = node;
    while is_qualified_name(store.node(store.node(node).parent.get())) {
        node = store.node(node).parent.get();
    }
    node
}

/// Go: `func isSideEffectImport(node *ast.Node) bool`.
pub fn is_side_effect_import(store: &dyn NodeStore, node: NodeId) -> bool {
    let ancestor = find_ancestor(store, node, &mut |n| n.kind == Kind::ImportDeclaration);
    match ancestor {
        Some(a) => store.node(a).as_import_declaration().is_none_or(|d| d.import_clause.is_none()),
        None => false,
    }
}

/// Go: `func getExternalModuleRequireArgument(node *ast.Node) *ast.Node`.
pub fn get_external_module_require_argument(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    if binder_shims::is_variable_declaration_initialized_to_require(store, node) {
        let initializer = ast_shims::initializer_of(store, node)?;
        tsc_binder::node_data::arguments_of(store, initializer).first().copied()
    } else {
        None
    }
}

/// Go: `func isRightSideOfAccessExpression(node *ast.Node) bool`.
pub fn is_right_side_of_access_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent = store.node(node).parent.get();
    if parent.is_none() {
        return false;
    }
    let p = store.node(parent);
    (is_property_access_expression(p) && p.name() == Some(node))
        || (is_element_access_expression(p)
            && p.as_element_access_expression()
                .is_some_and(|d| d.argument_expression == node))
}

/// Go: `func isTopLevelInExternalModuleAugmentation(node *ast.Node) bool`.
pub fn is_top_level_in_external_module_augmentation(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent = store.node(node).parent.get();
    if parent.is_none() {
        return false;
    }
    if store.node(parent).kind != Kind::ModuleBlock {
        return false;
    }
    let grandparent = store.node(parent).parent.get();
    !grandparent.is_none() && is_external_module_augmentation(store, grandparent)
}

/// Go: `ast.IsExternalModuleAugmentation` — the module is ambient and its
/// string-literal name makes it external.
pub(crate) fn is_external_module_augmentation(store: &dyn NodeStore, node: NodeId) -> bool {
    // Go: `IsAmbientModule(node) && IsModuleAugmentationExternal(node)` —
    // W3T7: the port's old shortcut (`is_ambient && ModuleDeclaration &&
    // StringLiteral-name`) classified EVERY ambient external module as an
    // augmentation, so `namespace M { export declare module "M" {} }` took
    // the augmentation branch and never reached TS2435. The REAL
    // IsModuleAugmentationExternal keys on the PARENT placement (Go
    // ast/utilities.go:1720):
    // - parent SourceFile → the file itself is an external module;
    // - parent ModuleBlock → the grandparent is an ambient module at the
    //   top level and the file is NOT an external module;
    // - otherwise false.
    if !tsc_binder::ast_shims::is_ambient_module(store, node) {
        return false;
    }
    let parent = store.node(node).parent.get();
    match store.node(parent).kind {
        Kind::SourceFile => tsc_binder::ast_shims::is_external_module(store.file(parent.file_id())),
        Kind::ModuleBlock => {
            let grandparent = store.node(parent).parent.get();
            let great_grandparent = store.node(grandparent).parent.get();
            tsc_binder::ast_shims::is_ambient_module(store, grandparent)
                && store.node(great_grandparent).kind == Kind::SourceFile
                && !tsc_binder::ast_shims::is_external_module(store.file(grandparent.file_id()))
        }
        _ => false,
    }
}

/// Go: `func isSyntacticDefault(node *ast.Node) bool`.
pub fn is_syntactic_default(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    (n.kind == Kind::ExportAssignment
        && !n.as_export_assignment().is_some_and(|d| d.is_export_equals))
        || has_syntactic_modifier(n, ModifierFlags::DEFAULT)
        || n.kind == Kind::ExportSpecifier
        || n.kind == Kind::NamespaceExport
}

/// Go: `func hasExportAssignmentSymbol(moduleSymbol *ast.Symbol) bool`.
pub fn has_export_assignment_symbol(module_symbol: &Symbol) -> bool {
    module_symbol.exports.get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS).is_some()
}

/// Go: `func isTypeAlias(node *ast.Node) bool` — `ast.IsTypeOrJSTypeAliasDeclaration`.
pub fn is_type_alias(node: &Node) -> bool {
    matches!(node.kind, Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration)
}

/// Go: `func hasOnlyExpressionInitializer(node *ast.Node) bool`.
pub fn has_only_expression_initializer(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::VariableDeclaration
            | Kind::Parameter
            | Kind::BindingElement
            | Kind::PropertyDeclaration
            | Kind::PropertyAssignment
            | Kind::EnumMember
    )
}

/// Go: `func hasDotDotDotToken(node *ast.Node) bool`.
pub fn has_dot_dot_dot_token(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::ParameterDeclaration(d) => d.dot_dot_dot_token.is_some(),
        tsc_ast::NodeData::BindingElement(d) => d.dot_dot_dot_token.is_some(),
        tsc_ast::NodeData::NamedTupleMember(d) => d.dot_dot_dot_token.is_some(),
        tsc_ast::NodeData::JsxExpression(d) => d.dot_dot_dot_token.is_some(),
        _ => false,
    }
}

/// Go: `func IsTypeAny(t *Type) bool`.
pub fn is_type_any(t: Option<&Type>) -> bool {
    t.is_some_and(|t| t.flags.intersects(TypeFlags::ANY))
}

/// Go: `func isJSDocOptionalParameter(node *ast.ParameterDeclaration) bool` —
/// `return false // !!!` verbatim (JSDoc support pending upstream too).
pub fn is_jsdoc_optional_parameter(_node: NodeId) -> bool {
    false
}

/// Go: `func isExclamationToken(node *ast.Node) bool`.
pub fn is_exclamation_token(node: Option<&Node>) -> bool {
    node.is_some_and(|n| n.kind == Kind::ExclamationToken)
}

/// Go: `func isOptionalDeclaration(declaration *ast.Node) bool`.
pub fn is_optional_declaration(store: &dyn NodeStore, declaration: NodeId) -> bool {
    ast_shims::has_question_token(store, declaration)
}

/// Go: `func (c *Checker) isOptionalParameter(node *ast.Node) bool` — the
/// first two checks are exact; the signature/arity paths need
/// `getSignatureFromDeclaration` (TODO(port M6)).
pub fn is_optional_parameter(_c: &Checker, store: &dyn NodeStore, node: NodeId) -> bool {
    // !!! TODO: JSDoc support
    if store.node(node).kind == Kind::Parameter
        && ast_shims::question_token_of(store, node).is_some()
    {
        return true;
    }
    if store.node(node).kind != Kind::Parameter {
        return false;
    }
    if ast_shims::initializer_of(store, node).is_some() {
        panic!("TODO(port M6): isOptionalParameter needs getSignatureFromDeclaration");
    }
    let iife = binder_shims::get_immediately_invoked_function_expression(store, store.node(node).parent.get());
    if iife.is_some() {
        panic!("TODO(port M6): isOptionalParameter needs getEffectiveCallArguments");
    }
    false
}

/// Go: `func isEmptyArrayLiteral(expression *ast.Node) bool`.
pub fn is_empty_array_literal(store: &dyn NodeStore, expression: NodeId) -> bool {
    let n = store.node(expression);
    if n.kind != Kind::ArrayLiteralExpression {
        return false;
    }
    n.as_array_literal_expression()
        .map(|d| d.elements.as_ref().is_none_or(|e| e.nodes.is_empty()))
        .unwrap_or(true)
}

/// Go: `func declarationBelongsToPrivateAmbientMember(declaration *ast.Node) bool`.
pub fn declaration_belongs_to_private_ambient_member(
    store: &dyn NodeStore,
    declaration: NodeId,
) -> bool {
    let root = binder_shims::get_root_declaration(store, declaration);
    let member_declaration = if store.node(root).kind == Kind::Parameter {
        store.node(root).parent.get()
    } else {
        root
    };
    is_private_within_ambient(store, member_declaration)
}

/// Go: `func isPrivateWithinAmbient(node *ast.Node) bool`.
pub fn is_private_within_ambient(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    (crate::ast_shims::has_modifier(n, ModifierFlags::PRIVATE)
        || tsc_ast::utilities::is_private_identifier_class_element_declaration(store, node))
        && n.flags.intersects(NodeFlags::AMBIENT)
}

/// Go: `func isTypeAssertion(node *ast.Node) bool`.
pub fn is_type_assertion(store: &dyn NodeStore, node: NodeId) -> bool {
    let skipped = tsc_ast::utilities::skip_parentheses(store, node);
    is_assertion_expression(store, skipped)
}

/// Go: `func createSymbolTable(symbols []*ast.Symbol) ast.SymbolTable`.
///
/// PORT (§5.4): symbol names live in the program-level symbol store, so the
/// checker is an explicit parameter. Go returns nil for the empty list; the
/// port's `SymbolTable` is a value — the None-ness Go callers test via
/// `== nil` is expressed by emptiness (`is_empty`).
pub fn create_symbol_table(c: &Checker, symbols: &[SymbolId]) -> SymbolTable {
    let mut result = SymbolTable::new();
    for &symbol in symbols {
        let name = c.program.symbol(symbol).name.clone();
        result.set(name, symbol);
    }
    result
}

// ────────────────────────────────────────────────────────────────────────────
// Symbol/node comparison (used by diagnostic sorting and union creation)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (c *Checker) sortSymbols(symbols []*ast.Symbol)`.
pub fn sort_symbols(c: &Checker, symbols: &mut [SymbolId]) {
    // sort_by with c.compare_symbols (compareSymbolsWorker); stable like
    // Go's slices.SortFunc? Go's SortFunc is NOT stable — but compareSymbols
    // total-orders by symbol id fallback, so stability doesn't matter.
    let store = c.program.node_store();
    symbols.sort_by(|&a, &b| compare_symbols_worker(c, store, a, b));
}

/// Go: `func (c *Checker) compareSymbolsWorker(s1, s2 *ast.Symbol) int`.
pub fn compare_symbols_worker(
    c: &Checker,
    store: &dyn NodeStore,
    s1: SymbolId,
    s2: SymbolId,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if s1 == s2 {
        return Ordering::Equal;
    }
    let sym1 = c.program.symbol(s1);
    let sym2 = c.program.symbol(s2);
    if !sym1.declarations.is_empty() && !sym2.declarations.is_empty() {
        let r = compare_nodes(c, store, sym1.declarations[0], sym2.declarations[0]);
        if r != Ordering::Equal {
            return r;
        }
    } else if !sym1.declarations.is_empty() {
        return Ordering::Less;
    } else if !sym2.declarations.is_empty() {
        return Ordering::Greater;
    }
    match sym1.name.cmp(&sym2.name) {
        Ordering::Equal => {}
        r => return r,
    }
    // Fall back to symbol IDs. This is a last resort that should happen only
    // when symbols have no declaration and duplicate names.
    (s1.0 as i64).cmp(&(s2.0 as i64))
}

/// Go: `func (c *Checker) compareNodes(n1, n2 *ast.Node) int`.
pub fn compare_nodes(
    c: &Checker,
    store: &dyn NodeStore,
    n1: NodeId,
    n2: NodeId,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if n1 == n2 {
        return Ordering::Equal;
    }
    let Some(f1) = tsc_ast::utilities::get_source_file_of_node(store, n1) else {
        return Ordering::Greater;
    };
    let Some(f2) = tsc_ast::utilities::get_source_file_of_node(store, n2) else {
        return Ordering::Less;
    };
    let id1 = f1.as_node_id().0;
    let id2 = f2.as_node_id().0;
    if id1 != id2 {
        // Order by index of file in the containing program
        let i1 = c.file_index_map.get(&id1).copied().unwrap_or(usize::MAX);
        let i2 = c.file_index_map.get(&id2).copied().unwrap_or(usize::MAX);
        return i1.cmp(&i2);
    }
    // In the same file, order by source position
    store
        .node(n1)
        .pos()
        .cmp(&store.node(n2).pos())
}

// ────────────────────────────────────────────────────────────────────────────
// Type comparison (CompareTypes family — SPEC §5.4: explicit `&Checker`)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func CompareTypes(t1, t2 *Type) int`.
///
/// PORT (§5.4): Go panics when `t1.checker != t2.checker`; the port compares
/// `checker_shard` and resolves all checker state through the explicit `c`.
/// Go's nil arguments (`t1 == nil` etc.) become `Option<TypeId>` — callers in
/// the spike always pass present types.
pub fn compare_types(c: &Checker, t1: TypeId, t2: TypeId) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if t1 == t2 {
        return Ordering::Equal;
    }
    let (t1r, t2r) = (c.type_(t1), c.type_(t2));
    if t1r.checker_shard != t2r.checker_shard {
        panic!("Cannot compare types from different checkers");
    }
    // First sort in order of increasing type flags values.
    let c0 = get_sort_order_flags(t1r).cmp(&get_sort_order_flags(t2r));
    if c0 != Ordering::Equal {
        return c0;
    }
    // Order named types by name and, in the case of aliased types, by alias
    // type arguments.
    let cn = compare_type_names(c, t1, t2);
    if cn != Ordering::Equal {
        return cn;
    }
    // We have unnamed types or types with identical names. Now sort by data
    // specific to the type.
    let store = c.program.node_store();
    if t1r.flags.intersects(
        TypeFlags::ANY
            | TypeFlags::UNKNOWN
            | TypeFlags::STRING
            | TypeFlags::NUMBER
            | TypeFlags::BOOLEAN
            | TypeFlags::BIGINT
            | TypeFlags::ESSYMBOL
            | TypeFlags::VOID
            | TypeFlags::UNDEFINED
            | TypeFlags::NULL
            | TypeFlags::NEVER
            | TypeFlags::NON_PRIMITIVE,
    ) {
        // Only distinguished by type IDs, handled below.
    } else if t1r.flags.intersects(TypeFlags::OBJECT) {
        // Order instantiation expression types without relying on lazy symbol
        // IDs. Order other unnamed or identically named object types by symbol.
        if t1r.object_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
            && t2r.object_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
        {
            let declaration1 = t1r
                .symbol
                .and_then(|s| c.program.symbol(s).declarations.first().copied());
            let declaration2 = t2r
                .symbol
                .and_then(|s| c.program.symbol(s).declarations.first().copied());
            // A single instantiation expression can produce multiple types for
            // union constituents, so compare their source declarations before
            // comparing the shared expression node.
            let cr = compare_opt_nodes(c, store, declaration1, declaration2);
            if cr != Ordering::Equal {
                return cr;
            }
            let n1 = t1r
                .as_instantiation_expression_type()
                .and_then(|d| d.node);
            let n2 = t2r
                .as_instantiation_expression_type()
                .and_then(|d| d.node);
            let cr = compare_opt_nodes(c, store, n1, n2);
            if cr != Ordering::Equal {
                return cr;
            }
        } else {
            let cs = compare_opt_symbols(c, store, t1r.symbol, t2r.symbol);
            if cs != Ordering::Equal {
                return cs;
            }
        }
        // When object types have the same or no symbol, order by kind. We
        // order type references before other kinds.
        if t1r.object_flags.intersects(ObjectFlags::REFERENCE)
            && t2r.object_flags.intersects(ObjectFlags::REFERENCE)
        {
            let r1 = t1r.as_type_reference().expect("TypeReference data");
            let r2 = t2r.as_type_reference().expect("TypeReference data");
            let target1 = c.type_(r1.base.target.expect("reference target"));
            let target2 = c.type_(r2.base.target.expect("reference target"));
            if target1.object_flags.intersects(ObjectFlags::TUPLE)
                && target2.object_flags.intersects(ObjectFlags::TUPLE)
            {
                // Tuple types have no associated symbol, instead we order by
                // tuple element information.
                let ct = compare_tuple_types(
                    target1.as_tuple_type().expect("TupleType data"),
                    target2.as_tuple_type().expect("TupleType data"),
                );
                if ct != Ordering::Equal {
                    return ct;
                }
            }
            // Here we know we have references to instantiations of the same
            // type because we have matching targets.
            if r1.node.is_none() && r2.node.is_none() {
                // Non-deferred type references with the same target are
                // sorted by their type argument lists.
                let cl = compare_type_lists(c, &r1.resolved_type_arguments, &r2.resolved_type_arguments);
                if cl != Ordering::Equal {
                    return cl;
                }
            } else {
                // Deferred type references with the same target are ordered by
                // the source location of the reference.
                let cn = compare_opt_nodes(c, store, r1.node, r2.node);
                if cn != Ordering::Equal {
                    return cn;
                }
                // Instantiations of the same deferred type reference are
                // ordered by their associated type mappers (which reflect the
                // mapping of in-scope type parameters to type arguments).
                let cm = compare_type_mappers(
                    c,
                    t1r.as_object_type().and_then(|d| d.mapper),
                    t2r.as_object_type().and_then(|d| d.mapper),
                );
                if cm != Ordering::Equal {
                    return cm;
                }
            }
        } else if t1r.object_flags.intersects(ObjectFlags::REFERENCE) {
            return Ordering::Less;
        } else if t2r.object_flags.intersects(ObjectFlags::REFERENCE) {
            return Ordering::Greater;
        } else {
            // Order unnamed non-reference object types by kind and
            // instantiation data.
            let ck = (t1r.object_flags & ObjectFlags::OBJECT_TYPE_KIND_MASK)
                .0
                .cmp(&(t2r.object_flags & ObjectFlags::OBJECT_TYPE_KIND_MASK).0);
            if ck != Ordering::Equal {
                return ck;
            }
            if t1r.object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
                let r1 = t1r.as_reverse_mapped_type().expect("ReverseMappedType data");
                let r2 = t2r.as_reverse_mapped_type().expect("ReverseMappedType data");
                let cs = compare_opt_types(c, r1.source, r2.source);
                if cs != Ordering::Equal {
                    return cs;
                }
                let cs = compare_opt_types(c, r1.mapped_type, r2.mapped_type);
                if cs != Ordering::Equal {
                    return cs;
                }
                let cs = compare_opt_types(c, r1.constraint_type, r2.constraint_type);
                if cs != Ordering::Equal {
                    return cs;
                }
            }
            let mut m1 = t1r.as_object_type().and_then(|d| d.mapper);
            let mut m2 = t2r.as_object_type().and_then(|d| d.mapper);
            if t1r.object_flags.intersects(ObjectFlags::MAPPED) {
                // instantiateAnonymousType prepends a fresh type parameter mapping.
                // Compare the effective instantiation, not the identity of that fresh parameter.
                // (Go: `if t1.objectFlags&ObjectFlagsMapped != 0 { m1 =
                // m1.data.(*CompositeTypeMapper).m2; ... }` — mapper.go W1
                // landed the composite shape this unwrap reads.)
                if let Some(id) = m1 {
                    m1 = c.mapper_composite_m2(id);
                }
                if let Some(id) = m2 {
                    m2 = c.mapper_composite_m2(id);
                }
            }
            let cm = compare_type_mappers(c, m1, m2);
            if cm != Ordering::Equal {
                return cm;
            }
        }
    } else if t1r.flags.intersects(TypeFlags::UNION) {
        // Unions are ordered by origin and then constituent type lists.
        let o1 = t1r.as_union_type().and_then(|d| d.origin);
        let o2 = t2r.as_union_type().and_then(|d| d.origin);
        match (o1, o2) {
            (None, None) => {
                let cl = compare_type_lists(c, t1r.types(), t2r.types());
                if cl != Ordering::Equal {
                    return cl;
                }
            }
            (None, Some(_)) => return Ordering::Greater,
            (Some(_), None) => return Ordering::Less,
            (Some(o1), Some(o2)) => {
                let co = compare_types(c, o1, o2);
                if co != Ordering::Equal {
                    return co;
                }
            }
        }
    } else if t1r.flags.intersects(TypeFlags::INTERSECTION) {
        // Intersections are ordered by their constituent type lists.
        let cl = compare_type_lists(c, t1r.types(), t2r.types());
        if cl != Ordering::Equal {
            return cl;
        }
    } else if t1r
        .flags
        .intersects(TypeFlags::ENUM | TypeFlags::ENUM_LITERAL | TypeFlags::UNIQUE_ESSYMBOL)
    {
        // Enum members are ordered by their symbol (and thus their
        // declaration order).
        let cs = compare_opt_symbols(c, store, t1r.symbol, t2r.symbol);
        if cs != Ordering::Equal {
            return cs;
        }
    } else if t1r.flags.intersects(TypeFlags::STRING_LITERAL) {
        let cl = literal_string(t1r).cmp(literal_string(t2r));
        if cl != Ordering::Equal {
            return cl;
        }
    } else if t1r.flags.intersects(TypeFlags::NUMBER_LITERAL) {
        let cn = literal_number(t1r)
            .0
            .partial_cmp(&literal_number(t2r).0)
            .unwrap_or(std::cmp::Ordering::Equal);
        if cn != Ordering::Equal {
            return cn;
        }
    } else if t1r.flags.intersects(TypeFlags::BIGINT_LITERAL) {
        let cb = pseudo_big_int_compare(
            &literal_big_int(t1r),
            &literal_big_int(t2r),
        );
        if cb != Ordering::Equal {
            return cb;
        }
    } else if t1r.flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
        let b1 = literal_bool(t1r);
        let b2 = literal_bool(t2r);
        if b1 != b2 {
            return if b1 { Ordering::Greater } else { Ordering::Less };
        }
    } else if t1r.flags.intersects(TypeFlags::TYPE_PARAMETER) {
        let cs = compare_opt_symbols(c, store, t1r.symbol, t2r.symbol);
        if cs != Ordering::Equal {
            return cs;
        }
    } else if t1r.flags.intersects(TypeFlags::INDEX) {
        let i1 = t1r.as_index_type().expect("IndexType data");
        let i2 = t2r.as_index_type().expect("IndexType data");
        let ct = compare_opt_types(c, i1.target, i2.target);
        if ct != Ordering::Equal {
            return ct;
        }
        let cf = (i1.index_flags.0 as i32).cmp(&(i2.index_flags.0 as i32));
        if cf != Ordering::Equal {
            return cf;
        }
    } else if t1r.flags.intersects(TypeFlags::INDEXED_ACCESS) {
        let i1 = t1r.as_indexed_access_type().expect("IndexedAccessType data");
        let i2 = t2r.as_indexed_access_type().expect("IndexedAccessType data");
        let c1 = compare_opt_types(c, i1.object_type, i2.object_type);
        if c1 != Ordering::Equal {
            return c1;
        }
        let c2 = compare_opt_types(c, i1.index_type, i2.index_type);
        if c2 != Ordering::Equal {
            return c2;
        }
    } else if t1r.flags.intersects(TypeFlags::CONDITIONAL) {
        let d1 = t1r.as_conditional_type().expect("ConditionalType data");
        let d2 = t2r.as_conditional_type().expect("ConditionalType data");
        let cn = compare_opt_nodes(
            c,
            store,
            d1.root.as_ref().and_then(|r| r.node),
            d2.root.as_ref().and_then(|r| r.node),
        );
        if cn != Ordering::Equal {
            return cn;
        }
        let cm = compare_type_mappers(c, d1.mapper, d2.mapper);
        if cm != Ordering::Equal {
            return cm;
        }
    } else if t1r.flags.intersects(TypeFlags::SUBSTITUTION) {
        let s1 = t1r.as_substitution_type().expect("SubstitutionType data");
        let s2 = t2r.as_substitution_type().expect("SubstitutionType data");
        let c1 = compare_opt_types(c, s1.base_type, s2.base_type);
        if c1 != Ordering::Equal {
            return c1;
        }
        let c2 = compare_opt_types(c, s1.constraint, s2.constraint);
        if c2 != Ordering::Equal {
            return c2;
        }
    } else if t1r.flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
        let s1 = t1r.as_template_literal_type().expect("TemplateLiteralType data");
        let s2 = t2r.as_template_literal_type().expect("TemplateLiteralType data");
        let ct: std::cmp::Ordering = s1.texts.cmp(&s2.texts);
        if ct != Ordering::Equal {
            return ct;
        }
        let cl = compare_type_lists(c, &s1.types, &s2.types);
        if cl != Ordering::Equal {
            return cl;
        }
    } else if t1r.flags.intersects(TypeFlags::STRING_MAPPING) {
        let s1 = t1r.as_string_mapping_type().expect("StringMappingType data");
        let s2 = t2r.as_string_mapping_type().expect("StringMappingType data");
        let ct = compare_opt_types(c, s1.target, s2.target);
        if ct != Ordering::Equal {
            return ct;
        }
    }
    // Fall back to type IDs. This results in type creation order for built-in
    // types.
    t1.0.cmp(&t2.0)
}

fn compare_opt_types(c: &Checker, t1: Option<TypeId>, t2: Option<TypeId>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (t1, t2) {
        (Some(a), Some(b)) => compare_types(c, a, b),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
    }
}

fn compare_opt_symbols(
    c: &Checker,
    store: &dyn NodeStore,
    s1: Option<SymbolId>,
    s2: Option<SymbolId>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (s1, s2) {
        (Some(a), Some(b)) => compare_symbols_worker(c, store, a, b),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
    }
}

fn compare_opt_nodes(
    c: &Checker,
    store: &dyn NodeStore,
    n1: Option<NodeId>,
    n2: Option<NodeId>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (n1, n2) {
        (Some(a), Some(b)) => compare_nodes(c, store, a, b),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
    }
}

fn literal_string(t: &Type) -> &str {
    match t.as_literal_type().expect("LiteralType data").value.as_ref() {
        Some(LiteralValue::String(s)) => s,
        _ => panic!("string literal type with non-string value"),
    }
}

fn literal_number(t: &Type) -> Number {
    match t.as_literal_type().expect("LiteralType data").value.as_ref() {
        Some(LiteralValue::Number(n)) => *n,
        _ => panic!("number literal type with non-number value"),
    }
}

fn literal_bool(t: &Type) -> bool {
    match t.as_literal_type().expect("LiteralType data").value.as_ref() {
        Some(LiteralValue::Boolean(b)) => *b,
        _ => panic!("boolean literal type with non-bool value"),
    }
}

fn literal_big_int(t: &Type) -> PseudoBigInt {
    match t.as_literal_type().expect("LiteralType data").value.as_ref() {
        Some(LiteralValue::BigInt(b)) => b.clone(),
        _ => panic!("bigint literal type with non-bigint value"),
    }
}

/// Go: `jsnum.PseudoBigInt.Compare` — sign, then base-10 magnitude.
fn pseudo_big_int_compare(a: &PseudoBigInt, b: &PseudoBigInt) -> std::cmp::Ordering {
    // Go compares negative first, then the numeric value of the base-10
    // string. `string()` renders the canonical decimal form.
    let sa = a.string();
    let sb = b.string();
    let neg_a = a.negative;
    let neg_b = b.negative;
    match (neg_a, neg_b) {
        (false, true) => std::cmp::Ordering::Greater,
        (true, false) => std::cmp::Ordering::Less,
        _ => {
            let ord = sa.cmp(&sb);
            if neg_a { ord.reverse() } else { ord }
        }
    }
}

/// Go: `func getSortOrderFlags(t *Type) int`.
pub fn get_sort_order_flags(t: &Type) -> u32 {
    // Return TypeFlagsEnum for all enum-like unit types (they'll be sorted by
    // their symbols)
    if t.flags.intersects(TypeFlags::ENUM_LITERAL | TypeFlags::ENUM)
        && !t.flags.intersects(TypeFlags::UNION)
    {
        TypeFlags::ENUM.0
    } else {
        t.flags.0
    }
}

/// Go: `func compareTypeNames(t1, t2 *Type) int`.
pub fn compare_type_names(c: &Checker, t1: TypeId, t2: TypeId) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let s1 = get_type_name_symbol(c.type_(t1));
    let s2 = get_type_name_symbol(c.type_(t2));
    if s1 == s2 {
        let a1 = c.type_(t1).alias.as_ref().map(|a| a.type_arguments.as_slice()).unwrap_or(&[]);
        let a2 = c.type_(t2).alias.as_ref().map(|a| a.type_arguments.as_slice()).unwrap_or(&[]);
        return compare_type_lists(c, a1, a2);
    }
    let (Some(s1), Some(s2)) = (s1, s2) else {
        return match (s1, s2) {
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            _ => Ordering::Equal,
        };
    };
    let name_ord = c.program.symbol(s1).name.cmp(&c.program.symbol(s2).name);
    if name_ord != Ordering::Equal {
        return name_ord;
    }
    // Keep distinct same-named declarations together before comparing alias
    // arguments or structure.
    let store = c.program.node_store();
    compare_symbols_worker(c, store, s1, s2)
}

/// Go: `func getTypeNameSymbol(t *Type) *ast.Symbol`.
pub fn get_type_name_symbol(t: &Type) -> Option<SymbolId> {
    if t.alias.is_some() {
        return t.alias.as_ref().and_then(|a| a.symbol);
    }
    if t.flags.intersects(TypeFlags::TYPE_PARAMETER | TypeFlags::STRING_MAPPING)
        || t
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::REFERENCE)
    {
        return t.symbol;
    }
    None
}

/// Go: `func getObjectTypeName(t *Type) *ast.Symbol`.
pub fn get_object_type_name(t: &Type) -> Option<SymbolId> {
    if t
        .object_flags
        .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::REFERENCE)
    {
        return t.symbol;
    }
    None
}

/// Go: `func compareTupleTypes(t1, t2 *TupleType) int`.
pub fn compare_tuple_types(
    t1: &crate::types::TupleType,
    t2: &crate::types::TupleType,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if t1.readonly != t2.readonly {
        return if t1.readonly { Ordering::Greater } else { Ordering::Less };
    }
    let cl = t1.element_infos.len().cmp(&t2.element_infos.len());
    if cl != Ordering::Equal {
        return cl;
    }
    for (e1, e2) in t1.element_infos.iter().zip(t2.element_infos.iter()) {
        let cf = e1.flags.0.cmp(&e2.flags.0);
        if cf != Ordering::Equal {
            return cf;
        }
    }
    // Element labels are compared without a store in the spike (labels are
    // Option<NodeId>s; Go reads their name text). The labeled-tuple arm is
    // TODO(port M6) until tuple creation lands; identical labels compare equal.
    for (e1, e2) in t1.element_infos.iter().zip(t2.element_infos.iter()) {
        let cf = (e1.labeled_declaration.is_none() as u8).cmp(&(e2.labeled_declaration.is_none() as u8));
        if cf != Ordering::Equal {
            return cf;
        }
    }
    Ordering::Equal
}

/// Go: `func compareElementLabels(n1, n2 *ast.Node) int` — needs the node
/// store for name text; the TupleType arm above documents the M6 seam.
///
/// TODO(port M6): store-carrying compare_element_labels.
/// Go: `func compareTypeLists(s1, s2 []*Type) int`.
pub fn compare_type_lists(c: &Checker, s1: &[TypeId], s2: &[TypeId]) -> std::cmp::Ordering {
    let cl = s1.len().cmp(&s2.len());
    if cl != std::cmp::Ordering::Equal {
        return cl;
    }
    for (i, t1) in s1.iter().enumerate() {
        let cr = compare_types(c, *t1, s2[i]);
        if cr != std::cmp::Ordering::Equal {
            return cr;
        }
    }
    std::cmp::Ordering::Equal
}

/// Go: `func compareTypeMappers(m1, m2 *TypeMapper) int`.
///
/// PORT (mapper.go W1): mappers are `TypeMapperId` indices into
/// `Checker::type_mappers` (mapper.rs); Go's pointer identity is id equality
/// (the first check), and the kind-wise payload comparison resolves through
/// the checker (§5.4) — `m1.data.(*XxxTypeMapper)` becomes the
/// `mapper_data()` match.
pub fn compare_type_mappers(
    c: &Checker,
    m1: Option<crate::todo_shapes::TypeMapperId>,
    m2: Option<crate::todo_shapes::TypeMapperId>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    use crate::mapper::TypeMapperData;

    if m1 == m2 {
        return Ordering::Equal;
    }
    match (m1, m2) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(m1), Some(m2)) => {
            let kind1 = c.mapper_kind(m1);
            let kind2 = c.mapper_kind(m2);
            if kind1 != kind2 {
                return (kind1 as i32).cmp(&(kind2 as i32));
            }
            match c.mapper_data(m1) {
                TypeMapperData::Simple { source: s1, target: t1 } => {
                    let TypeMapperData::Simple { source: s2, target: t2 } = c.mapper_data(m2) else {
                        unreachable!("kinds equal");
                    };
                    let cr = compare_types(c, *s1, *s2);
                    if cr != Ordering::Equal {
                        return cr;
                    }
                    compare_types(c, *t1, *t2)
                }
                TypeMapperData::Array { sources: s1, targets: t1 } => {
                    let TypeMapperData::Array { sources: s2, targets: t2 } = c.mapper_data(m2)
                    else {
                        unreachable!("kinds equal");
                    };
                    let cr = compare_type_lists(c, s1, s2);
                    if cr != Ordering::Equal {
                        return cr;
                    }
                    compare_type_lists(c, t1, t2)
                }
                TypeMapperData::Merged { m1: mm1, m2: mm2 } => {
                    let TypeMapperData::Merged { m1: mm2_1, m2: mm2_2 } = c.mapper_data(m2) else {
                        unreachable!("kinds equal");
                    };
                    let cr = compare_type_mappers(c, Some(*mm1), Some(*mm2_1));
                    if cr != Ordering::Equal {
                        return cr;
                    }
                    compare_type_mappers(c, *mm2, *mm2_2)
                }
                // Go's switch: every other kind falls to `return 0`.
                _ => Ordering::Equal,
            }
        }
    }
}

/// Go: `func getDeclarationModifierFlagsFromSymbol(s *ast.Symbol) ast.ModifierFlags`.
pub fn get_declaration_modifier_flags_from_symbol(
    c: &Checker,
    store: &dyn NodeStore,
    s: SymbolId,
) -> ModifierFlags {
    get_declaration_modifier_flags_from_symbol_ex(c, store, s, false)
}

/// Go: `func getDeclarationModifierFlagsFromSymbolEx(s *ast.Symbol, isWrite bool) ast.ModifierFlags`.
pub fn get_declaration_modifier_flags_from_symbol_ex(
    c: &Checker,
    store: &dyn NodeStore,
    s: SymbolId,
    is_write: bool,
) -> ModifierFlags {
    use tsc_ast::CheckFlags as CF;
    let symbol = c.program.symbol(s);
    if symbol.check_flags.intersects(CF::SYNTHETIC) {
        let access_modifier = if !is_write && symbol.check_flags.intersects(CF::CONTAINS_PUBLIC)
            || is_write && symbol.check_flags.intersects(CF::CONTAINS_WRITE_PUBLIC)
        {
            ModifierFlags::PUBLIC
        } else if !is_write && symbol.check_flags.intersects(CF::CONTAINS_PROTECTED)
            || is_write && symbol.check_flags.intersects(CF::CONTAINS_WRITE_PROTECTED)
        {
            ModifierFlags::PROTECTED
        } else if !is_write && symbol.check_flags.intersects(CF::CONTAINS_PRIVATE)
            || is_write && symbol.check_flags.intersects(CF::CONTAINS_WRITE_PRIVATE)
        {
            ModifierFlags::PRIVATE
        } else {
            ModifierFlags::NONE
        };
        if symbol.check_flags.intersects(CF::CONTAINS_STATIC) {
            return access_modifier | ModifierFlags::STATIC;
        }
        return access_modifier;
    }
    if let Some(value_declaration) = symbol.value_declaration {
        let mut declaration: Option<NodeId> = None;
        if is_write {
            declaration = symbol
                .declarations
                .iter()
                .copied()
                .find(|d| store.node(*d).kind == Kind::SetAccessor);
        }
        if declaration.is_none() && symbol.flags.intersects(SymbolFlags::GET_ACCESSOR) {
            declaration = symbol
                .declarations
                .iter()
                .copied()
                .find(|d| store.node(*d).kind == Kind::GetAccessor);
        }
        let declaration = declaration.unwrap_or(value_declaration);
        let flags = binder_shims::get_combined_modifier_flags(store, declaration);
        if let Some(parent) = symbol.parent {
            if c.program.symbol(parent).flags.intersects(SymbolFlags::CLASS) {
                return flags;
            }
        }
        return flags & !ModifierFlags::ACCESSIBILITY_MODIFIER;
    }
    if symbol.flags.intersects(SymbolFlags::PROTOTYPE) {
        return ModifierFlags::PUBLIC | ModifierFlags::STATIC;
    }
    ModifierFlags::NONE
}

// ────────────────────────────────────────────────────────────────────────────
// Binary operator predicates (pure Kind checks)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isExponentiationOperator(kind ast.Kind) bool`.
pub fn is_exponentiation_operator(kind: Kind) -> bool {
    kind == Kind::AsteriskAsteriskToken
}

/// Go: `func isMultiplicativeOperator(kind ast.Kind) bool`.
pub fn is_multiplicative_operator(kind: Kind) -> bool {
    matches!(kind, Kind::AsteriskToken | Kind::SlashToken | Kind::PercentToken)
}

/// Go: `func isMultiplicativeOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_multiplicative_operator_or_higher(kind: Kind) -> bool {
    is_exponentiation_operator(kind) || is_multiplicative_operator(kind)
}

/// Go: `func isAdditiveOperator(kind ast.Kind) bool`.
pub fn is_additive_operator(kind: Kind) -> bool {
    matches!(kind, Kind::PlusToken | Kind::MinusToken)
}

/// Go: `func isAdditiveOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_additive_operator_or_higher(kind: Kind) -> bool {
    is_additive_operator(kind) || is_multiplicative_operator_or_higher(kind)
}

/// Go: `func isShiftOperator(kind ast.Kind) bool`.
pub fn is_shift_operator(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::LessThanLessThanToken
            | Kind::GreaterThanGreaterThanToken
            | Kind::GreaterThanGreaterThanGreaterThanToken
    )
}

/// Go: `func isShiftOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_shift_operator_or_higher(kind: Kind) -> bool {
    is_shift_operator(kind) || is_additive_operator_or_higher(kind)
}

/// Go: `func isRelationalOperator(kind ast.Kind) bool`.
pub fn is_relational_operator(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::LessThanToken
            | Kind::LessThanEqualsToken
            | Kind::GreaterThanToken
            | Kind::GreaterThanEqualsToken
            | Kind::InstanceOfKeyword
            | Kind::InKeyword
    )
}

/// Go: `func isRelationalOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_relational_operator_or_higher(kind: Kind) -> bool {
    is_relational_operator(kind) || is_shift_operator_or_higher(kind)
}

/// Go: `func isEqualityOperator(kind ast.Kind) bool`.
pub fn is_equality_operator(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::EqualsEqualsToken
            | Kind::EqualsEqualsEqualsToken
            | Kind::ExclamationEqualsToken
            | Kind::ExclamationEqualsEqualsToken
    )
}

/// Go: `func isEqualityOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_equality_operator_or_higher(kind: Kind) -> bool {
    is_equality_operator(kind) || is_relational_operator_or_higher(kind)
}

/// Go: `func isBitwiseOperator(kind ast.Kind) bool`.
pub fn is_bitwise_operator(kind: Kind) -> bool {
    matches!(kind, Kind::AmpersandToken | Kind::BarToken | Kind::CaretToken)
}

/// Go: `func isBitwiseOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_bitwise_operator_or_higher(kind: Kind) -> bool {
    is_bitwise_operator(kind) || is_equality_operator_or_higher(kind)
}

/// Go: `func isLogicalOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_logical_operator_or_higher(kind: Kind) -> bool {
    binder_shims::is_logical_binary_operator(kind) || is_bitwise_operator_or_higher(kind)
}

/// Go: `func isAssignmentOperatorOrHigher(kind ast.Kind) bool`.
pub fn is_assignment_operator_or_higher(kind: Kind) -> bool {
    kind == Kind::QuestionQuestionToken
        || is_logical_operator_or_higher(kind)
        || tsc_ast::is_assignment_operator(kind)
}

/// Go: `func isBinaryOperator(kind ast.Kind) bool`.
pub fn is_binary_operator(kind: Kind) -> bool {
    is_assignment_operator_or_higher(kind) || kind == Kind::CommaToken
}

// ────────────────────────────────────────────────────────────────────────────
// Type predicates
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isObjectLiteralType(t *Type) bool`.
pub fn is_object_literal_type(t: &Type) -> bool {
    t.object_flags.intersects(ObjectFlags::OBJECT_LITERAL)
}

/// Go: `func isDeclarationReadonly(declaration *ast.Node) bool`.
pub fn is_declaration_readonly(store: &dyn NodeStore, declaration: NodeId) -> bool {
    binder_shims::get_combined_modifier_flags(store, declaration).intersects(ModifierFlags::READONLY)
        && !binder_shims::is_parameter_property_declaration(
            store,
            declaration,
            store.node(declaration).parent.get(),
        )
}

/// Go: `func isObjectOrArrayLiteralType(t *Type) bool`.
pub fn is_object_or_array_literal_type(t: &Type) -> bool {
    t.object_flags
        .intersects(ObjectFlags::OBJECT_LITERAL | ObjectFlags::ARRAY_LITERAL)
}

/// Go: `func isThisTypeParameter(t *Type) bool`.
pub fn is_this_type_parameter(t: &Type) -> bool {
    t.flags.intersects(TypeFlags::TYPE_PARAMETER)
        && t.as_type_parameter().is_some_and(|d| d.is_this_type)
}

/// Go: `orderedSetMapThreshold` + `type orderedSet[T comparable] struct`.
///
/// PORT: Go's dual representation (linear scan below the threshold, hash set
/// above) is preserved exactly, including the late map materialization.
pub const ORDERED_SET_MAP_THRESHOLD: usize = 16;

pub struct OrderedSet<T: Eq + std::hash::Hash + Clone> {
    values_by_key: Option<rustc_hash::FxHashSet<T>>,
    values: Vec<T>,
}

impl<T: Eq + std::hash::Hash + Clone> Default for OrderedSet<T> {
    fn default() -> Self {
        Self { values_by_key: None, values: Vec::new() }
    }
}

impl<T: Eq + std::hash::Hash + Clone> OrderedSet<T> {
    /// Go: `func (s *orderedSet[T]) contains(value T) bool`.
    pub fn contains(&self, value: &T) -> bool {
        match &self.values_by_key {
            None => self.values.iter().any(|v| v == value),
            Some(map) => map.contains(value),
        }
    }

    /// Go: `func (s *orderedSet[T]) add(value T)`.
    pub fn add(&mut self, value: T) {
        self.values.push(value.clone());
        if self.values_by_key.is_none() {
            if self.values.len() <= ORDERED_SET_MAP_THRESHOLD {
                return;
            }
            let mut map = rustc_hash::FxHashSet::default();
            map.reserve(self.values.len() - 1);
            for v in &self.values[..self.values.len() - 1] {
                map.insert(v.clone());
            }
            self.values_by_key = Some(map);
        }
        self.values_by_key.as_mut().expect("just materialized").insert(value);
    }

    /// Go iterates `values` (the ordered representation).
    pub fn values(&self) -> &[T] {
        &self.values
    }
}

/// Go: `func getContainingFunctionOrClassStaticBlock(node *ast.Node) *ast.Node`.
pub fn get_containing_function_or_class_static_block(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    let parent = store.node(node).parent.get();
    if parent.is_none() {
        return None;
    }
    find_ancestor(store, parent, &mut |n| {
        binder_shims::is_function_like_or_class_static_block_declaration(n)
    })
}

/// Go: `func isNodeDescendantOf(node *ast.Node, ancestor *ast.Node) bool`.
pub fn is_node_descendant_of(store: &dyn NodeStore, node: NodeId, ancestor: NodeId) -> bool {
    let mut node = node;
    loop {
        if node == ancestor {
            return true;
        }
        let parent = store.node(node).parent.get();
        if parent.is_none() {
            return false;
        }
        node = parent;
    }
}

/// Go: `func isTypeUsableAsPropertyName(t *Type) bool`.
pub fn is_type_usable_as_property_name(t: &Type) -> bool {
    t.flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)
}

/// Go: `func getPropertyNameFromType(t *Type) string` — gets the symbolic name
/// for a member from its type.
pub fn get_property_name_from_type(t: &Type) -> String {
    if t.flags.intersects(TypeFlags::STRING_LITERAL) {
        return literal_string(t).to_string();
    }
    if t.flags.intersects(TypeFlags::NUMBER_LITERAL) {
        return literal_number(t).string();
    }
    if t.flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
        return t
            .as_unique_essymbol_type()
            .expect("UniqueESSymbolType data")
            .name
            .clone();
    }
    panic!("Unhandled case in getPropertyNameFromType");
}

/// Go: `func isNumericLiteralName(name string) bool` — tests whether
/// 'ToString(ToNumber(name))' is exactly equal to 'name' (see the Go comment
/// for the full rationale; accepts 'Infinity', '-Infinity', 'NaN').
pub fn is_numeric_literal_name(name: &str) -> bool {
    jsnum::from_string(name).string() == name
}

/// Go: `func isThisProperty(node *ast.Node) bool`.
pub fn is_this_property(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    (is_property_access_expression(n) || is_element_access_expression(n))
        && ast_shims::expression_of(store, node).is_some_and(|e| store.node(e).kind == Kind::ThisKeyword)
}

/// Go: `func isValidNumberString(s string, roundTripOnly bool) bool`.
pub fn is_valid_number_string(s: &str, round_trip_only: bool) -> bool {
    if s.is_empty() {
        return false;
    }
    let n = jsnum::from_string(s);
    !n.is_nan() && !n.0.is_infinite() && (!round_trip_only || n.string() == s)
}

/// Go: `func isValidBigIntString(s string, roundTripOnly bool) bool` — the
/// scanner-driven validation, verbatim.
pub fn is_valid_big_int_string(s: &str, round_trip_only: bool) -> bool {
    if s.is_empty() {
        return false;
    }
    // Go: SetOnError flips `success`; the port's scanner keeps the callback
    // and text borrowed for its whole lifetime ('a), so both outlive the
    // scans and the success flag rides in a Cell (interior mutability).
    let success_cell = std::cell::Cell::new(true);
    let text = s.to_string() + "n";
    let mut scanner = tsc_scanner::Scanner::new();
    scanner.set_skip_trivia(false);
    scanner.set_on_error(|_diagnostic, _start, _length, _args: &[String]| {
        success_cell.set(false);
    });
    scanner.set_text(&text);
    let mut result = scanner.scan();
    let negative = result == Kind::MinusToken;
    if negative {
        result = scanner.scan();
    }
    let flags = scanner.token_flags();
    // validate that
    // * scanning proceeded without error
    // * a bigint can be scanned, and that when it is scanned, it is
    // * the full length of the input string (so the scanner is one character
    //   beyond the augmented input length)
    // * it does not contain a numeric separator (the `BigInt` constructor
    //   does not accept a numeric separator in its input)
    let success = success_cell.get();
    success
        && result == Kind::BigIntLiteral
        && scanner.token_end() == s.len() + 1
        && !flags.intersects(tsc_ast::TokenFlags::CONTAINS_SEPARATOR)
        && (!round_trip_only
            || s == pseudo_big_int_to_string(&jsnum::new_pseudo_big_int(
                &jsnum::parse_pseudo_big_int(std::str::from_utf8(scanner.token_value()).expect("scanner token value is UTF-8")),
                negative,
            )))
}

/// Go: `func isValidESSymbolDeclaration(node *ast.Node) bool`.
pub fn is_valid_es_symbol_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind == Kind::VariableDeclaration {
        return ast_shims::is_var_const(store, node)
            && n.name().is_some_and(|name| is_identifier(store.node(name)))
            && is_variable_declaration_in_variable_statement(store, node);
    }
    if n.kind == Kind::PropertyDeclaration {
        return has_readonly_modifier(n) && binder_shims::has_static_modifier(n);
    }
    n.kind == Kind::PropertySignature && has_readonly_modifier(n)
}

/// Go: `func isVariableDeclarationInVariableStatement(node *ast.Node) bool`.
pub fn is_variable_declaration_in_variable_statement(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let parent = store.node(node).parent.get();
    store.node(parent).kind == Kind::VariableDeclarationList
        && store.node(store.node(parent).parent.get()).kind == Kind::VariableStatement
}

/// Go: `func IsKnownSymbol(symbol *ast.Symbol) bool`.
pub fn is_known_symbol(symbol: &Symbol) -> bool {
    is_late_bound_name(&symbol.name)
}

/// Go: `func IsPrivateIdentifierSymbol(symbol *ast.Symbol) bool`.
pub fn is_private_identifier_symbol(symbol: Option<&Symbol>) -> bool {
    symbol.is_some_and(|s| s.name.starts_with(&format!("{}#", tsc_ast::INTERNAL_SYMBOL_NAME_PREFIX)))
}

/// Go: `func isLateBoundName(name string) bool` — `len >= 2 && name[0] ==
/// '\xFE' && name[1] == '@'`. PORT: Go's 0xFE is an invalid-UTF-8 sentinel
/// byte; the landed Rust convention (tsc_ast::symbol, INTERNAL_SYMBOL_NAME_PREFIX)
/// carries it as the code point `'\u{FE}'`, so the byte-index checks become
/// char checks on the same shape.
pub fn is_late_bound_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next() == Some('\u{FE}') && chars.next() == Some('@')
}

/// Go: `func getContainingClassExcludingClassDecorators(node *ast.Node) *ast.ClassLikeDeclaration`.
pub fn get_containing_class_excluding_class_decorators(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    let parent = store.node(node).parent.get();
    if parent.is_none() {
        return None;
    }
    let decorator = find_ancestor_or_quit(store, parent, &mut |n| {
        if is_class_like(n) {
            return FindAncestorResult::Quit;
        }
        if n.kind == Kind::Decorator {
            return FindAncestorResult::True;
        }
        FindAncestorResult::False
    });
    if let Some(d) = decorator {
        let decorator_parent = store.node(d).parent.get();
        if !decorator_parent.is_none() && is_class_like(store.node(decorator_parent)) {
            return get_containing_class(store, decorator_parent);
        }
        return get_containing_class(store, d);
    }
    get_containing_class(store, node)
}

/// Go: `ast.IsClassLike`.
fn is_class_like(node: &Node) -> bool {
    matches!(node.kind, Kind::ClassDeclaration | Kind::ClassExpression)
}

/// Go: `ast.GetContainingClass` — walks up to the nearest class-like
/// container through the non-decorated path.
pub(crate) fn get_containing_class(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    find_ancestor(store, node, &mut is_class_like)
}

/// Go: `func isClassInstanceProperty(node *ast.Node) bool`.
pub fn is_class_instance_property(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if is_in_js_file(n)
        && !node.is_none()
        && is_binary_expression(n)
    {
        let d = n.as_binary_expression().expect("BinaryExpression data");
        let left = d.left;
        return (!binder_shims::is_bindable_static_access_expression(store, left, false)
            || !is_prototype_access(store, left))
            && !binder_shims::is_bindable_static_name_expression(store, left, true);
    }
    let parent = n.parent.get();
    !parent.is_none()
        && is_class_like(store.node(parent))
        && n.kind == Kind::PropertyDeclaration
        && !binder_shims::has_accessor_modifier(n)
}

/// Go: `ast.IsPrototypeAccess` — `x.prototype` property access.
fn is_prototype_access(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    is_property_access_expression(n)
        && n.name().is_some_and(|name| tsc_ast::node_text(store, name) == "prototype")
}

/// Go: `func isThisInitializedObjectBindingExpression(node *ast.Node) bool`.
pub fn is_this_initialized_object_binding_expression(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    if node.is_none() {
        return false;
    }
    let n = store.node(node);
    if n.kind != Kind::ShorthandPropertyAssignment && n.kind != Kind::PropertyAssignment {
        return false;
    }
    let parent = n.parent.get();
    let grandparent = store.node(parent).parent.get();
    let gp = store.node(grandparent);
    if gp.kind != Kind::BinaryExpression {
        return false;
    }
    let d = gp.as_binary_expression().expect("BinaryExpression data");
    store.node(d.operator_token).kind == Kind::EqualsToken
        && store.node(d.right).kind == Kind::ThisKeyword
}

/// Go: `func isThisInitializedDeclaration(node *ast.Node) bool`.
pub fn is_this_initialized_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    if node.is_none() {
        return false;
    }
    let n = store.node(node);
    n.kind == Kind::VariableDeclaration
        && ast_shims::initializer_of(store, node).is_some_and(|i| store.node(i).kind == Kind::ThisKeyword)
}

/// Go: `func isInfinityOrNaNString(name string) bool`.
pub fn is_infinity_or_nan_string(name: &str) -> bool {
    name == "Infinity" || name == "-Infinity" || name == "NaN"
}

/// Go: `func (c *Checker) isConstantVariable(symbol *ast.Symbol) bool`.
pub fn is_constant_variable(c: &Checker, symbol: SymbolId) -> bool {
    let sym = c.program.symbol(symbol);
    sym.flags.intersects(SymbolFlags::VARIABLE)
        && c.get_declaration_node_flags_from_symbol(symbol).intersects(NodeFlags::CONSTANT)
}

/// Go: `func (c *Checker) isParameterOrMutableLocalVariable(symbol *ast.Symbol) bool`.
pub fn is_parameter_or_mutable_local_variable(c: &Checker, store: &dyn NodeStore, symbol: SymbolId) -> bool {
    // Return true if symbol is a parameter, a catch clause variable, or a
    // mutable local variable
    let value_declaration = c.program.symbol(symbol).value_declaration;
    if let Some(decl) = value_declaration {
        let declaration = binder_shims::get_root_declaration(store, decl);
        return store.node(declaration).kind == Kind::Parameter
            || (store.node(declaration).kind == Kind::VariableDeclaration
                && (store.node(store.node(declaration).parent.get()).kind == Kind::CatchClause
                    || is_mutable_local_variable_declaration(store, declaration)));
    }
    false
}

/// Go: `func (c *Checker) isMutableLocalVariableDeclaration(declaration *ast.Node) bool`.
pub fn is_mutable_local_variable_declaration(
    store: &dyn NodeStore,
    declaration: NodeId,
) -> bool {
    // Return true if symbol is a non-exported and non-global `let` variable
    let parent = store.node(declaration).parent.get();
    let pn = store.node(parent);
    if !pn.flags.intersects(NodeFlags::LET) {
        return false;
    }
    !(binder_shims::get_combined_modifier_flags(store, declaration)
        .intersects(ModifierFlags::EXPORT)
        || (store.node(pn.parent.get()).kind == Kind::VariableStatement
            && is_global_source_file(store, store.node(pn.parent.get()).parent.get())))
    // PORT (W3T4): Go probes `declaration.Parent.Parent.Parent` —
    // VariableDeclarationList → VariableStatement → SourceFile. The
    // former code hopped one parent too many (the SourceFile's parent is
    // the NodeId::NONE sentinel; Go's nil SourceFile-parent never reaches
    // IsGlobalSourceFile).
}

/// Go: `ast.IsGlobalSourceFile` — a source file that is neither an external
/// module nor a CommonJS module.
pub fn is_global_source_file(store: &dyn NodeStore, node: NodeId) -> bool {
    if store.node(node).kind != Kind::SourceFile {
        return false;
    }
    let file = store.file(node.file_id());
    let payload = file.payload();
    payload.external_module_indicator.get().is_none()
        && payload.common_js_module_indicator.get().is_none()
}

/// Go: `func isInAmbientOrTypeNode(node *ast.Node) bool`.
pub fn is_in_ambient_or_type_node(store: &dyn NodeStore, node: NodeId) -> bool {
    if store.node(node).flags.intersects(NodeFlags::AMBIENT) {
        return true;
    }
    let parent = store.node(node).parent.get();
    if parent.is_none() {
        return false;
    }
    find_ancestor(store, parent, &mut |n| {
        n.kind == Kind::InterfaceDeclaration || is_type_alias(n) || n.kind == Kind::TypeLiteral
    })
    .is_some()
}

/// Go: `func isLiteralExpressionOfObject(node *ast.Node) bool`.
pub fn is_literal_expression_of_object(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::ObjectLiteralExpression
            | Kind::ArrayLiteralExpression
            | Kind::RegularExpressionLiteral
            | Kind::FunctionExpression
            | Kind::ClassExpression
    )
}

/// Go: `func canHaveFlowNode(node *ast.Node) bool` — `node.FlowNodeData() != nil`.
pub fn can_have_flow_node(node: &Node) -> bool {
    // Go's FlowNodeData() is the generated dispatcher over payload kinds with
    // flow nodes; the port's payloads carry `flow_node: Option<FlowNodeId>`.
    // The kind set mirrors binder's flow-capable kinds (binder.rs assigns
    // flow nodes only to these).
    matches!(
        node.kind,
        Kind::Identifier
            | Kind::ThisKeyword
            | Kind::SuperKeyword
            | Kind::NonNullExpression
            | Kind::MetaProperty
            | Kind::QualifiedName
            | Kind::PropertyAccessExpression
            | Kind::ElementAccessExpression
            | Kind::CallExpression
            | Kind::NewExpression
            | Kind::BinaryExpression
            | Kind::PartiallyEmittedExpression
            | Kind::PrefixUnaryExpression
            | Kind::PostfixUnaryExpression
            | Kind::VariableDeclaration
            | Kind::BindingElement
            | Kind::ForInStatement
            | Kind::ForStatement
            | Kind::IfStatement
            | Kind::DoStatement
            | Kind::WhileStatement
            | Kind::ReturnStatement
            | Kind::WithStatement
            | Kind::SwitchStatement
            | Kind::CaseClause | Kind::DefaultClause
            | Kind::LabeledStatement
            | Kind::TryStatement
            | Kind::CatchClause
            | Kind::BreakStatement
            | Kind::ContinueStatement
            | Kind::ThrowStatement
            | Kind::ExpressionStatement
            | Kind::ArrowFunction
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::ClassStaticBlockDeclaration
            | Kind::SourceFile
    )
}

/// Go: `func isNonNullAccess(node *ast.Node) bool`.
pub fn is_non_null_access(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    is_access_expression(n)
        && ast_shims::expression_of(store, node).is_some_and(|e| store.node(e).kind == Kind::NonNullExpression)
}

/// Go: `func getBindingElementPropertyName(node *ast.Node) *ast.Node` —
/// `node.PropertyNameOrName()`.
pub fn get_binding_element_property_name(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    store
        .node(node)
        .as_binding_element()
        .and_then(|d| d.property_name.or(d.name))
}

/// Go: `func isCallChain(node *ast.Node) bool`.
pub fn is_call_chain(node: &Node) -> bool {
    node.kind == Kind::CallExpression && node.flags.intersects(NodeFlags::OPTIONAL_CHAIN)
}

/// Go: `func (c *Checker) callLikeExpressionMayHaveTypeArguments(node *ast.Node) bool`.
pub fn call_like_expression_may_have_type_arguments(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::CallExpression | Kind::NewExpression | Kind::TaggedTemplateExpression
    ) || tsc_ast::utilities::is_jsx_opening_like_element(node)
}

/// Go: `func isSuperCall(n *ast.Node) bool`.
pub fn is_super_call(store: &dyn NodeStore, n: NodeId) -> bool {
    let node = store.node(n);
    node.kind == Kind::CallExpression
        && ast_shims::expression_of(store, n).is_some_and(|e| store.node(e).kind == Kind::SuperKeyword)
}

/// Go: `func getMembersOfDeclaration(node *ast.Node) []*ast.Node`.
pub fn get_members_of_declaration(store: &dyn NodeStore, node: NodeId) -> &[NodeId] {
    let n = store.node(node);
    match n.kind {
        Kind::InterfaceDeclaration
        | Kind::ClassDeclaration
        | Kind::ClassExpression
        | Kind::TypeLiteral => tsc_binder::node_data::members_of(store, node),
        Kind::ObjectLiteralExpression => n
            .as_object_literal_expression()
            .and_then(|d| d.properties.as_ref())
            .map(|p| p.nodes.as_ref())
            .unwrap_or(&[]),
        _ => &[],
    }
}

/// Go: `func isInRightSideOfImportOrExportAssignment(node *ast.EntityName) bool`.
pub fn is_in_right_side_of_import_or_export_assignment(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let mut node = node;
    while store.node(store.node(node).parent.get()).kind == Kind::QualifiedName {
        node = store.node(node).parent.get();
    }
    let parent = store.node(node).parent.get();
    let pn = store.node(parent);
    (pn.kind == Kind::ImportEqualsDeclaration
        && pn.as_import_equals_declaration()
            .is_some_and(|d| d.module_reference == node))
        || (pn.kind == Kind::ExportAssignment && ast_shims::expression_of(store, parent) == Some(node))
}

/// Go: `func isJsxIntrinsicTagName(tagName *ast.Node) bool`.
pub fn is_jsx_intrinsic_tag_name(store: &dyn NodeStore, tag_name: NodeId) -> bool {
    let n = store.node(tag_name);
    (is_identifier(n) && tsc_scanner::utilities::is_intrinsic_jsx_name(tsc_ast::node_text(store, tag_name)))
        || n.kind == Kind::JsxNamespacedName
}

/// Go: `func getContainingObjectLiteral(f *ast.SignatureDeclaration) *ast.Node`.
pub fn get_containing_object_literal(store: &dyn NodeStore, f: NodeId) -> Option<NodeId> {
    let n = store.node(f);
    let parent = n.parent.get();
    if parent.is_none() {
        return None;
    }
    if (n.kind == Kind::MethodDeclaration
        || n.kind == Kind::GetAccessor
        || n.kind == Kind::SetAccessor)
        && store.node(parent).kind == Kind::ObjectLiteralExpression
    {
        return Some(parent);
    } else if n.kind == Kind::FunctionExpression
        && store.node(parent).kind == Kind::PropertyAssignment
    {
        return Some(store.node(parent).parent.get());
    }
    None
}

/// Go: `func isImportTypeQualifierPart(node *ast.Node) *ast.Node`.
pub fn is_import_type_qualifier_part(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut node = node;
    let mut parent = store.node(node).parent.get();
    while is_qualified_name(store.node(parent)) {
        node = parent;
        parent = store.node(node).parent.get();
    }
    if !parent.is_none() {
        let pn = store.node(parent);
        if pn.kind == Kind::ImportType
            && pn.as_import_type_node().is_some_and(|d| d.qualifier == Some(node))
        {
            return Some(parent);
        }
    }
    None
}

/// Go: `func isInNameOfExpressionWithTypeArgumentsOrHeritageTypeReference(node *ast.Node) bool`.
pub fn is_in_name_of_expression_with_type_arguments_or_heritage_type_reference(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let mut node = node;
    loop {
        let parent = store.node(node).parent.get();
        let pk = store.node(parent).kind;
        if pk != Kind::PropertyAccessExpression && pk != Kind::QualifiedName {
            break;
        }
        node = parent;
    }
    let parent = store.node(node).parent.get();
    store.node(parent).kind == Kind::ExpressionWithTypeArguments
        || ast_shims::is_name_of_heritage_clause_type_reference(store, node)
}

/// Go: `func getIndexSymbolFromSymbolTable(symbolTable ast.SymbolTable) *ast.Symbol`.
pub fn get_index_symbol_from_symbol_table(symbol_table: &SymbolTable) -> Option<SymbolId> {
    symbol_table.get(INTERNAL_SYMBOL_NAME_INDEX).copied()
}

/// Go: `func expressionResultIsUnused(node *ast.Node) bool` — indicates
/// whether the result of an `Expression` will be unused.
pub fn expression_result_is_unused(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut node = node;
    loop {
        let parent = store.node(node).parent.get();
        let p = store.node(parent);
        // walk up parenthesized expressions, but keep a pointer to the
        // top-most parenthesized expression
        if is_parenthesized_expression(p) {
            node = parent;
            continue;
        }
        // result is unused in an expression statement, `void` expression, or
        // the initializer or incrementer of a `for` loop
        let is_for_init_or_incrementer = p.kind == Kind::ForStatement
            && (ast_shims::initializer_of(store, parent) == Some(node)
                || p.as_for_statement().is_some_and(|d| d.incrementor == Some(node)));
        if p.kind == Kind::ExpressionStatement
            || p.kind == Kind::VoidExpression
            || is_for_init_or_incrementer
        {
            return true;
        }
        if p.kind == Kind::BinaryExpression
            && p.as_binary_expression().is_some_and(|d| store.node(d.operator_token).kind == Kind::CommaToken)
        {
            let d = p.as_binary_expression().expect("BinaryExpression data");
            // left side of comma is always unused
            if node == d.left {
                return true;
            }
            // right side of comma is unused if parent is unused
            node = parent;
            continue;
        }
        return false;
    }
}

/// Go: `func pseudoBigIntToString(value jsnum.PseudoBigInt) string`.
pub fn pseudo_big_int_to_string(value: &PseudoBigInt) -> String {
    value.string()
}

/// Go: `func getSuperContainer(node *ast.Node, stopOnFunctions bool) *ast.Node`.
pub fn get_super_container(
    store: &dyn NodeStore,
    node: NodeId,
    stop_on_functions: bool,
) -> Option<NodeId> {
    let mut node = node;
    loop {
        node = store.node(node).parent.get();
        if node.is_none() {
            return None;
        }
        let n = store.node(node);
        match n.kind {
            Kind::ComputedPropertyName => {
                node = n.parent.get();
                if node.is_none() {
                    return None;
                }
            }
            Kind::FunctionDeclaration | Kind::FunctionExpression | Kind::ArrowFunction => {
                if !stop_on_functions {
                    continue;
                }
                return Some(node);
            }
            Kind::PropertyDeclaration
            | Kind::PropertySignature
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::ClassStaticBlockDeclaration => return Some(node),
            Kind::Decorator => {
                // Decorators are always applied outside of the body of a class
                // or method.
                let parent = n.parent.get();
                let pkind = store.node(parent).kind;
                if pkind == Kind::Parameter
                    && is_class_element(store.node(store.node(parent).parent.get()))
                {
                    // If the decorator's parent is a Parameter, we resolve the
                    // this container from the grandparent class declaration.
                    node = store.node(parent).parent.get();
                } else if is_class_element(store.node(parent)) {
                    // If the decorator's parent is a class element, we resolve
                    // the 'this' container from the parent class declaration.
                    node = parent;
                }
            }
            _ => {}
        }
    }
}

/// Go: `ast.IsClassElement`.
fn is_class_element(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::PropertyDeclaration
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::ClassStaticBlockDeclaration
            | Kind::SemicolonClassElement
            | Kind::Constructor
            | Kind::IndexSignature
    )
}

/// Go: `func forEachYieldExpression(body *ast.Node, visitor func(expr *ast.Node) bool) bool`.
///
/// PORT (M6 S8): the visitor needs the checker (Go's closures capture `c`);
/// the store rides the checker (`c.store()`), which also lifts the
/// store-vs-checker split borrow the NodeId-only shape forced.
pub fn for_each_yield_expression(
    c: &mut crate::checker::Checker,
    body: NodeId,
    visitor: &mut dyn FnMut(&mut crate::checker::Checker, NodeId) -> bool,
) -> bool {
    fn traverse(
        c: &mut crate::checker::Checker,
        node: NodeId,
        visitor: &mut dyn FnMut(&mut crate::checker::Checker, NodeId) -> bool,
    ) -> bool {
        // PORT: `kind` is Copy; binding it first ends the store borrow so
        // the arms can take the checker mutably.
        let kind = c.store().node(node).kind;
        match kind {
            Kind::YieldExpression => {
                if visitor(c, node) {
                    return true;
                }
                let store = c.store();
                let Some(operand) = ast_shims::expression_of(store, node) else {
                    return false;
                };
                traverse(c, operand, visitor)
            }
            Kind::EnumDeclaration
            | Kind::InterfaceDeclaration
            | Kind::ModuleDeclaration
            | Kind::TypeAliasDeclaration => {
                // These are not allowed inside a generator now, but eventually
                // they may be allowed as local types. Regardless, skip them to
                // avoid the work.
                false
            }
            _ => {
                let n = c.store().node(node);
                if tsc_ast::utilities::is_function_like(n) {
                    if let Some(name) = n.name() {
                        if c.store().node(name).kind == Kind::ComputedPropertyName {
                            // Note that we will not include methods/accessors
                            // of a class because they would require first
                            // descending into the class. This is by design.
                            return traverse(c, name, visitor);
                        }
                    }
                } else if !is_part_of_type_node(c.store(), node) {
                    // This is the general case, which should include mostly
                    // expressions and statements. Also includes NodeArrays.
                    // PORT: children are collected before the visitor runs so
                    // the checker can be re-borrowed mutably per child.
                    let mut children: Vec<NodeId> = Vec::new();
                    collect_children_shim(c.store().node(node), &mut children);
                    for child in children {
                        if traverse(c, child, visitor) {
                            return true;
                        }
                    }
                }
                false
            }
        }
    }
    traverse(c, body, visitor)
}

/// The `ast.IsPartOfTypeNode` walk, partial surface: the type-node kind range
/// and the pure-keyword arms are exact; the identifier/qualified-name parent
/// recursions need the full ast/utilities.go port (TODO(port M7)).
pub(crate) fn is_part_of_type_node(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if matches!(
        n.kind,
        Kind::AnyKeyword
            | Kind::UnknownKeyword
            | Kind::NumberKeyword
            | Kind::BigIntKeyword
            | Kind::StringKeyword
            | Kind::BooleanKeyword
            | Kind::SymbolKeyword
            | Kind::ObjectKeyword
            | Kind::UndefinedKeyword
            | Kind::NullKeyword
            | Kind::NeverKeyword
            | Kind::TypeReference
            | Kind::UnionType
            | Kind::IntersectionType
            | Kind::ConditionalType
            | Kind::TypeOperator
            | Kind::ArrayType
            | Kind::IndexedAccessType
            | Kind::TupleType
            | Kind::TypeLiteral
            | Kind::TypeQuery
            | Kind::MappedType
            | Kind::FunctionType
            | Kind::ConstructorType
            | Kind::ParenthesizedType
            | Kind::ImportType
            | Kind::LiteralType
            | Kind::InferType
            | Kind::TemplateLiteralType
    ) {
        return true;
    }
    if n.kind == Kind::VoidKeyword {
        return store.node(n.parent.get()).kind != Kind::VoidExpression;
    }
    let _ = store;
    false
}

/// The `node.ForEachChild(traverse)` arm — Go visits every child in order,
/// stopping when the visitor returns true. The port drives the generic
/// per-kind child walk via the store's node payload (the same table the
/// deep-clone/visitor crates use).
///
/// PORT (spike): enumerating every payload's children by hand is the
/// ast/utilities.go wholesale wave's job; the generator-shaped walk below
/// covers the statement/expression kinds reachable from `forEachYieldExpression`
/// and panics on anything else rather than silently missing children.
#[allow(dead_code)] // TODO(port M7): the generated ForEachChild table wave owns it
fn for_each_child_stopping(
    store: &dyn NodeStore,
    node: NodeId,
    visitor: &mut dyn FnMut(NodeId) -> bool,
) -> bool {
    let n = store.node(node);
    let mut children: Vec<NodeId> = Vec::new();
    collect_children_shim(n, &mut children);
    for child in children {
        if visitor(child) {
            return true;
        }
    }
    let _ = store;
    false
}

/// Child enumeration for the kinds `forEachYieldExpression` can encounter.
/// TODO(port M7): replace with the generated ForEachChild table.
fn collect_children_shim(n: &Node, children: &mut Vec<NodeId>) {
    // TODO(port M7): the generated ForEachChild table replaces this shim
    // wholesale (payload-driven enumeration of the kinds the walker meets).
    use tsc_ast::NodeData as D;
    match &n.data {
        D::Block(d) => {
            if let Some(list) = &d.statements {
                children.extend(list.nodes.iter().copied())
            }
        }
        D::ExpressionStatement(d) => children.push(d.expression),
        D::VariableStatement(d) => children.push(d.declaration_list),
        D::VariableDeclarationList(d) => {
            if let Some(list) = &d.declarations {
                children.extend(list.nodes.iter().copied())
            }
        }
        D::VariableDeclaration(d) => {
            if let Some(i) = d.initializer {
                children.push(i);
            }
        }
        D::IfStatement(d) => {
            children.push(d.expression);
            children.push(d.then_statement);
            if let Some(e) = d.else_statement {
                children.push(e);
            }
        }
        D::DoStatement(d) => {
            children.push(d.statement);
            children.push(d.expression);
        }
        D::WhileStatement(d) => {
            children.push(d.statement);
            children.push(d.expression);
        }
        D::ForStatement(d) => {
            if let Some(i) = d.initializer {
                children.push(i);
            }
            if let Some(c) = d.condition {
                children.push(c);
            }
            if let Some(i) = d.incrementor {
                children.push(i);
            }
            children.push(d.statement);
        }
        D::ForInOrOfStatement(d) => {
            children.push(d.initializer);
            children.push(d.expression);
            children.push(d.statement);
        }
        D::ReturnStatement(d) => {
            if let Some(e) = d.expression {
                children.push(e);
            }
        }
        D::WithStatement(d) => {
            children.push(d.expression);
            children.push(d.statement);
        }
        D::SwitchStatement(d) => {
            children.push(d.expression);
            children.push(d.case_block);
        }
        D::CaseBlock(d) => {
            if let Some(clauses) = &d.clauses {
                children.extend(clauses.nodes.iter().copied())
            }
        }
        D::CaseOrDefaultClause(d) => {
            children.push(d.expression);
            if let Some(statements) = &d.statements {
                children.extend(statements.nodes.iter().copied())
            }
        }
        D::LabeledStatement(d) => {
            children.push(d.label);
            children.push(d.statement);
        }
        D::ThrowStatement(d) => children.push(d.expression),
        D::TryStatement(d) => {
            children.push(d.try_block);
            if let Some(c) = d.catch_clause {
                children.push(c);
            }
            if let Some(f) = d.finally_block {
                children.push(f);
            }
        }
        D::BinaryExpression(d) => {
            children.push(d.left);
            children.push(d.right);
        }
        D::PrefixUnaryExpression(d) => children.push(d.operand),
        D::PostfixUnaryExpression(d) => children.push(d.operand),
        D::ConditionalExpression(d) => {
            children.push(d.condition);
            children.push(d.when_true);
            children.push(d.when_false);
        }
        D::YieldExpression(d) => {
            if let Some(e) = d.expression {
                children.push(e);
            }
        }
        D::AwaitExpression(d) => children.push(d.expression),
        D::ArrayLiteralExpression(d) => {
            if let Some(elements) = &d.elements {
                children.extend(elements.nodes.iter().copied())
            }
        }
        D::ObjectLiteralExpression(d) => {
            if let Some(properties) = &d.properties {
                children.extend(properties.nodes.iter().copied())
            }
        }
        D::PropertyAssignment(d) => children.push(d.initializer),
        D::SpreadAssignment(d) => {
            children.push(d.expression);
        }
        D::ParenthesizedExpression(d) => children.push(d.expression),
        D::CallExpression(d) => {
            children.push(d.expression);
            if let Some(args) = &d.arguments {
                children.extend(args.nodes.iter().copied())
            }
        }
        D::NewExpression(d) => {
            children.push(d.expression);
            if let Some(args) = &d.arguments {
                children.extend(args.nodes.iter().copied())
            }
        }
        D::TaggedTemplateExpression(d) => {
            children.push(d.tag);
            children.push(d.template);
        }
        D::TemplateExpression(d) => {
            children.push(d.head);
            if let Some(spans) = &d.template_spans {
                children.extend(spans.nodes.iter().copied())
            }
        }
        D::TemplateSpan(d) => {
            children.push(d.expression);
            children.push(d.literal);
        }
        D::PropertyAccessExpression(d) => {
            children.push(d.expression);
            children.push(d.name);
        }
        D::ElementAccessExpression(d) => {
            children.push(d.expression);
            children.push(d.argument_expression);
        }
        D::NonNullExpression(d) => children.push(d.expression),
        D::DeleteExpression(d) => children.push(d.expression),
        D::TypeOfExpression(d) => children.push(d.expression),
        D::VoidExpression(d) => children.push(d.expression),
        D::AsExpression(d) => children.push(d.expression),
        D::SatisfiesExpression(d) => children.push(d.expression),
        D::TypeAssertion(d) => children.push(d.expression),
        D::SpreadElement(d) => children.push(d.expression),
        D::PartiallyEmittedExpression(d) => {
            children.push(d.expression);
        }
        _ => {
            // TODO(port M7): the remaining kinds are unreachable from
            // forEachYieldExpression's statement/expression traversal; the
            // generated ForEachChild table replaces this shim wholesale.
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Containers, signatures, features
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getEnclosingContainer(node *ast.Node) *ast.Node` —
/// `FindAncestor(node.Parent, GetContainerFlags(n)&IsContainer != 0)`.
/// PORT: `find_ancestor`'s visitor receives `&Node` (Go `*Node`), but the
/// container-flags table needs the `NodeId`, so the parent walk is inlined
/// (same traversal shape as `find_ancestor`'s simple-predicate form).
pub fn get_enclosing_container(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut current = store.node(node).parent.get();
    while !current.is_none() {
        if tsc_binder::binder::get_container_flags(store, current)
            .intersects(tsc_binder::binder::ContainerFlags::IS_CONTAINER)
        {
            return Some(current);
        }
        current = store.node(current).parent.get();
    }
    None
}

/// Go: `func getDeclarationsOfKind(symbol *ast.Symbol, kind ast.Kind) []*ast.Node`.
pub fn get_declarations_of_kind(
    c: &Checker,
    symbol: SymbolId,
    kind: Kind,
) -> Vec<NodeId> {
    c.program
        .symbol(symbol)
        .declarations
        .iter()
        .copied()
        .filter(|d| _kind_of(c, *d) == kind)
        .collect()
}

fn _kind_of(c: &Checker, d: NodeId) -> Kind {
    c.program.node_store().node(d).kind
}

/// Go: `func hasType(node *ast.Node) bool` — `node.Type() != nil`.
pub fn has_type(store: &dyn NodeStore, node: NodeId) -> bool {
    ast_shims::type_of_node(store, node).is_some()
}

/// Go: `func getNonRestParameterCount(sig *Signature) int`.
pub fn get_non_rest_parameter_count(sig: &Signature) -> usize {
    sig.parameters.len()
        - if signature_has_rest_parameter(sig) { 1 } else { 0 }
}

/// Go: `func signatureHasRestParameter(sig *Signature) bool` (checker.go) —
/// `sig.flags&SignatureFlagsHasRestParameter != 0`.
pub fn signature_has_rest_parameter(sig: &Signature) -> bool {
    sig.flags.intersects(crate::types::SignatureFlags::HAS_REST_PARAMETER)
}

/// Go: `func minAndMax[T any](slice []T, getValue func(value T) int) (int, int)`.
pub fn min_and_max<T>(slice: &[T], get_value: impl Fn(&T) -> i32) -> (i32, i32) {
    let mut min_value = 0;
    let mut max_value = 0;
    for (i, element) in slice.iter().enumerate() {
        let value = get_value(element);
        if i == 0 {
            min_value = value;
            max_value = value;
        } else {
            min_value = min_value.min(value);
            max_value = max_value.max(value);
        }
    }
    (min_value, max_value)
}

/// Go: `type FeatureMapEntry struct { lib string; props []string }`.
#[derive(Clone, Debug)]
pub struct FeatureMapEntry {
    pub lib: &'static str,
    pub props: &'static [&'static str],
}

/// Go: `var getFeatureMap = sync.OnceValue(...)` — the full static table,
/// byte-for-byte.
pub fn get_feature_map() -> &'static FxHashMap<&'static str, Vec<FeatureMapEntry>> {
    static FEATURE_MAP: OnceLock<FxHashMap<&'static str, Vec<FeatureMapEntry>>> = OnceLock::new();
    FEATURE_MAP.get_or_init(build_feature_map)
}

/// Go: `func (c *Checker) getSuggestedLibForNonExistentName(name) string`
/// (checker.go:1777) — REAL (M6 S10): the feature map's first lib for the
/// name (`typeFeatures[0].lib`).
pub(crate) fn get_suggested_lib_for_non_existent_name(name: &str) -> String {
    get_feature_map()
        .get(name)
        .and_then(|type_features| type_features.first())
        .map(|entry| entry.lib.to_string())
        .unwrap_or_default()
}

/// Inverse-table builder for the Go literal below — same data, same order.
fn feature_entries(entries: &[(&'static str, &'static [&'static str])]) -> Vec<FeatureMapEntry> {
    entries
        .iter()
        .map(|&(lib, props)| FeatureMapEntry { lib, props })
        .collect()
}

fn build_feature_map() -> FxHashMap<&'static str, Vec<FeatureMapEntry>> {
    let mut m = FxHashMap::default();
    m.insert(
        "Array",
        feature_entries(&[
            ("es2015", &["find", "findIndex", "fill", "copyWithin", "entries", "keys", "values"]),
            ("es2016", &["includes"]),
            ("es2019", &["flat", "flatMap"]),
            ("es2022", &["at"]),
            ("es2023", &["findLastIndex", "findLast", "toReversed", "toSorted", "toSpliced", "with"]),
        ]),
    );
    m.insert("Iterator", feature_entries(&[("es2015", &[])]));
    m.insert("IteratorConstructor", feature_entries(&[("es2026", &["concat"])]));
    m.insert("RawJSON", feature_entries(&[("es2026", &[])]));
    m.insert("JSON", feature_entries(&[("es2026", &["isRawJSON", "rawJSON"])]));
    m.insert("AsyncIterator", feature_entries(&[("es2015", &[])]));
    m.insert(
        "ArrayBuffer",
        feature_entries(&[(
            "es2024",
            &[
                "maxByteLength",
                "resizable",
                "resize",
                "detached",
                "transfer",
                "transferToFixedLength",
            ],
        )]),
    );
    m.insert(
        "Atomics",
        feature_entries(&[
            (
                "es2017",
                &[
                    "add",
                    "and",
                    "compareExchange",
                    "exchange",
                    "isLockFree",
                    "load",
                    "or",
                    "store",
                    "sub",
                    "wait",
                    "notify",
                    "xor",
                ],
            ),
            ("es2024", &["waitAsync"]),
        ]),
    );
    m.insert(
        "SharedArrayBuffer",
        feature_entries(&[
            ("es2017", &["byteLength", "slice"]),
            ("es2024", &["growable", "maxByteLength", "grow"]),
        ]),
    );
    m.insert("AsyncIterable", feature_entries(&[("es2018", &[])]));
    m.insert("AsyncIterableIterator", feature_entries(&[("es2018", &[])]));
    m.insert("AsyncGenerator", feature_entries(&[("es2018", &[])]));
    m.insert("AsyncGeneratorFunction", feature_entries(&[("es2018", &[])]));
    m.insert(
        "RegExp",
        feature_entries(&[
            ("es2015", &["flags", "sticky", "unicode"]),
            ("es2018", &["dotAll"]),
            ("es2024", &["unicodeSets"]),
        ]),
    );
    m.insert("RegExpConstructor", feature_entries(&[("es2025", &["escape"])]));
    m.insert(
        "Reflect",
        feature_entries(&[(
            "es2015",
            &[
                "apply",
                "construct",
                "defineProperty",
                "deleteProperty",
                "get",
                "getOwnPropertyDescriptor",
                "getPrototypeOf",
                "has",
                "isExtensible",
                "ownKeys",
                "preventExtensions",
                "set",
                "setPrototypeOf",
            ],
        )]),
    );
    m.insert(
        "ArrayConstructor",
        feature_entries(&[("es2015", &["from", "of"]), ("es2026", &["fromAsync"])]),
    );
    m.insert(
        "ObjectConstructor",
        feature_entries(&[
            ("es2015", &["assign", "getOwnPropertySymbols", "keys", "is", "setPrototypeOf"]),
            ("es2017", &["values", "entries", "getOwnPropertyDescriptors"]),
            ("es2019", &["fromEntries"]),
            ("es2022", &["hasOwn"]),
            ("es2024", &["groupBy"]),
        ]),
    );
    m.insert(
        "NumberConstructor",
        feature_entries(&[(
            "es2015",
            &["isFinite", "isInteger", "isNaN", "isSafeInteger", "parseFloat", "parseInt"],
        )]),
    );
    m.insert(
        "Math",
        feature_entries(&[
            (
                "es2015",
                &[
                    "clz32",
                    "imul",
                    "sign",
                    "log10",
                    "log2",
                    "log1p",
                    "expm1",
                    "cosh",
                    "sinh",
                    "tanh",
                    "acosh",
                    "asinh",
                    "atanh",
                    "hypot",
                    "trunc",
                    "fround",
                    "cbrt",
                ],
            ),
            ("es2025", &["f16round"]),
            ("es2026", &["sumPrecise"]),
        ]),
    );
    m.insert(
        "Map",
        feature_entries(&[
            ("es2015", &["entries", "keys", "values"]),
            ("es2026", &["getOrInsert", "getOrInsertComputed"]),
        ]),
    );
    m.insert("MapConstructor", feature_entries(&[("es2024", &["groupBy"])]));
    m.insert(
        "Set",
        feature_entries(&[
            ("es2015", &["entries", "keys", "values"]),
            (
                "es2025",
                &[
                    "union",
                    "intersection",
                    "difference",
                    "symmetricDifference",
                    "isSubsetOf",
                    "isSupersetOf",
                    "isDisjointFrom",
                ],
            ),
        ]),
    );
    m.insert(
        "PromiseConstructor",
        feature_entries(&[
            ("es2015", &["all", "race", "reject", "resolve"]),
            ("es2020", &["allSettled"]),
            ("es2021", &["any"]),
            ("es2024", &["withResolvers"]),
            ("es2025", &["try"]),
        ]),
    );
    m.insert(
        "Symbol",
        feature_entries(&[
            ("es2015", &["for", "keyFor"]),
            ("es2019", &["description"]),
        ]),
    );
    m.insert(
        "WeakMap",
        feature_entries(&[
            ("es2015", &[]),
            ("es2026", &["getOrInsert", "getOrInsertComputed"]),
        ]),
    );
    m.insert("WeakSet", feature_entries(&[("es2015", &[])]));
    m.insert(
        "String",
        feature_entries(&[
            (
                "es2015",
                &[
                    "codePointAt",
                    "includes",
                    "endsWith",
                    "normalize",
                    "repeat",
                    "startsWith",
                    "anchor",
                    "big",
                    "blink",
                    "bold",
                    "fixed",
                    "fontcolor",
                    "fontsize",
                    "italics",
                    "link",
                    "small",
                    "strike",
                    "sub",
                    "sup",
                ],
            ),
            ("es2017", &["padStart", "padEnd"]),
            ("es2019", &["trimStart", "trimEnd", "trimLeft", "trimRight"]),
            ("es2020", &["matchAll"]),
            ("es2021", &["replaceAll"]),
            ("es2022", &["at"]),
            ("es2024", &["isWellFormed", "toWellFormed"]),
        ]),
    );
    m.insert(
        "StringConstructor",
        feature_entries(&[("es2015", &["fromCodePoint", "raw"])]),
    );
    m.insert(
        "DateTimeFormat",
        feature_entries(&[("es2017", &["formatToParts"])]),
    );
    m.insert("Promise", feature_entries(&[("es2015", &[]), ("es2018", &["finally"])]));
    m.insert("RegExpMatchArray", feature_entries(&[("es2018", &["groups"])]));
    m.insert("RegExpExecArray", feature_entries(&[("es2018", &["groups"])]));
    m.insert(
        "Intl",
        feature_entries(&[
            ("es2018", &["PluralRules"]),
            ("es2020", &["RelativeTimeFormat", "Locale", "DisplayNames"]),
            ("es2021", &["ListFormat", "DateTimeFormat"]),
            ("es2022", &["Segmenter"]),
            ("es2025", &["DurationFormat"]),
        ]),
    );
    m.insert(
        "NumberFormat",
        feature_entries(&[("es2018", &["formatToParts"])]),
    );
    m.insert(
        "SymbolConstructor",
        feature_entries(&[
            ("es2020", &["matchAll"]),
            ("esnext", &["metadata", "dispose", "asyncDispose"]),
        ]),
    );
    m.insert(
        "DataView",
        feature_entries(&[
            ("es2020", &["setBigInt64", "setBigUint64", "getBigInt64", "getBigUint64"]),
            ("es2025", &["setFloat16", "getFloat16"]),
        ]),
    );
    m.insert("BigInt", feature_entries(&[("es2020", &[])]));
    m.insert(
        "RelativeTimeFormat",
        feature_entries(&[("es2020", &["format", "formatToParts", "resolvedOptions"])]),
    );
    let typed_array_tail = &[
        "findLastIndex",
        "findLast",
        "toReversed",
        "toSorted",
        "toSpliced",
        "with",
    ];
    for name in [
        "Int8Array",
        "Uint8Array",
        "Uint8ClampedArray",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
        "Float32Array",
        "Float64Array",
        "BigInt64Array",
        "BigUint64Array",
    ] {
        m.insert(
            name,
            feature_entries(&[("es2022", &["at"]), ("es2023", typed_array_tail)]),
        );
    }
    m.insert(
        "Uint8Array",
        feature_entries(&[
            ("es2022", &["at"]),
            ("es2023", typed_array_tail),
            ("es2026", &["toBase64", "setFromBase64", "toHex", "setFromHex"]),
        ]),
    );
    m.insert(
        "BigInt64Array",
        feature_entries(&[
            ("es2020", &[]),
            ("es2022", &["at"]),
            ("es2023", typed_array_tail),
        ]),
    );
    m.insert(
        "BigUint64Array",
        feature_entries(&[
            ("es2020", &[]),
            ("es2022", &["at"]),
            ("es2023", typed_array_tail),
        ]),
    );
    m.insert("Float16Array", feature_entries(&[("es2025", &[])]));
    m.insert("Error", feature_entries(&[("es2022", &["cause"])]));
    m.insert("ErrorConstructor", feature_entries(&[("es2026", &["isError"])]));
    m.insert(
        "Uint8ArrayConstructor",
        feature_entries(&[("es2026", &["fromBase64", "fromHex"])]),
    );
    m.insert("DisposableStack", feature_entries(&[("esnext", &[])]));
    m.insert("AsyncDisposableStack", feature_entries(&[("esnext", &[])]));
    m.insert(
        "Date",
        feature_entries(&[("esnext", &["toTemporalInstant"])]),
    );
    m
}

/// Go: `func rangeOfTypeParameters(sourceFile *ast.SourceFile, typeParameters *ast.NodeList) core.TextRange`.
pub fn range_of_type_parameters(
    source_file: &tsc_ast::SourceFile,
    type_parameters: &NodeList,
) -> TextRange {
    let text = source_file.text();
    TextRange::new(
        type_parameters.pos().saturating_sub(1),
        (text.len() as i32).min(tsc_scanner::skip_trivia(text, type_parameters.end()) + 1),
    )
}

/// Go: `func tryGetPropertyAccessOrIdentifierToString(expr *ast.Node) string`.
pub fn try_get_property_access_or_identifier_to_string(
    store: &dyn NodeStore,
    expr: NodeId,
) -> String {
    let n = store.node(expr);
    if is_property_access_expression(n) {
        let d = n.as_property_access_expression().expect("PropertyAccessExpression data");
        let base_str = try_get_property_access_or_identifier_to_string(store, d.expression);
        if !base_str.is_empty() {
            return format!("{base_str}.{}", entity_name_to_string(store, d.name));
        }
    } else if is_element_access_expression(n) {
        let d = n
            .as_element_access_expression()
            .expect("ElementAccessExpression data");
        let base_str = try_get_property_access_or_identifier_to_string(store, d.expression);
        if !base_str.is_empty() && is_property_name(store, d.argument_expression) {
            return format!(
                "{base_str}.{}",
                get_property_name_for_property_name_node(store, d.argument_expression)
            );
        }
    } else if is_identifier(n) {
        return tsc_ast::node_text(store, expr).to_string();
    } else if n.kind == Kind::JsxNamespacedName {
        return entity_name_to_string(store, expr);
    }
    String::new()
}

/// Go: `ast.IsPropertyName` — pub(crate) (M6 S10: index.rs's
/// getPropertyNameFromIndex reads it).
pub(crate) fn is_property_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    matches!(
        n.kind,
        Kind::Identifier
            | Kind::PrivateIdentifier
            | Kind::StringLiteral
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::ComputedPropertyName
            | Kind::JsxNamespacedName
    )
    && {
        let _ = store;
        true
    }
}

/// Go: `func allDeclarationsInSameSourceFile(symbol *ast.Symbol) bool`.
pub fn all_declarations_in_same_source_file(
    c: &Checker,
    symbol: SymbolId,
) -> bool {
    let declarations = &c.program.symbol(symbol).declarations;
    if declarations.len() > 1 {
        let store = c.program.node_store();
        let first = tsc_ast::utilities::get_source_file_of_node(store, declarations[0]);
        declarations[1..].iter().all(|d| {
            tsc_ast::utilities::get_source_file_of_node(store, *d).is_some()
                && tsc_ast::utilities::get_source_file_of_node(store, *d).map(|f| f.as_node_id())
                    == first.map(|f| f.as_node_id())
        })
    } else {
        true
    }
}

/// Go: `func containsNonMissingUndefinedType(c *Checker, t *Type) bool`.
pub fn contains_non_missing_undefined_type(c: &Checker, t: TypeId) -> bool {
    let tr = c.type_(t);
    let candidate = if tr.flags.intersects(TypeFlags::UNION) {
        tr.as_union_type()
            .and_then(|d| d.base.types.first().copied())
            .map(|id| c.type_(id))
    } else {
        Some(tr)
    };
    match candidate {
        Some(candidate) => {
            candidate.flags.intersects(TypeFlags::UNDEFINED) && candidate.id != c.missing_type_id()
        }
        None => false,
    }
}

/// Go: `func getAnyImportSyntax(node *ast.Node) *ast.Node` — walks parents to
/// the enclosing import clause of any shape.
pub fn get_any_import_syntax(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    let import_node = match n.kind {
        Kind::ImportEqualsDeclaration => node,
        Kind::ImportClause => n.parent.get(),
        Kind::NamespaceImport => store.node(n.parent.get()).parent.get(),
        Kind::ImportSpecifier => {
            let parent = n.parent.get();
            store.node(store.node(parent).parent.get()).parent.get()
        }
        _ => return None,
    };
    if import_node.is_none() {
        None
    } else {
        Some(import_node)
    }
}

/// Go: `func isReservedMemberName(name string) bool` — a reserved member name
/// consists of the byte 0xFE followed by one or more characters where the
/// first character is not '@' or '#'. PORT: same '\u{FE}'-code-point
/// convention as [`is_late_bound_name`].
pub fn is_reserved_member_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next() == Some('\u{FE}')
        && !matches!(chars.next(), None | Some('@') | Some('#'))
}

/// Go: `func introducesArgumentsExoticObject(node *ast.Node) bool`.
pub fn introduces_arguments_exotic_object(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
    )
}

/// Go: `func symbolsToArray(symbols ast.SymbolTable) []*ast.Symbol`.
pub fn symbols_to_array(symbols: &SymbolTable) -> Vec<SymbolId> {
    let mut result = Vec::new();
    for (id, symbol) in symbols.entries() {
        if !is_reserved_member_name(id) {
            result.push(*symbol);
        }
    }
    result
}

/// Go: `func SkipAlias(symbol *ast.Symbol, checker *Checker) *ast.Symbol`.
pub fn skip_alias(symbol: SymbolId, c: &Checker) -> SymbolId {
    if c.program.symbol(symbol).flags.intersects(SymbolFlags::ALIAS) {
        c.get_aliased_symbol(symbol)
    } else {
        symbol
    }
}

/// Go: `func IsExternalModuleSymbol(moduleSymbol *ast.Symbol) bool` —
/// `moduleSymbol.IsExternalModule()`:
/// `s.Flags&SymbolFlagsModule != 0 && IsAmbientModuleSymbolName(s.Name)`.
pub fn is_external_module_symbol(c: &Checker, module_symbol: SymbolId) -> bool {
    let symbol = c.program.symbol(module_symbol);
    symbol.flags.intersects(SymbolFlags::MODULE)
        && tsc_ast::utilities::is_ambient_module_symbol_name(&symbol.name)
}

/// Go: `func (c *Checker) isCanceled() bool`.
///
/// PORT: Go checks `c.ctx.Err()`; the cancellation-context plumbing lands with
/// the checker-pool wave (SPEC §5.5). The spike has no cancellation source.
pub fn is_canceled(_c: &Checker) -> bool {
    false
}

/// Go: `func (c *Checker) checkNotCanceled()`.
pub fn check_not_canceled(c: &Checker) {
    if c.was_canceled {
        panic!("Checker was previously cancelled");
    }
}

/// Go: `func (c *Checker) getPackagesMap() map[string]bool`.
///
/// PORT: needs `program.GetResolvedModules()` + the module crate's
/// package-name helpers — the module seam is the M5 compiler crate's surface.
pub fn get_packages_map(_c: &Checker) -> FxHashMap<String, bool> {
    panic!("TODO(port M5/M6): program.GetResolvedModules seam (packagesMap)")
}

/// Go: `func (c *Checker) typesPackageExists(packageName string) bool`.
pub fn types_package_exists(c: &Checker, package_name: &str) -> bool {
    let packages_map = get_packages_map(c);
    packages_map.contains_key(&types_package_name(package_name))
}

/// Go: `module.GetTypesPackageName` — "foo" → "@types/foo".
fn types_package_name(package_name: &str) -> String {
    format!("@types/{package_name}")
}

/// Go: `func (c *Checker) packageBundlesTypes(packageName string) bool`.
pub fn package_bundles_types(c: &Checker, package_name: &str) -> bool {
    let packages_map = get_packages_map(c);
    packages_map.get(package_name).copied().unwrap_or(false)
}

/// Go: `func ValueToString(value any) string`.
pub fn value_to_string(value: &LiteralValue) -> String {
    match value {
        LiteralValue::String(s) => format!("\"{}\"", escape_string(s)),
        LiteralValue::Number(n) => n.string(),
        LiteralValue::Boolean(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        LiteralValue::BigInt(b) => format!("{}n", b.string()),
        LiteralValue::None => panic!("unhandled value type in valueToString"),
    }
}

/// Go: `printer.EscapeString(value, '"')`.
///
/// PORT (banner): the full escapeStringWorker (UTF-16 escape sequences for
/// control/surrogate handling, the jsx/ascii variants) is printer-package
/// machinery that lands with the printer crate (M7). The spike escapes the
/// double quote and backslash, which is exact for all strings without control
/// characters — ValueToString's only quote char is `"`.
pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out
}

/// Go: `func nodeStartsNewLexicalEnvironment(node *ast.Node) bool`.
pub fn node_starts_new_lexical_environment(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::Constructor
            | Kind::FunctionExpression
            | Kind::FunctionDeclaration
            | Kind::ArrowFunction
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::ModuleDeclaration
            | Kind::SourceFile
    )
}

/// Go: `func (c *Checker) isUncheckedJSSuggestion(node, suggestion, excludeClasses) bool`.
///
/// PORT (banner): the full body needs CheckJs directives, heritage-clause
/// walking and decoration checks (ast/utilities + JSDoc waves). The
/// compilerOptions.CheckJs file-level gate is the spike's reachable surface;
/// everything past it is M6.
pub fn is_unchecked_js_suggestion(c: &Checker, _node: NodeId, _suggestion: Option<SymbolId>, _exclude_classes: bool) -> bool {
    if c.program.options().check_js.is_unknown() {
        return false;
    }
    false
}

/// Go: `func (c *Checker) isJSLiteralType(t *Type) bool` (utilities.go:1807).
pub fn is_js_literal_type(c: &mut Checker, t: TypeId) -> bool {
    if c.no_implicit_any {
        return false;
        // Flag is meaningless under `noImplicitAny` mode
    }
    let tr = c.type_(t);
    if tr.object_flags.intersects(ObjectFlags::JS_LITERAL) {
        return true;
    }
    if tr.flags.intersects(TypeFlags::UNION) {
        let constituents = tr.types().to_vec();
        return constituents.iter().all(|&u| is_js_literal_type(c, u));
    }
    if tr.flags.intersects(TypeFlags::INTERSECTION) {
        let constituents = tr.types().to_vec();
        return constituents.iter().any(|&u| is_js_literal_type(c, u));
    }
    if tr.flags.intersects(TypeFlags::INSTANTIABLE) {
        // Go: `constraint := c.getResolvedBaseConstraint(t, nil); return
        // constraint != t && c.isJSLiteralType(constraint)` — REAL as of
        // W3T6 (was a panic TODO; the class-thisType now flows here via
        // tryGetThisTypeAtEx's class-like-parent arm).
        let constraint = c.get_resolved_base_constraint(t, Vec::new());
        return match constraint {
            Some(constraint) => constraint != t && is_js_literal_type(c, constraint),
            None => false,
        };
    }
    false
}

/// Go: `type DiagnosticDetails struct`.
#[derive(Clone, Debug, Default)]
pub struct DiagnosticDetails {
    pub message: Option<&'static Message>,
    pub args: Vec<String>,
}

/// Go: `func CreateModuleNotFoundChain(program Program, file, moduleReference, mode, packageName) DiagnosticDetails`.
///
/// PORT (banner): needs the resolved-module seam (`program.GetResolvedModule`)
/// and the module crate's package-name helpers; the module-not-found chain is
/// M6 territory (module resolution + the diagnostics wave).
pub fn create_module_not_found_chain(
    _program: &dyn crate::checker::Program,
    _file: NodeId,
    _module_reference: &str,
    _mode: tsc_core::compileroptions::ResolutionMode,
    _package_name: &str,
) -> DiagnosticDetails {
    panic!("TODO(port M6): module-not-found chain (resolved-module seam)")
}

/// Go: `func CreateModeMismatchDetails(program Program, file) DiagnosticDetails`.
///
/// PORT (banner): needs `program.GetSourceFileMetaData`; M6 (compiler crate).
pub fn create_mode_mismatch_details(
    _program: &dyn crate::checker::Program,
    _file: NodeId,
) -> DiagnosticDetails {
    panic!("TODO(port M6): mode-mismatch details (source-file metadata seam)")
}

/// Go: `func walkUpOuterExpressions(node *ast.Node) *ast.Node`.
pub fn walk_up_outer_expressions(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    ast_shims::walk_up_outer_expressions(store, node)
}

/// Go: `func GetSetAccessorValueParameter(accessor *ast.Node) *ast.Node`.
pub fn get_set_accessor_value_parameter(store: &dyn NodeStore, accessor: NodeId) -> Option<NodeId> {
    let parameters = tsc_binder::node_data::parameters_of(store, accessor)?;
    if !parameters.nodes.is_empty() {
        let has_this = parameters.nodes.len() == 2
            && ast_shims::is_this_parameter(store, parameters.nodes[0]);
        return Some(parameters.nodes[if has_this { 1 } else { 0 }]);
    }
    None
}

/// Go: `func quotedAndCommaSeparated(items []string) string`.
pub fn quoted_and_comma_separated(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("'{item}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Go: `func findInMap[K comparable, V any](m map[K]V, predicate func(V) bool) V`.
/// PORT: HashMap iteration order is nondeterministic in Rust *and* Go — Go
/// callers rely on arbitrary-pick semantics; the port keeps them (scratch
/// lookups only, never output order).
pub fn find_in_map<K: std::hash::Hash + Eq + Clone, V: Default + Clone>(
    m: &FxHashMap<K, V>,
    predicate: impl Fn(&V) -> bool,
) -> V {
    for value in m.values() {
        if predicate(value) {
            return value.clone();
        }
    }
    V::default()
}

impl Checker {
    /// Go: `func (c *Checker) isCanceled() bool` — `c.ctx != nil &&
    /// c.ctx.Err() != nil` (utilities.go:1712).
    ///
    /// PORT: the port has no context plumbing (the runner's check stage is
    /// single-threaded); Go's ctx-derived cancellation becomes a host-set
    /// flag on the checker (`Checker::canceled`, S1's spine.rs PORT note).
    pub(crate) fn is_canceled(&self) -> bool {
        self.canceled
    }

    /// Go: `func (c *Checker) checkNotCanceled()` (utilities.go:1716).
    pub(crate) fn check_not_canceled(&self) {
        if self.was_canceled {
            panic!("Checker was previously cancelled");
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Declaration-kind predicates (Go ast/utilities.go) — thin routes onto the
// binder/ast shims plus the two local kind-matches that had no Rust home yet.
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsPartOfParameterDeclaration(node *Node) bool`.
pub fn is_part_of_parameter_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    tsc_binder::ast_shims::is_part_of_parameter_declaration(store, node)
}

/// Go: `func IsParameterDeclaration(node *Node) bool`.
pub fn is_parameter_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    tsc_ast::ast_generated::is_parameter_declaration(store.node(node))
}

/// Go: `func IsVariableLike(node *Node) bool` (ast/utilities.go:3108).
pub fn is_variable_like_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::BindingElement
            | Kind::EnumMember
            | Kind::Parameter
            | Kind::PropertyAssignment
            | Kind::PropertyDeclaration
            | Kind::PropertySignature
            | Kind::ShorthandPropertyAssignment
            | Kind::VariableDeclaration
    )
}

/// Go: `func IsTypeDeclaration(node *Node) bool` — routed to spine's port.
pub fn is_type_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    crate::spine::is_type_declaration(store, node)
}

/// Go: `func IsParameterPropertyDeclaration(node, parent) bool`.
pub fn is_parameter_property_declaration(
    store: &dyn NodeStore,
    node: NodeId,
    parent: NodeId,
) -> bool {
    tsc_binder::ast_shims::is_parameter_property_declaration(store, node, parent)
}

/// Go: `func IsThisParameter(node *Node) bool`.
pub fn is_this_parameter(store: &dyn NodeStore, node: NodeId) -> bool {
    crate::ast_shims::is_this_parameter(store, node)
}

/// Go: `func IsEntityNameExpression(node *Node) bool`.
pub fn is_entity_name_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    tsc_binder::ast_shims::is_entity_name_expression(store, node)
}

/// Go: `func IsVariableDeclarationInitializedToRequire(node *Node) bool`.
pub fn is_variable_declaration_initialized_to_require(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    tsc_binder::ast_shims::is_variable_declaration_initialized_to_require(store, node)
}

/// Go: `func GetContainingFunction(node *Node) *Node` — nearest function-like
/// ancestor (function-like kinds mirror statements::is_function_like).
pub fn get_containing_function(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut current = store.node(node).parent.get();
    while current != NodeId::NONE {
        if crate::statements::is_function_like(store, current) {
            return Some(current);
        }
        current = store.node(current).parent.get();
    }
    None
}

// ────────────────────────────────────────────────────────────────────────────
// core.go: levenshteinWithMax (checker-local copy)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func levenshteinWithMax(buffers, s1, s2 []rune, maxValue float64) float64`
/// (core.go:560) — the case-weighted Levenshtein distance of `getSpellingSuggestion`,
/// or -1 when the distance exceeds `max_value`.
///
/// PORT: `tsc_scanner::go_shims` has a private twin for the scanner's own
/// suggestion sites (regexp.rs); the scanner crate is outside this tranche's
/// writable scope, so the checker keeps a copy for the symbol-level
/// `getSpellingSuggestionForName` (resolution.rs). Go pools the two `[]float64`
/// buffers; the port allocates them per call (spell-suggestion misses only).
pub(crate) fn levenshtein_with_max(s1: &[char], s2: &[char], max_value: f64) -> f64 {
    let buffer_size = s2.len() + 1;
    let mut previous = vec![0.0; buffer_size];
    let mut current = vec![0.0; buffer_size];

    let big = max_value + 0.01;
    for (i, slot) in previous.iter_mut().enumerate() {
        *slot = i as f64;
    }
    for i in 1..=s1.len() {
        let c1 = s1[i - 1];
        let i_f = i as f64;
        let min_j = std::cmp::max((i_f - max_value).ceil() as usize, 1);
        let max_j = std::cmp::min((max_value + i_f).floor() as usize, s2.len());
        let mut col_min = i_f;
        current[0] = col_min;
        for slot in current.iter_mut().take(min_j).skip(1) {
            *slot = big;
        }
        for j in min_j..=max_j {
            // Go: `unicode.ToLower` — simple 1:1 case folding (ASCII here:
            // identifier spell-suggestions are ECMA-262 names).
            let substitution_distance = if s1[i - 1].to_lowercase().eq(s2[j - 1].to_lowercase()) {
                previous[j - 1] + 0.1
            } else {
                previous[j - 1] + 2.0
            };
            let dist = if c1 == s2[j - 1] {
                previous[j - 1]
            } else {
                (previous[j] + 1.0).min((current[j - 1] + 1.0).min(substitution_distance))
            };
            current[j] = dist;
            col_min = col_min.min(dist);
        }
        for slot in current.iter_mut().take(s2.len() + 1).skip(max_j + 1) {
            *slot = big;
        }
        if col_min > max_value {
            // Give up -- everything in this column is > max and it can't get better in future columns.
            return -1.0;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let result = previous[s2.len()];
    if result > max_value {
        -1.0
    } else {
        result
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (ported where Go has direct coverage; Go's utilities_test.go is
// the printer/emitter harness, so the pins here are Go-comment-verified).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_predicates_go_parity() {
        assert!(is_exponentiation_operator(Kind::AsteriskAsteriskToken));
        assert!(is_multiplicative_operator(Kind::AsteriskToken));
        assert!(is_multiplicative_operator_or_higher(Kind::PercentToken));
        assert!(is_additive_operator(Kind::PlusToken));
        assert!(is_additive_operator_or_higher(Kind::SlashToken));
        assert!(is_shift_operator(Kind::LessThanLessThanToken));
        assert!(is_shift_operator_or_higher(Kind::MinusToken));
        assert!(is_relational_operator(Kind::InKeyword));
        assert!(is_relational_operator_or_higher(Kind::InstanceOfKeyword));
        assert!(is_equality_operator(Kind::EqualsEqualsToken));
        assert!(is_equality_operator_or_higher(Kind::GreaterThanGreaterThanGreaterThanToken));
        assert!(is_bitwise_operator(Kind::BarToken));
        assert!(is_bitwise_operator_or_higher(Kind::ExclamationEqualsToken));
        assert!(is_logical_operator_or_higher(Kind::AmpersandToken));
        assert!(is_assignment_operator_or_higher(Kind::QuestionQuestionToken));
        assert!(is_assignment_operator_or_higher(Kind::EqualsToken));
        assert!(is_binary_operator(Kind::CommaToken));
        // token predicates
        assert!(token_is_identifier_or_keyword(Kind::Identifier));
        assert!(token_is_identifier_or_keyword_or_greater_than(Kind::GreaterThanToken));
    }

    #[test]
    fn numeric_literal_names() {
        // Go comment: '0xF00D' is NOT a valid numeric name ('ToString' form
        // differs); plain and negative decimal forms round-trip, and
        // 'Infinity'/'NaN' are intentionally accepted.
        assert!(is_numeric_literal_name("0"));
        assert!(is_numeric_literal_name("1"));
        assert!(is_numeric_literal_name("-1"));
        assert!(!is_numeric_literal_name("0xF00D"));
        assert!(is_numeric_literal_name("Infinity"));
        assert!(is_numeric_literal_name("-Infinity"));
        assert!(is_numeric_literal_name("NaN"));
        assert!(!is_numeric_literal_name("x"));
    }

    #[test]
    fn reserved_and_late_bound_names() {
        // Go: 0xFE prefix, '@' → late-bound symbol, '#' → private identifier,
        // anything else → reserved.
        assert!(is_reserved_member_name("\u{FE}iterator"));
        assert!(!is_reserved_member_name("\u{FE}@iterator"));
        assert!(!is_reserved_member_name("\u{FE}#priv"));
        assert!(is_late_bound_name("\u{FE}@Symbol.iterator"));
        assert!(!is_late_bound_name("\u{FE}iterator"));
        assert!(is_infinity_or_nan_string("NaN"));
        assert!(!is_infinity_or_nan_string("1"));
    }

    #[test]
    fn valid_number_and_bigint_strings() {
        assert!(is_valid_number_string("123", false));
        assert!(is_valid_number_string("123", true));
        assert!(!is_valid_number_string("0xF00D", true));
        assert!(is_valid_number_string("0xF00D", false));
        assert!(is_valid_big_int_string("123", false));
        assert!(is_valid_big_int_string("123", true));
        assert!(!is_valid_big_int_string("1_000", false)); // separators rejected
        assert!(!is_valid_big_int_string("1.0", false));
    }

    #[test]
    fn value_to_string_go_parity() {
        assert_eq!(value_to_string(&LiteralValue::String("hi".into())), "\"hi\"");
        assert_eq!(value_to_string(&LiteralValue::Boolean(true)), "true");
        assert_eq!(value_to_string(&LiteralValue::Boolean(false)), "false");
        assert_eq!(value_to_string(&LiteralValue::Number(Number(1.5))), "1.5");
        // BigInt renders with the trailing `n`.
        let bi = jsnum::new_pseudo_big_int("9007199254740993", false);
        assert_eq!(value_to_string(&LiteralValue::BigInt(bi)), "9007199254740993n");
    }

    #[test]
    fn quoted_and_comma_separated_go_parity() {
        assert_eq!(quoted_and_comma_separated(&[]), "");
        assert_eq!(
            quoted_and_comma_separated(&["a".to_string(), "b".to_string()]),
            "'a', 'b'"
        );
    }

    #[test]
    fn ordered_set_threshold_semantics() {
        let mut s: OrderedSet<u32> = OrderedSet::default();
        for i in 0..ORDERED_SET_MAP_THRESHOLD {
            s.add(i as u32);
            assert!(s.contains(&(i as u32)));
            assert!(!s.values_by_key.is_some(), "linear scan below threshold");
        }
        s.add(ORDERED_SET_MAP_THRESHOLD as u32);
        assert!(s.values_by_key.is_some(), "map materialized at threshold+1");
        assert!(s.contains(&3));
        assert!(!s.contains(&999));
        assert_eq!(s.values().len(), ORDERED_SET_MAP_THRESHOLD + 1);
    }

    #[test]
    fn feature_map_entries_match_go_table() {
        let map = get_feature_map();
        assert_eq!(map["Array"][0].lib, "es2015");
        assert_eq!(map["Array"][0].props[0], "find");
        assert_eq!(map["Array"][4].lib, "es2023");
        assert_eq!(map["Math"][0].props.len(), 17);
        assert_eq!(map["String"][0].props.last().copied(), Some("sup"));
        assert_eq!(map.len(), 57);
    }

    #[test]
    fn min_and_max_go_parity() {
        let (min, max) = min_and_max(&[3, 1, 2], |v| v * 2);
        assert_eq!((min, max), (2, 6));
        let (min, max) = min_and_max(&[7], |v| v + 1);
        assert_eq!((min, max), (8, 8));
    }
}
