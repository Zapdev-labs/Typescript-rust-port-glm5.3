// The tsc/internal/ast/utilities.go subset the binder consumes (M4 wave A).
//
// tsc-ast carries only a "minimal subset" of utilities.go (its header notes
// the wholesale port is a separate task); every helper binder.go /
// nameresolver.go calls that is not yet ported lives HERE as a faithful shim
// with its Go symbol documented, so the binder crate stays within its
// wave-A edit scope (rust/crates/binder/** only).
//
// PORT: dedup candidate — fold this module into the ast/utilities.go port
// when that task lands; the Go bodies are transcribed verbatim, so the fold
// is a file move, not a rewrite.

use std::sync::atomic::{AtomicU64, Ordering};

use tsc_ast::{
    get_assignment_declaration_kind, is_arrow_function, is_assignment_operator,
    is_binary_expression, is_binding_element,
    is_call_expression, is_class_declaration, is_class_expression, is_class_static_block_declaration,
    is_constructor_declaration, is_element_access_expression, is_enum_declaration,
    is_function_declaration, is_function_expression, is_identifier, is_module_declaration,
    is_non_null_expression, is_numeric_literal, is_object_literal_expression,
    is_parameter_declaration, is_parenthesized_expression, is_prefix_unary_expression,
    is_property_access_expression, is_property_declaration, is_source_file, is_type_reference_node,
    is_variable_statement, JSDeclarationKind, Kind, ModifierFlags, Node, NodeData, NodeFlags,
    NodeId, NodeStore, SourceFile, Symbol, SymbolId, INTERNAL_SYMBOL_NAME_DEFAULT,
};
use tsc_scanner::go_shims::is_type_node_kind;

use crate::node_data;

// ────────────────────────────────────────────────────────────────────────────
// Atomic ids (Go utilities.go:17)
// ────────────────────────────────────────────────────────────────────────────

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_SYMBOL_ID: AtomicU64 = AtomicU64::new(0);

/// Go: `func GetNodeId(node *Node) NodeId` — the lazily-assigned *link-store*
/// id (NOT the port's packed arena handle; ids.rs repurposed that name). The
/// port returns the raw `u64` backing `Node.id: Cell<u64>`.
pub fn get_node_id(node: &Node) -> u64 {
    let mut id = node.id.get();
    if id == 0 {
        // Worst case, we burn a few ids if we have to CAS.
        id = NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed) + 1;
        // Cell has no CAS; a racing overwrite keeps the winner's value, same
        // observable result as Go's CompareAndSwap loser branch.
        node.id.set(id);
        id = node.id.get();
    }
    id
}

/// Go: `func GetSymbolId(symbol *Symbol) SymbolId` — same lazy-id contract as
/// [`get_node_id`] (the raw `u64`, not the arena `SymbolId` handle).
pub fn get_symbol_id(symbol: &Symbol) -> u64 {
    let mut id = symbol.id.get();
    if id == 0 {
        id = NEXT_SYMBOL_ID.fetch_add(1, Ordering::Relaxed) + 1;
        symbol.id.set(id);
        id = symbol.id.get();
    }
    id
}

// ────────────────────────────────────────────────────────────────────────────
// Small node helpers (Go *Node field access through the store)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `node.Parent` (nil → `None`).
#[inline]
pub fn parent_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let parent = store.node(node).parent.get();
    (!parent.is_none()).then_some(parent)
}

/// Go: `node.Text()` for the text-bearing kinds the binder reads.
#[inline]
pub fn node_text(store: &dyn NodeStore, node: NodeId) -> &str {
    tsc_ast::node_text(store, node)
}

/// Go: the `KindJsxNamespacedName` arm of `(*Node).Text()` (ast.go) —
/// `namespace.Text() + ":" + name.Text()`; every other kind falls through
/// to the ast crate's `node_text` (the ported Text() arms).
///
/// PORT: the ast crate's `node_text` does not yet carry the JSX-namespaced
/// arm (its `Unhandled case in Node.Text: KindJsxNamespacedName` panic arm
/// is hit through getDeclarationName's `IsPropertyNameLiteral ||
/// IsJsxNamespacedName` branch); this shim mirrors the Go arm for the
/// binder's call site (dedup candidate — fold into the ast crate's
/// node_text when the utilities.go wave lands).
pub fn jsx_namespaced_name_text(store: &dyn NodeStore, node: NodeId) -> String {
    let (namespace, name) = match &store.node(node).data {
        NodeData::JsxNamespacedName(d) => (d.namespace, d.name),
        _ => {
            return tsc_ast::node_text(store, node).to_string();
        }
    };
    let namespace_text = tsc_ast::node_text(store, namespace);
    let name_text = tsc_ast::node_text(store, name);
    format!("{namespace_text}:{name_text}")
}

// ────────────────────────────────────────────────────────────────────────────
// Assignment expressions (utilities.go:112-227)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsAssignmentExpression(node *Node, excludeCompoundAssignment bool) bool`.
pub fn is_assignment_expression(
    store: &dyn NodeStore,
    node: NodeId,
    exclude_compound_assignment: bool,
) -> bool {
    let n = store.node(node);
    if is_binary_expression(n) {
        let expr = n.as_binary_expression().expect("BinaryExpression data");
        let operator = store.node(expr.operator_token).kind;
        return (operator == Kind::EqualsToken
            || !exclude_compound_assignment && is_assignment_operator(operator))
            && is_left_hand_side_expression(store, expr.left);
    }
    false
}

/// Go: `func IsDestructuringAssignment(node *Node) bool`.
pub fn is_destructuring_assignment(store: &dyn NodeStore, node: NodeId) -> bool {
    if is_assignment_expression(store, node, true /*excludeCompoundAssignment*/) {
        let left = store
            .node(node)
            .as_binary_expression()
            .expect("BinaryExpression data")
            .left;
        let kind = store.node(left).kind;
        return kind == Kind::ObjectLiteralExpression || kind == Kind::ArrayLiteralExpression;
    }
    false
}

/// Go: `func IsBindingPattern(node *Node) bool`.
pub fn is_binding_pattern(node: &Node) -> bool {
    node.kind == Kind::ObjectBindingPattern || node.kind == Kind::ArrayBindingPattern
}

/// Go: `func IsForInOrOfStatement(node *Node) bool`.
pub fn is_for_in_or_of_statement(node: &Node) -> bool {
    node.kind == Kind::ForInStatement || node.kind == Kind::ForOfStatement
}

// A node is an assignment target if it is on the left hand side of an '=' token, if it is parented by a property
// assignment in an object literal that is an assignment target, or if it is parented by an array literal that is
// an assignment target. Examples include 'a = xxx', '{ p: a } = xxx', '[{ a }] = xxx'.
// (Note that `p` is not a target in the above examples, only `a`.)
/// Go: `func IsAssignmentTarget(node *Node) bool`.
pub fn is_assignment_target(store: &dyn NodeStore, node: NodeId) -> bool {
    get_assignment_target(store, node).is_some()
}

/// Go: `func GetAssignmentTarget(node *Node) *Node` — returns the
/// BinaryExpression, PrefixUnaryExpression, PostfixUnaryExpression, or
/// ForInOrOfStatement that references the given node as an assignment target.
pub fn get_assignment_target(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut node = node;
    loop {
        let parent = parent_of(store, node)?;
        match store.node(parent).kind {
            Kind::BinaryExpression => {
                let expr = store
                    .node(parent)
                    .as_binary_expression()
                    .expect("BinaryExpression data");
                let operator = store.node(expr.operator_token).kind;
                return if is_assignment_operator(operator) && expr.left == node {
                    Some(parent)
                } else {
                    None
                };
            }
            Kind::PrefixUnaryExpression => {
                let expr = store
                    .node(parent)
                    .as_prefix_unary_expression()
                    .expect("PrefixUnaryExpression data");
                return if expr.operator == Kind::PlusPlusToken || expr.operator == Kind::MinusMinusToken {
                    Some(parent)
                } else {
                    None
                };
            }
            Kind::PostfixUnaryExpression => {
                let expr = store
                    .node(parent)
                    .as_postfix_unary_expression()
                    .expect("PostfixUnaryExpression data");
                return if expr.operator == Kind::PlusPlusToken || expr.operator == Kind::MinusMinusToken {
                    Some(parent)
                } else {
                    None
                };
            }
            Kind::ForInStatement | Kind::ForOfStatement => {
                return if node_data::expression_of(store, parent).is_none()
                    && initializer_of_for(store, parent) == Some(node)
                {
                    Some(parent)
                } else {
                    None
                };
            }
            Kind::ParenthesizedExpression
            | Kind::ArrayLiteralExpression
            | Kind::SpreadElement
            | Kind::NonNullExpression => {
                node = parent;
            }
            Kind::SpreadAssignment => {
                node = parent_of(store, parent)?;
            }
            Kind::ShorthandPropertyAssignment => {
                if store.node(parent).name() != Some(node) {
                    return None;
                }
                node = parent_of(store, parent)?;
            }
            Kind::PropertyAssignment => {
                if store.node(parent).name() == Some(node) {
                    return None;
                }
                node = parent_of(store, parent)?;
            }
            _ => return None,
        }
    }
}

