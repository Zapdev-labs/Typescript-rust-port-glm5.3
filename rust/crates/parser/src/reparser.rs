//! Rust port of `tsc/internal/parser/reparser.go` @
//! ec47d33c23e464a17cdf2475632cba629bee8763 — the JSDoc reparse machinery.
//!
//! Hosted tags find a host and add their children to the correct location
//! under the host; unhosted tags add synthetic nodes to the reparse list.
//! Mirrors reparser.go function-by-function (same names snake_cased, same
//! structure, same comments where they carry semantics).
//!
//! PORT (representation):
//!   * Go's `*ast.Node` nil-able pointers → `NodeId` handles +
//!     `Option<NodeId>` / the `NodeId::NONE` sentinel (SPEC §5.1)
//!   * Go's generated `Node.X()` dispatches (`TypeExpression()`,
//!     `Parameters()`, `FunctionLikeData()`, `AsMutable().SetType()`, …)
//!     have no Rust counterpart beyond `Node::modifiers()`/`name()`; the
//!     narrow dispatches the reparse paths actually hit are mirrored as
//!     module-private helpers below, each panicking on the unhandled kinds
//!     exactly like Go's generated `default:` arms.
//!   * `nodeSliceArena.NewSlice` → `Vec` (the arena is a GC-era space
//!     optimization with no observable semantics, see PORTING-NOTES).

use tsc_ast::{
    deep_clone_reparse, deep_clone_reparse_modifiers, get_assignment_declaration_kind,
    get_right_most_assigned_expression, has_same_property_access_name, is_constructor_declaration,
    is_function_declaration, is_function_like, is_identifier, is_jsdoc_callback_tag,
    is_jsdoc_template_tag, is_jsdoc_typedef_tag, is_method_declaration, is_module_declaration,
    is_qualified_name, is_this_identifier, node_text, JSDeclarationKind, Kind, ModifierList,
    NodeData, NodeFlags, NodeList, NodeId, NodeStore,
};
use tsc_core::text::{TextPos, TextRange};
use tsc_diagnostics::IDENTIFIER_EXPECTED;

use crate::parser::{JSDocInfo, Parser};
use crate::PC_OBJECT_LITERAL_MEMBERS;

// ────────────────────────────────────────────────────────────────────────────
// Node dispatch helpers (mirrors of Go's generated ast.go dispatch methods —
// narrowed to the kinds the reparse paths actually reach; the `panic!` arms
// mirror Go's generated `default: panic(...)`).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (n *Node) TypeExpression() *Node` — narrowed dispatch.
fn type_expression_of(store: &dyn NodeStore, tag: NodeId) -> Option<NodeId> {
    let n = store.node(tag);
    let present = |id: NodeId| if id == NodeId::NONE { None } else { Some(id) };
    match &n.data {
        NodeData::JSDocParameterOrPropertyTag(d) => d.type_expression,
        NodeData::JSDocReturnTag(d) => d.type_expression,
        NodeData::JSDocTypeTag(d) => present(d.type_expression),
        NodeData::JSDocTypedefTag(d) => d.type_expression,
        NodeData::JSDocCallbackTag(d) => present(d.type_expression),
        NodeData::JSDocSatisfiesTag(d) => present(d.type_expression),
        NodeData::JSDocThrowsTag(d) => d.type_expression,
        _ => panic!("Unhandled case in Node.TypeExpression: {}", n.kind_string()),
    }
}

/// Go: `n.TypeExpression().Type()` — the wrapped type of a
/// KindJSDocTypeExpression node (`Node.Type()`'s JSDocTypeExpression arm).
fn jsdoc_type_expression_type(store: &dyn NodeStore, type_expression: NodeId) -> NodeId {
    store
        .node(type_expression)
        .as_jsdoc_type_expression()
        .expect("JSDocTypeExpression data")
        .type_
}

/// Go: `func (n *Node) TagName() *Node` — narrowed to the JSDoc tag kinds.
fn tag_name_of(store: &dyn NodeStore, tag: NodeId) -> NodeId {
    let n = store.node(tag);
    let name = match &n.data {
        NodeData::JSDocUnknownTag(d) => Some(d.tag_name),
        NodeData::JSDocAugmentsTag(d) => Some(d.tag_name),
        NodeData::JSDocImplementsTag(d) => Some(d.tag_name),
        NodeData::JSDocDeprecatedTag(d) => Some(d.tag_name),
        NodeData::JSDocPublicTag(d) => Some(d.tag_name),
        NodeData::JSDocPrivateTag(d) => Some(d.tag_name),
        NodeData::JSDocProtectedTag(d) => Some(d.tag_name),
        NodeData::JSDocReadonlyTag(d) => Some(d.tag_name),
        NodeData::JSDocOverrideTag(d) => Some(d.tag_name),
        NodeData::JSDocTypeTag(d) => Some(d.tag_name),
        NodeData::JSDocTemplateTag(d) => Some(d.tag_name),
        NodeData::JSDocReturnTag(d) => Some(d.tag_name),
        NodeData::JSDocSatisfiesTag(d) => Some(d.tag_name),
        NodeData::JSDocThrowsTag(d) => Some(d.tag_name),
        NodeData::JSDocThisTag(d) => Some(d.tag_name),
        NodeData::JSDocImportTag(d) => Some(d.tag_name),
        NodeData::JSDocCallbackTag(d) => Some(d.tag_name),
        NodeData::JSDocOverloadTag(d) => Some(d.tag_name),
        NodeData::JSDocTypedefTag(d) => Some(d.tag_name),
        NodeData::JSDocParameterOrPropertyTag(d) => Some(d.tag_name),
        _ => None,
    };
    match name {
        Some(tag_name) => tag_name,
        None => panic!("Unhandled case in Node.TagName: {}", n.kind_string()),
    }
}

/// Go: `func (n *Node) CommentList() *NodeList` — narrowed to the JSDoc kinds
/// the reparse paths read.
fn comment_list_of(store: &dyn NodeStore, tag: NodeId) -> Option<&NodeList> {
    let n = store.node(tag);
    match &n.data {
        NodeData::JSDoc(d) => d.comment.as_ref(),
        NodeData::JSDocUnknownTag(d) => d.comment.as_ref(),
        NodeData::JSDocAugmentsTag(d) => d.comment.as_ref(),
        NodeData::JSDocImplementsTag(d) => d.comment.as_ref(),
        NodeData::JSDocDeprecatedTag(d) => d.comment.as_ref(),
        NodeData::JSDocPublicTag(d) => d.comment.as_ref(),
        NodeData::JSDocPrivateTag(d) => d.comment.as_ref(),
        NodeData::JSDocProtectedTag(d) => d.comment.as_ref(),
        NodeData::JSDocReadonlyTag(d) => d.comment.as_ref(),
        NodeData::JSDocOverrideTag(d) => d.comment.as_ref(),
        NodeData::JSDocCallbackTag(d) => d.comment.as_ref(),
        NodeData::JSDocOverloadTag(d) => d.comment.as_ref(),
        NodeData::JSDocParameterOrPropertyTag(d) => d.comment.as_ref(),
        NodeData::JSDocReturnTag(d) => d.comment.as_ref(),
        NodeData::JSDocTypeTag(d) => d.comment.as_ref(),
        NodeData::JSDocTemplateTag(d) => d.comment.as_ref(),
        NodeData::JSDocThisTag(d) => d.comment.as_ref(),
        NodeData::JSDocThrowsTag(d) => d.comment.as_ref(),
        NodeData::JSDocTypedefTag(d) => d.comment.as_ref(),
        NodeData::JSDocSatisfiesTag(d) => d.comment.as_ref(),
        NodeData::JSDocSeeTag(d) => d.comment.as_ref(),
        _ => None,
    }
}

/// Go: `func (n *Node) ClassName() *Node` (JSDocAugmentsTag/JSDocImplementsTag).
fn class_name_of(store: &dyn NodeStore, tag: NodeId) -> NodeId {
    let n = store.node(tag);
    match &n.data {
        NodeData::JSDocAugmentsTag(d) => d.class_name,
        NodeData::JSDocImplementsTag(d) => d.class_name,
        _ => panic!("Unhandled case in Node.ClassName: {}", n.kind_string()),
    }
}

