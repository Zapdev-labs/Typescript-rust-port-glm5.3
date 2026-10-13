// Go ast/scanner-package helpers the checker utilities family needs that no
// landed crate exports yet (same pattern as the binder crate's ast_shims.rs;
// every item names its Go home and is a dedup candidate — fold on the
// ast/utilities.go wholesale wave).

use tsc_ast::{
    is_binding_element, is_parenthesized_expression, is_qualified_name, Kind,
    ModifierFlags, Node, NodeId, NodeFlags, NodeStore,
};

use crate::ast_shims::outer::{is_outer_expression};

/// Go: `func HasModifier(node *Node, flags ModifierFlags) bool`
/// (ast/utilities.go). Distinct from `HasSyntacticModifier`, which masks
/// `ModifierFlagsSyntacticModifiers` first.
pub fn has_modifier(node: &Node, flags: ModifierFlags) -> bool {
    node.modifier_flags().intersects(flags)
}

/// Go: the generated `nodeData.Initializer()` accessor family (ast.go)
/// — reached through the store (SPEC §5.1: handles, not `*Node`).
/// The variant set is exactly the payloads carrying an `initializer` field;
/// `NodeDefault` returns nil, mirrored by the `_` arm.
pub fn initializer_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::ForStatement(d) => d.initializer,
        tsc_ast::NodeData::ForInOrOfStatement(d) => Some(d.initializer),
        tsc_ast::NodeData::VariableDeclaration(d) => d.initializer,
        tsc_ast::NodeData::ParameterDeclaration(d) => d.initializer,
        tsc_ast::NodeData::BindingElement(d) => d.initializer,
        tsc_ast::NodeData::EnumMember(d) => d.initializer,
        // PORT (M6 S3): PropertySignatureDeclaration.initializer is a
        // NodeId sentinel (not Option); NodeId::NONE is Go's nil.
        tsc_ast::NodeData::PropertySignatureDeclaration(d) => {
            (d.initializer != NodeId::NONE).then_some(d.initializer)
        }
        tsc_ast::NodeData::PropertyDeclaration(d) => d.initializer,
        tsc_ast::NodeData::PropertyAssignment(d) => Some(d.initializer),
        tsc_ast::NodeData::JsxAttribute(d) => d.initializer,
        _ => None,
    }
}

/// Go: the generated `nodeData.Expression()` accessor family (ast.go) —
/// same store-carrying pattern as [`initializer_of`].
pub fn expression_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::ComputedPropertyName(d) => Some(d.expression),
        tsc_ast::NodeData::Decorator(d) => Some(d.expression),
        tsc_ast::NodeData::IfStatement(d) => Some(d.expression),
        tsc_ast::NodeData::DoStatement(d) => Some(d.expression),
        tsc_ast::NodeData::WhileStatement(d) => Some(d.expression),
        tsc_ast::NodeData::ForInOrOfStatement(d) => Some(d.expression),
        tsc_ast::NodeData::ReturnStatement(d) => d.expression,
        tsc_ast::NodeData::WithStatement(d) => Some(d.expression),
        tsc_ast::NodeData::SwitchStatement(d) => Some(d.expression),
        tsc_ast::NodeData::CaseOrDefaultClause(d) => Some(d.expression),
        tsc_ast::NodeData::ThrowStatement(d) => Some(d.expression),
        tsc_ast::NodeData::ExpressionStatement(d) => Some(d.expression),
        tsc_ast::NodeData::ExternalModuleReference(d) => Some(d.expression),
        tsc_ast::NodeData::ExportAssignment(d) => Some(d.expression),
        tsc_ast::NodeData::YieldExpression(d) => d.expression,
        tsc_ast::NodeData::AsExpression(d) => Some(d.expression),
        tsc_ast::NodeData::SatisfiesExpression(d) => Some(d.expression),
        tsc_ast::NodeData::PropertyAccessExpression(d) => Some(d.expression),
        tsc_ast::NodeData::ElementAccessExpression(d) => Some(d.expression),
        tsc_ast::NodeData::CallExpression(d) => Some(d.expression),
        tsc_ast::NodeData::NewExpression(d) => Some(d.expression),
        tsc_ast::NodeData::NonNullExpression(d) => Some(d.expression),
        tsc_ast::NodeData::SpreadElement(d) => Some(d.expression),
        tsc_ast::NodeData::TemplateSpan(d) => Some(d.expression),
        tsc_ast::NodeData::ParenthesizedExpression(d) => Some(d.expression),
        tsc_ast::NodeData::SpreadAssignment(d) => Some(d.expression),
        tsc_ast::NodeData::DeleteExpression(d) => Some(d.expression),
        tsc_ast::NodeData::TypeOfExpression(d) => Some(d.expression),
        tsc_ast::NodeData::VoidExpression(d) => Some(d.expression),
        tsc_ast::NodeData::AwaitExpression(d) => Some(d.expression),
        tsc_ast::NodeData::TypeAssertion(d) => Some(d.expression),
        tsc_ast::NodeData::ExpressionWithTypeArguments(d) => Some(d.expression),
        tsc_ast::NodeData::PartiallyEmittedExpression(d) => Some(d.expression),
        tsc_ast::NodeData::JsxSpreadAttribute(d) => Some(d.expression),
        tsc_ast::NodeData::JsxExpression(d) => d.expression,
        tsc_ast::NodeData::SyntheticReferenceExpression(d) => Some(d.expression),
        _ => None,
    }
}