/// Go: `parent.Initializer() == node` on ForIn/ForOf — the initializer field
/// of ForInOrOfStatement (non-optional there, unlike ForStatement).
fn initializer_of_for(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::ForInOrOfStatement(d) => Some(d.initializer),
        NodeData::ForStatement(d) => d.initializer,
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Logical operators / property names (utilities.go:228-345)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsLogicalBinaryOperator(token Kind) bool`.
pub fn is_logical_binary_operator(token: Kind) -> bool {
    token == Kind::BarBarToken || token == Kind::AmpersandAmpersandToken
}

/// Go: `func IsLogicalOrCoalescingBinaryOperator(token Kind) bool`.
pub fn is_logical_or_coalescing_binary_operator(token: Kind) -> bool {
    is_logical_binary_operator(token) || token == Kind::QuestionQuestionToken
}

/// Go: `func IsLogicalOrCoalescingBinaryExpression(expr *Node) bool`.
pub fn is_logical_or_coalescing_binary_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    is_binary_expression(n)
        && is_logical_or_coalescing_binary_operator(
            store
                .node(store
                    .node(node)
                    .as_binary_expression()
                    .expect("BinaryExpression data")
                    .operator_token)
                .kind,
        )
}

/// Go: `func IsLogicalOrCoalescingAssignmentExpression(expr *Node) bool`.
pub fn is_logical_or_coalescing_assignment_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    is_binary_expression(n)
        && tsc_ast::is_logical_or_coalescing_assignment_operator(
            store
                .node(store
                    .node(node)
                    .as_binary_expression()
                    .expect("BinaryExpression data")
                    .operator_token)
                .kind,
        )
}

/// Go: `func IsLogicalExpression(node *Node) bool`.
pub fn is_logical_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut node = node;
    loop {
        let n = store.node(node);
        if is_parenthesized_expression(n) {
            node = node_data::expression_of(store, node).expect("ParenthesizedExpression expression");
        } else if is_prefix_unary_expression(n)
            && n.as_prefix_unary_expression().expect("PrefixUnaryExpression data").operator
                == Kind::ExclamationToken
        {
            node = n.as_prefix_unary_expression().expect("PrefixUnaryExpression data").operand;
        } else {
            return is_logical_or_coalescing_binary_expression(store, node);
        }
    }
}

/// Go: `func IsPropertyNameLiteral(node *Node) bool`.
pub fn is_property_name_literal(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::Identifier | Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral | Kind::NumericLiteral
    )
}

/// Go: `func IsMemberName(node *Node) bool`.
pub fn is_member_name(node: &Node) -> bool {
    node.kind == Kind::Identifier || node.kind == Kind::PrivateIdentifier
}

/// Go: `func IsEntityName(node *Node) bool`.
pub fn is_entity_name(node: &Node) -> bool {
    node.kind == Kind::Identifier || node.kind == Kind::QualifiedName
}

/// Go: `func IsPropertyName(node *Node) bool`.
pub fn is_property_name(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::Identifier
            | Kind::PrivateIdentifier
            | Kind::StringLiteral
            | Kind::NumericLiteral
            | Kind::ComputedPropertyName
    )
}

/// Go: `func IsIdentifierName(node *Node) bool` — true if the given identifier
/// is classified as an IdentifierName by inspecting the parent of the node.
pub fn is_identifier_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let Some(parent) = parent_of(store, node) else {
        return false;
    };
    match store.node(parent).kind {
        Kind::PropertyDeclaration
        | Kind::PropertySignature
        | Kind::MethodDeclaration
        | Kind::MethodSignature
        | Kind::GetAccessor
        | Kind::SetAccessor
        | Kind::EnumMember
        | Kind::PropertyAssignment
        | Kind::PropertyAccessExpression => store.node(parent).name() == Some(node),
        Kind::QualifiedName => {
            store
                .node(parent)
                .as_qualified_name()
                .expect("QualifiedName data")
                .right
                == node
        }
        Kind::BindingElement => node_data::property_name_of(store, parent) == Some(node),
        Kind::ImportSpecifier => node_data::property_name_of(store, parent) == Some(node),
        Kind::ExportSpecifier
        | Kind::JsxAttribute
        | Kind::JsxSelfClosingElement
        | Kind::JsxOpeningElement
        | Kind::JsxClosingElement => true,
        _ => false,
    }
}

/// Go: `func IsPushOrUnshiftIdentifier(node *Node) bool`.
pub fn is_push_or_unshift_identifier(store: &dyn NodeStore, node: NodeId) -> bool {
    let text = node_text(store, node);
    text == "push" || text == "unshift"
}

/// Go: `func IsBooleanLiteral(node *Node) bool`.
pub fn is_boolean_literal(node: &Node) -> bool {
    node.kind == Kind::TrueKeyword || node.kind == Kind::FalseKeyword
}

/// Go: `func IsLiteralExpression(node *Node) bool`.
pub fn is_literal_expression(node: &Node) -> bool {
    tsc_ast::is_literal_kind(node.kind)
}

/// Go: `func IsStringLiteralLike(node *Node) bool`.
pub fn is_string_literal_like(node: &Node) -> bool {
    matches!(node.kind, Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral)
}

/// Go: `func IsStringOrNumericLiteralLike(node *Node) bool`.
pub fn is_string_or_numeric_literal_like(node: &Node) -> bool {
    is_string_literal_like(node) || is_numeric_literal(node)
}

/// Go: `func IsSignedNumericLiteral(node *Node) bool` (utilities.go copy —
/// binder.go carries an identical package-local copy) — store-taking, since
/// the operand is a `NodeId` handle.
pub fn is_signed_numeric_literal(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind == Kind::PrefixUnaryExpression {
        let expr = n.as_prefix_unary_expression().expect("PrefixUnaryExpression data");
        return (expr.operator == Kind::PlusToken || expr.operator == Kind::MinusToken)
            && is_numeric_literal(store.node(expr.operand));
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// Optional chains (utilities.go:344-394)
// ────────────────────────────────────────────────────────────────────────────

pub use tsc_ast::utilities::is_optional_chain;

/// Go: `func getQuestionDotToken(node *Expression) *TokenNode`.
fn get_question_dot_token(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    node_data::question_dot_token_of(store, node)
}

/// Go: `func IsOptionalChainRoot(node *Expression) bool` — node is the root
/// expression of an OptionalChain.
pub fn is_optional_chain_root(store: &dyn NodeStore, node: NodeId) -> bool {
    is_optional_chain(store.node(node))
        && !is_non_null_expression(store.node(node))
        && get_question_dot_token(store, node).is_some()
}

/// Go: `func IsOutermostOptionalChain(node *Expression) bool`.
pub fn is_outermost_optional_chain(store: &dyn NodeStore, node: NodeId) -> bool {
    let Some(parent) = parent_of(store, node) else {
        return true;
    };
    !is_optional_chain(store.node(parent)) // cases 1, 2, and 3
        || is_optional_chain_root(store, parent) // case 4
        || node != node_data::expression_of(store, parent).unwrap_or_default() // case 5
}

/// Go: `func IsExpressionOfOptionalChainRoot(node *Node) bool`.
pub fn is_expression_of_optional_chain_root(store: &dyn NodeStore, node: NodeId) -> bool {
    match parent_of(store, node) {
        Some(parent) => {
            is_optional_chain_root(store, parent)
                && node_data::expression_of(store, parent) == Some(node)
        }
        None => false,
    }
}

/// Go: `func IsNullishCoalesce(node *Node) bool`.
pub fn is_nullish_coalesce(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    is_binary_expression(n)
        && store
            .node(n.as_binary_expression().expect("BinaryExpression data").operator_token)
            .kind
            == Kind::QuestionQuestionToken
}

// ────────────────────────────────────────────────────────────────────────────
// Expression kinds (utilities.go:396-452)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isLeftHandSideExpressionKind(kind Kind) bool`.
pub fn is_left_hand_side_expression_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::PropertyAccessExpression
            | Kind::ElementAccessExpression
            | Kind::NewExpression
            | Kind::CallExpression
            | Kind::JsxElement
            | Kind::JsxSelfClosingElement
            | Kind::JsxFragment
            | Kind::TaggedTemplateExpression
            | Kind::ArrayLiteralExpression
            | Kind::ParenthesizedExpression
            | Kind::ObjectLiteralExpression
            | Kind::ClassExpression
            | Kind::FunctionExpression
            | Kind::Identifier
            | Kind::PrivateIdentifier
            | Kind::RegularExpressionLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::StringLiteral
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::TemplateExpression
            | Kind::FalseKeyword
            | Kind::NullKeyword
            | Kind::ThisKeyword
            | Kind::TrueKeyword
            | Kind::SuperKeyword
            | Kind::NonNullExpression
            | Kind::ExpressionWithTypeArguments
            | Kind::MetaProperty
            | Kind::ImportKeyword
            | Kind::MissingDeclaration
    )
}

