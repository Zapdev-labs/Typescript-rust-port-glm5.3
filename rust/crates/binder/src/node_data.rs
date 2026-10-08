// Go ast interface dispatchers, binder-side (M4 wave A).
//
// Go's `*Node` methods that dispatch into `nodeData` interfaces —
// `FlowNodeData()`, `DeclarationData()`, `LocalsContainerData()`, `BodyData()`,
// `ExportableData()` and the `Expression()`/`Type()`/`Initializer()`/... field
// dispatchers — have no Rust counterpart on the frozen generated
// `NodeData` enum. This module carries the binder's dispatch shims over the
// same payloads (one match per Go interface method). Each mirrors its Go
// behavior exactly: a payload without the field reads `None`/false (Go's
// NodeDefault returns nil), and setters no-op where Go's nil-interface check
// would skip.
//
// PORT: dedup candidate — fold into the ast crate when the NodeData
// dispatch surface lands there (the M4 wave-A constraint scopes edits to
// rust/crates/binder/** plus ast/src/flow.rs).

use tsc_ast::{FlowNodeId, Node, NodeData, NodeId, NodeStore, SymbolId, SymbolTable};

/// Expands a get/set pair over every `NodeData` variant whose payload carries
/// `pub $field: Option<$ty>` (plus `Direct` entries for non-`Option` fields).
macro_rules! field_dispatch {
    (
        $get:ident, $set:ident, $field:ident, $ty:ty:
            opt [$($variant_opt:ident),* $(,)?]
            direct [$($variant_direct:ident),* $(,)?]
    ) => {
        pub fn $get(store: &dyn NodeStore, node: NodeId) -> Option<$ty> {
            match &store.node(node).data {
                $(NodeData::$variant_opt(d) => d.$field,)*
                $(NodeData::$variant_direct(d) => Some(d.$field),)*
                _ => None,
            }
        }

        pub fn $set(store: &mut dyn NodeStore, node: NodeId, value: Option<$ty>) {
            let n = store.node_mut(node);
            match &mut n.data {
                $(NodeData::$variant_opt(d) => d.$field = value,)*
                $(NodeData::$variant_direct(d) => d.$field = value.unwrap_or_default(),)*
                _ => {} // Go: FlowNodeData()/DeclarationData()/... returned nil — no-op.
            }
        }
    };
}

/// Expands a read-only dispatcher (the binder never writes these fields).
macro_rules! field_get {
    (
        $get:ident, $field:ident, $ty:ty:
            opt [$($variant_opt:ident),* $(,)?]
            direct [$($variant_direct:ident),* $(,)?]
    ) => {
        pub fn $get(store: &dyn NodeStore, node: NodeId) -> Option<$ty> {
            match &store.node(node).data {
                $(NodeData::$variant_opt(d) => d.$field,)*
                $(NodeData::$variant_direct(d) => Some(d.$field),)*
                _ => None,
            }
        }
    };
}

// ────────────────────────────────────────────────────────────────────────────
// FlowNodeData() — the `flow_node` payload field (Go FlowNodeBase)
// ────────────────────────────────────────────────────────────────────────────

field_dispatch!(flow_node_of, set_flow_node, flow_node, FlowNodeId: opt [
    Identifier, QualifiedName, EmptyStatement, IfStatement, DoStatement, WhileStatement,
    ForStatement, ForInOrOfStatement, BreakStatement, ContinueStatement, ReturnStatement,
    WithStatement, SwitchStatement, ThrowStatement, TryStatement, DebuggerStatement,
    LabeledStatement, ExpressionStatement, Block, VariableStatement, BindingElement,
    MissingDeclaration, FunctionDeclaration, ClassDeclaration, InterfaceDeclaration,
    TypeAliasDeclaration, EnumDeclaration, ModuleBlock, NotEmittedStatement, ImportDeclaration,
    ExportAssignment, NamespaceExportDeclaration, GetAccessorDeclaration, SetAccessorDeclaration,
    MethodDeclaration, KeywordExpression, ArrowFunction, FunctionExpression,
    PropertyAccessExpression, ElementAccessExpression, MetaProperty, ModuleDeclaration,
    ImportEqualsDeclaration, ExportDeclaration,
] direct []);