/// Go: `func IsQuestionToken(node *Node) bool` (ast/utilities.go).
pub fn is_question_token(node: &Node) -> bool {
    node.kind == Kind::QuestionToken
}

/// Go: `func (n *Node) PostfixToken() *SyntaxToken` (ast.go) — the kinds that
/// carry a `postfixToken` field.
pub fn postfix_token_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::PropertySignatureDeclaration(d) => d.postfix_token,
        tsc_ast::NodeData::PropertyAssignment(d) => d.postfix_token,
        tsc_ast::NodeData::PropertyDeclaration(d) => d.postfix_token,
        tsc_ast::NodeData::EnumMember(d) => d.postfix_token,
        tsc_ast::NodeData::GetAccessorDeclaration(d) => d.postfix_token,
        tsc_ast::NodeData::SetAccessorDeclaration(d) => d.postfix_token,
        _ => None,
    }
}

/// Go: `func (n *Node) QuestionToken() *TokenNode` (ast.go) — explicit fields
/// for Parameter/ConditionalExpression/MappedType/NamedTupleMember, else the
/// PostfixToken when it is a question token.
pub fn question_token_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::ParameterDeclaration(d) => d.question_token,
        tsc_ast::NodeData::ConditionalExpression(d) => Some(d.question_token),
        tsc_ast::NodeData::MappedTypeNode(d) => d.question_token,
        tsc_ast::NodeData::NamedTupleMember(d) => d.question_token,
        _ => match postfix_token_of(store, node) {
            Some(p) if store.node(p).kind == Kind::QuestionToken => Some(p),
            _ => None,
        },
    }
}

/// Go: `func HasQuestionToken(node *Node) bool` (ast/utilities.go) —
/// `IsQuestionToken(node.QuestionToken())`.
pub fn has_question_token(store: &dyn NodeStore, node: NodeId) -> bool {
    question_token_of(store, node).is_some_and(|t| is_question_token(store.node(t)))
}

/// Go: `func IsVarConst(node *Node) bool` (ast/utilities.go) —
/// `GetCombinedNodeFlags(node)&NodeFlagsBlockScoped == NodeFlagsConst`.
pub fn is_var_const(store: &dyn NodeStore, node: NodeId) -> bool {
    let combined = tsc_binder::ast_shims::get_combined_node_flags(store, node);
    (combined & NodeFlags::BLOCK_SCOPED) == NodeFlags::CONST
}

/// Go: `func IsNullLiteral(node *Node) bool` / `IsUndefinedLiteral` — kind
/// guards (ast/utilities.go).
pub fn is_null_literal(node: &Node) -> bool {
    node.kind == Kind::NullKeyword
}

pub fn is_undefined_literal(node: &Node) -> bool {
    node.kind == Kind::UndefinedKeyword
}