/// Go: `func SkipPartiallyEmittedExpressions(node *Expression) *Expression`
/// (the OEKPartiallyEmittedExpressions-only reduction of SkipOuterExpressions).
pub fn skip_partially_emitted_expressions(store: &dyn NodeStore, node: NodeId) -> NodeId {
    let mut node = node;
    while matches!(&store.node(node).data, NodeData::PartiallyEmittedExpression(_)) {
        node = node_data::expression_of(store, node).expect("PartiallyEmittedExpression expression");
    }
    node
}

/// Go: `func IsLeftHandSideExpression(node *Node) bool`.
pub fn is_left_hand_side_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    is_left_hand_side_expression_kind(store.node(skip_partially_emitted_expressions(store, node)).kind)
}

// ────────────────────────────────────────────────────────────────────────────
// Function-like / class-like kinds (utilities.go:479-611)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsAccessExpression(node *Node) bool`.
pub fn is_access_expression(node: &Node) -> bool {
    node.kind == Kind::PropertyAccessExpression || node.kind == Kind::ElementAccessExpression
}

/// Go: `func isFunctionLikeDeclarationKind(kind Kind) bool`.
pub fn is_function_like_declaration_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::FunctionDeclaration
            | Kind::MethodDeclaration
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionExpression
            | Kind::ArrowFunction
    )
}

/// Go: `func IsFunctionLikeDeclaration(node *Node) bool`.
pub fn is_function_like_declaration(node: &Node) -> bool {
    is_function_like_declaration_kind(node.kind)
}

/// Go: `func IsFunctionLikeKind(kind Kind) bool`.
pub fn is_function_like_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::MethodSignature
            | Kind::CallSignature
            | Kind::JSDocSignature
            | Kind::ConstructSignature
            | Kind::IndexSignature
            | Kind::FunctionType
            | Kind::ConstructorType
    ) || is_function_like_declaration_kind(kind)
}

/// Go: `func IsFunctionLike(node *Node) bool`.
pub fn is_function_like(node: &Node) -> bool {
    is_function_like_kind(node.kind)
}

/// Go: `func IsFunctionLikeOrClassStaticBlockDeclaration(node *Node) bool`.
pub fn is_function_like_or_class_static_block_declaration(node: &Node) -> bool {
    is_function_like(node) || is_class_static_block_declaration(node)
}

/// Go: `func IsFunctionOrSourceFile(node *Node) bool`.
pub fn is_function_or_source_file(node: &Node) -> bool {
    is_function_like(node) || is_source_file(node)
}

/// Go: `func IsClassLike(node *Node) bool`.
pub fn is_class_like(node: &Node) -> bool {
    node.kind == Kind::ClassDeclaration || node.kind == Kind::ClassExpression
}

/// Go: `func IsClassOrInterfaceLike(node *Node) bool`.
pub fn is_class_or_interface_like(node: &Node) -> bool {
    node.kind == Kind::ClassDeclaration
        || node.kind == Kind::ClassExpression
        || node.kind == Kind::InterfaceDeclaration
}

/// Go: `func IsClassElement(node *Node) bool`.
pub fn is_class_element(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::Constructor
            | Kind::PropertyDeclaration
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::IndexSignature
            | Kind::ClassStaticBlockDeclaration
            | Kind::SemicolonClassElement
    )
}

/// Go: `func IsObjectLiteralElement(node *Node) bool`.
pub fn is_object_literal_element(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::PropertyAssignment
            | Kind::ShorthandPropertyAssignment
            | Kind::SpreadAssignment
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
    )
}

/// Go: `func IsObjectLiteralOrClassExpressionMethodOrAccessor(node *Node) bool`.
pub fn is_object_literal_or_class_expression_method_or_accessor(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let n = store.node(node);
    let kind = n.kind;
    (kind == Kind::MethodDeclaration || kind == Kind::GetAccessor || kind == Kind::SetAccessor)
        && parent_of(store, node)
            .map(|p| {
                let pk = store.node(p).kind;
                pk == Kind::ObjectLiteralExpression || pk == Kind::ClassExpression
            })
            .unwrap_or(false)
}

/// Go: `func IsObjectLiteralMethod(node *Node) bool`.
pub fn is_object_literal_method(store: &dyn NodeStore, node: NodeId) -> bool {
    store.node(node).kind == Kind::MethodDeclaration
        && parent_of(store, node).is_some_and(|p| store.node(p).kind == Kind::ObjectLiteralExpression)
}

/// Go: `func IsAutoAccessorPropertyDeclaration(node *Node) bool`.
pub fn is_auto_accessor_property_declaration(node: &Node) -> bool {
    is_property_declaration(node) && has_accessor_modifier(node)
}

/// Go: `func IsParameterPropertyDeclaration(node *Node, parent *Node) bool`.
pub fn is_parameter_property_declaration(store: &dyn NodeStore, node: NodeId, parent: NodeId) -> bool {
    is_parameter_declaration_store(store, node)
        && tsc_ast::has_syntactic_modifier(store.node(node), ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
        && store.node(parent).kind == Kind::Constructor
}

fn is_parameter_declaration_store(store: &dyn NodeStore, node: NodeId) -> bool {
    is_parameter_declaration(store.node(node))
}

/// Go: `func HasAccessorModifier(node *Node) bool`.
pub fn has_accessor_modifier(node: &Node) -> bool {
    tsc_ast::has_syntactic_modifier(node, ModifierFlags::ACCESSOR)
}

/// Go: `func IsFunctionExpressionOrArrowFunction(node *Node) bool`.
pub fn is_function_expression_or_arrow_function(node: &Node) -> bool {
    is_function_expression(node) || is_arrow_function(node)
}

// ────────────────────────────────────────────────────────────────────────────
// Declaration statements / block scoping (utilities.go:628-725)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isDeclarationStatementKind(kind Kind) bool`.
pub fn is_declaration_statement_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::FunctionDeclaration
            | Kind::MissingDeclaration
            | Kind::ClassDeclaration
            | Kind::InterfaceDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::JSTypeAliasDeclaration
            | Kind::EnumDeclaration
            | Kind::ModuleDeclaration
            | Kind::ImportDeclaration
            | Kind::JSImportDeclaration
            | Kind::ImportEqualsDeclaration
            | Kind::ExportDeclaration
            | Kind::ExportAssignment
            | Kind::NamespaceExportDeclaration
    )
}

/// Go: `func IsDeclarationStatement(node *Node) bool`.
pub fn is_declaration_statement(node: &Node) -> bool {
    is_declaration_statement_kind(node.kind)
}

/// Go: `func GetRootDeclaration(node *Node) *Node`.
pub fn get_root_declaration(store: &dyn NodeStore, node: NodeId) -> NodeId {
    let mut node = node;
    while store.node(node).kind == Kind::BindingElement {
        node = parent_of(store, parent_of(store, node).expect("BindingElement parent"))
            .expect("BindingElement grandparent");
    }
    node
}