/// Go: `if data := node.FlowNodeData(); data != nil` — the node carries a
/// flow-node slot at all (payload match; the KeywordExpression payload
/// covers the this/super/true/false/null/import keyword kinds at once).
pub fn has_flow_node_data(node: &Node) -> bool {
    matches!(
        &node.data,
        NodeData::Identifier(_)
            | NodeData::QualifiedName(_)
            | NodeData::EmptyStatement(_)
            | NodeData::IfStatement(_)
            | NodeData::DoStatement(_)
            | NodeData::WhileStatement(_)
            | NodeData::ForStatement(_)
            | NodeData::ForInOrOfStatement(_)
            | NodeData::BreakStatement(_)
            | NodeData::ContinueStatement(_)
            | NodeData::ReturnStatement(_)
            | NodeData::WithStatement(_)
            | NodeData::SwitchStatement(_)
            | NodeData::ThrowStatement(_)
            | NodeData::TryStatement(_)
            | NodeData::DebuggerStatement(_)
            | NodeData::LabeledStatement(_)
            | NodeData::ExpressionStatement(_)
            | NodeData::Block(_)
            | NodeData::ModuleBlock(_)
            | NodeData::VariableStatement(_)
            | NodeData::BindingElement(_)
            | NodeData::MissingDeclaration(_)
            | NodeData::FunctionDeclaration(_)
            | NodeData::ClassDeclaration(_)
            | NodeData::InterfaceDeclaration(_)
            | NodeData::TypeAliasDeclaration(_)
            | NodeData::EnumDeclaration(_)
            | NodeData::NotEmittedStatement(_)
            | NodeData::ImportDeclaration(_)
            | NodeData::ExportAssignment(_)
            | NodeData::NamespaceExportDeclaration(_)
            | NodeData::GetAccessorDeclaration(_)
            | NodeData::SetAccessorDeclaration(_)
            | NodeData::MethodDeclaration(_)
            | NodeData::KeywordExpression(_)
            | NodeData::ArrowFunction(_)
            | NodeData::FunctionExpression(_)
            | NodeData::PropertyAccessExpression(_)
            | NodeData::ElementAccessExpression(_)
            | NodeData::MetaProperty(_)
            | NodeData::ModuleDeclaration(_)
            | NodeData::ImportEqualsDeclaration(_)
            | NodeData::ExportDeclaration(_)
    )
}

// ────────────────────────────────────────────────────────────────────────────
// DeclarationData() — the `symbol` payload field (Go DeclarationBase)
// ────────────────────────────────────────────────────────────────────────────

field_dispatch!(symbol_of, set_node_symbol, symbol, SymbolId: opt [
    VariableDeclaration, ParameterDeclaration, BindingElement, MissingDeclaration,
    FunctionDeclaration, ClassDeclaration, ClassExpression, InterfaceDeclaration,
    TypeAliasDeclaration, EnumMember, EnumDeclaration, NotEmittedTypeElement,
    ImportDeclaration, NamespaceImport, ExportAssignment, NamespaceExportDeclaration,
    NamespaceExport, ExportSpecifier, CallSignatureDeclaration, ConstructSignatureDeclaration,
    ConstructorDeclaration, GetAccessorDeclaration, SetAccessorDeclaration,
    IndexSignatureDeclaration, MethodSignatureDeclaration, MethodDeclaration,
    PropertySignatureDeclaration, PropertyDeclaration, SemicolonClassElement,
    ClassStaticBlockDeclaration, NoSubstitutionTemplateLiteral, BinaryExpression,
    ArrowFunction, FunctionExpression, CallExpression, ObjectLiteralExpression,
    SpreadAssignment, PropertyAssignment, ShorthandPropertyAssignment, MappedTypeNode,
    TypeLiteralNode, NamedTupleMember, FunctionTypeNode, ConstructorTypeNode, JsxAttributes,
    JsxAttribute, JsxSpreadAttribute, JSDocSignature, ModuleDeclaration, ImportEqualsDeclaration,
    ExportDeclaration, ImportClause, ImportSpecifier, TypeParameterDeclaration,
    JSDocTypeLiteral, SourceFile,
] direct []);