/// Go: `func IsThisParameter(node *Node) bool` — the parameter's name is the
/// `this` keyword.
pub fn is_this_parameter(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    n.kind == Kind::Parameter && n.name().is_some_and(|name| store.node(name).kind == Kind::ThisKeyword)
}

/// Go: `type FindAncestorResult int` + `FindAncestorFalse/True/Quit` (ast).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindAncestorResult {
    False,
    True,
    Quit,
}

/// Go: `func ToFindAncestorResult(b bool) FindAncestorResult`.
pub fn to_find_ancestor_result(b: bool) -> FindAncestorResult {
    if b {
        FindAncestorResult::True
    } else {
        FindAncestorResult::False
    }
}

/// Go: `func FindAncestorOrQuit(node *Node, callback) *Node` (ast/utilities.go)
/// — walks up the parents of a node to find the ancestor that matches the
/// callback.
pub fn find_ancestor_or_quit(
    store: &dyn NodeStore,
    node: NodeId,
    callback: &mut dyn FnMut(&Node) -> FindAncestorResult,
) -> Option<NodeId> {
    let mut node = node;
    loop {
        let n = store.node(node);
        match callback(n) {
            FindAncestorResult::Quit => return None,
            FindAncestorResult::True => return Some(node),
            FindAncestorResult::False => {}
        }
        let parent = n.parent.get();
        if parent.is_none() {
            return None;
        }
        node = parent;
    }
}

/// Go: `func FindAncestor(node *Node, callback func(*Node) bool) *Node`.
pub fn find_ancestor(
    store: &dyn NodeStore,
    node: NodeId,
    callback: &mut dyn FnMut(&Node) -> bool,
) -> Option<NodeId> {
    let mut node = node;
    loop {
        let n = store.node(node);
        if callback(n) {
            return Some(node);
        }
        let parent = n.parent.get();
        if parent.is_none() {
            return None;
        }
        node = parent;
    }
}

/// Go: `func WalkUpParenthesizedExpressions(node *Expression) *Node`
/// (ast/utilities.go) — walks up the parents of a parenthesized expression to
/// find the containing node.
pub fn walk_up_parenthesized_expressions(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut node = node;
    while !node.is_none() && is_parenthesized_expression(store.node(node)) {
        node = store.node(node).parent.get();
    }
    if node.is_none() {
        None
    } else {
        Some(node)
    }
}

/// Go: `func WalkUpBindingElementsAndPatterns(binding *Node) *Node`
/// (ast/utilities.go) — note the Go body's double-parent hop (binding element →
/// its pattern) and that it returns the *parent* of the outermost element.
pub fn walk_up_binding_elements_and_patterns(store: &dyn NodeStore, binding: NodeId) -> NodeId {
    let mut node = store.node(binding).parent.get();
    while is_binding_element(store.node(store.node(node).parent.get())) {
        node = store.node(store.node(node).parent.get()).parent.get();
    }
    store.node(node).parent.get()
}

/// Go: scanner `OuterExpressionKinds` + `func IsOuterExpression(node, kinds)`
/// (ast/utilities.go). The store-carrying form resolves the operand kinds
/// (JSDoc type assertions, assignment/comma tokens) that Go reads through
/// pointers.
pub mod outer {
    use tsc_ast::{Kind, NodeId, NodeStore};

    pub type OuterExpressionKinds = u16;

    pub const OEK_PARENTHESSES: OuterExpressionKinds = 1 << 0;
    pub const OEK_TYPE_ASSERTIONS: OuterExpressionKinds = 1 << 1;
    pub const OEK_NON_NULL_ASSERTIONS: OuterExpressionKinds = 1 << 2;
    pub const OEK_PARTIALLY_EMITTED_EXPRESSIONS: OuterExpressionKinds = 1 << 3;
    pub const OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS: OuterExpressionKinds = 1 << 4;
    pub const OEK_SATISFIES: OuterExpressionKinds = 1 << 5;
    pub const OEK_EXCLUDE_JSDOC_TYPE_ASSERTION: OuterExpressionKinds = 1 << 6;
    pub const OEK_ASSIGNMENTS: OuterExpressionKinds = 1 << 7;
    pub const OEK_COMMA: OuterExpressionKinds = 1 << 8;