/// Go: `func IsBlockOrCatchScoped(declaration *Node) bool`.
pub fn is_block_or_catch_scoped(store: &dyn NodeStore, node: NodeId) -> bool {
    get_combined_node_flags(store, node).intersects(NodeFlags::BLOCK_SCOPED)
        || is_catch_clause_variable_declaration_or_binding_element(store, node)
}

/// Go: `func IsCatchClauseVariableDeclarationOrBindingElement(declaration *Node) bool`.
pub fn is_catch_clause_variable_declaration_or_binding_element(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let node = get_root_declaration(store, node);
    store.node(node).kind == Kind::VariableDeclaration
        && parent_of(store, node).is_some_and(|p| store.node(p).kind == Kind::CatchClause)
}

/// Go: `func IsTypeNode(node *Node) bool`.
pub fn is_type_node(node: &Node) -> bool {
    is_type_node_kind(node.kind)
}

/// Go: `func IsPrologueDirective(node *Node) bool`.
pub fn is_prologue_directive(store: &dyn NodeStore, node: NodeId) -> bool {
    store.node(node).kind == Kind::ExpressionStatement
        && node_data::expression_of(store, node).is_some_and(|e| store.node(e).kind == Kind::StringLiteral)
}

// ────────────────────────────────────────────────────────────────────────────
// Parentheses / ancestors (utilities.go:829-949)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func SkipParentheses(node *Expression) *Expression`.
///
/// PORT: the full OuterExpressionKinds machinery (OEKExcludeJSDocTypeAssertion
/// etc.) arrives with the wholesale utilities.go port; with only
/// `OEKParentheses` in play the Go loop reduces to this paren walk.
pub fn skip_parentheses(store: &dyn NodeStore, node: NodeId) -> NodeId {
    let mut node = node;
    while is_parenthesized_expression(store.node(node)) {
        node = node_data::expression_of(store, node).expect("ParenthesizedExpression expression");
    }
    node
}

/// Go: `func FindAncestor(node *Node, callback func(*Node) bool) *Node` —
/// walks up the parents of a node to find the ancestor that matches the
/// callback.
pub fn find_ancestor(
    store: &dyn NodeStore,
    node: NodeId,
    callback: &mut dyn FnMut(NodeId) -> bool,
) -> Option<NodeId> {
    let mut node = Some(node);
    while let Some(current) = node {
        if callback(current) {
            return Some(current);
        }
        node = parent_of(store, current);
    }
    None
}

// ────────────────────────────────────────────────────────────────────────────
// Modifiers (utilities.go:1036-1196)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func HasStaticModifier(node *Node) bool`.
pub fn has_static_modifier(node: &Node) -> bool {
    tsc_ast::has_syntactic_modifier(node, ModifierFlags::STATIC)
}

/// Go: `func IsStatic(node *Node) bool` —
/// https://tc39.es/ecma262/#sec-static-semantics-isstatic
pub fn is_static(node: &Node) -> bool {
    is_class_element(node) && has_static_modifier(node) || is_class_static_block_declaration(node)
}

/// Go: `func GetCombinedModifierFlags(node *Node) ModifierFlags`.
pub fn get_combined_modifier_flags(store: &dyn NodeStore, node: NodeId) -> ModifierFlags {
    let mut node = get_root_declaration(store, node);
    let mut flags = store.node(node).modifier_flags();
    if store.node(node).kind == Kind::VariableDeclaration {
        node = parent_of(store, node).unwrap_or_default();
    }
    if !node.is_none() && store.node(node).kind == Kind::VariableDeclarationList {
        flags |= store.node(node).modifier_flags();
        node = parent_of(store, node).unwrap_or_default();
    }
    if !node.is_none() && store.node(node).kind == Kind::VariableStatement {
        flags |= store.node(node).modifier_flags();
    }
    flags
}

/// Go: `func GetCombinedNodeFlags(node *Node) NodeFlags`.
pub fn get_combined_node_flags(store: &dyn NodeStore, node: NodeId) -> NodeFlags {
    let mut node = get_root_declaration(store, node);
    let mut flags = store.node(node).flags;
    if store.node(node).kind == Kind::VariableDeclaration {
        node = parent_of(store, node).unwrap_or_default();
    }
    if !node.is_none() && store.node(node).kind == Kind::VariableDeclarationList {
        flags |= store.node(node).flags;
        node = parent_of(store, node).unwrap_or_default();
    }
    if !node.is_none() && store.node(node).kind == Kind::VariableStatement {
        flags |= store.node(node).flags;
    }
    flags
}

// ────────────────────────────────────────────────────────────────────────────
// Module / enum / declaration names (utilities.go:1334-1744)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsModuleOrEnumDeclaration(node *Node) bool`.
pub fn is_module_or_enum_declaration(node: &Node) -> bool {
    node.kind == Kind::ModuleDeclaration || node.kind == Kind::EnumDeclaration
}

/// Go: `func IsModuleBlock(node *Node) bool` (generated guard; re-exported for
/// parity with the Go import list).
pub use tsc_ast::is_module_block;

/// Go: `func GetElementOrPropertyAccessName(node *Node) *Node` — does not
/// handle signed numeric names like `a[+0]`.
pub fn get_element_or_property_access_name(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::PropertyAccessExpression => {
            let name = n.as_property_access_expression().expect("PropertyAccessExpression data").name;
            if is_identifier(store.node(name)) {
                return Some(name);
            }
            None
        }
        Kind::ElementAccessExpression => {
            let argument = n
                .as_element_access_expression()
                .expect("ElementAccessExpression data")
                .argument_expression;
            let arg = skip_parentheses(store, argument);
            if is_string_or_numeric_literal_like(store.node(arg)) {
                return Some(arg);
            }
            None
        }
        _ => panic!("Unhandled case in GetElementOrPropertyAccessName"),
    }
}

/// Go: `func GetNameOfDeclaration(declaration *Node) *Node`.
pub fn get_name_of_declaration(store: &dyn NodeStore, declaration: NodeId) -> Option<NodeId> {
    let non_assigned_name = get_non_assigned_name_of_declaration(store, declaration);
    if non_assigned_name.is_some() {
        return non_assigned_name;
    }
    if is_function_expression(store.node(declaration))
        || is_arrow_function(store.node(declaration))
        || is_class_expression(store.node(declaration))
    {
        return get_assigned_name(store, declaration);
    }
    None
}

/// Go: `func GetNonAssignedNameOfDeclaration(declaration *Node) *Node`.
pub fn get_non_assigned_name_of_declaration(
    store: &dyn NodeStore,
    declaration: NodeId,
) -> Option<NodeId> {
    let n = store.node(declaration);
    match n.kind {
        Kind::BinaryExpression | Kind::CallExpression => {
            match get_assignment_declaration_kind(store, declaration) {
                JSDeclarationKind::Property
                | JSDeclarationKind::ThisProperty
                | JSDeclarationKind::ExportsProperty => {
                    let left = n.as_binary_expression().expect("BinaryExpression data").left;
                    if let Some(name) = get_element_or_property_access_name(store, left) {
                        return Some(name);
                    }
                    return Some(left);
                }
                JSDeclarationKind::ObjectDefinePropertyValue | JSDeclarationKind::ObjectDefinePropertyExports => {
                    return node_data::arguments_of(store, declaration).get(1).copied();
                }
                // Go: JSDeclarationKindModuleExports and the rest fall
                // through to `return nil`.
                JSDeclarationKind::None | JSDeclarationKind::ModuleExports => {}
            }
            None
        }
        Kind::ExportAssignment => {
            let expr = node_data::expression_of(store, declaration).expect("ExportAssignment expression");
            if is_identifier(store.node(expr)) {
                return Some(expr);
            }
            None
        }
        _ => n.name(),
    }
}