// ExportableData() — the `local_symbol` payload field (Go ExportableBase).

field_dispatch!(local_symbol_of, set_local_symbol, local_symbol, SymbolId: opt [
    VariableDeclaration, BindingElement, FunctionDeclaration, ClassDeclaration,
    ClassExpression, InterfaceDeclaration, TypeAliasDeclaration, EnumDeclaration,
    NamespaceImport, ExportSpecifier, ModuleDeclaration, ImportEqualsDeclaration,
    ImportClause, ImportSpecifier, SourceFile,
] direct []);

// ────────────────────────────────────────────────────────────────────────────
// LocalsContainerData() — `locals` / `next_container` (Go LocalsContainerBase)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsLocalsContainer(node *Node) bool` (ast.go —
/// `node.LocalsContainerData() != nil`). The Rust payloads flatten the base,
/// so this is the same payload-kind set as the `locals` dispatchers below.
pub fn is_locals_container(node: &Node) -> bool {
    locals_of_node(node).is_some()
}

/// Go: `func (node *Node) Locals() SymbolTable` — the RAW accessor (no lazy
/// create; `GetLocals` in the binder creates). An empty table is Go's
/// nil map (the field is not lazily allocated in the Rust payload).
pub fn locals_of(store: &dyn NodeStore, node: NodeId) -> Option<&SymbolTable> {
    match &store.node(node).data {
        NodeData::SourceFile(d) => Some(&d.locals),
        NodeData::ForStatement(d) => Some(&d.locals),
        NodeData::ForInOrOfStatement(d) => Some(&d.locals),
        NodeData::CaseBlock(d) => Some(&d.locals),
        NodeData::CatchClause(d) => Some(&d.locals),
        NodeData::Block(d) => Some(&d.locals),
        NodeData::FunctionDeclaration(d) => Some(&d.locals),
        NodeData::ClassDeclaration(d) => Some(&d.locals),
        NodeData::ClassExpression(d) => Some(&d.locals),
        NodeData::TypeAliasDeclaration(d) => Some(&d.locals),
        NodeData::CallSignatureDeclaration(d) => Some(&d.locals),
        NodeData::ConstructSignatureDeclaration(d) => Some(&d.locals),
        NodeData::ConstructorDeclaration(d) => Some(&d.locals),
        NodeData::GetAccessorDeclaration(d) => Some(&d.locals),
        NodeData::SetAccessorDeclaration(d) => Some(&d.locals),
        NodeData::IndexSignatureDeclaration(d) => Some(&d.locals),
        NodeData::MethodSignatureDeclaration(d) => Some(&d.locals),
        NodeData::MethodDeclaration(d) => Some(&d.locals),
        NodeData::ClassStaticBlockDeclaration(d) => Some(&d.locals),
        NodeData::ArrowFunction(d) => Some(&d.locals),
        NodeData::FunctionExpression(d) => Some(&d.locals),
        NodeData::ConditionalTypeNode(d) => Some(&d.locals),
        NodeData::MappedTypeNode(d) => Some(&d.locals),
        NodeData::FunctionTypeNode(d) => Some(&d.locals),
        NodeData::ConstructorTypeNode(d) => Some(&d.locals),
        NodeData::JSDocSignature(d) => Some(&d.locals),
        NodeData::ModuleDeclaration(d) => Some(&d.locals),
        _ => None,
    }
}