/// Go: `n.FunctionLikeData()` read accessors, narrowed to the function-like
/// payloads the reparse paths reach (parsed function-like hosts + the
/// freshly-reparsed signature kinds). The `panic!` mirrors Go's nil
/// `FunctionLikeData()` on non-function-like nodes.
///
/// The arms cover: FunctionDeclaration, MethodDeclaration, Constructor,
/// GetAccessor, SetAccessor, FunctionExpression, ArrowFunction, FunctionType
/// (the FunctionTypeNode payload of KindFunctionType).
///
/// Go: `fun.Parameters()` — the parameter slice (empty when the list is
/// absent, mirroring Go's nil slice).
fn parameters_of(store: &dyn NodeStore, fun: NodeId) -> &[NodeId] {
    fn list(parameters: &Option<NodeList>) -> &[NodeId] {
        parameters.as_ref().map(|l| &l.nodes[..]).unwrap_or(&[])
    }
    match &store.node(fun).data {
        NodeData::FunctionDeclaration(d) => list(&d.parameters),
        NodeData::MethodDeclaration(d) => list(&d.parameters),
        NodeData::ConstructorDeclaration(d) => list(&d.parameters),
        NodeData::GetAccessorDeclaration(d) => list(&d.parameters),
        NodeData::SetAccessorDeclaration(d) => list(&d.parameters),
        NodeData::FunctionExpression(d) => list(&d.parameters),
        NodeData::ArrowFunction(d) => list(&d.parameters),
        NodeData::FunctionTypeNode(d) => list(&d.parameters),
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.ParameterList()` (`.Loc`) — the parameters NodeList itself.
fn fun_parameter_list(store: &dyn NodeStore, fun: NodeId) -> Option<&NodeList> {
    match &store.node(fun).data {
        NodeData::FunctionDeclaration(d) => d.parameters.as_ref(),
        NodeData::MethodDeclaration(d) => d.parameters.as_ref(),
        NodeData::ConstructorDeclaration(d) => d.parameters.as_ref(),
        NodeData::GetAccessorDeclaration(d) => d.parameters.as_ref(),
        NodeData::SetAccessorDeclaration(d) => d.parameters.as_ref(),
        NodeData::FunctionExpression(d) => d.parameters.as_ref(),
        NodeData::ArrowFunction(d) => d.parameters.as_ref(),
        NodeData::FunctionTypeNode(d) => d.parameters.as_ref(),
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.TypeParameterList()` (the FunctionLikeData arm).
fn fun_type_parameter_list(store: &dyn NodeStore, fun: NodeId) -> Option<&NodeList> {
    match &store.node(fun).data {
        NodeData::FunctionDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::MethodDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::ConstructorDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::GetAccessorDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::SetAccessorDeclaration(d) => d.type_parameters.as_ref(),
        NodeData::FunctionExpression(d) => d.type_parameters.as_ref(),
        NodeData::ArrowFunction(d) => d.type_parameters.as_ref(),
        NodeData::FunctionTypeNode(d) => d.type_parameters.as_ref(),
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.Type()` (the FunctionLikeData arm).
fn fun_type(store: &dyn NodeStore, fun: NodeId) -> Option<NodeId> {
    match &store.node(fun).data {
        NodeData::FunctionDeclaration(d) => d.type_,
        NodeData::MethodDeclaration(d) => d.type_,
        NodeData::ConstructorDeclaration(d) => d.type_,
        NodeData::GetAccessorDeclaration(d) => d.type_,
        NodeData::SetAccessorDeclaration(d) => d.type_,
        NodeData::FunctionExpression(d) => d.type_,
        NodeData::ArrowFunction(d) => d.type_,
        NodeData::FunctionTypeNode(d) => d.type_,
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.FunctionLikeData().FullSignature`.
fn fun_full_signature(store: &dyn NodeStore, fun: NodeId) -> Option<NodeId> {
    match &store.node(fun).data {
        NodeData::FunctionDeclaration(d) => d.full_signature,
        NodeData::MethodDeclaration(d) => d.full_signature,
        NodeData::ConstructorDeclaration(d) => d.full_signature,
        NodeData::GetAccessorDeclaration(d) => d.full_signature,
        NodeData::SetAccessorDeclaration(d) => d.full_signature,
        NodeData::FunctionExpression(d) => d.full_signature,
        NodeData::ArrowFunction(d) => d.full_signature,
        NodeData::FunctionTypeNode(d) => d.full_signature,
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.FunctionLikeData().TypeParameters = …` (`AsMutable()` field write).
fn set_fun_type_parameters(store: &mut dyn NodeStore, fun: NodeId, tps: Option<NodeList>) {
    match &mut store.node_mut(fun).data {
        NodeData::FunctionDeclaration(d) => d.type_parameters = tps,
        NodeData::MethodDeclaration(d) => d.type_parameters = tps,
        NodeData::ConstructorDeclaration(d) => d.type_parameters = tps,
        NodeData::GetAccessorDeclaration(d) => d.type_parameters = tps,
        NodeData::SetAccessorDeclaration(d) => d.type_parameters = tps,
        NodeData::FunctionExpression(d) => d.type_parameters = tps,
        NodeData::ArrowFunction(d) => d.type_parameters = tps,
        NodeData::FunctionTypeNode(d) => d.type_parameters = tps,
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.FunctionLikeData().Parameters = …`.
fn set_fun_parameters(store: &mut dyn NodeStore, fun: NodeId, parameter_list: Option<NodeList>) {
    match &mut store.node_mut(fun).data {
        NodeData::FunctionDeclaration(d) => d.parameters = parameter_list,
        NodeData::MethodDeclaration(d) => d.parameters = parameter_list,
        NodeData::ConstructorDeclaration(d) => d.parameters = parameter_list,
        NodeData::GetAccessorDeclaration(d) => d.parameters = parameter_list,
        NodeData::SetAccessorDeclaration(d) => d.parameters = parameter_list,
        NodeData::FunctionExpression(d) => d.parameters = parameter_list,
        NodeData::ArrowFunction(d) => d.parameters = parameter_list,
        NodeData::FunctionTypeNode(d) => d.parameters = parameter_list,
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.FunctionLikeData().Type = …` (`MutableNode.SetType`'s
/// FunctionLikeData default arm).
fn set_fun_type(store: &mut dyn NodeStore, fun: NodeId, ty: Option<NodeId>) {
    match &mut store.node_mut(fun).data {
        NodeData::FunctionDeclaration(d) => d.type_ = ty,
        NodeData::MethodDeclaration(d) => d.type_ = ty,
        NodeData::ConstructorDeclaration(d) => d.type_ = ty,
        NodeData::GetAccessorDeclaration(d) => d.type_ = ty,
        NodeData::SetAccessorDeclaration(d) => d.type_ = ty,
        NodeData::FunctionExpression(d) => d.type_ = ty,
        NodeData::ArrowFunction(d) => d.type_ = ty,
        NodeData::FunctionTypeNode(d) => d.type_ = ty,
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `fun.FunctionLikeData().FullSignature = …`.
fn set_fun_full_signature(store: &mut dyn NodeStore, fun: NodeId, full_signature: Option<NodeId>) {
    match &mut store.node_mut(fun).data {
        NodeData::FunctionDeclaration(d) => d.full_signature = full_signature,
        NodeData::MethodDeclaration(d) => d.full_signature = full_signature,
        NodeData::ConstructorDeclaration(d) => d.full_signature = full_signature,
        NodeData::GetAccessorDeclaration(d) => d.full_signature = full_signature,
        NodeData::SetAccessorDeclaration(d) => d.full_signature = full_signature,
        NodeData::FunctionExpression(d) => d.full_signature = full_signature,
        NodeData::ArrowFunction(d) => d.full_signature = full_signature,
        NodeData::FunctionTypeNode(d) => d.full_signature = full_signature,
        _ => panic!("Node has no FunctionLikeData"),
    }
}

/// Go: `func (m *MutableNode) SetType(t *Node)` — narrowed to the kinds the
/// reparse paths mutate. Rust fields typed `NodeId` (not `Option`) store the
/// `NodeId::NONE` sentinel for Go's nil.
fn set_type_annotation(store: &mut dyn NodeStore, node: NodeId, ty: Option<NodeId>) {
    let n = store.node_mut(node);
    match &mut n.data {
        NodeData::VariableDeclaration(d) => d.type_ = ty,
        NodeData::ParameterDeclaration(d) => d.type_ = ty,
        NodeData::PropertyDeclaration(d) => d.type_ = ty,
        NodeData::PropertyAssignment(d) => d.type_ = ty.unwrap_or(NodeId::NONE),
        NodeData::ShorthandPropertyAssignment(d) => d.type_ = ty.unwrap_or(NodeId::NONE),
        NodeData::BinaryExpression(d) => d.type_ = ty,
        NodeData::ExportAssignment(d) => d.type_ = ty.unwrap_or(NodeId::NONE),
        // Go: the FunctionLikeData default arm (accessors here; the reparse
        // signatures set their own payloads instead).
        NodeData::GetAccessorDeclaration(d) => d.type_ = ty,
        NodeData::SetAccessorDeclaration(d) => d.type_ = ty,
        _ => {
            let kind = n.kind;
            panic!("Unhandled case in mutableNode.SetType: {}", kind.name())
        }
    }
}

/// Go: `func (m *MutableNode) SetExpression(expr *Node)` — narrowed.
fn set_expression(store: &mut dyn NodeStore, node: NodeId, expr: NodeId) {
    let n = store.node_mut(node);
    match &mut n.data {
        NodeData::ReturnStatement(d) => d.expression = Some(expr),
        NodeData::ParenthesizedExpression(d) => d.expression = expr,
        _ => {
            let kind = n.kind;
            panic!(
                "Unhandled case in mutableNode.SetExpression: {}",
                kind.name()
            )
        }
    }
}

/// Go: `func (m *MutableNode) SetInitializer(initializer *Node)` — narrowed.
fn set_initializer(store: &mut dyn NodeStore, node: NodeId, initializer: Option<NodeId>) {
    let n = store.node_mut(node);
    match &mut n.data {
        NodeData::VariableDeclaration(d) => d.initializer = initializer,
        NodeData::PropertyDeclaration(d) => d.initializer = initializer,
        NodeData::PropertyAssignment(d) => d.initializer = initializer.unwrap_or(NodeId::NONE),
        _ => panic!("Unhandled case in mutableNode.SetInitializer"),
    }
}

/// Go: `func (n *MutableNode) SetModifiers(modifiers *ModifierList)` — narrowed
/// to the member kinds the JSDoc modifier tags rewrite.
fn set_modifiers(store: &mut dyn NodeStore, node: NodeId, modifiers: ModifierList) {
    let n = store.node_mut(node);
    match &mut n.data {
        NodeData::MethodDeclaration(d) => d.modifiers = Some(modifiers),
        NodeData::GetAccessorDeclaration(d) => d.modifiers = Some(modifiers),
        NodeData::SetAccessorDeclaration(d) => d.modifiers = Some(modifiers),
        NodeData::PropertyDeclaration(d) => d.modifiers = Some(modifiers),
        NodeData::ConstructorDeclaration(d) => d.modifiers = Some(modifiers),
        NodeData::BinaryExpression(d) => d.modifiers = Some(modifiers),
        _ => {
            let kind = n.kind;
            panic!(
                "Unhandled case in MutableNode.SetModifiers: {}",
                kind.name()
            )
        }
    }
}

/// Go: `class.TypeParameters = …` on ClassDeclaration/ClassExpression
/// (`getClassLikeData(parent)` field write).
fn set_class_like_type_parameters(store: &mut dyn NodeStore, parent: NodeId, tps: Option<NodeList>) {
    let n = store.node_mut(parent);
    match &mut n.data {
        NodeData::ClassDeclaration(d) => d.type_parameters = tps,
        NodeData::ClassExpression(d) => d.type_parameters = tps,
        _ => panic!("getClassLikeData: not a class-like node"),
    }
}

/// Go: `class.HeritageClauses = …` (getClassLikeData payload field write).
fn set_class_heritage_clauses(store: &mut dyn NodeStore, parent: NodeId, clauses: Option<NodeList>) {
    let n = store.node_mut(parent);
    match &mut n.data {
        NodeData::ClassDeclaration(d) => d.heritage_clauses = clauses,
        NodeData::ClassExpression(d) => d.heritage_clauses = clauses,
        _ => panic!("getClassLikeData: not a class-like node"),
    }
}

/// Go: `getClassLikeData(parent)` — non-nil for ClassDeclaration/ClassExpression.
fn is_class_like(node: &tsc_ast::Node) -> bool {
    matches!(node.kind, Kind::ClassDeclaration | Kind::ClassExpression)
}

/// Go: `class.HeritageClauses` field read (getClassLikeData payload).
fn class_like_heritage_clauses(store: &dyn NodeStore, parent: NodeId) -> Option<&NodeList> {
    match &store.node(parent).data {
        NodeData::ClassDeclaration(d) => d.heritage_clauses.as_ref(),
        NodeData::ClassExpression(d) => d.heritage_clauses.as_ref(),
        _ => None,
    }
}

/// Go: `class.HeritageClauses.Nodes = append(class.HeritageClauses.Nodes,
/// heritageClause)` — the Box<[NodeId]> backing store is rebuilt on append
/// (PORT: Rust `NodeList.nodes` cannot grow in place; same contents).
fn append_class_heritage_clause(store: &mut dyn NodeStore, parent: NodeId, clause: NodeId) {
    let n = store.node_mut(parent);
    let clauses = match &mut n.data {
        NodeData::ClassDeclaration(d) => &mut d.heritage_clauses,
        NodeData::ClassExpression(d) => &mut d.heritage_clauses,
        _ => panic!("getClassLikeData: not a class-like node"),
    };
    append_node_to_list(clauses, clause);
}

fn append_node_to_list(clauses: &mut Option<NodeList>, clause: NodeId) {
    match clauses {
        None => {
            *clauses = Some(NodeList {
                loc: TextRange::new(-1, -1),
                nodes: vec![clause].into_boxed_slice(),
            });
        }
        Some(list) => {
            let mut nodes = list.nodes.to_vec();
            nodes.push(clause);
            list.nodes = nodes.into_boxed_slice();
        }
    }
}

/// Go: `implementsClause.AsHeritageClause().Types.Nodes = append(..., type)`
/// (same Box<[NodeId]> rebuild note as append_class_heritage_clause).
fn append_heritage_clause_type(store: &mut dyn NodeStore, clause: NodeId, ty: NodeId) {
    let n = store.node_mut(clause);
    match &mut n.data {
        NodeData::HeritageClause(d) => {
            let mut types = d.types.clone().unwrap_or_default();
            let mut nodes = types.nodes.to_vec();
            nodes.push(ty);
            types.nodes = nodes.into_boxed_slice();
            d.types = Some(types);
        }
        _ => panic!("not a HeritageClause"),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// reparser.go — Parser methods
// ────────────────────────────────────────────────────────────────────────────

impl<'a, 'f> Parser<'a, 'f> {
    /// Go: `func (p *Parser) finishReparsedNode(node *ast.Node, locationNode *ast.Node)`.
    pub(crate) fn finish_reparsed_node(&mut self, node: NodeId, location_node: NodeId) {
        let location = {
            let s = self.factory.store();
            s.node(location_node).loc
        };
        {
            let s = self.factory.store();
            let n = s.node_mut(node);
            n.flags = self.context_flags | NodeFlags::REPARSED;
            n.loc = location;
        }
        self.override_parent_in_immediate_children(node);
    }

    /// Go: `func (p *Parser) finishMutatedNode(node *ast.Node)`.
    pub(crate) fn finish_mutated_node(&mut self, node: NodeId) {
        self.override_parent_in_immediate_children(node);
    }

    /// Deep-clone the given node and add the clone to the reparsed clone list. The list is used by ast.GetReparsedNodeForNode
    /// to locate reparsed clones of JSDoc nodes. Since the binder attaches symbols to reparsed nodes and not to JSDoc nodes, we
    /// need the mapping when obtaining symbols and types from JSDoc nodes.
    ///
    /// Go: `func (p *Parser) addDeepCloneReparse(node *ast.Node) *ast.Node`.
    pub(crate) fn add_deep_clone_reparse(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        let clone = deep_clone_reparse(self.factory.store(), node);
        if let Some(clone) = clone {
            self.reparsed_clones.push(clone);
        }
        clone
    }

    /// Go: `func (p *Parser) addTransformedReparse(newNode *ast.Node, old *ast.Node) *ast.Node`.
    pub(crate) fn add_transformed_reparse(&mut self, new_node: NodeId, old: NodeId) -> NodeId {
        self.finish_reparsed_node(new_node, old);
        {
            let s = self.factory.store();
            s.node_mut(new_node).flags |= NodeFlags::REPARSER_TRANSFORMED_LITERAL;
        }
        self.reparsed_clones.push(new_node);
        new_node
    }

    /// Go: `func (p *Parser) checkNonIdentifierName(name *ast.Node) *ast.Node`.
    pub(crate) fn check_non_identifier_name(&mut self, name: Option<NodeId>) -> Option<NodeId> {
        // Handles the case of anonymous functions
        let name = name?;
        let (is_ident, is_valid, loc) = {
            let s = self.factory.store();
            let n = s.node(name);
            (
                tsc_ast::is_identifier(n),
                tsc_scanner::is_valid_identifier(node_text(s, name)),
                n.loc,
            )
        };
        if is_ident && !is_valid {
            let mut err_loc = loc;
            if err_loc.len() == 0 {
                // missing name, emit error on the character before the missing name node
                err_loc = TextRange::new(loc.pos() - 1, loc.pos());
            }
            self.parse_error_at_range(err_loc, &IDENTIFIER_EXPECTED, &[]);
        }
        Some(name)
    }

    /// Hosted tags find a host and add their children to the correct location under the host.
    /// Unhosted tags add synthetic nodes to the reparse list.
    ///
    /// Go: `func (p *Parser) reparseTags(parent *ast.Node, jsDoc []*ast.Node)`.
    pub(crate) fn reparse_tags(&mut self, parent: NodeId, jsdoc: &[NodeId]) {
        for (index, &j) in jsdoc.iter().enumerate() {
            let is_last = index == jsdoc.len() - 1;
            let tags: Option<NodeList> = {
                let s = self.factory.store();
                s.node(j).as_jsdoc().expect("JSDoc data").tags.clone()
            };
            let Some(tags) = tags else {
                continue;
            };
            for &tag in tags.nodes.iter() {
                self.reparse_unhosted(tag, parent, j);
                if is_last {
                    self.reparse_hosted(tag, parent, j);
                }
            }
        }
    }

    /// Go: `func (p *Parser) reparseUnhosted(tag *ast.Node, parent *ast.Node, jsDoc *ast.Node)`.
    fn reparse_unhosted(&mut self, tag: NodeId, parent: NodeId, js_doc: NodeId) {
        let kind = {
            let s = self.factory.store();
            s.node(tag).kind
        };
        match kind {
            Kind::JSDocTypedefTag => {
                let Some(type_expression) = type_expression_of(self.factory.store(), tag) else {
                    return;
                };
                let full_name = {
                    let s = self.factory.store();
                    s.node(tag)
                        .as_jsdoc_typedef_tag()
                        .expect("JSDocTypedefTag data")
                        .name
                };
                let is_namespace = full_name.is_some_and(|full_name| {
                    let s = self.factory.store();
                    is_module_declaration(s.node(full_name))
                });
                let modifiers = if is_namespace {
                    Some(self.create_export_modifier(tag))
                } else {
                    None
                };
                let innermost = self.get_innermost_name_of_jsdoc_namespace(full_name);
                let innermost = self.check_non_identifier_name(innermost);
                let type_alias = {
                    let name = self.add_deep_clone_reparse(innermost);
                    self.factory.new_js_type_alias_declaration(
                        modifiers,
                        name.unwrap_or(NodeId::NONE),
                        None,
                        NodeId::NONE,
                    )
                };
                let type_parameters =
                    self.gather_type_parameters(js_doc, /*typedefOrCallback:*/ true);
                {
                    let s = self.factory.store();
                    match &mut s.node_mut(type_alias).data {
                        NodeData::TypeAliasDeclaration(d) => d.type_parameters = type_parameters,
                        _ => panic!("JSTypeAlias must be a TypeAliasDeclaration"),
                    }
                }
                let t: Option<NodeId> = {
                    let te_kind = {
                        let s = self.factory.store();
                        s.node(type_expression).kind
                    };
                    match te_kind {
                        Kind::JSDocTypeExpression => {
                            let inner =
                                jsdoc_type_expression_type(self.factory.store(), type_expression);
                            self.add_deep_clone_reparse(Some(inner))
                        }
                        Kind::JSDocTypeLiteral => {
                            self.reparse_jsdoc_type_literal(Some(type_expression))
                        }
                        _ => panic!(
                            "typedef tag type expression should be a name reference or a type expression{}",
                            {
                                let s = self.factory.store();
                                s.node(type_expression).kind_string()
                            }
                        ),
                    }
                };
                {
                    let s = self.factory.store();
                    match &mut s.node_mut(type_alias).data {
                        NodeData::TypeAliasDeclaration(d) => d.type_ = t.unwrap_or(NodeId::NONE),
                        _ => panic!("JSTypeAlias must be a TypeAliasDeclaration"),
                    }
                }
                self.finish_reparsed_node(type_alias, tag);
                self.jsdoc_infos.push(JSDocInfo {
                    parent: type_alias,
                    js_docs: vec![js_doc],
                });
                {
                    let s = self.factory.store();
                    s.node_mut(type_alias).flags |= NodeFlags::HAS_JSDOC;
                }
                let result =
                    self.wrap_in_jsdoc_namespace(full_name, type_alias, /*nested:*/ false);
                self.reparse_list.push(result);
            }
            Kind::JSDocCallbackTag => {
                let Some(type_expression) = type_expression_of(self.factory.store(), tag) else {
                    return;
                };
                let full_name = {
                    let s = self.factory.store();
                    s.node(tag)
                        .as_jsdoc_callback_tag()
                        .expect("JSDocCallbackTag data")
                        .name
                };
                let is_namespace = full_name.is_some_and(|full_name| {
                    let s = self.factory.store();
                    is_module_declaration(s.node(full_name))
                });
                let modifiers = if is_namespace {
                    Some(self.create_export_modifier(tag))
                } else {
                    None
                };
                let function_type =
                    self.reparse_jsdoc_signature(type_expression, tag, js_doc, tag, None);
                let innermost = self.get_innermost_name_of_jsdoc_namespace(full_name);
                let innermost = self.check_non_identifier_name(innermost);
                let type_alias = {
                    let name = self.add_deep_clone_reparse(innermost);
                    self.factory.new_js_type_alias_declaration(
                        modifiers,
                        name.unwrap_or(NodeId::NONE),
                        None,
                        function_type,
                    )
                };
                let type_parameters =
                    self.gather_type_parameters(js_doc, /*typedefOrCallback:*/ true);
                {
                    let s = self.factory.store();
                    match &mut s.node_mut(type_alias).data {
                        NodeData::TypeAliasDeclaration(d) => d.type_parameters = type_parameters,
                        _ => panic!("JSTypeAlias must be a TypeAliasDeclaration"),
                    }
                }
                self.finish_reparsed_node(type_alias, tag);
                self.jsdoc_infos.push(JSDocInfo {
                    parent: type_alias,
                    js_docs: vec![js_doc],
                });
                {
                    let s = self.factory.store();
                    s.node_mut(type_alias).flags |= NodeFlags::HAS_JSDOC;
                }
                let result =
                    self.wrap_in_jsdoc_namespace(full_name, type_alias, /*nested:*/ false);
                self.reparse_list.push(result);
            }
            Kind::JSDocImportTag => {
                let (import_clause, module_specifier, attributes) = {
                    let s = self.factory.store();
                    let d = s
                        .node(tag)
                        .as_jsdoc_import_tag()
                        .expect("JSDocImportTag data");
                    (d.import_clause, d.module_specifier, d.attributes)
                };
                let Some(import_clause) = import_clause else {
                    return;
                };
                let import_clause = self
                    .add_deep_clone_reparse(Some(import_clause))
                    .expect("import clause present");
                {
                    let s = self.factory.store();
                    match &mut s.node_mut(import_clause).data {
                        NodeData::ImportClause(d) => d.phase_modifier = Some(Kind::TypeKeyword),
                        _ => panic!("not an ImportClause"),
                    }
                }
                // Go: `p.factory.DeepCloneReparseModifiers(importTag.Modifiers())`
                // — the JSDocImportTag payload has no modifiers field, so Go's
                // `Node.Modifiers()` default returns nil (mirrored by None).
                let cloned_modifiers = deep_clone_reparse_modifiers(self.factory.store(), None);
                let cloned_specifier = self
                    .add_deep_clone_reparse(Some(module_specifier))
                    .unwrap_or(NodeId::NONE);
                let cloned_attributes = self.add_deep_clone_reparse(attributes);
                let import_declaration = self.factory.new_js_import_declaration(
                    cloned_modifiers,
                    Some(import_clause),
                    cloned_specifier,
                    cloned_attributes,
                );
                self.finish_reparsed_node(import_declaration, tag);
                self.reparse_list.push(import_declaration);
            }
            Kind::JSDocOverloadTag => {
                // Create overload signatures only for function, method, and constructor declarations outside object literals
                let parent_modifiers = {
                    let s = self.factory.store();
                    s.node(parent).modifiers().cloned()
                };
                let parent_is_hostable = {
                    let s = self.factory.store();
                    is_function_declaration(s.node(parent))
                        || is_method_declaration(s.node(parent))
                        || is_constructor_declaration(s.node(parent))
                };
                if parent_is_hostable
                    && self.parsing_contexts & (1 << PC_OBJECT_LITERAL_MEMBERS) == 0
                {
                    let type_expression = {
                        let s = self.factory.store();
                        s.node(tag)
                            .as_jsdoc_overload_tag()
                            .expect("JSDocOverloadTag data")
                            .type_expression
                    };
                    let signature = self.reparse_jsdoc_signature(
                        type_expression,
                        parent,
                        js_doc,
                        tag,
                        parent_modifiers,
                    );
                    self.reparse_list.push(signature);
                }
            }
            _ => {}
        }
    }

    /// Go: `func (p *Parser) reparseJSDocSignature(jsSignature *ast.Node, fun *ast.Node, jsDoc *ast.Node, tag *ast.Node, modifiers *ast.ModifierList) *ast.Node`.
    ///
    /// PORT: `modifiers` is passed by value — Go shares the `*ModifierList`
    /// pointer, but its only use (`DeepCloneReparseModifiers`) reads without
    /// mutating, so a clone is semantically identical.
    fn reparse_jsdoc_signature(
        &mut self,
        js_signature: NodeId,
        fun: NodeId,
        js_doc: NodeId,
        tag: NodeId,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let fun_kind = {
            let s = self.factory.store();
            s.node(fun).kind
        };
        let tag_kind = {
            let s = self.factory.store();
            s.node(tag).kind
        };
        let cloned_modifiers =
            deep_clone_reparse_modifiers(self.factory.store(), modifiers.as_ref());
        let signature = match fun_kind {
            Kind::FunctionDeclaration => {
                let name = {
                    let s = self.factory.store();
                    s.node(fun).name()
                };
                let name = self.check_non_identifier_name(name);
                let name = deep_clone_reparse(self.factory.store(), name);
                self.factory.new_function_declaration(
                    cloned_modifiers,
                    None,
                    name,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            }
            Kind::MethodDeclaration => {
                let name = {
                    let s = self.factory.store();
                    s.node(fun).name()
                };
                let name = self.check_non_identifier_name(name);
                let name = deep_clone_reparse(self.factory.store(), name);
                self.factory.new_method_declaration(
                    cloned_modifiers,
                    None,
                    name.unwrap_or(NodeId::NONE),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            }
            Kind::Constructor => self.factory.new_constructor_declaration(
                cloned_modifiers,
                None,
                None,
                None,
                None,
                None,
            ),
            Kind::JSDocCallbackTag => {
                let any_type = self.factory.new_keyword_type_node(Kind::AnyKeyword);
                self.factory
                    .new_function_type_node(None, None, Some(any_type))
            }
            _ => panic!(
                "Unexpected kind {}",
                {
                    let s = self.factory.store();
                    s.node(fun).kind_string()
                }
            ),
        };

        if tag_kind != Kind::JSDocCallbackTag {
            let type_parameters =
                self.gather_type_parameters(js_doc, /*typedefOrCallback:*/ false);
            set_fun_type_parameters(self.factory.store(), signature, type_parameters);
        }
        let (js_params, js_params_loc) = {
            let s = self.factory.store();
            let d = s
                .node(js_signature)
                .as_jsdoc_signature()
                .expect("JSDocSignature data");
            match &d.parameters {
                Some(list) => (list.nodes.to_vec(), list.loc),
                None => (Vec::new(), TextRange::undefined()),
            }
        };
        let mut parameters: Vec<NodeId> = Vec::new();
        for (pi, &param) in js_params.iter().enumerate() {
            let param_kind = {
                let s = self.factory.store();
                s.node(param).kind
            };
            let parameter: NodeId;
            if param_kind == Kind::JSDocThisTag {
                let (this_tag_loc, this_type_expression) = {
                    let s = self.factory.store();
                    let d = s.node(param).as_jsdoc_this_tag().expect("JSDocThisTag data");
                    (s.node(param).loc, d.type_expression)
                };
                let this_ident = self.factory.new_identifier("this");
                {
                    let s = self.factory.store();
                    let n = s.node_mut(this_ident);
                    n.loc = this_tag_loc;
                    n.flags = self.context_flags | NodeFlags::REPARSED;
                }
                parameter = self
                    .factory
                    .new_parameter_declaration(None, None, this_ident, None, None, None);
                if this_type_expression != NodeId::NONE {
                    let inner =
                        jsdoc_type_expression_type(self.factory.store(), this_type_expression);
                    let cloned = self.add_deep_clone_reparse(Some(inner));
                    {
                        let s = self.factory.store();
                        match &mut s.node_mut(parameter).data {
                            NodeData::ParameterDeclaration(d) => d.type_ = cloned,
                            _ => panic!("not a ParameterDeclaration"),
                        }
                    }
                }
            } else if param_kind == Kind::JSDocParameterTag
                || param_kind == Kind::JSDocPropertyTag
            {
                let (name, param_type_expression, param_loc) = {
                    let s = self.factory.store();
                    let d = s
                        .node(param)
                        .as_jsdoc_parameter_or_property_tag()
                        .expect("JSDocParameterOrPropertyTag data");
                    (d.name, d.type_expression, s.node(param).loc)
                };
                // Skip sub-property parameters (e.g., @param x.y) - these have QualifiedNames
                // and describe properties of a parent parameter, not standalone parameters.
                let name_is_qualified = {
                    let s = self.factory.store();
                    is_qualified_name(s.node(name))
                };
                if name_is_qualified {
                    continue;
                }
                let mut dot_dot_dot_token: Option<NodeId> = None;
                let mut param_type: Option<NodeId> = None;

                if let Some(te) = param_type_expression {
                    let te_inner_kind = {
                        let s = self.factory.store();
                        s.node(jsdoc_type_expression_type(s, te)).kind
                    };
                    if te_inner_kind == Kind::JSDocVariadicType {
                        let dot = self.factory.new_token(Kind::DotDotDotToken);
                        {
                            let s = self.factory.store();
                            let n = s.node_mut(dot);
                            n.loc = param_loc;
                            n.flags = self.context_flags | NodeFlags::REPARSED;
                        }
                        dot_dot_dot_token = Some(dot);
                        let variadic_type = {
                            let s = self.factory.store();
                            let inner = jsdoc_type_expression_type(s, te);
                            s.node(inner)
                                .as_jsdoc_variadic_type()
                                .expect("JSDocVariadicType data")
                                .type_
                        };
                        param_type = self.reparse_jsdoc_type_literal(Some(variadic_type));
                    } else {
                        let inner = jsdoc_type_expression_type(self.factory.store(), te);
                        param_type = self.reparse_jsdoc_type_literal(Some(inner));
                    }
                }
                let mut name = Some(name);
                let name_is_invalid = {
                    let s = self.factory.store();
                    is_identifier(s.node(name.unwrap()))
                        && !tsc_scanner::is_valid_identifier(node_text(s, name.unwrap()))
                };
                if name_is_invalid {
                    // drop invalid chars for _, if empty, write _0, etc., so we have a valid param name to emit later
                    let text = {
                        let s = self.factory.store();
                        node_text(s, name.unwrap()).to_string()
                    };
                    let mut result = String::new();
                    for (i, ch) in text.chars().enumerate() {
                        if i == 0 {
                            if !tsc_scanner::is_identifier_start(ch as i32) {
                                result.push('_');
                            } else {
                                result.push(ch);
                            }
                        } else if !tsc_scanner::is_identifier_part(ch as i32) {
                            result.push('_');
                        } else {
                            result.push(ch);
                        }
                    }
                    if result.is_empty() {
                        result.push('_');
                        // Go: strconv.Itoa(pi)
                        result.push_str(&pi.to_string());
                    }
                    let new_ident = self.factory.new_identifier(&result);
                    name = Some(self.add_transformed_reparse(new_ident, name.unwrap()));
                } else {
                    name = self.add_deep_clone_reparse(name);
                }
                let question = self.make_question_if_optional(param);
                parameter = self.factory.new_parameter_declaration(
                    None,
                    dot_dot_dot_token,
                    name.unwrap_or(NodeId::NONE),
                    question,
                    param_type,
                    None,
                );
            } else {
                // Go: `parameter` stays nil and nil-derefs in finishReparsedNode
                // below — unreachable for the same reason (JSDoc signature
                // parameter lists only contain the tag kinds above).
                panic!(
                    "Unexpected JSDoc signature parameter kind {}",
                    {
                        let s = self.factory.store();
                        s.node(param).kind_string()
                    }
                );
            }
            self.finish_reparsed_node(parameter, param);
            parameters.push(parameter);
            self.reparse_jsdoc_comment(parameter, param);
        }
        let parameter_list = self.new_node_list(js_params_loc, parameters);
        set_fun_parameters(self.factory.store(), signature, Some(parameter_list));

        let sig_type = {
            let s = self.factory.store();
            s.node(js_signature)
                .as_jsdoc_signature()
                .expect("JSDocSignature data")
                .type_
        };
        if let Some(return_tag) = sig_type {
            let te = {
                let s = self.factory.store();
                s.node(return_tag)
                    .as_jsdoc_return_tag()
                    .expect("JSDocReturnTag data")
                    .type_expression
            };
            if let Some(te) = te {
                let inner = jsdoc_type_expression_type(self.factory.store(), te);
                let cloned = self.add_deep_clone_reparse(Some(inner));
                set_fun_type(self.factory.store(), signature, cloned);
            }
        }
        let loc = if tag_kind == Kind::JSDocOverloadTag {
            tag_name_of(self.factory.store(), tag)
        } else {
            js_signature
        };
        self.finish_reparsed_node(signature, loc);
        signature
    }

    /// Go: `func (p *Parser) reparseJSDocTypeLiteral(t *ast.TypeNode) *ast.Node`.
    fn reparse_jsdoc_type_literal(&mut self, t: Option<NodeId>) -> Option<NodeId> {
        let t = t?;
        let kind = {
            let s = self.factory.store();
            s.node(t).kind
        };
        if kind == Kind::JSDocTypeLiteral {
            let (is_array_type, property_tags, literal_loc) = {
                let s = self.factory.store();
                let d = s
                    .node(t)
                    .as_jsdoc_type_literal()
                    .expect("JSDocTypeLiteral data");
                (d.is_array_type, d.jsdoc_property_tags.clone(), s.node(t).loc)
            };
            let mut properties: Vec<NodeId> = Vec::new();
            for &prop in property_tags.iter() {
                let prop_kind = {
                    let s = self.factory.store();
                    s.node(prop).kind
                };
                if prop_kind != Kind::JSDocPropertyTag && prop_kind != Kind::JSDocParameterTag {
                    continue;
                }
                let (mut name, prop_type_expression) = {
                    let s = self.factory.store();
                    let d = s
                        .node(prop)
                        .as_jsdoc_parameter_or_property_tag()
                        .expect("JSDocParameterOrPropertyTag data");
                    (d.name, d.type_expression)
                };
                let name_kind = {
                    let s = self.factory.store();
                    s.node(name).kind
                };
                if name_kind == Kind::QualifiedName {
                    name = {
                        let s = self.factory.store();
                        s.node(name)
                            .as_qualified_name()
                            .expect("QualifiedName data")
                            .right
                    };
                }
                let name_is_invalid = {
                    let s = self.factory.store();
                    is_identifier(s.node(name))
                        && !tsc_scanner::is_valid_identifier(node_text(s, name))
                };
                if name_is_invalid {
                    let text = {
                        let s = self.factory.store();
                        node_text(s, name).to_string()
                    };
                    let literal = self
                        .factory
                        .new_string_literal(&text, tsc_ast::TokenFlags::NONE);
                    name = self.add_transformed_reparse(literal, name);
                } else {
                    name = self
                        .add_deep_clone_reparse(Some(name))
                        .expect("DeepCloneReparse preserves the node");
                }
                let question = self.make_question_if_optional(prop);
                let property = self.factory.new_property_signature_declaration(
                    None,
                    name,
                    question,
                    NodeId::NONE,
                    NodeId::NONE,
                );
                if let Some(te) = prop_type_expression {
                    let inner = jsdoc_type_expression_type(self.factory.store(), te);
                    let prop_type = self.reparse_jsdoc_type_literal(Some(inner));
                    {
                        let s = self.factory.store();
                        match &mut s.node_mut(property).data {
                            NodeData::PropertySignatureDeclaration(d) => {
                                d.type_ = prop_type.unwrap_or(NodeId::NONE)
                            }
                            _ => panic!("not a PropertySignatureDeclaration"),
                        }
                    }
                }
                self.finish_reparsed_node(property, prop);
                properties.push(property);
                self.reparse_jsdoc_comment(property, prop);
            }
            let members = self.new_node_list(literal_loc, properties);
            let mut new_t = self.factory.new_type_literal_node(Some(members));
            if is_array_type {
                self.finish_reparsed_node(new_t, t);
                new_t = self.factory.new_array_type_node(new_t);
            }
            self.finish_reparsed_node(new_t, t);
            return Some(new_t);
        }
        self.add_deep_clone_reparse(Some(t))
    }

    /// Go: `func (p *Parser) reparseJSDocComment(node *ast.Node, tag *ast.Node)`.
    fn reparse_jsdoc_comment(&mut self, node: NodeId, tag: NodeId) {
        let comment: Option<NodeList> = comment_list_of(self.factory.store(), tag).cloned();
        if let Some(comment) = comment {
            let mut cloned: Vec<NodeId> = Vec::with_capacity(comment.nodes.len());
            for &c in comment.nodes.iter() {
                // Go: `core.Map(comment.Nodes, p.factory.DeepCloneReparse)` —
                // the raw DeepCloneReparse (no clone-list bookkeeping).
                cloned.push(
                    deep_clone_reparse(self.factory.store(), Some(c))
                        .expect("DeepCloneReparse preserves the node"),
                );
            }
            // Go: `p.factory.NewNodeList(...)` + `newComment.Loc = comment.Loc`.
            let new_comment = NodeList {
                loc: comment.loc,
                nodes: cloned.into_boxed_slice(),
            };
            let prop_jsdoc = self.factory.new_jsdoc(Some(new_comment), None);
            self.finish_reparsed_node(prop_jsdoc, tag);
            {
                let s = self.factory.store();
                s.node_mut(prop_jsdoc).parent.set(node);
            }
            self.jsdoc_infos.push(JSDocInfo {
                parent: node,
                js_docs: vec![prop_jsdoc],
            });
            {
                let s = self.factory.store();
                s.node_mut(node).flags |= NodeFlags::HAS_JSDOC;
            }
        }
    }

    /// Go: `func (p *Parser) gatherTypeParameters(j *ast.Node, typedefOrCallback bool) *ast.NodeList`.
    fn gather_type_parameters(
        &mut self,
        j: NodeId,
        typedef_or_callback: bool,
    ) -> Option<NodeList> {
        let tags: Option<NodeList> = {
            let s = self.factory.store();
            s.node(j).as_jsdoc().expect("JSDoc data").tags.clone()
        };
        let mut type_parameters: Option<Vec<NodeId>> = None;
        let mut pos: TextPos = -1;
        let mut end_pos: TextPos = -1;
        let mut first_template = true;
        let Some(tags) = tags else {
            // Go: `j.AsJSDoc().Tags.Nodes` on a nil Tags is a nil slice —
            // the loop body never runs.
            return None;
        };
        for &tag in tags.nodes.iter() {
            // When a JSDoc comment contains an `@typedef` or `@callback` tag, `@template` type parameter
            // declarations apply to the type being defined.
            let is_defining_tag = {
                let s = self.factory.store();
                is_jsdoc_typedef_tag(s.node(tag)) || is_jsdoc_callback_tag(s.node(tag))
            };
            if !typedef_or_callback && is_defining_tag {
                return None;
            }
            let is_template = {
                let s = self.factory.store();
                is_jsdoc_template_tag(s.node(tag))
            };
            if !is_template {
                continue;
            }
            let (tag_pos, tag_end, constraint, template_type_parameters) = {
                let s = self.factory.store();
                let n = s.node(tag);
                let d = n.as_jsdoc_template_tag().expect("JSDocTemplateTag data");
                (n.pos(), n.end(), d.constraint, d.type_parameters.clone())
            };
            if first_template {
                pos = tag_pos;
                first_template = false;
            }
            end_pos = tag_end;
            let mut first_type_parameter = true;
            let template_type_parameters = template_type_parameters
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default();
            for &tp in template_type_parameters.iter() {
                let reparse: NodeId = if constraint != NodeId::NONE && first_type_parameter {
                    let (tp_modifiers, tp_name, tp_default_type) = {
                        let s = self.factory.store();
                        let n = s.node(tp);
                        let d = n
                            .as_type_parameter_declaration()
                            .expect("TypeParameterDeclaration data");
                        (n.modifiers().cloned(), Some(d.name), d.default_type)
                    };
                    let cloned_modifiers =
                        deep_clone_reparse_modifiers(self.factory.store(), tp_modifiers.as_ref());
                    let checked_name = self.check_non_identifier_name(tp_name);
                    let cloned_name = self.add_deep_clone_reparse(checked_name);
                    let constraint_type =
                        jsdoc_type_expression_type(self.factory.store(), constraint);
                    let cloned_constraint = self.add_deep_clone_reparse(Some(constraint_type));
                    let cloned_default = self.add_deep_clone_reparse(tp_default_type);
                    let type_parameter = self.factory.new_type_parameter_declaration(
                        cloned_modifiers,
                        cloned_name.unwrap_or(NodeId::NONE),
                        cloned_constraint,
                        None, // expression
                        cloned_default,
                    );
                    self.finish_reparsed_node(type_parameter, tp);
                    type_parameter
                } else {
                    self.add_deep_clone_reparse(Some(tp))
                        .expect("DeepCloneReparse preserves the node")
                };
                type_parameters.get_or_insert_with(Vec::new).push(reparse);
                first_type_parameter = false;
            }
        }
        match type_parameters {
            Some(type_parameters) if !type_parameters.is_empty() => Some(
                self.new_node_list(TextRange::new(pos, end_pos), type_parameters),
            ),
            _ => None,
        }
    }

    /// Go: `func (p *Parser) reparseHosted(tag *ast.Node, parent *ast.Node, jsDoc *ast.Node)`.
    fn reparse_hosted(&mut self, tag: NodeId, parent: NodeId, js_doc: NodeId) {
        let tag_kind = {
            let s = self.factory.store();
            s.node(tag).kind
        };
        match tag_kind {
            Kind::JSDocTypeTag => {
                let type_expression = type_expression_of(self.factory.store(), tag);
                let te_type = type_expression
                    .map(|te| jsdoc_type_expression_type(self.factory.store(), te));
                let parent_kind = {
                    let s = self.factory.store();
                    s.node(parent).kind
                };
                match parent_kind {
                    Kind::VariableStatement => {
                        let declarations = {
                            let s = self.factory.store();
                            let dl = s
                                .node(parent)
                                .as_variable_statement()
                                .expect("VariableStatement data")
                                .declaration_list;
                            if dl == NodeId::NONE {
                                Vec::new()
                            } else {
                                s.node(dl)
                                    .as_variable_declaration_list()
                                    .expect("VariableDeclarationList data")
                                    .declarations
                                    .as_ref()
                                    .map(|l| l.nodes.to_vec())
                                    .unwrap_or_default()
                            }
                        };
                        for declaration in declarations {
                            let decl_type = {
                                let s = self.factory.store();
                                s.node(declaration)
                                    .as_variable_declaration()
                                    .expect("VariableDeclaration data")
                                    .type_
                            };
                            if decl_type.is_none() && te_type.is_some() {
                                let cloned = self.add_deep_clone_reparse(te_type);
                                set_type_annotation(self.factory.store(), declaration, cloned);
                                self.finish_mutated_node(declaration);
                                return;
                            }
                        }
                    }
                    Kind::VariableDeclaration
                    | Kind::ExportAssignment
                    | Kind::PropertyDeclaration
                    | Kind::PropertyAssignment
                    | Kind::ShorthandPropertyAssignment
                    | Kind::GetAccessor => {
                        let parent_type = self.node_type_annotation(parent);
                        if parent_type.is_none() && te_type.is_some() {
                            let cloned = self.add_deep_clone_reparse(te_type);
                            set_type_annotation(self.factory.store(), parent, cloned);
                            self.finish_mutated_node(parent);
                            return;
                        }
                    }
                    Kind::Parameter => {
                        let parent_type = self.node_type_annotation(parent);
                        if parent_type.is_none() && te_type.is_some() {
                            let reparsed = self.reparse_jsdoc_type_literal(te_type);
                            set_type_annotation(self.factory.store(), parent, reparsed);
                            self.finish_mutated_node(parent);
                            return;
                        }
                    }
                    Kind::ReturnStatement | Kind::ParenthesizedExpression => {
                        let parent_expression = self.node_expression(parent);
                        if let (Some(pe), Some(te)) = (parent_expression, te_type) {
                            let cloned = self.add_deep_clone_reparse(Some(te));
                            let cast = self.make_new_cast(
                                cloned.expect("type expression present"),
                                pe,
                                /*isAssertion:*/ true,
                            );
                            set_expression(self.factory.store(), parent, cast);
                            self.finish_mutated_node(parent);
                            return;
                        }
                    }
                    Kind::ExpressionStatement => {
                        // PORT: FCS tranche 52 — Go reparser.go:373-381: a
                        // `/** @type T */ a.b = expr` JS assignment
                        // declaration attaches the tag's type node to the
                        // BINARY (`bin.AsMutable().SetType(...)`) so the
                        // checker's getContextualTypeForBinaryOperand reads
                        // it (checker.go:30360) and the RHS object literal
                        // is contextually typed. The arm was absent, so
                        // contextualTypedSpecialAssignment's object-literal
                        // methods fell to implicit-any parameters (TS7006 x8).
                        let bin = {
                            let s = self.factory.store();
                            let expr = s
                                .node(parent)
                                .as_expression_statement()
                                .expect("ExpressionStatement data")
                                .expression;
                            if s.node(expr).kind == Kind::BinaryExpression {
                                Some(expr)
                            } else {
                                None
                            }
                        };
                        if let Some(bin) = bin {
                            let kind = get_assignment_declaration_kind(self.factory.store(), bin);
                            if kind != JSDeclarationKind::None && te_type.is_some() {
                                let cloned = self.add_deep_clone_reparse(te_type);
                                set_type_annotation(self.factory.store(), bin, cloned);
                                self.finish_mutated_node(bin);
                                return;
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(fun) = get_function_like_host(self.factory.store(), parent) {
                    let host_params: Vec<NodeId> = parameters_of(self.factory.store(), fun).to_vec();
                    let no_typed_params = host_params.iter().all(|&param| {
                        let s = self.factory.store();
                        node_type_of_hosted_param(s, param).is_none()
                    });
                    let fun_has_no_type_parameters =
                        fun_type_parameter_list(self.factory.store(), fun).is_none();
                    let fun_has_no_type = fun_type(self.factory.store(), fun).is_none();
                    if fun_has_no_type_parameters
                        && fun_has_no_type
                        && no_typed_params
                        && te_type.is_some()
                    {
                        let cloned = self.add_deep_clone_reparse(te_type);
                        set_fun_full_signature(self.factory.store(), fun, cloned);
                        self.finish_mutated_node(fun);
                    }
                }
            }
            Kind::JSDocSatisfiesTag => {
                let te_type = {
                    // Go: `tag.AsJSDocSatisfiesTag().TypeExpression` (non-nil).
                    let s = self.factory.store();
                    let d = s
                        .node(tag)
                        .as_jsdoc_satisfies_tag()
                        .expect("JSDocSatisfiesTag data")
                        .type_expression;
                    if d == NodeId::NONE {
                        None
                    } else {
                        Some(jsdoc_type_expression_type(s, d))
                    }
                };
                let parent_kind = {
                    let s = self.factory.store();
                    s.node(parent).kind
                };
                match parent_kind {
                    Kind::VariableStatement => {
                        let declarations = {
                            let s = self.factory.store();
                            let dl = s
                                .node(parent)
                                .as_variable_statement()
                                .expect("VariableStatement data")
                                .declaration_list;
                            if dl == NodeId::NONE {
                                Vec::new()
                            } else {
                                s.node(dl)
                                    .as_variable_declaration_list()
                                    .expect("VariableDeclarationList data")
                                    .declarations
                                    .as_ref()
                                    .map(|l| l.nodes.to_vec())
                                    .unwrap_or_default()
                            }
                        };
                        for declaration in declarations {
                            let initializer = {
                                let s = self.factory.store();
                                s.node(declaration)
                                    .as_variable_declaration()
                                    .expect("VariableDeclaration data")
                                    .initializer
                            };
                            if let (Some(init), Some(te)) = (initializer, te_type) {
                                let cloned = self.add_deep_clone_reparse(Some(te));
                                let cast = self.make_new_cast(
                                    cloned.expect("type expression present"),
                                    init,
                                    /*isAssertion:*/ false,
                                );
                                set_initializer(self.factory.store(), declaration, Some(cast));
                                self.finish_mutated_node(declaration);
                                // Go: `break` — exits the declaration loop.
                                break;
                            }
                        }
                    }
                    Kind::VariableDeclaration
                    | Kind::PropertyDeclaration
                    | Kind::PropertyAssignment => {
                        let initializer = self.node_initializer(parent);
                        if let (Some(init), Some(te)) = (initializer, te_type) {
                            let cloned = self.add_deep_clone_reparse(Some(te));
                            let cast = self.make_new_cast(
                                cloned.expect("type expression present"),
                                init,
                                /*isAssertion:*/ false,
                            );
                            set_initializer(self.factory.store(), parent, Some(cast));
                            self.finish_mutated_node(parent);
                        }
                    }
                    Kind::ShorthandPropertyAssignment => {
                        let object_assignment_initializer = {
                            let s = self.factory.store();
                            s.node(parent)
                                .as_shorthand_property_assignment()
                                .expect("ShorthandPropertyAssignment data")
                                .object_assignment_initializer
                        };
                        if let (Some(init), Some(te)) = (object_assignment_initializer, te_type) {
                            let cloned = self.add_deep_clone_reparse(Some(te));
                            let cast = self.make_new_cast(
                                cloned.expect("type expression present"),
                                init,
                                /*isAssertion:*/ false,
                            );
                            {
                                let s = self.factory.store();
                                match &mut s.node_mut(parent).data {
                                    NodeData::ShorthandPropertyAssignment(d) => {
                                        d.object_assignment_initializer = Some(cast)
                                    }
                                    _ => panic!("not a ShorthandPropertyAssignment"),
                                }
                            }
                            self.finish_mutated_node(parent);
                        }
                    }
                    Kind::ReturnStatement
                    | Kind::ParenthesizedExpression
                    | Kind::ExportAssignment => {
                        let parent_expression = self.node_expression(parent);
                        if let (Some(pe), Some(te)) = (parent_expression, te_type) {
                            let cloned = self.add_deep_clone_reparse(Some(te));
                            let cast = self.make_new_cast(
                                cloned.expect("type expression present"),
                                pe,
                                /*isAssertion:*/ false,
                            );
                            set_expression(self.factory.store(), parent, cast);
                            self.finish_mutated_node(parent);
                        }
                    }
                    Kind::ExpressionStatement => {
                        let (bin, bin_right) = {
                            let s = self.factory.store();
                            let expr = s
                                .node(parent)
                                .as_expression_statement()
                                .expect("ExpressionStatement data")
                                .expression;
                            if s.node(expr).kind != Kind::BinaryExpression {
                                (None, NodeId::NONE)
                            } else {
                                let d =
                                    s.node(expr).as_binary_expression().expect("BinaryExpression data");
                                (Some(expr), d.right)
                            }
                        };
                        if let Some(bin) = bin {
                            let kind = get_assignment_declaration_kind(self.factory.store(), bin);
                            if kind != JSDeclarationKind::None && te_type.is_some() {
                                let cloned = self.add_deep_clone_reparse(te_type);
                                let cast = self.make_new_cast(
                                    cloned.expect("type expression present"),
                                    bin_right,
                                    /*isAssertion:*/ false,
                                );
                                {
                                    let s = self.factory.store();
                                    match &mut s.node_mut(bin).data {
                                        NodeData::BinaryExpression(d) => d.right = cast,
                                        _ => panic!("not a BinaryExpression"),
                                    }
                                }
                                self.finish_mutated_node(bin);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Kind::JSDocTemplateTag => {
                // Go: the else-if chain discriminates on the parent's kind
                // (function-like host / class declaration / class expression).
                let parent_is_class_declaration = {
                    let s = self.factory.store();
                    s.node(parent).kind == Kind::ClassDeclaration
                };
                let parent_is_class_expression = {
                    let s = self.factory.store();
                    s.node(parent).kind == Kind::ClassExpression
                };
                if let Some(fun) = get_function_like_host(self.factory.store(), parent) {
                    let fun_has_no_type_parameters =
                        fun_type_parameter_list(self.factory.store(), fun).is_none();
                    let fun_has_no_full_signature =
                        fun_full_signature(self.factory.store(), fun).is_none();
                    if fun_has_no_type_parameters && fun_has_no_full_signature {
                        let type_parameters =
                            self.gather_type_parameters(js_doc, /*typedefOrCallback:*/ false);
                        set_fun_type_parameters(self.factory.store(), fun, type_parameters);
                        self.finish_mutated_node(fun);
                    }
                } else if parent_is_class_declaration {
                    let has_no_type_parameters = {
                        let s = self.factory.store();
                        s.node(parent)
                            .as_class_declaration()
                            .expect("ClassDeclaration data")
                            .type_parameters
                            .is_none()
                    };
                    if has_no_type_parameters {
                        let type_parameters =
                            self.gather_type_parameters(js_doc, /*typedefOrCallback:*/ false);
                        set_class_like_type_parameters(
                            self.factory.store(),
                            parent,
                            type_parameters,
                        );
                        self.finish_mutated_node(parent);
                    }
                } else if parent_is_class_expression {
                    let has_no_type_parameters = {
                        let s = self.factory.store();
                        s.node(parent)
                            .as_class_expression()
                            .expect("ClassExpression data")
                            .type_parameters
                            .is_none()
                    };
                    if has_no_type_parameters {
                        let type_parameters =
                            self.gather_type_parameters(js_doc, /*typedefOrCallback:*/ false);
                        set_class_like_type_parameters(
                            self.factory.store(),
                            parent,
                            type_parameters,
                        );
                        self.finish_mutated_node(parent);
                    }
                }
            }
            Kind::JSDocParameterTag => {
                if let Some(fun) = get_function_like_host(self.factory.store(), parent) {
                    if fun_full_signature(self.factory.store(), fun).is_none() {
                        if let Some(param) =
                            find_matching_parameter(self.factory.store(), fun, tag, js_doc)
                        {
                            let (param_type, param_question) = {
                                let s = self.factory.store();
                                let d = s
                                    .node(param)
                                    .as_parameter_declaration()
                                    .expect("ParameterDeclaration data");
                                (d.type_, d.question_token)
                            };
                            let tag_type_expression =
                                type_expression_of(self.factory.store(), tag);
                            if param_type.is_none() {
                                if let Some(te) = tag_type_expression {
                                    let inner =
                                        jsdoc_type_expression_type(self.factory.store(), te);
                                    let reparsed = self.reparse_jsdoc_type_literal(Some(inner));
                                    {
                                        let s = self.factory.store();
                                        match &mut s.node_mut(param).data {
                                            NodeData::ParameterDeclaration(d) => {
                                                d.type_ = reparsed
                                            }
                                            _ => panic!("not a ParameterDeclaration"),
                                        }
                                    }
                                }
                            }
                            if param_question.is_none() {
                                if let Some(question) = self.make_question_if_optional(tag) {
                                    {
                                        let s = self.factory.store();
                                        match &mut s.node_mut(param).data {
                                            NodeData::ParameterDeclaration(d) => {
                                                d.question_token = Some(question)
                                            }
                                            _ => panic!("not a ParameterDeclaration"),
                                        }
                                    }
                                }
                            }
                            self.finish_mutated_node(param);
                        }
                    }
                }
            }
            Kind::JSDocThisTag => {
                if let Some(fun) = get_function_like_host(self.factory.store(), parent) {
                    let params = parameters_of(self.factory.store(), fun).to_vec();
                    let needs_this_param = if params.is_empty() {
                        true
                    } else {
                        let s = self.factory.store();
                        let first_name = s.node(params[0]).name().unwrap_or(NodeId::NONE);
                        first_name == NodeId::NONE
                            || ({
                                let n = s.node(first_name);
                                n.kind != Kind::ThisKeyword && !is_this_identifier(s, first_name)
                            })
                    };
                    if needs_this_param {
                        let (tag_name, this_type_expression) = {
                            let s = self.factory.store();
                            let d = s.node(tag).as_jsdoc_this_tag().expect("JSDocThisTag data");
                            (d.tag_name, d.type_expression)
                        };
                        let this_ident = self.factory.new_identifier("this");
                        let this_param = self.factory.new_parameter_declaration(
                            None, /* decorators */
                            None, /* modifiers */
                            this_ident,
                            None, /* questionToken */
                            None, /* type */
                            None, /* initializer */
                        );
                        if this_type_expression != NodeId::NONE {
                            let inner = jsdoc_type_expression_type(
                                self.factory.store(),
                                this_type_expression,
                            );
                            let cloned = self.add_deep_clone_reparse(Some(inner));
                            {
                                let s = self.factory.store();
                                match &mut s.node_mut(this_param).data {
                                    NodeData::ParameterDeclaration(d) => d.type_ = cloned,
                                    _ => panic!("not a ParameterDeclaration"),
                                }
                            }
                        }
                        self.finish_reparsed_node(this_param, tag_name);

                        let mut new_params: Vec<NodeId> = Vec::with_capacity(params.len() + 1);
                        new_params.push(this_param);
                        new_params.extend(params.iter());

                        // Go: `fun.ParameterList().Loc` (nil-deref if the
                        // list is absent — mirrored by the undefined range).
                        let params_loc = fun_parameter_list(self.factory.store(), fun)
                            .map(|l| l.loc)
                            .unwrap_or_else(TextRange::undefined);
                        let list = self.new_node_list(params_loc, new_params);
                        set_fun_parameters(self.factory.store(), fun, Some(list));
                        self.finish_mutated_node(fun);
                    }
                }
            }
            Kind::JSDocReturnTag => {
                if let Some(fun) = get_function_like_host(self.factory.store(), parent) {
                    if fun_full_signature(self.factory.store(), fun).is_none() {
                        let fun_has_no_type = fun_type(self.factory.store(), fun).is_none();
                        let te = type_expression_of(self.factory.store(), tag);
                        if fun_has_no_type {
                            if let Some(te) = te {
                                let inner = {
                                    let s = self.factory.store();
                                    jsdoc_type_expression_type(s, te)
                                };
                                let cloned = self.add_deep_clone_reparse(Some(inner));
                                set_fun_type(self.factory.store(), fun, cloned);
                                self.finish_mutated_node(fun);
                            }
                        }
                    }
                }
            }
            Kind::JSDocReadonlyTag
            | Kind::JSDocPrivateTag
            | Kind::JSDocPublicTag
            | Kind::JSDocProtectedTag
            | Kind::JSDocOverrideTag => {
                let mut parent = parent;
                let parent_kind = {
                    let s = self.factory.store();
                    if s.node(parent).kind == Kind::ExpressionStatement {
                        parent = s
                            .node(parent)
                            .as_expression_statement()
                            .expect("ExpressionStatement data")
                            .expression;
                    }
                    s.node(parent).kind
                };
                match parent_kind {
                    Kind::MethodDeclaration | Kind::GetAccessor | Kind::SetAccessor => {
                        // In object literals these aren't class-like members, so JSDoc modifiers like @override
                        // or @readonly aren't real modifiers there; reparsing them produces spurious grammar errors (#4437).
                        if self.parsing_contexts & (1 << PC_OBJECT_LITERAL_MEMBERS) != 0 {
                            return;
                        }
                        // Go: fallthrough into the shared modifier case.
                        self.apply_jsdoc_modifier_tag(tag, parent, tag_kind);
                    }
                    Kind::PropertyDeclaration | Kind::Constructor | Kind::BinaryExpression => {
                        self.apply_jsdoc_modifier_tag(tag, parent, tag_kind);
                    }
                    _ => {}
                }
            }
            Kind::JSDocImplementsTag => {
                let class_like = {
                    let s = self.factory.store();
                    is_class_like(s.node(parent))
                };
                if class_like {
                    let class_name = class_name_of(self.factory.store(), tag);
                    let clause_ids: Vec<NodeId> = class_like_heritage_clauses(
                        self.factory.store(),
                        parent,
                    )
                    .map(|clauses| clauses.nodes.to_vec())
                    .unwrap_or_default();
                    let existing_implements: Option<NodeId> = clause_ids
                        .iter()
                        .copied()
                        .find(|&clause| {
                            let s = self.factory.store();
                            s.node(clause)
                                .as_heritage_clause()
                                .expect("HeritageClause data")
                                .token
                                == Kind::ImplementsKeyword
                        });
                    if let Some(implements_clause) = existing_implements {
                        let cloned = self
                            .add_deep_clone_reparse(Some(class_name))
                            .expect("DeepCloneReparse preserves the node");
                        append_heritage_clause_type(
                            self.factory.store(),
                            implements_clause,
                            cloned,
                        );
                        self.finish_mutated_node(implements_clause);
                        return;
                    }
                    let class_name_loc = {
                        let s = self.factory.store();
                        s.node(class_name).loc
                    };
                    let cloned_class_name = self
                        .add_deep_clone_reparse(Some(class_name))
                        .expect("DeepCloneReparse preserves the node");
                    let types_list = self.new_node_list(class_name_loc, vec![cloned_class_name]);
                    let heritage_clause = self
                        .factory
                        .new_heritage_clause(Kind::ImplementsKeyword, Some(types_list));
                    self.finish_reparsed_node(heritage_clause, class_name);
                    // Go: `if class.HeritageClauses == nil { heritageClauses
                    // = p.newNodeList(class_name.Loc, [heritageClause]);
                    // class.HeritageClauses = heritageClauses } else { append }`.
                    if class_like_heritage_clauses(self.factory.store(), parent).is_none() {
                        let list = self.new_node_list(class_name_loc, vec![heritage_clause]);
                        set_class_heritage_clauses(self.factory.store(), parent, Some(list));
                    } else {
                        append_class_heritage_clause(self.factory.store(), parent, heritage_clause);
                    }
                    self.finish_mutated_node(parent);
                }
            }
            Kind::JSDocAugmentsTag => {
                let class_like = {
                    let s = self.factory.store();
                    is_class_like(s.node(parent))
                };
                if class_like {
                    let clause_ids: Vec<NodeId> = class_like_heritage_clauses(
                        self.factory.store(),
                        parent,
                    )
                    .map(|clauses| clauses.nodes.to_vec())
                    .unwrap_or_default();
                    let extends_clause: Option<NodeId> = clause_ids
                        .iter()
                        .copied()
                        .find(|&clause| {
                            let s = self.factory.store();
                            s.node(clause)
                                .as_heritage_clause()
                                .expect("HeritageClause data")
                                .token
                                == Kind::ExtendsKeyword
                        });
                    let Some(extends_clause) = extends_clause else {
                        return;
                    };
                    let target: Option<NodeId> = {
                        let s = self.factory.store();
                        let types = s
                            .node(extends_clause)
                            .as_heritage_clause()
                            .expect("HeritageClause data")
                            .types
                            .as_ref()
                            .map(|l| l.nodes.to_vec());
                        match types {
                            Some(types) if types.len() == 1 => Some(types[0]),
                            _ => None,
                        }
                    };
                    let Some(target) = target else {
                        return;
                    };
                    let class_name = class_name_of(self.factory.store(), tag);
                    let (target_expression, source_expression, source_type_arguments) = {
                        let s = self.factory.store();
                        let t = s
                            .node(target)
                            .as_expression_with_type_arguments()
                            .expect("ExpressionWithTypeArguments data");
                        let src = s
                            .node(class_name)
                            .as_expression_with_type_arguments()
                            .expect("ExpressionWithTypeArguments data");
                        (t.expression, src.expression, src.type_arguments.clone())
                    };
                    if has_same_property_access_name(
                        self.factory.store(),
                        target_expression,
                        source_expression,
                    ) {
                        let target_type_arguments = {
                            let s = self.factory.store();
                            s.node(target)
                                .as_expression_with_type_arguments()
                                .expect("ExpressionWithTypeArguments data")
                                .type_arguments
                                .clone()
                        };
                        if target_type_arguments.is_none() {
                            if let Some(source_type_arguments) = source_type_arguments {
                                let mut new_arguments: Vec<NodeId> =
                                    Vec::with_capacity(source_type_arguments.nodes.len());
                                for &arg in source_type_arguments.nodes.iter() {
                                    new_arguments.push(
                                        self.add_deep_clone_reparse(Some(arg))
                                            .expect("DeepCloneReparse preserves the node"),
                                    );
                                }
                                let list =
                                    self.new_node_list(source_type_arguments.loc, new_arguments);
                                {
                                    let s = self.factory.store();
                                    match &mut s.node_mut(target).data {
                                        NodeData::ExpressionWithTypeArguments(d) => {
                                            d.type_arguments = Some(list)
                                        }
                                        _ => panic!("not an ExpressionWithTypeArguments"),
                                    }
                                }
                                self.finish_mutated_node(target);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Shared body of Go's JSDoc modifier-tag case (readonly/private/public/
    /// protected/override): build the keyword modifier and rewrite the host's
    /// modifier list. (Go inlines these statements in reparseHosted's
    /// fallthrough arms; the Rust port shares one helper — same statements,
    /// same order.)
    fn apply_jsdoc_modifier_tag(&mut self, tag: NodeId, parent: NodeId, tag_kind: Kind) {
        let keyword = match tag_kind {
            Kind::JSDocReadonlyTag => Kind::ReadonlyKeyword,
            Kind::JSDocPrivateTag => Kind::PrivateKeyword,
            Kind::JSDocPublicTag => Kind::PublicKeyword,
            Kind::JSDocProtectedTag => Kind::ProtectedKeyword,
            Kind::JSDocOverrideTag => Kind::OverrideKeyword,
            _ => return,
        };
        let (tag_loc, parent_modifiers) = {
            let s = self.factory.store();
            (s.node(tag).loc, s.node(parent).modifiers().cloned())
        };
        let modifier = self.factory.new_modifier(keyword);
        {
            let s = self.factory.store();
            let n = s.node_mut(modifier);
            n.loc = tag_loc;
            n.flags = self.context_flags | NodeFlags::REPARSED;
        }
        let (nodes, loc) = match parent_modifiers {
            None => (vec![modifier], tag_loc),
            Some(modifiers) => {
                let mut nodes: Vec<NodeId> = modifiers.nodes.to_vec();
                nodes.push(modifier);
                (nodes, modifiers.loc)
            }
        };
        let list = self.new_modifier_list(loc, nodes);
        set_modifiers(self.factory.store(), parent, list);
        self.finish_mutated_node(parent);
    }

    /// Go: `func (p *Parser) makeQuestionIfOptional(parameter *ast.JSDocParameterOrPropertyTag) *ast.Node`.
    fn make_question_if_optional(&mut self, parameter_tag: NodeId) -> Option<NodeId> {
        let (is_bracketed, type_expression, tag_loc) = {
            let s = self.factory.store();
            let n = s.node(parameter_tag);
            let d = n
                .as_jsdoc_parameter_or_property_tag()
                .expect("JSDocParameterOrPropertyTag data");
            (d.is_bracketed, d.type_expression, n.loc)
        };
        let is_optional_type = type_expression.is_some_and(|te| {
            let s = self.factory.store();
            s.node(jsdoc_type_expression_type(s, te)).kind == Kind::JSDocOptionalType
        });
        if is_bracketed || is_optional_type {
            let question_token = self.factory.new_token(Kind::QuestionToken);
            {
                let s = self.factory.store();
                let n = s.node_mut(question_token);
                n.loc = tag_loc;
                n.flags = self.context_flags | NodeFlags::REPARSED;
            }
            return Some(question_token);
        }
        None
    }

    /// Go: `func (p *Parser) makeNewCast(t *ast.TypeNode, e *ast.Node, isAssertion bool) *ast.Node`.
    fn make_new_cast(&mut self, t: NodeId, e: NodeId, is_assertion: bool) -> NodeId {
        let assert = if is_assertion {
            self.factory.new_as_expression(e, t)
        } else {
            self.factory.new_satisfies_expression(e, t)
        };
        let (pos, end) = {
            let s = self.factory.store();
            (s.node(e).pos(), s.node(e).end())
        };
        self.finish_node_with_end(assert, pos, end)
    }

    /// Go: `func (p *Parser) createExportModifier(locationNode *ast.Node) *ast.ModifierList`.
    fn create_export_modifier(&mut self, location_node: NodeId) -> ModifierList {
        let export_modifier = self.factory.new_modifier(Kind::ExportKeyword);
        let location = {
            let s = self.factory.store();
            s.node(location_node).loc
        };
        {
            let s = self.factory.store();
            let n = s.node_mut(export_modifier);
            n.loc = location;
            n.flags = self.context_flags | NodeFlags::REPARSED;
        }
        let nodes = vec![export_modifier];
        self.new_modifier_list(location, nodes)
    }

    /// getInnermostNameOfJSDocNamespace returns the innermost identifier from a
    /// JSDoc namespace chain (ModuleDeclaration). For a simple identifier, it returns
    /// the identifier itself. For "A.B.C", it returns the identifier "C".
    ///
    /// Go: `func (p *Parser) getInnermostNameOfJSDocNamespace(fullName *ast.Node) *ast.Node`.
    fn get_innermost_name_of_jsdoc_namespace(
        &mut self,
        full_name: Option<NodeId>,
    ) -> Option<NodeId> {
        let mut full_name = full_name?;
        loop {
            let (next, name) = {
                let s = self.factory.store();
                let n = s.node(full_name);
                if n.kind != Kind::ModuleDeclaration {
                    return Some(full_name);
                }
                let d = n.as_module_declaration().expect("ModuleDeclaration data");
                (d.body, n.name())
            };
            match next {
                None => return name,
                Some(body) => full_name = body,
            }
        }
    }

    /// wrapInJSDocNamespace wraps a statement (typically a type alias) in namespace
    /// declarations corresponding to a JSDoc dotted name. For example, given name
    /// "A.B.C" and a type alias for C, this produces:
    ///
    /// (Go comment, tab-indented there) namespace A { namespace B { type C = ... } }
    ///
    /// If the name is a simple identifier (not a ModuleDeclaration), it returns the
    /// statement as-is.
    ///
    /// Go: `func (p *Parser) wrapInJSDocNamespace(fullName *ast.Node, statement *ast.Node, nested bool) *ast.Node`.
    fn wrap_in_jsdoc_namespace(
        &mut self,
        full_name: Option<NodeId>,
        statement: NodeId,
        nested: bool,
    ) -> NodeId {
        let Some(full_name) = full_name else {
            return statement;
        };
        let full_name_is_namespace = {
            let s = self.factory.store();
            is_module_declaration(s.node(full_name))
        };
        if !full_name_is_namespace {
            return statement;
        }
        // Recursively wrap from outermost to innermost. Inner namespaces always get an export modifier
        // so members are accessible via dotted access from outside. The outermost namespace is treated as
        // exported only in module files via IsImplicitlyExportedJSDocDeclaration (in the binder), so it
        // does not get an explicit export modifier here.
        let body = {
            let s = self.factory.store();
            s.node(full_name)
                .as_module_declaration()
                .expect("ModuleDeclaration data")
                .body
        };
        let wrapped = self.wrap_in_jsdoc_namespace(body, statement, /*nested:*/ true);
        let full_name_loc = {
            let s = self.factory.store();
            s.node(full_name).loc
        };
        let block_statements = self.new_node_list(full_name_loc, vec![wrapped]);
        let block = self.factory.new_module_block(Some(block_statements));
        self.finish_reparsed_node(block, full_name);
        let modifiers = if nested {
            Some(self.create_export_modifier(full_name))
        } else {
            None
        };
        let name = {
            let s = self.factory.store();
            s.node(full_name)
                .as_module_declaration()
                .expect("ModuleDeclaration data")
                .name
        };
        let cloned_name = self
            .add_deep_clone_reparse(Some(name))
            .expect("DeepCloneReparse preserves the node");
        let result = self.factory.new_module_declaration(
            modifiers,
            Kind::NamespaceKeyword,
            cloned_name,
            None,
            Some(block),
        );
        self.finish_reparsed_node(result, full_name);
        self.reparsed_clones.push(result);
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // Node-dispatch read helpers used by the hosted cases (mirrors of Go's
    // generated Node.Type()/Node.Expression()/Node.Initializer()).
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `parent.Type()` — the Node.Type() dispatch for the @type case's
    /// parent kinds (narrowed; panics otherwise like Go).
    fn node_type_annotation(&mut self, node: NodeId) -> Option<NodeId> {
        let s = self.factory.store();
        let n = s.node(node);
        match &n.data {
            NodeData::VariableDeclaration(d) => d.type_,
            NodeData::ParameterDeclaration(d) => d.type_,
            NodeData::PropertyDeclaration(d) => d.type_,
            NodeData::PropertyAssignment(d) => Some(d.type_).filter(|id| *id != NodeId::NONE),
            NodeData::ShorthandPropertyAssignment(d) => {
                Some(d.type_).filter(|id| *id != NodeId::NONE)
            }
            NodeData::GetAccessorDeclaration(d) => d.type_,
            NodeData::ExportAssignment(d) => Some(d.type_).filter(|id| *id != NodeId::NONE),
            _ => {
                let kind = n.kind;
                panic!("Unhandled case in Node.Type: {}", kind.name())
            }
        }
    }

    /// Go: `parent.Expression()` (narrowed dispatch).
    fn node_expression(&mut self, node: NodeId) -> Option<NodeId> {
        let s = self.factory.store();
        let n = s.node(node);
        match &n.data {
            NodeData::ReturnStatement(d) => d.expression,
            NodeData::ParenthesizedExpression(d) => Some(d.expression),
            NodeData::ExpressionStatement(d) => Some(d.expression),
            NodeData::ExportAssignment(d) => Some(d.expression),
            _ => {
                let kind = n.kind;
                panic!("Unhandled case in Node.Expression: {}", kind.name())
            }
        }
    }

    /// Go: `parent.Initializer()` (narrowed dispatch).
    fn node_initializer(&mut self, node: NodeId) -> Option<NodeId> {
        let s = self.factory.store();
        let n = s.node(node);
        match &n.data {
            NodeData::VariableDeclaration(d) => d.initializer,
            NodeData::PropertyDeclaration(d) => d.initializer,
            NodeData::PropertyAssignment(d) => Some(d.initializer).filter(|id| *id != NodeId::NONE),
            _ => {
                let kind = n.kind;
                panic!("Unhandled case in Node.Initializer: {}", kind.name())
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// reparser.go — package-scope free functions
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func findMatchingParameter(fun *ast.Node, parameterTag *ast.JSDocParameterOrPropertyTag, jsDoc *ast.Node) (*ast.ParameterDeclaration, bool)`.
///
/// PORT: Go identifies the parameter tag by `*JSDocParameterOrPropertyTag`
/// pointer identity; the port passes the tag's `NodeId` (the same identity
/// through the arena).
fn find_matching_parameter(
    store: &dyn NodeStore,
    fun: NodeId,
    parameter_tag: NodeId,
    js_doc: NodeId,
) -> Option<NodeId> {
    let mut tag_index: i64 = -1;
    let mut param_count: i64 = -1;
    let js_doc_tags: Vec<NodeId> = store
        .node(js_doc)
        .as_jsdoc()
        .expect("JSDoc data")
        .tags
        .as_ref()
        .map(|t| t.nodes.to_vec())
        .unwrap_or_default();
    for &tag in js_doc_tags.iter() {
        if store.node(tag).kind == Kind::JSDocParameterTag {
            param_count += 1;
            if tag == parameter_tag {
                tag_index = param_count;
                break;
            }
        }
    }
    for (parameter_index, &parameter) in parameters_of(store, fun).iter().enumerate() {
        let name = store.node(parameter).name();
        let name_kind = name.map(|n| store.node(n).kind).unwrap_or(Kind::Unknown);
        if name_kind == Kind::Identifier {
            // Go guards the tag-name Text() read with
            // `parameterTag.Name().Kind == ast.KindIdentifier` (the name may
            // be a QualifiedName, e.g. `@param ns.b`) — the tag comparison
            // only applies to plain-identifier tag names.
            let tag_name_kind = store
                .node(
                    store
                        .node(parameter_tag)
                        .as_jsdoc_parameter_or_property_tag()
                        .expect("JSDocParameterOrPropertyTag data")
                        .name,
                )
                .kind;
            if tag_name_kind == Kind::Identifier {
                let parameter_text = name.map(|n| node_text(store, n)).unwrap_or("");
                let tag_name = store
                    .node(parameter_tag)
                    .as_jsdoc_parameter_or_property_tag()
                    .expect("JSDocParameterOrPropertyTag data")
                    .name;
                let tag_name_text = node_text(store, tag_name);
                if (parameter_text == tag_name_text)
                    || (parameter_index as i64 == tag_index && tag_name_text.is_empty())
                {
                    return Some(parameter);
                }
            }
        } else if parameter_index as i64 == tag_index {
            return Some(parameter);
        }
    }
    None
}

/// Go: `func skipSatisfiesExpressions(node *ast.Node) *ast.Node`.
fn skip_satisfies_expressions(store: &dyn NodeStore, node: Option<NodeId>) -> Option<NodeId> {
    let mut node = node;
    while let Some(current) = node {
        let n = store.node(current);
        if n.kind != Kind::SatisfiesExpression {
            break;
        }
        node = Some(
            n.as_satisfies_expression()
                .expect("SatisfiesExpression data")
                .expression,
        );
    }
    node
}

/// Go: `func getFunctionLikeHost(host *ast.Node) *ast.Node`.
fn get_function_like_host(store: &dyn NodeStore, host: NodeId) -> Option<NodeId> {
    let mut fun: Option<NodeId> = Some(host);
    let host_kind = store.node(host).kind;
    match host_kind {
        Kind::VariableStatement => {
            let dl = store
                .node(host)
                .as_variable_statement()
                .expect("VariableStatement data")
                .declaration_list;
            if dl != NodeId::NONE {
                let nodes: Vec<NodeId> = store
                    .node(dl)
                    .as_variable_declaration_list()
                    .expect("VariableDeclarationList data")
                    .declarations
                    .as_ref()
                    .map(|l| l.nodes.to_vec())
                    .unwrap_or_default();
                if !nodes.is_empty() {
                    fun = store
                        .node(nodes[0])
                        .as_variable_declaration()
                        .expect("VariableDeclaration data")
                        .initializer;
                }
            }
        }
        Kind::PropertyAssignment => {
            let initializer = store
                .node(host)
                .as_property_assignment()
                .expect("PropertyAssignment data")
                .initializer;
            fun = if initializer == NodeId::NONE {
                None
            } else {
                Some(initializer)
            };
        }
        Kind::PropertyDeclaration => {
            fun = store
                .node(host)
                .as_property_declaration()
                .expect("PropertyDeclaration data")
                .initializer;
        }
        Kind::ExportAssignment => {
            fun = Some(expression_of_statement(store, host));
        }
        // Go: `fun = host.Expression()` — a valueless `return` yields nil,
        // which the skip/IsFunctionLike chain below treats as no host.
        Kind::ReturnStatement => {
            fun = store
                .node(host)
                .as_return_statement()
                .expect("ReturnStatement data")
                .expression;
        }
        Kind::ExpressionStatement => {
            let expression = expression_of_statement(store, host);
            fun = Some(get_right_most_assigned_expression(store, expression));
        }
        _ => {}
    }
    let fun = skip_satisfies_expressions(store, fun)?;
    if is_function_like(store.node(fun)) {
        return Some(fun);
    }
    None
}

/// Go: `host.Expression()` for the ExportAssignment/ExpressionStatement arms
/// of getFunctionLikeHost (both carry a non-optional expression; the
/// ReturnStatement arm reads its optional payload directly).
fn expression_of_statement(store: &dyn NodeStore, host: NodeId) -> NodeId {
    let n = store.node(host);
    match &n.data {
        NodeData::ExportAssignment(d) => d.expression,
        NodeData::ExpressionStatement(d) => d.expression,
        _ => panic!("Unhandled case in Node.Expression: {}", n.kind_string()),
    }
}

/// Go: `param.Type() == nil` over the function-host parameter check
/// (`core.Every` predicate of the @type case).
fn node_type_of_hosted_param(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        NodeData::ParameterDeclaration(d) => d.type_,
        _ => {
            let kind = n.kind;
            panic!("Unhandled case in Node.Type: {}", kind.name())
        }
    }
}