/// Go: `func GetAssignedName(node *Node) *Node`.
pub fn get_assigned_name(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let parent = parent_of(store, node)?;
    match store.node(parent).kind {
        Kind::PropertyAssignment => store.node(parent).name(),
        Kind::BindingElement => store.node(parent).name(),
        Kind::BinaryExpression => {
            let expr = store
                .node(parent)
                .as_binary_expression()
                .expect("BinaryExpression data");
            if node == expr.right {
                let left = expr.left;
                match store.node(left).kind {
                    Kind::Identifier => Some(left),
                    Kind::PropertyAccessExpression => {
                        Some(store.node(left).name().expect("PropertyAccessExpression name"))
                    }
                    Kind::ElementAccessExpression => {
                        let arg = skip_parentheses(
                            store,
                            store
                                .node(left)
                                .as_element_access_expression()
                                .expect("ElementAccessExpression data")
                                .argument_expression,
                        );
                        if is_string_or_numeric_literal_like(store.node(arg)) {
                            Some(arg)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
        Kind::VariableDeclaration => {
            let name = store
                .node(parent)
                .as_variable_declaration()
                .expect("VariableDeclaration data")
                .name;
            if is_identifier(store.node(name)) {
                Some(name)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// JSDeclarationKind (utilities.go:1525-1580)
// ────────────────────────────────────────────────────────────────────────────

// PORT (dedup): `JSDeclarationKind` and `GetAssignmentDeclarationKind` were
// transcribed here for M4 wave A and are now canonical in
// tsc_ast::utilities (M5 wave F, the reparser's JSDoc machinery) — this
// module imports them from there. The section's remaining helpers
// (IsExportsIdentifier etc.) are still binder-local pending their
// ast-canonical fold.

/// Go: `func IsExportsIdentifier(node *Node) bool`.
pub fn is_exports_identifier(store: &dyn NodeStore, node: NodeId) -> bool {
    is_identifier(store.node(node)) && node_text(store, node) == "exports"
}

/// Go: `func IsModuleIdentifier(node *Node) bool`.
pub fn is_module_identifier(store: &dyn NodeStore, node: NodeId) -> bool {
    is_identifier(store.node(node)) && node_text(store, node) == "module"
}

/// Go: `func IsModuleExportsAccessExpression(node *Node) bool`.
pub fn is_module_exports_access_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if is_access_expression(n) && is_module_identifier(store, node_data::expression_of(store, node).expect("access expression")) {
        if let Some(name) = get_element_or_property_access_name(store, node) {
            return node_text(store, name) == "exports";
        }
    }
    false
}

/// Go: `func IsBindableObjectDefinePropertyCall(node *Node) bool`.
pub fn is_bindable_object_define_property_call(store: &dyn NodeStore, node: NodeId) -> bool {
    let args = node_data::arguments_of(store, node);
    if args.len() == 3 {
        if let Some(expr) = node_data::expression_of(store, node) {
            let e = store.node(expr);
            if is_property_access_expression(e) {
                let access = e.as_property_access_expression().expect("PropertyAccessExpression data");
                let access_expr = access.expression;
                if is_identifier(store.node(access_expr))
                    && node_text(store, access_expr) == "Object"
                    && node_text(store, access.name) == "defineProperty"
                    && is_string_or_numeric_literal_like(store.node(args[1]))
                    && is_bindable_static_name_expression(store, args[0], true)
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Go: `func IsBindableStaticNameExpression(node *Node, excludeThisKeyword bool) bool`.
pub fn is_bindable_static_name_expression(
    store: &dyn NodeStore,
    node: NodeId,
    exclude_this_keyword: bool,
) -> bool {
    is_entity_name_expression(store, node)
        || is_bindable_static_access_expression(store, node, exclude_this_keyword)
}

/// Go: `func IsBindableStaticAccessExpression(node *Node, excludeThisKeyword bool) bool`.
pub fn is_bindable_static_access_expression(
    store: &dyn NodeStore,
    node: NodeId,
    exclude_this_keyword: bool,
) -> bool {
    is_literal_like_element_access(store, node)
        && ((!exclude_this_keyword
            && node_data::expression_of(store, node).is_some_and(|e| store.node(e).kind == Kind::ThisKeyword))
            || is_entity_name_expression(
                store,
                node_data::expression_of(store, node).expect("access expression"),
            )
            || is_bindable_static_access_expression(
                store,
                node_data::expression_of(store, node).expect("access expression"),
                true, /*excludeThisKeyword*/
            ))
}

/// Go: `func IsLiteralLikeElementAccess(node *Node) bool`.
pub fn is_literal_like_element_access(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    is_element_access_expression(n)
        && is_string_or_numeric_literal_like(
            store
                .node(n.as_element_access_expression().expect("ElementAccessExpression data").argument_expression),
        )
}

// PORT (dedup): `GetAssignmentDeclarationKind` is canonical in
// tsc_ast::utilities (M5 wave F); the M4-wave-A transcription here was
// removed — imported from `tsc_ast` above instead.

// ────────────────────────────────────────────────────────────────────────────
// Dynamic names / dotted names (utilities.go:1602-1656)
// ────────────────────────────────────────────────────────────────────────────

/// A declaration has a dynamic name if all of the following are true:
///   1. The declaration has a computed property name.
///   2. The computed name is *not* expressed as a StringLiteral.
///   3. The computed name is *not* expressed as a NumericLiteral.
///   4. The computed name is *not* expressed as a PlusToken or MinusToken
///      immediately followed by a NumericLiteral.
///
/// Go: `func HasDynamicName(declaration *Node) bool`.
pub fn has_dynamic_name(store: &dyn NodeStore, declaration: NodeId) -> bool {
    get_name_of_declaration(store, declaration)
        .is_some_and(|name| is_dynamic_name(store, name))
}

/// Go: `func IsDynamicName(name *Node) bool`.
pub fn is_dynamic_name(store: &dyn NodeStore, name: NodeId) -> bool {
    let n = store.node(name);
    let expr = match n.kind {
        Kind::ComputedPropertyName => node_data::expression_of(store, name).expect("ComputedPropertyName expression"),
        Kind::ElementAccessExpression => skip_parentheses(
            store,
            n.as_element_access_expression().expect("ElementAccessExpression data").argument_expression,
        ),
        _ => return false,
    };
    !is_string_or_numeric_literal_like(store.node(expr)) && !is_signed_numeric_literal(store, expr)
}

/// Go: `func IsEntityNameExpression(node *Node) bool`.
pub fn is_entity_name_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    is_entity_name_expression_ex(store, node, false /*allowJS*/)
}

/// Go: `func IsEntityNameExpressionEx(node *Node, allowJS bool) bool`.
pub fn is_entity_name_expression_ex(store: &dyn NodeStore, node: NodeId, allow_js: bool) -> bool {
    let n = store.node(node);
    is_identifier(n)
        || is_property_access_entity_name_expression(store, node, allow_js)
        || (allow_js
            && (n.kind == Kind::ThisKeyword
                || is_element_access_entity_name_expression(store, node, allow_js)))
}

/// Go: `func IsPropertyAccessEntityNameExpression(node *Node, allowJS bool) bool`.
pub fn is_property_access_entity_name_expression(
    store: &dyn NodeStore,
    node: NodeId,
    allow_js: bool,
) -> bool {
    let n = store.node(node);
    is_property_access_expression(n)
        && is_identifier(store.node(n.name().expect("PropertyAccessExpression name")))
        && is_entity_name_expression_ex(
            store,
            n.as_property_access_expression().expect("PropertyAccessExpression data").expression,
            allow_js,
        )
}

/// Go: `func isElementAccessEntityNameExpression(node *Node, allowJS bool) bool`.
fn is_element_access_entity_name_expression(
    store: &dyn NodeStore,
    node: NodeId,
    allow_js: bool,
) -> bool {
    let n = store.node(node);
    is_element_access_expression(n)
        && is_string_or_numeric_literal_like(
            store.node(n.as_element_access_expression().expect("ElementAccessExpression data").argument_expression),
        )
        && is_entity_name_expression_ex(
            store,
            n.as_element_access_expression().expect("ElementAccessExpression data").expression,
            allow_js,
        )
}

/// Go: `func IsDottedName(node *Node) bool`.
pub fn is_dotted_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::Identifier | Kind::ThisKeyword | Kind::SuperKeyword | Kind::MetaProperty => true,
        Kind::PropertyAccessExpression | Kind::ParenthesizedExpression => {
            is_dotted_name(store, node_data::expression_of(store, node).expect("expression"))
        }
        _ => false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Modules (utilities.go:1658-1733, 1686-1693)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsAmbientModule(node *Node) bool` — store-taking, since the
/// module name is a `NodeId` handle and its kind resolves through the store.
pub fn is_ambient_module(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if !is_module_declaration(n) {
        return false;
    }
    let name = n.as_module_declaration().expect("ModuleDeclaration data").name;
    let name_kind = store.node(name).kind;
    name_kind == Kind::StringLiteral || is_global_scope_augmentation(n)
}

/// Go: `func IsGlobalScopeAugmentation(node *Node) bool`.
pub fn is_global_scope_augmentation(node: &Node) -> bool {
    is_module_declaration(node)
        && node.as_module_declaration().expect("ModuleDeclaration data").keyword == Kind::GlobalKeyword
}

/// Go: `func IsExternalModule(file *SourceFile) bool`.
pub fn is_external_module(file: &SourceFile) -> bool {
    file.payload().external_module_indicator.get().is_some()
}

/// Go: `func IsExternalOrCommonJSModule(file *SourceFile) bool`.
pub fn is_external_or_common_js_module(file: &SourceFile) -> bool {
    file.payload().external_module_indicator.get().is_some()
        || file.payload().common_js_module_indicator.get().is_some()
}

/// Go: `func IsModuleAugmentationExternal(node *Node) bool` — external module
/// augmentation is an ambient module declaration that is either defined in
/// the top level scope and source file is an external module, or defined
/// inside ambient module declaration located in the top level scope and
/// source file not an external module.
pub fn is_module_augmentation_external(store: &dyn NodeStore, node: NodeId) -> bool {
    let Some(parent) = parent_of(store, node) else {
        return false;
    };
    match store.node(parent).kind {
        Kind::SourceFile => {
            let file = store.file(parent.file_id());
            is_external_module(file)
        }
        Kind::ModuleBlock => {
            let Some(grand_parent) = parent_of(store, parent) else {
                return false;
            };
            let Some(great_grand_parent) = parent_of(store, grand_parent) else {
                return false;
            };
            is_ambient_module(store, grand_parent)
                && store.node(great_grand_parent).kind == Kind::SourceFile
                && !is_external_module(store.file(great_grand_parent.file_id()))
        }
        _ => false,
    }
}

/// Go: `func GetContainingClass(node *Node) *Node`.
pub fn get_containing_class(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let parent = parent_of(store, node)?;
    find_ancestor(store, parent, &mut |n| is_class_like(store.node(n)))
}

// ────────────────────────────────────────────────────────────────────────────
// Scope queries (utilities.go:1799-1828, 1892-1940)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsPartOfTypeQuery(node *Node) bool`.
pub fn is_part_of_type_query(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut node = Some(node);
    while let Some(current) = node {
        let kind = store.node(current).kind;
        if kind != Kind::QualifiedName && kind != Kind::Identifier {
            break;
        }
        node = parent_of(store, current);
    }
    store
        .node(node.expect("IsPartOfTypeQuery walked past the root"))
        .kind
        == Kind::TypeQuery
}

/// This function returns true if the this node's root declaration is a parameter.
/// For example, passing a `ParameterDeclaration` will return true, as will passing a
/// binding element that is a child of a `ParameterDeclaration`.
///
/// Go: `func IsPartOfParameterDeclaration(node *Node) bool`.
pub fn is_part_of_parameter_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    store.node(get_root_declaration(store, node)).kind == Kind::Parameter
}

/// Go: `func IsInTopLevelContext(node *Node) bool` — the name of a class or
/// function declaration is a BindingIdentifier in its surrounding scope.
pub fn is_in_top_level_context(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut node = node;
    if is_identifier(store.node(node)) {
        if let Some(parent) = parent_of(store, node) {
            if (is_class_declaration(store.node(parent)) || is_function_declaration(store.node(parent)))
                && store.node(parent).name() == Some(node)
            {
                node = parent;
            }
        }
    }
    let container =
        get_this_container(store, node, true /*includeArrowFunctions*/, false /*includeClassComputedPropertyName*/);
    is_source_file(store.node(container))
}

/// Go: `func GetThisContainer(node *Node, includeArrowFunctions bool, includeClassComputedPropertyName bool) *Node`.
pub fn get_this_container(
    store: &dyn NodeStore,
    node: NodeId,
    include_arrow_functions: bool,
    include_class_computed_property_name: bool,
) -> NodeId {
    let mut node = node;
    loop {
        node = parent_of(store, node).expect("nil parent in getThisContainer");
        let n = store.node(node);
        match n.kind {
            Kind::ComputedPropertyName => {
                if include_class_computed_property_name {
                    let parent = parent_of(store, node).expect("ComputedPropertyName parent");
                    let grand = parent_of(store, parent).expect("ComputedPropertyName grandparent");
                    if is_class_like(store.node(grand)) {
                        return node;
                    }
                }
                let parent = parent_of(store, node).expect("ComputedPropertyName parent");
                node = parent_of(store, parent).expect("ComputedPropertyName grandparent");
            }
            Kind::Decorator => {
                let parent = parent_of(store, node).expect("Decorator parent");
                if store.node(parent).kind == Kind::Parameter {
                    let grand = parent_of(store, parent).expect("Parameter parent");
                    if is_class_element(store.node(grand)) {
                        // If the decorator's parent is a ParameterDeclaration, we resolve the this container from
                        // the grandparent class declaration.
                        node = grand;
                        continue;
                    }
                }
                if is_class_element(store.node(parent)) {
                    // If the decorator's parent is a class element, we resolve the 'this' container
                    // from the parent class declaration.
                    node = parent;
                    continue;
                }
            }
            Kind::ArrowFunction => {
                if include_arrow_functions {
                    return node;
                }
            }
            Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::ModuleDeclaration
            | Kind::ClassStaticBlockDeclaration
            | Kind::PropertyDeclaration
            | Kind::PropertySignature
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::CallSignature
            | Kind::ConstructSignature
            | Kind::IndexSignature
            | Kind::EnumDeclaration
            | Kind::SourceFile => return node,
            _ => {}
        }
    }
}

/// Go: `func GetImmediatelyInvokedFunctionExpression(fn *Node) *Node`.
pub fn get_immediately_invoked_function_expression(store: &dyn NodeStore, fn_node: NodeId) -> Option<NodeId> {
    if is_function_expression_or_arrow_function(store.node(fn_node)) {
        let mut prev = fn_node;
        let mut parent = parent_of(store, prev);
        while let Some(p) = parent {
            if store.node(p).kind == Kind::ParenthesizedExpression {
                prev = p;
                parent = parent_of(store, p);
            } else {
                break;
            }
        }
        if let Some(p) = parent {
            if store.node(p).kind == Kind::CallExpression
                && node_data::expression_of(store, p) == Some(prev)
            {
                return Some(p);
            }
        }
    }
    None
}

/// Go: `func IsEnumConst(node *Node) bool`.
pub fn is_enum_const(store: &dyn NodeStore, node: NodeId) -> bool {
    get_combined_modifier_flags(store, node).intersects(ModifierFlags::CONST)
}

/// Go: `func ExpressionIsAlias(node *Node) bool`.
pub fn expression_is_alias(store: &dyn NodeStore, node: NodeId) -> bool {
    is_entity_name_expression(store, node) || is_class_expression(store.node(node))
}

/// Go: `func IsJsonSourceFile(file *SourceFile) bool`.
pub fn is_json_source_file(file: &SourceFile) -> bool {
    file.payload().script_kind == tsc_core::scriptkind::ScriptKind::JSON
}

// ────────────────────────────────────────────────────────────────────────────
// ModuleInstanceState (utilities.go:2371-2528)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ModuleInstanceState int32` + iota consts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(i32)]
pub enum ModuleInstanceState {
    /// Go: `ModuleInstanceStateUnknown ModuleInstanceState = iota`.
    Unknown,
    /// Go: `ModuleInstanceStateNonInstantiated`.
    NonInstantiated,
    /// Go: `ModuleInstanceStateInstantiated`.
    Instantiated,
    /// Go: `ModuleInstanceStateConstEnumOnly`.
    ConstEnumOnly,
}

/// Go: `func pushAncestor(ancestors []*Node, parent *Node) []*Node`.
fn push_ancestor(ancestors: &mut Vec<NodeId>, parent: NodeId) {
    ancestors.push(parent);
}

/// Go: `func popAncestor(ancestors []*Node, node *Node) ([]*Node, *Node)`.
fn pop_ancestor(ancestors: &mut Vec<NodeId>, node: NodeId, store: &dyn NodeStore) -> Option<NodeId> {
    if ancestors.is_empty() {
        return parent_of(store, node);
    }
    ancestors.pop()
}

/// Go: `func GetModuleInstanceState(node *Node) ModuleInstanceState`.
pub fn get_module_instance_state(store: &dyn NodeStore, node: NodeId) -> ModuleInstanceState {
    let mut visited = rustc_hash::FxHashMap::default();
    get_module_instance_state_inner(store, node, &mut Vec::new(), &mut visited)
}

/// Go: `func getModuleInstanceState(node *Node, ancestors []*Node, visited map[NodeId]ModuleInstanceState) ModuleInstanceState`.
fn get_module_instance_state_inner(
    store: &dyn NodeStore,
    node: NodeId,
    ancestors: &mut Vec<NodeId>,
    visited: &mut rustc_hash::FxHashMap<u64, ModuleInstanceState>,
) -> ModuleInstanceState {
    let module = store
        .node(node)
        .as_module_declaration()
        .expect("ModuleDeclaration data");
    if let Some(body) = module.body {
        push_ancestor(ancestors, node);
        get_module_instance_state_cached(store, body, ancestors, visited)
    } else {
        ModuleInstanceState::Instantiated
    }
}

/// Go: `func getModuleInstanceStateCached(...)`.
fn get_module_instance_state_cached(
    store: &dyn NodeStore,
    node: NodeId,
    ancestors: &mut Vec<NodeId>,
    visited: &mut rustc_hash::FxHashMap<u64, ModuleInstanceState>,
) -> ModuleInstanceState {
    let node_id = get_node_id(store.node(node));
    if let Some(&cached) = visited.get(&node_id) {
        if cached != ModuleInstanceState::Unknown {
            return cached;
        }
        return ModuleInstanceState::NonInstantiated;
    }
    visited.insert(node_id, ModuleInstanceState::Unknown);
    let result = get_module_instance_state_worker(store, node, ancestors, visited);
    visited.insert(node_id, result);
    result
}

/// Go: `func getModuleInstanceStateWorker(...)`.
fn get_module_instance_state_worker(
    store: &dyn NodeStore,
    node: NodeId,
    ancestors: &mut Vec<NodeId>,
    visited: &mut rustc_hash::FxHashMap<u64, ModuleInstanceState>,
) -> ModuleInstanceState {
    // A module is uninstantiated if it contains only
    let n = store.node(node);
    match n.kind {
        Kind::InterfaceDeclaration | Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration => {
            return ModuleInstanceState::NonInstantiated;
        }
        Kind::EnumDeclaration => {
            if is_enum_const(store, node) {
                return ModuleInstanceState::ConstEnumOnly;
            }
        }
        Kind::ImportDeclaration | Kind::JSImportDeclaration | Kind::ImportEqualsDeclaration => {
            if !tsc_ast::has_syntactic_modifier(n, ModifierFlags::EXPORT) {
                return ModuleInstanceState::NonInstantiated;
            }
        }
        Kind::ExportDeclaration => {
            let decl = n.as_export_declaration().expect("ExportDeclaration data");
            if decl.module_specifier.is_none() {
                if let Some(export_clause) = decl.export_clause {
                    if store.node(export_clause).kind == Kind::NamedExports {
                        let mut state = ModuleInstanceState::NonInstantiated;
                        push_ancestor(ancestors, node);
                        push_ancestor(ancestors, export_clause);
                        for &specifier in node_data::elements_of(store, export_clause) {
                            let specifier_state =
                                get_module_instance_state_for_alias_target(store, specifier, ancestors, visited);
                            if specifier_state > state {
                                state = specifier_state;
                            }
                            if state == ModuleInstanceState::Instantiated {
                                return state;
                            }
                        }
                        return state;
                    }
                }
            }
        }
        Kind::ModuleBlock => {
            let mut state = ModuleInstanceState::NonInstantiated;
            push_ancestor(ancestors, node);
            // Go iterates children with early stop on Instantiated; the port
            // collects children first (the walk cannot mutate while running).
            let mut children = Vec::new();
            store.node(node).for_each_child(&mut |c| {
                children.push(c);
                false
            });
            for child in children {
                let child_state = get_module_instance_state_cached(store, child, ancestors, visited);
                match child_state {
                    ModuleInstanceState::NonInstantiated => {}
                    ModuleInstanceState::ConstEnumOnly => {
                        state = ModuleInstanceState::ConstEnumOnly;
                    }
                    ModuleInstanceState::Instantiated => {
                        state = ModuleInstanceState::Instantiated;
                        break;
                    }
                    ModuleInstanceState::Unknown => {
                        panic!("Unhandled case in getModuleInstanceStateWorker")
                    }
                }
            }
            return state;
        }
        Kind::ModuleDeclaration => {
            return get_module_instance_state_inner(store, node, ancestors, visited);
        }
        _ => {}
    }
    ModuleInstanceState::Instantiated
}

/// Go: `func getModuleInstanceStateForAliasTarget(...)`.
fn get_module_instance_state_for_alias_target(
    store: &dyn NodeStore,
    node: NodeId,
    ancestors: &mut Vec<NodeId>,
    visited: &mut rustc_hash::FxHashMap<u64, ModuleInstanceState>,
) -> ModuleInstanceState {
    let Some(name) = node_data::property_name_or_name(store, node) else {
        // Skip for invalid syntax like this: export { "x" }
        return ModuleInstanceState::Instantiated;
    };
    if store.node(name).kind != Kind::Identifier {
        // Skip for invalid syntax like this: export { "x" }
        return ModuleInstanceState::Instantiated;
    }
    let name_text = node_text(store, name);
    while let Some(p) = pop_ancestor(ancestors, node, store) {
        let pk = store.node(p).kind;
        if pk == Kind::Block || pk == Kind::ModuleBlock || pk == Kind::SourceFile {
            let mut found = ModuleInstanceState::Unknown;
            push_ancestor(ancestors, p);
            let statements_ancestors = ancestors.clone();
            let statements = node_data::statements_of(store, p)
                .map(|list| list.nodes.as_ref())
                .unwrap_or(&[]);
            for &statement in statements {
                if node_has_name(store, statement, name_text) {
                    let state = get_module_instance_state_cached(store, statement, ancestors, visited);
                    if found == ModuleInstanceState::Unknown || state > found {
                        found = state;
                    }
                    if found == ModuleInstanceState::Instantiated {
                        return found;
                    }
                    if store.node(statement).kind == Kind::ImportEqualsDeclaration {
                        // Treat re-exports of import aliases as instantiated since they're ambiguous. This is consistent
                        // with `export import x = mod.x` being treated as instantiated:
                        //   import x = mod.x;
                        //   export { x };
                        found = ModuleInstanceState::Instantiated;
                    }
                }
            }
            let _ = statements_ancestors;
            if found != ModuleInstanceState::Unknown {
                return found;
            }
        }
    }
    // Couldn't locate, assume could refer to a value
    ModuleInstanceState::Instantiated
}

/// Go: `func NodeHasName(statement *Node, id *Node) bool`.
fn node_has_name(store: &dyn NodeStore, statement: NodeId, id_text: &str) -> bool {
    if let Some(name) = store.node(statement).name() {
        return is_identifier(store.node(name)) && node_text(store, name) == id_text;
    }
    if is_variable_statement(store.node(statement)) {
        let declaration_list = store
            .node(statement)
            .as_variable_statement()
            .expect("VariableStatement data")
            .declaration_list;
        let declarations = match &store.node(declaration_list).data {
            NodeData::VariableDeclarationList(d) => d
                .declarations
                .as_ref()
                .map(|list| list.nodes.as_ref())
                .unwrap_or(&[]),
            _ => &[],
        };
        return declarations.iter().any(|&d| node_has_name(store, d, id_text));
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// Const assertions / declaration-of-kind (utilities.go:2545-2640)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsConstAssertion(node *Node) bool`.
pub fn is_const_assertion(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::AsExpression | Kind::TypeAssertionExpression => {
            let type_ = node_data::type_of(store, node).expect("assertion type");
            is_const_type_reference(store, type_)
        }
        _ => false,
    }
}

/// Go: `func IsConstTypeReference(node *Node) bool`.
pub fn is_const_type_reference(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if !is_type_reference_node(n) {
        return false;
    }
    let d = n.as_type_reference_node().expect("TypeReferenceNode data");
    d.type_arguments.is_none()
        && is_identifier(store.node(d.type_name))
        && node_text(store, d.type_name) == "const"
}

/// Go: `func IsGlobalSourceFile(node *Node) bool`.
pub fn is_global_source_file(store: &dyn NodeStore, node: NodeId) -> bool {
    store.node(node).kind == Kind::SourceFile
        && !is_external_or_common_js_module(store.file(node.file_id()))
}

/// Go: `func GetDeclarationOfKind(symbol *Symbol, kind Kind) *Node` —
/// store-taking, since the declaration kind resolves through the store.
/// PORT: `symbols` is the caller's symbol arena (Go's `*Symbol` carried the
/// `Declarations` slice directly; the port's `SymbolId` is an arena index).
pub fn get_declaration_of_kind(
    store: &dyn NodeStore,
    symbols: &[Symbol],
    symbol: SymbolId,
    kind: Kind,
) -> Option<NodeId> {
    symbols[symbol.0 as usize]
        .declarations
        .iter()
        .copied()
        .find(|&declaration| store.node(declaration).kind == kind)
}

/// Go: `func FindConstructorDeclaration(node *ClassLikeDeclaration) *Node`.
pub fn find_constructor_declaration(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    node_data::members_of(store, node)
        .iter()
        .copied()
        .find(|&member| {
            is_constructor_declaration(store.node(member))
                && tsc_ast::node_is_present(store.node(body_of_or_missing(store, member)))
        })
}

fn body_of_or_missing(store: &dyn NodeStore, member: NodeId) -> NodeId {
    node_data::body_of(store, member).unwrap_or_default()
}

/// Go: `func ModuleExportNameIsDefault(node *Node) bool`.
pub fn module_export_name_is_default(store: &dyn NodeStore, node: NodeId) -> bool {
    node_text(store, node) == INTERNAL_SYMBOL_NAME_DEFAULT
}

// ────────────────────────────────────────────────────────────────────────────
// require() (utilities.go:2828-2930)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsRequireCall(node *Node, requireStringLiteralLikeArgument bool) bool`.
pub fn is_require_call(
    store: &dyn NodeStore,
    node: NodeId,
    require_string_literal_like_argument: bool,
) -> bool {
    let n = store.node(node);
    if !is_call_expression(n) {
        return false;
    }
    let call = n.as_call_expression().expect("CallExpression data");
    let expression = call.expression;
    if !is_identifier(store.node(expression)) || node_text(store, expression) != "require" {
        return false;
    }
    let args: &[NodeId] = call.arguments.as_ref().map(|l| l.nodes.as_ref()).unwrap_or(&[]);
    if args.len() != 1 {
        return false;
    }
    !require_string_literal_like_argument || is_string_literal_like(store.node(args[0]))
}

// Of the form: `const x = require("x")` or `const { x } = require("x")` or with `var` or `let`
// The variable must not be exported and must not have a type annotation, even a jsdoc one.
// The initializer must be a call to `require` with a string literal or a string literal-like argument.
/// Go: `func IsVariableDeclarationInitializedToRequire(node *Node) bool`.
pub fn is_variable_declaration_initialized_to_require(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let mut node = node;
    if is_binding_element(store.node(node)) {
        let parent = parent_of(store, node).expect("BindingElement parent");
        node = parent_of(store, parent).expect("BindingElement grandparent");
    }
    is_variable_declaration_initialized_with_require_helper(store, node, false /*allowAccessedRequire*/)
}

/// Go: `func isVariableDeclarationInitializedWithRequireHelper(node *Node, allowAccessedRequire bool) bool`.
fn is_variable_declaration_initialized_with_require_helper(
    store: &dyn NodeStore,
    node: NodeId,
    allow_accessed_require: bool,
) -> bool {
    if !tsc_ast::utilities::is_in_js_file(store.node(node)) {
        return false;
    }
    if store.node(node).kind != Kind::VariableDeclaration {
        return false;
    }
    let Some(mut initializer) = node_data::initializer_of(store, node) else {
        return false;
    };
    if allow_accessed_require {
        // Go: GetLeftmostAccessExpression — only reachable on the
        // allowAccessedRequire path, which the binder never takes.
        while is_access_expression(store.node(initializer)) {
            initializer = node_data::expression_of(store, initializer).expect("access expression");
        }
    }
    let parent = parent_of(store, node).expect("VariableDeclaration parent");
    let grand = parent_of(store, parent).expect("VariableDeclarationList parent");
    // PORT: the Go conjunction is `node.Parent.Parent.ModifierFlags()&
    // ModifierFlagsExport == 0 && node.Type() == nil && IsRequireCall(...)`.
    !store.node(grand).modifier_flags().intersects(ModifierFlags::EXPORT)
        && node_data::type_of(store, node).is_none()
        && is_require_call(store, initializer, true /*requireStringLiteralLikeArgument*/)
}

// ────────────────────────────────────────────────────────────────────────────
// Expandos / executability (utilities.go:4209-4340)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsExpandoInitializer(declaration *Node, initializer *Node) bool`.
pub fn is_expando_initializer(store: &dyn NodeStore, declaration: NodeId, initializer: Option<NodeId>) -> bool {
    let Some(initializer) = initializer else {
        return false;
    };
    if is_function_expression_or_arrow_function(store.node(initializer)) {
        return true;
    }
    if tsc_ast::utilities::is_in_js_file(store.node(initializer)) {
        let n = store.node(initializer);
        if is_class_expression(n) {
            return true;
        }
        if is_object_literal_expression(n) {
            let properties = n
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression data")
                .properties
                .as_ref()
                .map(|l| l.nodes.len())
                .unwrap_or(0);
            return properties == 0 && node_data::type_of(store, declaration).is_none();
        }
    }
    false
}

/// Go: `func IsImplicitlyExportedJSDocDeclaration(node *Node) bool`.
pub fn is_implicitly_exported_jsdoc_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    let Some(parent) = parent_of(store, node) else {
        return false;
    };
    if store.node(parent).kind != Kind::SourceFile
        || !is_external_or_common_js_module(store.file(parent.file_id()))
    {
        return false;
    }
    if store.node(node).kind == Kind::JSTypeAliasDeclaration {
        return true;
    }
    // A reparsed ModuleDeclaration synthesized from a JSDoc @typedef/@callback
    // dotted name should also be treated as implicitly exported in modules.
    is_module_declaration(store.node(node)) && store.node(node).flags.intersects(NodeFlags::REPARSED)
}

/// Go: `func IsPotentiallyExecutableNode(node *Node) bool` — true for nodes
/// that are considered executable for the purposes of unreachable code
/// detection.
pub fn is_potentially_executable_node(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if (Kind::FirstStatement..=Kind::LastStatement).contains(&n.kind) {
        if is_variable_statement(n) {
            let declaration_list = n
                .as_variable_statement()
                .expect("VariableStatement data")
                .declaration_list;
            if get_combined_node_flags(store, declaration_list).intersects(NodeFlags::BLOCK_SCOPED) {
                return true;
            }
            let declarations = match &store.node(declaration_list).data {
                NodeData::VariableDeclarationList(d) => d
                    .declarations
                    .as_ref()
                    .map(|list| list.nodes.as_ref())
                    .unwrap_or(&[]),
                _ => &[],
            };
            return declarations
                .iter()
                .any(|&d| node_data::initializer_of(store, d).is_some());
        }
        return true;
    }
    is_class_declaration(n) || is_enum_declaration(n) || is_module_declaration(n)
}

// ────────────────────────────────────────────────────────────────────────────
// Misc (utilities.go:4507)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsAsyncFunction(node *Node) bool`.
pub fn is_async_function(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::FunctionDeclaration | Kind::FunctionExpression | Kind::ArrowFunction | Kind::MethodDeclaration => {
            let body = node_data::body_of(store, node);
            body.is_some()
                && node_data::asterisk_token_of(store, node).is_none()
                && tsc_ast::has_syntactic_modifier(n, ModifierFlags::ASYNC)
        }
        _ => false,
    }
}