fn locals_of_node(node: &Node) -> Option<()> {
    match &node.data {
        NodeData::SourceFile(_)
        | NodeData::ForStatement(_)
        | NodeData::ForInOrOfStatement(_)
        | NodeData::CaseBlock(_)
        | NodeData::CatchClause(_)
        | NodeData::Block(_)
        | NodeData::FunctionDeclaration(_)
        | NodeData::ClassDeclaration(_)
        | NodeData::ClassExpression(_)
        | NodeData::TypeAliasDeclaration(_)
        | NodeData::CallSignatureDeclaration(_)
        | NodeData::ConstructSignatureDeclaration(_)
        | NodeData::ConstructorDeclaration(_)
        | NodeData::GetAccessorDeclaration(_)
        | NodeData::SetAccessorDeclaration(_)
        | NodeData::IndexSignatureDeclaration(_)
        | NodeData::MethodSignatureDeclaration(_)
        | NodeData::MethodDeclaration(_)
        | NodeData::ClassStaticBlockDeclaration(_)
        | NodeData::ArrowFunction(_)
        | NodeData::FunctionExpression(_)
        | NodeData::ConditionalTypeNode(_)
        | NodeData::MappedTypeNode(_)
        | NodeData::FunctionTypeNode(_)
        | NodeData::ConstructorTypeNode(_)
        | NodeData::JSDocSignature(_)
        | NodeData::ModuleDeclaration(_) => Some(()),
        _ => None,
    }
}

/// Go: `GetSymbolTable(&container.LocalsContainerData().Locals)` — mutable
/// access with the lazy-create folded away (the Rust field is always
/// present; empty ≡ Go's nil map).
pub fn locals_mut(store: &mut dyn NodeStore, node: NodeId) -> Option<&mut SymbolTable> {
    let n = store.node_mut(node);
    match &mut n.data {
        NodeData::SourceFile(d) => Some(&mut d.locals),
        NodeData::ForStatement(d) => Some(&mut d.locals),
        NodeData::ForInOrOfStatement(d) => Some(&mut d.locals),
        NodeData::CaseBlock(d) => Some(&mut d.locals),
        NodeData::CatchClause(d) => Some(&mut d.locals),
        NodeData::Block(d) => Some(&mut d.locals),
        NodeData::FunctionDeclaration(d) => Some(&mut d.locals),
        NodeData::ClassDeclaration(d) => Some(&mut d.locals),
        NodeData::ClassExpression(d) => Some(&mut d.locals),
        NodeData::TypeAliasDeclaration(d) => Some(&mut d.locals),
        NodeData::CallSignatureDeclaration(d) => Some(&mut d.locals),
        NodeData::ConstructSignatureDeclaration(d) => Some(&mut d.locals),
        NodeData::ConstructorDeclaration(d) => Some(&mut d.locals),
        NodeData::GetAccessorDeclaration(d) => Some(&mut d.locals),
        NodeData::SetAccessorDeclaration(d) => Some(&mut d.locals),
        NodeData::IndexSignatureDeclaration(d) => Some(&mut d.locals),
        NodeData::MethodSignatureDeclaration(d) => Some(&mut d.locals),
        NodeData::MethodDeclaration(d) => Some(&mut d.locals),
        NodeData::ClassStaticBlockDeclaration(d) => Some(&mut d.locals),
        NodeData::ArrowFunction(d) => Some(&mut d.locals),
        NodeData::FunctionExpression(d) => Some(&mut d.locals),
        NodeData::ConditionalTypeNode(d) => Some(&mut d.locals),
        NodeData::MappedTypeNode(d) => Some(&mut d.locals),
        NodeData::FunctionTypeNode(d) => Some(&mut d.locals),
        NodeData::ConstructorTypeNode(d) => Some(&mut d.locals),
        NodeData::JSDocSignature(d) => Some(&mut d.locals),
        NodeData::ModuleDeclaration(d) => Some(&mut d.locals),
        _ => None,
    }
}