    pub const OEK_ASSERTIONS: OuterExpressionKinds =
        OEK_TYPE_ASSERTIONS | OEK_NON_NULL_ASSERTIONS | OEK_SATISFIES;
    pub const OEK_ALL: OuterExpressionKinds =
        OEK_PARENTHESSES | OEK_ASSERTIONS | OEK_PARTIALLY_EMITTED_EXPRESSIONS | OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS;
    pub const OEK_ALL_EXCEPT_ASSERTIONS_OR_EXPRESSIONS_WITH_TYPE_ARGUMENTS: OuterExpressionKinds =
        OEK_ALL & !OEK_ASSERTIONS & !OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS;
    pub const OEK_EXPRESSION_TYPE_PASSTHROUGH: OuterExpressionKinds =
        OEK_PARENTHESSES | OEK_ASSIGNMENTS | OEK_COMMA;

    /// Go: `func IsOuterExpression(node *Expression, kinds OuterExpressionKinds) bool`.
    pub fn is_outer_expression(store: &dyn NodeStore, node: NodeId, kinds: OuterExpressionKinds) -> bool {
        let n = store.node(node);
        match n.kind {
            Kind::ParenthesizedExpression => {
                kinds & OEK_PARENTHESSES != 0
                    && !(kinds & OEK_EXCLUDE_JSDOC_TYPE_ASSERTION != 0 && is_jsdoc_type_assertion(store, node))
            }
            Kind::TypeAssertionExpression | Kind::AsExpression => kinds & OEK_TYPE_ASSERTIONS != 0,
            Kind::SatisfiesExpression => kinds & (OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS | OEK_SATISFIES) != 0,
            Kind::ExpressionWithTypeArguments => kinds & OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS != 0,
            Kind::NonNullExpression => kinds & OEK_NON_NULL_ASSERTIONS != 0,
            Kind::PartiallyEmittedExpression => kinds & OEK_PARTIALLY_EMITTED_EXPRESSIONS != 0,
            Kind::BinaryExpression => {
                let d = n.as_binary_expression().expect("BinaryExpression data");
                match store.node(d.operator_token).kind {
                    Kind::EqualsToken => kinds & OEK_ASSIGNMENTS != 0,
                    Kind::CommaToken => kinds & OEK_COMMA != 0,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Go: `func IsJSDocTypeAssertion(node *Node) bool` — a parenthesized
    /// expression wrapping a JSDocTypeExpression.
    fn is_jsdoc_type_assertion(store: &dyn NodeStore, node: NodeId) -> bool {
        let n = store.node(node);
        match &n.data {
            tsc_ast::NodeData::ParenthesizedExpression(d) => {
                store.node(d.expression).kind == Kind::JSDocTypeExpression
            }
            _ => false,
        }
    }
}

/// Go: checker/utilities.go `walkUpOuterExpressions` helper — walks up while
/// the parent is an outer expression of `OEKAll` kinds. Defined here with the
/// shims because it is the only IsOuterExpression consumer in the ported
/// surface; checker-side it is re-exported as `walk_up_outer_expressions`.
pub fn walk_up_outer_expressions(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut parent = store.node(node).parent.get();
    while !parent.is_none() && is_outer_expression(store, parent, outer::OEK_ALL) {
        parent = store.node(parent).parent.get();
    }
    if parent.is_none() {
        None
    } else {
        Some(parent)
    }
}

/// Go: `func EntityNameToString(name *Node, getTextOfNode func(*Node) string) string`
/// (ast/utilities.go) — with scanner.GetTextOfNode wired in (the checker's
/// `entityNameToString` passes exactly that).
pub fn entity_name_to_string(store: &dyn NodeStore, name: NodeId) -> String {
    let n = store.node(name);
    match n.kind {
        Kind::ThisKeyword => "this".to_string(),
        Kind::Identifier | Kind::PrivateIdentifier => {
            // Go: NodeIsSynthesized(name) || getTextOfNode == nil → name.Text();
            // else getTextOfNode(name). Parsed files are never synthesized,
            // and getTextOfNode slices the source text — the port routes
            // through it exactly like the checker's Go call site.
            if n.flags.intersects(NodeFlags::SYNTHESIZED) {
                tsc_ast::node_text(store, name).to_string()
            } else {
                get_text_of_node(store, name)
            }
        }
        Kind::QualifiedName => {
            let d = n.as_qualified_name().expect("QualifiedName data");
            format!(
                "{}.{}",
                entity_name_to_string(store, d.left),
                entity_name_to_string(store, d.right)
            )
        }
        Kind::PropertyAccessExpression => {
            let d = n.as_property_access_expression().expect("PropertyAccessExpression data");
            format!(
                "{}.{}",
                entity_name_to_string(store, d.expression),
                entity_name_to_string(store, d.name)
            )
        }
        Kind::JsxNamespacedName => tsc_binder::ast_shims::jsx_namespaced_name_text(store, name),
        _ => panic!("Unhandled case in EntityNameToString: {}", n.kind_string()),
    }
}

/// Go: `(*Node).Text()` read on a JsxAttribute's name — the jsx.go wave's
/// `attribute.Name().Text()` sites (jsx.go:224/299). The JsxNamespacedName
/// arm routes through the binder's ported `Text()` arm
/// (`jsx_namespaced_name_text`, "ns:name"); identifiers fall through to
/// `node_text`.
pub fn jsx_name_text(store: &dyn NodeStore, node: NodeId) -> String {
    tsc_binder::ast_shims::jsx_namespaced_name_text(store, node)
}

/// Go: `func GetTextOfNode(node *ast.Node) string` (scanner/utilities.go) —
/// the includeTrivia=false path. The JSDoc source normalization and
/// reparser-transformed-literal arms are unreachable for the spike's parsed
/// sources (TODO(port): fold into the scanner/utilities.go wholesale port).
pub fn get_text_of_node(store: &dyn NodeStore, node: NodeId) -> String {
    let n = store.node(node);
    if tsc_ast::utilities::node_is_missing(n) {
        return String::new();
    }
    let file =
        tsc_ast::utilities::get_source_file_of_node(store, node).expect("node with a source file");
    let text = file.text();
    let pos = tsc_scanner::skip_trivia(text, n.pos());
    let end = n.end();
    if (pos as usize) > end as usize {
        // Go slices text[pos:end], which panics; keep the panic.
        panic!("GetTextOfNode: pos {pos} > end {end}");
    }
    text[pos as usize..end as usize].to_string()
}

/// Go: `func GetPropertyNameForPropertyNameNode(name *Node) string`
/// (ast/utilities.go).
pub fn get_property_name_for_property_name_node(store: &dyn NodeStore, name: NodeId) -> String {
    let n = store.node(name);
    match n.kind {
        Kind::Identifier
        | Kind::PrivateIdentifier
        | Kind::StringLiteral
        | Kind::NoSubstitutionTemplateLiteral
        | Kind::NumericLiteral
        | Kind::BigIntLiteral
        | Kind::JsxNamespacedName => tsc_ast::node_text(store, name).to_string(),
        Kind::ComputedPropertyName => {
            let d = n.as_computed_property_name().expect("ComputedPropertyName data");
            let name_expression = d.expression;
            let ne = store.node(name_expression);
            if tsc_ast::utilities::is_string_or_numeric_literal_like(ne) {
                tsc_ast::node_text(store, name_expression).to_string()
            } else if tsc_binder::ast_shims::is_signed_numeric_literal(store, name_expression) {
                let d = ne.as_prefix_unary_expression().expect("PrefixUnaryExpression data");
                let mut text = tsc_ast::node_text(store, d.operand).to_string();
                if d.operator == Kind::MinusToken {
                    text = format!("-{text}");
                }
                text
            } else {
                // PORT(FCS tranche 5): Go's ast/utilities.go returns
                // `InternalSymbolNameMissing` ("\u{FE}missing") here, NOT the
                // empty string — the S-era shim returned `String::new()`,
                // which `getEffectivePropertyNameForPropertyNameNode`'s
                // `name != INTERNAL_SYMBOL_NAME_MISSING` gate read as a REAL
                // (empty) property name: every non-literal-like computed name
                // then collided on the "" key in
                // `checkGrammarObjectLiteralExpression`'s seen map, emitting
                // the phantom Duplicate_identifier_2300 family
                // (computedPropertyNames10/11/12... object-literal methods).
                tsc_ast::INTERNAL_SYMBOL_NAME_MISSING.to_string()
            }
        }
        _ => panic!(
            "Unhandled case in GetPropertyNameForPropertyNameNode: {}",
            n.kind_string()
        ),
    }
}

/// Go: `func IsNameOfHeritageClauseTypeReference(node *Node) bool`
/// (ast/utilities.go).
pub fn is_name_of_heritage_clause_type_reference(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut node = node;
    while is_qualified_name(store.node(store.node(node).parent.get())) {
        node = store.node(node).parent.get();
    }
    let parent = store.node(node).parent.get();
    let parent_node = store.node(parent);
    let is_type_ref_name = parent_node.kind == Kind::TypeReference
        && parent_node
            .as_type_reference_node()
            .expect("TypeReferenceNode data")
            .type_name
            == node;
    is_type_ref_name && store.node(parent_node.parent.get()).kind == Kind::HeritageClause
}

/// Go: `func (n *Node) Type() *TypeNode` — the generated nodeData dispatcher,
/// partial surface (the declaration kinds that carry a `type` annotation field
/// and are reachable from the checker's `hasType`/`tryGetTypeFromTypeNode`
/// paths). TODO(port): complete on the ast/utilities.go wholesale wave.
pub fn type_of_node(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::VariableDeclaration(d) => d.type_,
        tsc_ast::NodeData::ParameterDeclaration(d) => d.type_,
        tsc_ast::NodeData::PropertyDeclaration(d) => d.type_,
        // PORT(FCS tranche 3): the plain-NodeId `type_` fields (Go *Node
        // pointers) carry NodeId::NONE when the annotation is absent — Go's
        // `Node.Type()` returns nil there, so the Some() arms filter the
        // sentinel (e.g. the shorthand `[Symbol.iterator]: { x; }`
        // PropertySignature: checkSourceElement(Some(NONE)) was panicking
        // via checkVariableLikeDeclaration).
        tsc_ast::NodeData::PropertySignatureDeclaration(d) => {
            (d.type_ != NodeId::NONE).then_some(d.type_)
        }
        tsc_ast::NodeData::FunctionDeclaration(d) => d.type_,
        tsc_ast::NodeData::FunctionExpression(d) => d.type_,
        tsc_ast::NodeData::ArrowFunction(d) => d.type_,
        tsc_ast::NodeData::MethodDeclaration(d) => d.type_,
        tsc_ast::NodeData::MethodSignatureDeclaration(d) => d.type_,
        tsc_ast::NodeData::GetAccessorDeclaration(d) => d.type_,
        tsc_ast::NodeData::SetAccessorDeclaration(d) => d.type_,
        tsc_ast::NodeData::TypeAliasDeclaration(d) => (d.type_ != NodeId::NONE).then_some(d.type_),
        tsc_ast::NodeData::CallSignatureDeclaration(d) => d.type_,
        tsc_ast::NodeData::ConstructSignatureDeclaration(d) => d.type_,
        tsc_ast::NodeData::IndexSignatureDeclaration(d) => d.type_,
        tsc_ast::NodeData::JSDocTypeExpression(d) => (d.type_ != NodeId::NONE).then_some(d.type_),
        // Go: Node.Type()'s default arm — `funcLike := n.FunctionLikeData();
        // return funcLike.Type` (ast.go:679-681). The W4 nodebuilder's
        // serializeReturnTypeForSignature reaches these via
        // getReturnTypeOfSignature → getReturnTypeFromAnnotation for
        // FunctionTypeNode/ConstructorTypeNode-declared signatures; the
        // remaining Node.Type() kinds (PropertyAssignment, TypePredicate,
        // ParenthesizedType, TypeOperator, MappedType, JSDoc tags, …) land
        // with the ast/utilities.go wholesale wave (PORT note).
        tsc_ast::NodeData::FunctionTypeNode(d) => d.type_,
        tsc_ast::NodeData::ConstructorTypeNode(d) => d.type_,
        // Go: `node.Type() on the assertion family (checker.go's
        // checkAssertion/checkSatisfiesExpression call sites).
        // (Same PORT(FCS tranche 3) sentinel filter as the declaration
        // arms above — these types are also plain-NodeId fields.)
        tsc_ast::NodeData::TypeAssertion(d) => (d.type_ != NodeId::NONE).then_some(d.type_),
        tsc_ast::NodeData::AsExpression(d) => (d.type_ != NodeId::NONE).then_some(d.type_),
        tsc_ast::NodeData::SatisfiesExpression(d) => (d.type_ != NodeId::NONE).then_some(d.type_),
        _ => None,
    }
}

/// Go: `func GetNextJSDocCommentLocation(node *Node) *Node`
/// (ast/utilities.go:4131) — "Finds the declaration that owns the JSDoc for a
/// function-like node. Keep these hosts aligned with JSDoc parameter
/// reparsing so unmatched @param diagnostics use the same attachment rules."
/// (Go comment, verbatim.)
///
/// Needed by checker/jsdoc.go's `getAllJSDocTags` (M6 W1); dedup candidate on
/// the ast/utilities.go wholesale wave.
pub fn get_next_jsdoc_comment_location(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let parent = store.node(node).parent.get();
    if parent == NodeId::NONE {
        return None;
    }
    let p = store.node(parent);
    match p.kind {
        Kind::PropertyAssignment
        | Kind::ExportAssignment
        | Kind::PropertyDeclaration
        | Kind::VariableDeclaration
        | Kind::SatisfiesExpression
        | Kind::ReturnStatement
        | Kind::VariableStatement
        | Kind::ExpressionStatement => Some(parent),
        Kind::VariableDeclarationList => {
            // Go: `if parent.AsVariableDeclarationList().Declarations.Nodes[0] == node`.
            let declarations = &p
                .as_variable_declaration_list()
                .expect("VariableDeclarationList data")
                .declarations;
            if declarations
                .as_ref()
                .and_then(|d| d.nodes.first())
                == Some(&node)
            {
                Some(parent)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Go: the generated `nodeData.Parameters()` accessor family (ast.go) — the
/// parameter slice of a function-like node (empty when the list is absent,
/// mirroring Go's nil slice).
///
/// Needed by checker/jsdoc.go's `checkUnmatchedJSDocParameters` (M6 W1);
/// dedup candidate with the parser crate's `reparser.rs::parameters_of`
/// (identical Go source) on the ast/utilities.go wholesale wave.
pub fn parameters_of(store: &dyn NodeStore, fun: NodeId) -> &[NodeId] {
    fn list(parameters: &Option<tsc_ast::NodeList>) -> &[NodeId] {
        parameters.as_ref().map(|l| &l.nodes[..]).unwrap_or(&[])
    }
    match &store.node(fun).data {
        tsc_ast::NodeData::FunctionDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::MethodDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::ConstructorDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::GetAccessorDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::SetAccessorDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::FunctionExpression(d) => list(&d.parameters),
        tsc_ast::NodeData::ArrowFunction(d) => list(&d.parameters),
        tsc_ast::NodeData::FunctionTypeNode(d) => list(&d.parameters),
        // PORT (M6 S3): interface method signatures flow here via
        // checkSignatureDeclaration (Go reads them through the same
        // SignatureDeclaration interface).
        tsc_ast::NodeData::MethodSignatureDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::CallSignatureDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::ConstructSignatureDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::IndexSignatureDeclaration(d) => list(&d.parameters),
        tsc_ast::NodeData::ConstructorTypeNode(d) => list(&d.parameters),
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: the generated `nodeData.ModuleSpecifier()` accessor family (ast.go)
/// — the import/export declaration kinds that carry a module specifier.
pub fn module_specifier_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        tsc_ast::NodeData::ImportDeclaration(d) => Some(d.module_specifier),
        tsc_ast::NodeData::ExportDeclaration(d) => d.module_specifier,
        _ => None,
    }
}