field_dispatch!(next_container_of, set_next_container, next_container, NodeId: opt [
    ForStatement, ForInOrOfStatement, CaseBlock, CatchClause, Block, FunctionDeclaration,
    ClassDeclaration, ClassExpression, TypeAliasDeclaration, CallSignatureDeclaration,
    ConstructSignatureDeclaration, ConstructorDeclaration, GetAccessorDeclaration,
    SetAccessorDeclaration, IndexSignatureDeclaration, MethodSignatureDeclaration,
    MethodDeclaration, ClassStaticBlockDeclaration, ArrowFunction, FunctionExpression,
    ConditionalTypeNode, MappedTypeNode, FunctionTypeNode, ConstructorTypeNode,
    JSDocSignature, ModuleDeclaration, SourceFile,
] direct []);

// ────────────────────────────────────────────────────────────────────────────
// BodyData() — `body` + `asterisk_token` (Go BodyBase)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (node *Node) Body() *Node` (the BodyData dispatch).
pub fn body_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::FunctionDeclaration(d) => d.body,
        NodeData::ConstructorDeclaration(d) => d.body,
        NodeData::GetAccessorDeclaration(d) => d.body,
        NodeData::SetAccessorDeclaration(d) => d.body,
        NodeData::MethodDeclaration(d) => d.body,
        NodeData::ClassStaticBlockDeclaration(d) => Some(d.body),
        NodeData::ArrowFunction(d) => d.body,
        NodeData::FunctionExpression(d) => d.body,
        NodeData::ModuleDeclaration(d) => d.body,
        _ => None,
    }
}

/// Go: `data := node.BodyData(); data.AsteriskToken` (reachable only on
/// BodyBase payloads; `None` elsewhere, where Go's nil interface would
/// nil-deref — the binder guards those calls).
pub fn asterisk_token_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::FunctionDeclaration(d) => d.asterisk_token,
        NodeData::ConstructorDeclaration(d) => d.asterisk_token,
        NodeData::GetAccessorDeclaration(d) => d.asterisk_token,
        NodeData::SetAccessorDeclaration(d) => d.asterisk_token,
        NodeData::MethodDeclaration(d) => d.asterisk_token,
        NodeData::ArrowFunction(d) => d.asterisk_token,
        NodeData::FunctionExpression(d) => d.asterisk_token,
        NodeData::ModuleDeclaration(d) => d.asterisk_token,
        _ => None,
    }
}

/// Go: `if bodyData := node.BodyData(); bodyData != nil` — the node carries a
/// BodyBase payload at all (the binder calls `.Body()` through it).
pub fn has_body_data(node: &Node) -> bool {
    matches!(
        node.kind,
        tsc_ast::Kind::FunctionDeclaration
            | tsc_ast::Kind::Constructor
            | tsc_ast::Kind::GetAccessor
            | tsc_ast::Kind::SetAccessor
            | tsc_ast::Kind::MethodDeclaration
            | tsc_ast::Kind::ClassStaticBlockDeclaration
            | tsc_ast::Kind::ArrowFunction
            | tsc_ast::Kind::FunctionExpression
            | tsc_ast::Kind::ModuleDeclaration
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Flow graph fields written by the binder
// ────────────────────────────────────────────────────────────────────────────

field_dispatch!(end_flow_node_of, set_end_flow_node, end_flow_node, FlowNodeId: opt [
    FunctionDeclaration, ConstructorDeclaration, GetAccessorDeclaration, SetAccessorDeclaration,
    MethodDeclaration, ArrowFunction, FunctionExpression, ModuleDeclaration,
] direct []);

field_dispatch!(return_flow_node_of, set_return_flow_node, return_flow_node, FlowNodeId: opt [
    FunctionDeclaration, ConstructorDeclaration, ClassStaticBlockDeclaration, FunctionExpression,
] direct []);

field_dispatch!(
    fallthrough_flow_node_of,
    set_fallthrough_flow_node,
    fallthrough_flow_node,
    FlowNodeId: opt [CaseOrDefaultClause] direct []
);

// ────────────────────────────────────────────────────────────────────────────
// Field dispatchers — `Expression()` (Go nodeData.Expression())
// ────────────────────────────────────────────────────────────────────────────

field_get!(expression_of, expression, NodeId: opt [
    ReturnStatement, YieldExpression, JsxExpression, TypeParameterDeclaration,
] direct [
    ComputedPropertyName, TemplateSpan, SpreadAssignment, JsxSpreadAttribute, Decorator,
    IfStatement, DoStatement, WhileStatement, ForInOrOfStatement, WithStatement, SwitchStatement,
    CaseOrDefaultClause, ThrowStatement, ExpressionStatement, ExternalModuleReference,
    ExportAssignment, AsExpression, SatisfiesExpression, PropertyAccessExpression,
    ElementAccessExpression, CallExpression, NewExpression, NonNullExpression, SpreadElement,
    ParenthesizedExpression, DeleteExpression, TypeOfExpression, VoidExpression,
    AwaitExpression, TypeAssertion, ExpressionWithTypeArguments, PartiallyEmittedExpression,
    SyntheticReferenceExpression,
]);

// ────────────────────────────────────────────────────────────────────────────
// Field dispatchers — `Type()` (Go nodeData.Type())
// ────────────────────────────────────────────────────────────────────────────

field_get!(type_of, type_, NodeId: opt [
    VariableDeclaration, ParameterDeclaration, FunctionDeclaration,
    CallSignatureDeclaration, ConstructSignatureDeclaration, ConstructorDeclaration,
    GetAccessorDeclaration, SetAccessorDeclaration, IndexSignatureDeclaration,
    MethodSignatureDeclaration, MethodDeclaration, PropertyDeclaration, ArrowFunction,
    FunctionExpression, BinaryExpression, TypePredicateNode, MappedTypeNode,
    FunctionTypeNode, ConstructorTypeNode, JSDocSignature,
] direct [
    TypeAliasDeclaration, ExportAssignment, PropertySignatureDeclaration, PropertyAssignment,
    ShorthandPropertyAssignment, AsExpression, SatisfiesExpression, TypeAssertion,
    TypeOperatorNode, NamedTupleMember, OptionalTypeNode, RestTypeNode, ParenthesizedTypeNode,
    TemplateLiteralTypeSpan, JSDocTypeExpression, JSDocNonNullableType, JSDocNullableType,
    JSDocVariadicType, JSDocOptionalType,
]);
// PORT: Go's SyntheticExpression carries `Type any` (an opaque checker
// value), typed `Arc<dyn Any>` in the payload — outside the NodeId
// dispatcher's surface, hence excluded here.

// ────────────────────────────────────────────────────────────────────────────
// Field dispatchers — `Initializer()` (Go nodeData.Initializer())
// ────────────────────────────────────────────────────────────────────────────

field_get!(initializer_of, initializer, NodeId: opt [
    VariableDeclaration, ParameterDeclaration, BindingElement, EnumMember, PropertyDeclaration,
    JsxAttribute, ForStatement,
] direct [PropertyAssignment, PropertySignatureDeclaration, ForInOrOfStatement]);

// ────────────────────────────────────────────────────────────────────────────
// Field dispatchers — `Statements()` / `StatementList()` (Go ast.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (node *Node) Statements() *NodeList` — SourceFile, ModuleBlock,
/// Block, CaseOrDefaultClause (an empty list ≡ Go's nil list).
pub fn statements_of(store: &dyn NodeStore, node: NodeId) -> Option<&tsc_ast::NodeList> {
    match &store.node(node).data {
        NodeData::SourceFile(d) => d.statements.as_ref(),
        NodeData::ModuleBlock(d) => d.statements.as_ref(),
        NodeData::Block(d) => d.statements.as_ref(),
        NodeData::CaseOrDefaultClause(d) => d.statements.as_ref(),
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Field dispatchers — Parameters() / Arguments() / Elements() / Properties()
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (node *Node) Parameters() *NodeList` (the function-likes).
pub fn parameters_of(store: &dyn NodeStore, node: NodeId) -> Option<&tsc_ast::NodeList> {
    match &store.node(node).data {
        NodeData::FunctionDeclaration(d) => d.parameters.as_ref(),
        NodeData::CallSignatureDeclaration(d) => d.parameters.as_ref(),
        NodeData::ConstructSignatureDeclaration(d) => d.parameters.as_ref(),
        NodeData::ConstructorDeclaration(d) => d.parameters.as_ref(),
        NodeData::GetAccessorDeclaration(d) => d.parameters.as_ref(),
        NodeData::SetAccessorDeclaration(d) => d.parameters.as_ref(),
        NodeData::IndexSignatureDeclaration(d) => d.parameters.as_ref(),
        NodeData::MethodSignatureDeclaration(d) => d.parameters.as_ref(),
        NodeData::MethodDeclaration(d) => d.parameters.as_ref(),
        NodeData::ArrowFunction(d) => d.parameters.as_ref(),
        NodeData::FunctionExpression(d) => d.parameters.as_ref(),
        NodeData::FunctionTypeNode(d) => d.parameters.as_ref(),
        NodeData::ConstructorTypeNode(d) => d.parameters.as_ref(),
        NodeData::JSDocSignature(d) => d.parameters.as_ref(),
        _ => None,
    }
}

/// Go: `func (node *Node) Arguments() []*Node` — CallExpression (always
/// present) and NewExpression (optional).
pub fn arguments_of(store: &dyn NodeStore, node: NodeId) -> &[NodeId] {
    match &store.node(node).data {
        NodeData::CallExpression(d) => d
            .arguments
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::NewExpression(d) => d
            .arguments
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        _ => &[],
    }
}

/// Go: `func (node *Node) Elements() []*NodeList.Nodes` — binding patterns,
/// named imports/exports, array literals, tuple types.
pub fn elements_of(store: &dyn NodeStore, node: NodeId) -> &[NodeId] {
    match &store.node(node).data {
        NodeData::BindingPattern(d) => d
            .elements
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::NamedImports(d) => d
            .elements
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::NamedExports(d) => d
            .elements
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::ArrayLiteralExpression(d) => d
            .elements
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::TupleTypeNode(d) => d
            .elements
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        _ => &[],
    }
}

/// Go: `func (node *Node) Properties() []*Node` — object literals and JSX
/// attribute lists.
pub fn properties_of(store: &dyn NodeStore, node: NodeId) -> &[NodeId] {
    match &store.node(node).data {
        NodeData::ObjectLiteralExpression(d) => d
            .properties
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::JsxAttributes(d) => d
            .properties
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        _ => &[],
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Field dispatchers — the remaining binder-called node.X() accessors
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (node *Node) QuestionDotToken() *TokenNode`.
pub fn question_dot_token_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::PropertyAccessExpression(d) => d.question_dot_token,
        NodeData::ElementAccessExpression(d) => d.question_dot_token,
        NodeData::CallExpression(d) => d.question_dot_token,
        _ => None,
    }
}

/// Go: `func (node *Node) PostfixToken() *TokenNode`.
pub fn postfix_token_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::EnumMember(d) => d.postfix_token,
        NodeData::GetAccessorDeclaration(d) => d.postfix_token,
        NodeData::SetAccessorDeclaration(d) => d.postfix_token,
        NodeData::MethodSignatureDeclaration(d) => d.postfix_token,
        NodeData::MethodDeclaration(d) => d.postfix_token,
        NodeData::PropertySignatureDeclaration(d) => d.postfix_token,
        NodeData::PropertyDeclaration(d) => d.postfix_token,
        NodeData::PropertyAssignment(d) => d.postfix_token,
        NodeData::ShorthandPropertyAssignment(d) => d.postfix_token,
        _ => None,
    }
}

/// Go: `func (node *Node) PropertyName() *Node` (ImportSpecifier/
/// ExportSpecifier/BindingElement; nil elsewhere).
pub fn property_name_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::BindingElement(d) => d.property_name,
        NodeData::ExportSpecifier(d) => d.property_name,
        NodeData::ImportSpecifier(d) => d.property_name,
        _ => None,
    }
}

/// Go: `func (node *Node) PropertyNameOrName() *Node` — PropertyName, else
/// Name (used by getModuleInstanceStateForAliasTarget on export specifiers).
pub fn property_name_or_name(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    property_name_of(store, node).or_else(|| store.node(node).name())
}

/// Go: `func (node *Node) Label() *Node` (Break/Continue/Labeled statements).
pub fn label_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::BreakStatement(d) => d.label,
        NodeData::ContinueStatement(d) => d.label,
        NodeData::LabeledStatement(d) => Some(d.label),
        _ => None,
    }
}

/// Go: `func (node *Node) ModuleSpecifier() *Expression` (the parser-set
/// subset: ImportDeclaration, JSDocImportTag, ExportDeclaration).
pub fn module_specifier_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::ImportDeclaration(d) => Some(d.module_specifier),
        NodeData::JSDocImportTag(d) => Some(d.module_specifier),
        NodeData::ExportDeclaration(d) => d.module_specifier,
        _ => None,
    }
}

/// Go: `func (node *Node) Members() *NodeList` (class/interface/enum/enum-like
/// member lists; the class-like subset the binder reads).
pub fn members_of(store: &dyn NodeStore, node: NodeId) -> &[NodeId] {
    match &store.node(node).data {
        NodeData::ClassDeclaration(d) => d
            .members
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::ClassExpression(d) => d
            .members
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::InterfaceDeclaration(d) => d
            .members
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::EnumDeclaration(d) => d
            .members
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        _ => &[],
    }
}

/// Go: `func (node *Node) TypeParameters() *NodeList` — the function-like /
/// class-like type-parameter lists (the nameresolver reads these through
/// HasContextSensitiveParameters' caller surface; binder uses the payload
/// fields directly).
pub fn type_parameters_of(store: &dyn NodeStore, node: NodeId) -> Option<&tsc_ast::NodeList> {
    match &store.node(node).data {
        NodeData::FunctionDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::ClassDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::ClassExpression(d) => d.type_parameters.as_ref(),
        NodeData::InterfaceDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::TypeAliasDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::CallSignatureDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::ConstructSignatureDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::ConstructorDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::GetAccessorDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::SetAccessorDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::IndexSignatureDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::MethodSignatureDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::MethodDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::ArrowFunction(d) => d.type_parameters.as_ref(),
        NodeData::FunctionExpression(d) => d.type_parameters.as_ref(),
        NodeData::FunctionTypeNode(d) => d.type_parameters.as_ref(),
        NodeData::ConstructorTypeNode(d) => d.type_parameters.as_ref(),
        NodeData::JSDocSignature(d) => d.type_parameters.as_ref(),
        _ => None,
    }
}

/// Go: `func (node *Node) HeritageClauses() *NodeList`.
pub fn heritage_clauses_of(store: &dyn NodeStore, node: NodeId) -> &[NodeId] {
    match &store.node(node).data {
        NodeData::ClassDeclaration(d) => d
            .heritage_clauses
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::ClassExpression(d) => d
            .heritage_clauses
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        NodeData::InterfaceDeclaration(d) => d
            .heritage_clauses
            .as_ref()
            .map(|list| list.nodes.as_ref())
            .unwrap_or(&[]),
        _ => &[],
    }
}
