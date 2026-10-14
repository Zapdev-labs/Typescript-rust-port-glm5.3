// Minimal subset of tsc/internal/ast/utilities.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: utilities.go (~150KB) is ported wholesale in a separate M2 task; this
// module carries only the helpers the M2 hand-written surface needs, each
// with its Go symbol documented. Deduplicate into the full utilities port.

use crate::{Kind, Node, NodeId, NodeFlags};

/// Go: `func IsSourceFileJS(file *SourceFile) bool`.
pub fn is_source_file_js(script_kind: tsc_core::scriptkind::ScriptKind) -> bool {
    script_kind == tsc_core::scriptkind::ScriptKind::JS
        || script_kind == tsc_core::scriptkind::ScriptKind::JSX
}

/// Go: `func IsInJSFile(node *Node) bool` (flag half only — the nil check is
/// expressed by the caller's `Option`).
pub fn is_in_js_file(node: &Node) -> bool {
    node.flags.intersects(NodeFlags::JAVASCRIPT_FILE)
}

/// Go: `func IsOptionalChain(node *Node) bool`.
pub fn is_optional_chain(node: &Node) -> bool {
    if node.flags.intersects(NodeFlags::OPTIONAL_CHAIN) {
        return matches!(
            node.kind,
            Kind::PropertyAccessExpression
                | Kind::ElementAccessExpression
                | Kind::CallExpression
                | Kind::NonNullExpression
        );
    }
    false
}

/// Go: `func IsMethodOrAccessor(node *Node) bool`.
pub fn is_method_or_accessor(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::MethodDeclaration | Kind::GetAccessor | Kind::SetAccessor
    )
}

/// Go: `func IsPrivateIdentifierClassElementDeclaration(node *Node) bool`.
pub fn is_private_identifier_class_element_declaration(store: &dyn crate::NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    (crate::is_property_declaration(n) || is_method_or_accessor(n))
        && crate::is_private_identifier(store.node(n.name().expect("class element has a name")))
}

/// Go: `func IsJsxOpeningLikeElement(node *Node) bool` (utilities.go — the
/// kind guard `is_jsx_fragment` comes from the generated kind table; Go's
/// `IsJsxFragment` is generated in kind_generated.go the same way).
pub fn is_jsx_opening_like_element(node: &Node) -> bool {
    matches!(node.kind, Kind::JsxOpeningElement | Kind::JsxSelfClosingElement)
}

/// Go: `func GetSourceFileOfNode(node *Node) *SourceFile` — walks the parent
/// chain (the packed `file_id` in `NodeId` resolves the owning file).
pub fn get_source_file_of_node(store: &dyn crate::NodeStore, node: NodeId) -> Option<&crate::SourceFile> {
    let mut id = node;
    loop {
        let n = store.node(id);
        if n.kind == Kind::SourceFile {
            return Some(store.file(id.file_id()));
        }
        let parent = n.parent.get();
        if parent == NodeId::NONE {
            return None;
        }
        id = parent;
    }
}

/// Go: `func SetParentInChildren(node *Node)` (via `newParentInChildrenSetter`
/// in the pooled closure) — pre-order walk assigning `parent` on every child;
/// the root's own parent is left untouched (Go's initial state.parent is nil).
pub fn set_parent_in_children(store: &mut dyn crate::NodeStore, root: NodeId) {
    fn walk(store: &mut dyn crate::NodeStore, id: NodeId, parent: Option<NodeId>) {
        if let Some(p) = parent {
            store.node_mut(id).parent.set(p);
        }
        let mut children = Vec::new();
        store.node(id).for_each_child(&mut |c| {
            children.push(c);
            false
        });
        for c in children {
            walk(store, c, Some(id));
        }
    }
    walk(store, root, None);
}

/// Go: `func TryGetAmbientModuleNameFromSymbolName(s string) (string, bool)`.
pub fn try_get_ambient_module_name_from_symbol_name(s: &str) -> Option<String> {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return Some(s[1..s.len() - 1].to_string());
    }
    let pattern_prefix = format!("{}\"", crate::symbol::INTERNAL_SYMBOL_NAME_PREFIX);
    let rest = s.strip_prefix(&pattern_prefix)?;
    // Go: strings.LastIndex(rest, "\"pattern@")
    let marker_index = rest.rfind("\"pattern@")?;
    if marker_index < 1 {
        return None;
    }
    Some(rest[..marker_index].to_string())
}

/// Go: `func findChildNode(root *Node, check func(*Node) bool) *Node`
/// (parseoptions.go) — pre-order search stopping at the first match.
///
/// PORT: `check` receives the child handle (Go's `*Node`); resolve through
/// the store.
pub fn find_child_node(store: &dyn crate::NodeStore, root: NodeId, check: &mut dyn FnMut(NodeId) -> bool) -> Option<NodeId> {
    fn walk(
        store: &dyn crate::NodeStore,
        id: NodeId,
        check: &mut dyn FnMut(NodeId) -> bool,
        result: &mut Option<NodeId>,
    ) -> bool {
        if check(id) {
            *result = Some(id);
            return true;
        }
        let mut children = Vec::new();
        store.node(id).for_each_child(&mut |c| {
            children.push(c);
            false
        });
        for c in children {
            if walk(store, c, check, result) {
                return true;
            }
        }
        false
    }
    let mut result = None;
    walk(store, root, check, &mut result);
    result
}

/// Go: `func IsImportMeta(node *Node) bool` (utilities.go).
///
/// PORT: takes the store — Go reads `node.Name().Text()` through the pointer;
/// the port resolves the name handle (node text spans source text, so the
/// identifier's text is the node's `loc` slice of the file, exactly Go's
/// `NodeText`).
pub fn is_import_meta(store: &dyn crate::NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind == Kind::MetaProperty {
        let d = n.as_meta_property().expect("MetaProperty data");
        return d.keyword_token == Kind::ImportKeyword
            && n.name().is_some_and(|name| crate::node_text(store, name) == "meta");
    }
    false
}

/// Go: `func IsAmbientModuleSymbolName(s string) bool`.
pub fn is_ambient_module_symbol_name(s: &str) -> bool {
    try_get_ambient_module_name_from_symbol_name(s).is_some()
}

// ────────────────────────────────────────────────────────────────────────────
// PORT (parser-wave additions): the utilities.go subset the M3 parser wave
// calls that were not yet ported. Each mirrors its Go body exactly; dedup
// into the wholesale utilities.go port when that task lands (tsc-scanner's
// go_shims::node_is_missing is the same Go function — it carries its own
// dedup note and now has this ast-side home to fold into).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func NodeIsMissing(node *Node) bool` — "Determines if a node is
/// missing (either `nil` or empty)". The `node == nil` half is expressed by
/// the caller's `Option`/`NodeId::NONE`; a resolved node checks the same
/// empty-non-negative-range condition (KindEndOfFile exempt, as in Go).
pub fn node_is_missing(node: &Node) -> bool {
    node.loc.pos() == node.loc.end()
        && node.loc.pos() >= 0
        && node.kind != Kind::EndOfFile
}

/// Go: `func NodeIsPresent(node *Node) bool` — "Determines if a node is
/// present" (the negation of [`node_is_missing`], same `nil` note).
pub fn node_is_present(node: &Node) -> bool {
    !node_is_missing(node)
}

/// Go: `func ModifiersToFlags(modifiers []*Node) ModifierFlags` — ORs
/// `ModifierToFlag` over the modifier nodes. `ModifierToFlag` lives in
/// visitor.rs (crate::modifier_to_flag); an already-built `ModifierList`
/// carries the same value in `modifier_flags` (see NodeFactory::
/// new_modifier_list).
pub fn modifiers_to_flags(store: &dyn crate::NodeStore, modifiers: &[NodeId]) -> crate::ModifierFlags {
    let mut flags = crate::ModifierFlags::NONE;
    for &modifier in modifiers {
        if modifier == NodeId::NONE {
            continue; // Go skips nil modifiers (as new_modifier_list does)
        }
        flags |= crate::visitor::modifier_to_flag(store.node(modifier).kind);
    }
    flags
}

/// Go: `func TagNamesAreEquivalent(lhs *Expression, rhs *Expression) bool`
/// — the JSX tag-name equivalence used by the parser's element/fragment
/// recovery. Both tags are always non-nil at the Go call sites (typed
/// payload fields), so the port takes handles; the final `panic!` mirrors
/// Go's "Unhandled case in TagNamesAreEquivalent".
pub fn tag_names_are_equivalent(store: &dyn crate::NodeStore, lhs: NodeId, rhs: NodeId) -> bool {
    let l = store.node(lhs);
    let r = store.node(rhs);
    if l.kind != r.kind {
        return false;
    }
    match l.kind {
        Kind::Identifier => crate::node_text(store, lhs) == crate::node_text(store, rhs),
        Kind::ThisKeyword => true,
        Kind::JsxNamespacedName => {
            let ld = l.as_jsx_namespaced_name().expect("JsxNamespacedName data");
            let rd = r.as_jsx_namespaced_name().expect("JsxNamespacedName data");
            crate::node_text(store, ld.namespace) == crate::node_text(store, rd.namespace)
                && crate::node_text(store, ld.name) == crate::node_text(store, rd.name)
        }
        Kind::PropertyAccessExpression => {
            let ld = l.as_property_access_expression().expect("PropertyAccessExpression data");
            let rd = r.as_property_access_expression().expect("PropertyAccessExpression data");
            crate::node_text(store, ld.name) == crate::node_text(store, rd.name)
                && tag_names_are_equivalent(store, ld.expression, rd.expression)
        }
        _ => panic!("Unhandled case in TagNamesAreEquivalent: {}", l.kind_string()),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// PORT (reparser wave): the utilities.go subset the JSDoc reparser
// (tsc/internal/parser/reparser.go) and the assignment-declaration
// classification need. Each mirrors its Go body exactly; dedup into the
// wholesale utilities.go port when that task lands. (binder/ast_shims.rs
// carries its own copies of the GetAssignmentDeclarationKind family from
// the binder wave — fold those onto this canonical home then.)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (n *Node) Expression() *Node` — the narrow dispatch the reparse
/// paths actually hit (the generated Go method covers every ExpressionBase
/// kind; the port mirrors the used subset and panics otherwise, like Go's
/// generated `default`).
fn expression_of(store: &dyn crate::NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        crate::NodeData::PropertyAccessExpression(d) => Some(d.expression),
        crate::NodeData::ElementAccessExpression(d) => Some(d.expression),
        crate::NodeData::CallExpression(d) => Some(d.expression),
        crate::NodeData::NewExpression(d) => Some(d.expression),
        crate::NodeData::ExpressionStatement(d) => Some(d.expression),
        crate::NodeData::ParenthesizedExpression(d) => Some(d.expression),
        crate::NodeData::ReturnStatement(d) => d.expression,
        crate::NodeData::ExportAssignment(d) => Some(d.expression),
        crate::NodeData::AsExpression(d) => Some(d.expression),
        crate::NodeData::SatisfiesExpression(d) => Some(d.expression),
        crate::NodeData::NonNullExpression(d) => Some(d.expression),
        crate::NodeData::TypeAssertion(d) => Some(d.expression),
        crate::NodeData::BinaryExpression(d) => Some(d.right),
        _ => panic!("Unhandled case in Node.Expression: {}", n.kind_string()),
    }
}

/// Go: `func (n *Node) Arguments() []*Node` — narrow dispatch (CallExpression
/// is the only reparse-reachable kind).
fn arguments_of(store: &dyn crate::NodeStore, node: NodeId) -> &[NodeId] {
    let n = store.node(node);
    match &n.data {
        crate::NodeData::CallExpression(d) => {
            d.arguments.as_ref().map(|l| &l.nodes[..]).unwrap_or(&[])
        }
        _ => panic!("Unhandled case in Node.Arguments: {}", n.kind_string()),
    }
}

/// Go: `func IsAccessExpression(node *Node) bool` (generated guard; the Go
/// ast_generated.go equivalent is hand-mirrored here).
pub fn is_access_expression(node: &Node) -> bool {
    matches!(node.kind, crate::Kind::PropertyAccessExpression | crate::Kind::ElementAccessExpression)
}

/// Go: `func SkipParentheses(node *Expression) *Expression`
/// (= `SkipOuterExpressions(node, OEKParentheses)`).
pub fn skip_parentheses(store: &dyn crate::NodeStore, node: NodeId) -> NodeId {
    let mut node = node;
    loop {
        let n = store.node(node);
        if n.kind == crate::Kind::ParenthesizedExpression {
            node = expression_of(store, node).expect("ParenthesizedExpression expression");
        } else {
            return node;
        }
    }
}

/// Go: `func isLeftHandSideExpressionKind(kind Kind) bool`.
fn is_left_hand_side_expression_kind(kind: crate::Kind) -> bool {
    matches!(
        kind,
        crate::Kind::PropertyAccessExpression
            | crate::Kind::ElementAccessExpression
            | crate::Kind::NewExpression
            | crate::Kind::CallExpression
            | crate::Kind::JsxElement
            | crate::Kind::JsxSelfClosingElement
            | crate::Kind::JsxFragment
            | crate::Kind::TaggedTemplateExpression
            | crate::Kind::ArrayLiteralExpression
            | crate::Kind::ParenthesizedExpression
            | crate::Kind::ObjectLiteralExpression
            | crate::Kind::ClassExpression
            | crate::Kind::FunctionExpression
            | crate::Kind::Identifier
            | crate::Kind::PrivateIdentifier
            | crate::Kind::RegularExpressionLiteral
            | crate::Kind::NumericLiteral
            | crate::Kind::BigIntLiteral
            | crate::Kind::StringLiteral
            | crate::Kind::NoSubstitutionTemplateLiteral
            | crate::Kind::TemplateExpression
            | crate::Kind::FalseKeyword
            | crate::Kind::NullKeyword
            | crate::Kind::ThisKeyword
            | crate::Kind::TrueKeyword
            | crate::Kind::SuperKeyword
            | crate::Kind::NonNullExpression
            | crate::Kind::ExpressionWithTypeArguments
            | crate::Kind::MetaProperty
            | crate::Kind::ImportKeyword
            | crate::Kind::MissingDeclaration
    )
}

/// Go: `func IsLeftHandSideExpression(node *Node) bool`.
pub fn is_left_hand_side_expression(node: &Node) -> bool {
    // Go: SkipPartiallyEmittedExpressions first — a PartiallyEmittedExpression
    // never reaches the pure-kind check in the reparse paths' inputs, so the
    // port applies the kind check directly (the skip only descends
    // KindPartiallyEmittedExpression chains).
    is_left_hand_side_expression_kind(node.kind)
}

/// Go: `func isUnaryExpressionKind(kind Kind) bool` (ast/utilities.go:418).
fn is_unary_expression_kind(kind: crate::Kind) -> bool {
    matches!(
        kind,
        crate::Kind::PrefixUnaryExpression
            | crate::Kind::PostfixUnaryExpression
            | crate::Kind::DeleteExpression
            | crate::Kind::TypeOfExpression
            | crate::Kind::VoidExpression
            | crate::Kind::AwaitExpression
            | crate::Kind::TypeAssertionExpression
    ) || is_left_hand_side_expression_kind(kind)
}

/// Go: `func isExpressionKind(kind Kind) bool` (ast/utilities.go:434).
fn is_expression_kind(kind: crate::Kind) -> bool {
    matches!(
        kind,
        crate::Kind::ConditionalExpression
            | crate::Kind::YieldExpression
            | crate::Kind::ArrowFunction
            | crate::Kind::BinaryExpression
            | crate::Kind::SpreadElement
            | crate::Kind::AsExpression
            | crate::Kind::OmittedExpression
            | crate::Kind::PartiallyEmittedExpression
            | crate::Kind::SatisfiesExpression
    ) || is_unary_expression_kind(kind)
}

/// Go: `func IsExpression(node *Node) bool` (ast/utilities.go:451) —
/// `isExpressionKind(SkipPartiallyEmittedExpressions(node).Kind)`.
/// PORT(FCS tranche 11): same &Node shape as
/// [`is_left_hand_side_expression`]; PartiallyEmittedExpression is in
/// isExpressionKind's own list, so the skip is a no-op for the wrapper.
pub fn is_expression(node: &Node) -> bool {
    is_expression_kind(node.kind)
}

/// Go: `func IsAssignmentExpression(node *Node, excludeCompoundAssignment bool) bool`.
pub fn is_assignment_expression(
    store: &dyn crate::NodeStore,
    node: NodeId,
    exclude_compound_assignment: bool,
) -> bool {
    let n = store.node(node);
    if n.kind == crate::Kind::BinaryExpression {
        let expr = n.as_binary_expression().expect("BinaryExpression data");
        // Go: the operator-token payload resolves through the arena.
        let operator = store.node(expr.operator_token).kind;
        return (operator == crate::Kind::EqualsToken
            || (!exclude_compound_assignment && crate::is_assignment_operator(operator)))
            && is_left_hand_side_expression_kind(store.node(expr.left).kind);
    }
    false
}

/// Go: `func GetRightMostAssignedExpression(node *Node) *Node`.
pub fn get_right_most_assigned_expression(store: &dyn crate::NodeStore, node: NodeId) -> NodeId {
    let mut node = node;
    while is_assignment_expression(store, node, /*excludeCompoundAssignment:*/ false) {
        node = store
            .node(node)
            .as_binary_expression()
            .expect("BinaryExpression data")
            .right;
    }
    node
}

/// Go: `func IsThisIdentifier(node *Node) bool`.
///
/// PORT: store-taking — Go reads `node.Text()` through the pointer; the port
/// resolves the identifier's text through the arena (same as
/// [`crate::node_text`]).
pub fn is_this_identifier(store: &dyn crate::NodeStore, node: NodeId) -> bool {
    crate::is_identifier(store.node(node)) && crate::node_text(store, node) == "this"
}

/// Go: `func isFunctionLikeDeclarationKind(kind Kind) bool`.
fn is_function_like_declaration_kind(kind: crate::Kind) -> bool {
    matches!(
        kind,
        crate::Kind::FunctionDeclaration
            | crate::Kind::MethodDeclaration
            | crate::Kind::Constructor
            | crate::Kind::GetAccessor
            | crate::Kind::SetAccessor
            | crate::Kind::FunctionExpression
            | crate::Kind::ArrowFunction
    )
}

/// Go: `func IsFunctionLikeKind(kind Kind) bool`.
pub fn is_function_like_kind(kind: crate::Kind) -> bool {
    matches!(
        kind,
        crate::Kind::MethodSignature
            | crate::Kind::CallSignature
            | crate::Kind::JSDocSignature
            | crate::Kind::ConstructSignature
            | crate::Kind::IndexSignature
            | crate::Kind::FunctionType
            | crate::Kind::ConstructorType
    ) || is_function_like_declaration_kind(kind)
}

/// Go: `func IsFunctionLike(node *Node) bool` — "Determines if a node is
/// function- or signature-like."
pub fn is_function_like(node: &Node) -> bool {
    is_function_like_kind(node.kind)
}

/// Go: `func HasSamePropertyAccessName(node1, node2 *Node) bool`.
pub fn has_same_property_access_name(
    store: &dyn crate::NodeStore,
    node1: NodeId,
    node2: NodeId,
) -> bool {
    let n1 = store.node(node1);
    let n2 = store.node(node2);
    if crate::is_identifier(n1) && crate::is_identifier(n2) {
        crate::node_text(store, node1) == crate::node_text(store, node2)
    } else if crate::is_property_access_expression(n1) && crate::is_property_access_expression(n2) {
        let p1 = n1.as_property_access_expression().expect("PropertyAccessExpression data");
        let p2 = n2.as_property_access_expression().expect("PropertyAccessExpression data");
        crate::node_text(store, p1.name) == crate::node_text(store, p2.name)
            && has_same_property_access_name(store, p1.expression, p2.expression)
    } else {
        false
    }
}

/// Go: `func CompareNodePositions(n1, n2 *Node) int`.
pub fn compare_node_positions(
    store: &dyn crate::NodeStore,
    n1: NodeId,
    n2: NodeId,
) -> std::cmp::Ordering {
    let r = tsc_core::text::compare_text_ranges(&store.node(n1).loc, &store.node(n2).loc);
    r.cmp(&0)
}

/// Go: `func GetReparsedNodeForNode(node *Node) *Node` — "If the given node
/// is part of a subtree of JSDoc nodes that have been cloned into a reparsed
/// construct, return the corresponding reparsed clone in the subtree.
/// Otherwise, just return the node."
///
/// PORT: Go's nil in/nil out is expressed by the `NodeId::NONE` sentinel
/// (returns the sentinel unchanged).
pub fn get_reparsed_node_for_node(store: &dyn crate::NodeStore, node: NodeId) -> NodeId {
    if node != NodeId::NONE {
        let n = store.node(node);
        if n.flags.intersects(crate::NodeFlags::JSDOC)
            && !n.flags.intersects(crate::NodeFlags::REPARSED)
        {
            if let Some(file) = get_source_file_of_node(store, node) {
                let reparsed_clones = &file.payload().reparsed_clones;
                if !reparsed_clones.is_empty() {
                    // Go: slices.BinarySearchFunc(file.ReparsedClones, node,
                    // CompareNodePositions) — found gives the match index,
                    // otherwise the insertion point.
                    let search = reparsed_clones.binary_search_by(|probe| {
                        compare_node_positions(store, *probe, node)
                    });
                    let mut pos = match search {
                        Ok(found) => found,
                        Err(insertion) => {
                            if insertion > 0 {
                                insertion - 1
                            } else {
                                0
                            }
                        }
                    };
                    if pos >= reparsed_clones.len() {
                        pos = reparsed_clones.len() - 1;
                    }
                    let candidate = reparsed_clones[pos];
                    if store.node(node).loc.contained_by(&store.node(candidate).loc) {
                        if let Some(reparsed) = find_clone_in_node(store, candidate, node) {
                            return reparsed;
                        }
                    }
                }
            }
        }
    }
    node
}

/// Go: `func findCloneInNode(node *Node, original *Node) *Node`.
fn find_clone_in_node(
    store: &dyn crate::NodeStore,
    node: NodeId,
    original: NodeId,
) -> Option<NodeId> {
    let mut node = node;
    loop {
        let (kind, loc) = {
            let n = store.node(node);
            (n.kind, n.loc)
        };
        let o = store.node(original);
        if kind == o.kind && loc == o.loc {
            return Some(node);
        }
        let mut next: Option<NodeId> = None;
        store.node(node).for_each_child(&mut |child| {
            if next.is_none() && o.loc.contained_by(&store.node(child).loc) {
                next = Some(child);
                return true; // stop the traversal
            }
            false
        });
        node = next?;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// JSDeclarationKind (utilities.go:1525-1580)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type JSDeclarationKind int` + iota consts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
#[repr(i32)]
pub enum JSDeclarationKind {
    /// Go: `JSDeclarationKindNone JSDeclarationKind = iota`.
    #[default]
    None = 0,
    /// Go: `JSDeclarationKindModuleExports` — module.exports = expr, except
    /// for module.exports = exports.
    ModuleExports,
    /// Go: `JSDeclarationKindExportsProperty` — exports.name = expr /
    /// module.exports.name = expr.
    ExportsProperty,
    /// Go: `JSDeclarationKindThisProperty` — this.name = expr.
    ThisProperty,
    /// Go: `JSDeclarationKindProperty` — F.name = expr, F[name] = expr, in
    /// JS or TS file.
    Property,
    /// Go: `JSDeclarationKindObjectDefinePropertyValue` —
    /// Object.defineProperty(x, 'name', { value: any, writable?: boolean
    /// (false by default) }); …
    ObjectDefinePropertyValue,
    /// Go: `JSDeclarationKindObjectDefinePropertyExports` —
    /// Object.defineProperty(exports || module.exports, 'name', ...);
    ObjectDefinePropertyExports,
}

/// Go: `func IsExportsIdentifier(node *Node) bool`.
pub fn is_exports_identifier(store: &dyn crate::NodeStore, node: NodeId) -> bool {
    crate::is_identifier(store.node(node)) && crate::node_text(store, node) == "exports"
}

/// Go: `func IsModuleIdentifier(node *Node) bool`.
pub fn is_module_identifier(store: &dyn crate::NodeStore, node: NodeId) -> bool {
    crate::is_identifier(store.node(node)) && crate::node_text(store, node) == "module"
}

/// Go: `func IsModuleExportsAccessExpression(node *Node) bool`.
pub fn is_module_exports_access_expression(
    store: &dyn crate::NodeStore,
    node: NodeId,
) -> bool {
    let n = store.node(node);
    if is_access_expression(n) {
        let expr = expression_of(store, node).expect("access expression");
        if is_module_identifier(store, expr) {
            if let Some(name) = get_element_or_property_access_name(store, node) {
                return crate::node_text(store, name) == "exports";
            }
        }
    }
    false
}

/// Go: `func GetElementOrPropertyAccessName(node *Node) *Node` — "does not
/// handle signed numeric names like `a[+0]`."
pub fn get_element_or_property_access_name(
    store: &dyn crate::NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    let n = store.node(node);
    match n.kind {
        crate::Kind::PropertyAccessExpression => {
            let name = n.as_property_access_expression().expect("PropertyAccessExpression data").name;
            if crate::is_identifier(store.node(name)) {
                return Some(name);
            }
            None
        }
        crate::Kind::ElementAccessExpression => {
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

/// Go: `func IsStringLiteralLike(node *Node) bool`.
pub fn is_string_literal_like(node: &Node) -> bool {
    matches!(
        node.kind,
        crate::Kind::StringLiteral | crate::Kind::NoSubstitutionTemplateLiteral
    )
}

/// Go: `func IsStringOrNumericLiteralLike(node *Node) bool`.
pub fn is_string_or_numeric_literal_like(node: &Node) -> bool {
    is_string_literal_like(node) || crate::is_numeric_literal(node)
}

/// Go: `func IsEntityNameExpression(node *Node) bool`.
pub fn is_entity_name_expression(store: &dyn crate::NodeStore, node: NodeId) -> bool {
    is_entity_name_expression_ex(store, node, /*allowJS:*/ false)
}

/// Go: `func IsEntityNameExpressionEx(node *Node, allowJS bool) bool`.
pub fn is_entity_name_expression_ex(
    store: &dyn crate::NodeStore,
    node: NodeId,
    allow_js: bool,
) -> bool {
    let n = store.node(node);
    crate::is_identifier(n)
        || is_property_access_entity_name_expression(store, node, allow_js)
        || (allow_js
            && (n.kind == crate::Kind::ThisKeyword
                || is_element_access_entity_name_expression(store, node, allow_js)))
}

/// Go: `func IsPropertyAccessEntityNameExpression(node *Node, allowJS bool) bool`.
pub fn is_property_access_entity_name_expression(
    store: &dyn crate::NodeStore,
    node: NodeId,
    allow_js: bool,
) -> bool {
    let n = store.node(node);
    crate::is_property_access_expression(n)
        && crate::is_identifier(store.node(n.name().expect("PropertyAccessExpression name")))
        && is_entity_name_expression_ex(
            store,
            n.as_property_access_expression()
                .expect("PropertyAccessExpression data")
                .expression,
            allow_js,
        )
}

/// Go: `func isElementAccessEntityNameExpression(node *Node, allowJS bool) bool`.
fn is_element_access_entity_name_expression(
    store: &dyn crate::NodeStore,
    node: NodeId,
    allow_js: bool,
) -> bool {
    let n = store.node(node);
    crate::is_element_access_expression(n)
        && is_string_or_numeric_literal_like(
            store.node(
                n.as_element_access_expression()
                    .expect("ElementAccessExpression data")
                    .argument_expression,
            ),
        )
        && is_entity_name_expression_ex(
            store,
            n.as_element_access_expression()
                .expect("ElementAccessExpression data")
                .expression,
            allow_js,
        )
}

/// Go: `func IsBindableObjectDefinePropertyCall(node *Node) bool`.
pub fn is_bindable_object_define_property_call(
    store: &dyn crate::NodeStore,
    node: NodeId,
) -> bool {
    let args = arguments_of(store, node);
    if args.len() == 3 {
        if let Some(expr) = expression_of(store, node) {
            let e = store.node(expr);
            if crate::is_property_access_expression(e) {
                let access = e
                    .as_property_access_expression()
                    .expect("PropertyAccessExpression data");
                return crate::is_identifier(store.node(access.expression))
                    && crate::node_text(store, access.expression) == "Object"
                    && crate::node_text(store, access.name) == "defineProperty"
                    && is_string_or_numeric_literal_like(store.node(args[1]))
                    && is_bindable_static_name_expression(store, args[0], /*excludeThisKeyword:*/ true);
            }
        }
    }
    false
}

/// Go: `func IsBindableStaticNameExpression(node *Node, excludeThisKeyword bool) bool`.
pub fn is_bindable_static_name_expression(
    store: &dyn crate::NodeStore,
    node: NodeId,
    exclude_this_keyword: bool,
) -> bool {
    is_entity_name_expression(store, node)
        || is_bindable_static_access_expression(store, node, exclude_this_keyword)
}

/// Go: `func IsBindableStaticAccessExpression(node *Node, excludeThisKeyword bool) bool`.
pub fn is_bindable_static_access_expression(
    store: &dyn crate::NodeStore,
    node: NodeId,
    exclude_this_keyword: bool,
) -> bool {
    // Go: IsLiteralLikeElementAccess(node) && (…) — the first operand gates
    // the whole expression; mirror it exactly.
    if !is_literal_like_element_access(store, node) {
        return false;
    }
    let expr = expression_of(store, node).expect("access expression");
    (!exclude_this_keyword && store.node(expr).kind == crate::Kind::ThisKeyword)
        || is_entity_name_expression(store, expr)
        || is_bindable_static_access_expression(store, expr, /*excludeThisKeyword:*/ true)
}

/// Go: `func IsLiteralLikeElementAccess(node *Node) bool`.
pub fn is_literal_like_element_access(store: &dyn crate::NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    crate::is_element_access_expression(n)
        && is_string_or_numeric_literal_like(
            store.node(
                n.as_element_access_expression()
                    .expect("ElementAccessExpression data")
                    .argument_expression,
            ),
        )
}

/// Go: `func GetAssignmentDeclarationKind(node *Node) JSDeclarationKind`.
pub fn get_assignment_declaration_kind(
    store: &dyn crate::NodeStore,
    node: NodeId,
) -> JSDeclarationKind {
    let n = store.node(node);
    match n.kind {
        crate::Kind::BinaryExpression => {
            let bin = n.as_binary_expression().expect("BinaryExpression data");
            let operator = store.node(bin.operator_token).kind;
            if operator == crate::Kind::EqualsToken && is_access_expression(store.node(bin.left)) {
                if is_in_js_file(store.node(bin.left)) {
                    if is_module_exports_access_expression(store, bin.left)
                        && !is_exports_identifier(store, bin.right)
                    {
                        return JSDeclarationKind::ModuleExports;
                    }
                    let left_expr = expression_of(store, bin.left).expect("access expression");
                    if (is_module_exports_access_expression(store, left_expr)
                        || is_exports_identifier(store, left_expr))
                        && get_element_or_property_access_name(store, bin.left).is_some()
                    {
                        return JSDeclarationKind::ExportsProperty;
                    }
                    if store.node(left_expr).kind == crate::Kind::ThisKeyword {
                        return JSDeclarationKind::ThisProperty;
                    }
                }
                // Go: `bin.Left.Kind == KindPropertyAccessExpression && … ||
                //      bin.Left.Kind == KindElementAccessExpression && …` —
                // same operator precedence as written.
                let left_is_property_access = store.node(bin.left).kind == crate::Kind::PropertyAccessExpression;
                let left_is_element_access = store.node(bin.left).kind == crate::Kind::ElementAccessExpression;
                let left_expr = expression_of(store, bin.left).expect("access expression");
                let left_in_js = is_in_js_file(store.node(bin.left));
                if left_is_property_access
                    && is_entity_name_expression_ex(store, left_expr, left_in_js)
                    && crate::is_identifier(store.node(store.node(bin.left).name().expect("access name")))
                    || left_is_element_access
                        && is_entity_name_expression_ex(store, left_expr, left_in_js)
                {
                    return JSDeclarationKind::Property;
                }
            }
        }
        crate::Kind::CallExpression
            if is_in_js_file(store.node(node))
                && is_bindable_object_define_property_call(store, node) =>
        {
            let entity_name = arguments_of(store, node)[0];
            if is_exports_identifier(store, entity_name)
                || is_module_exports_access_expression(store, entity_name)
            {
                return JSDeclarationKind::ObjectDefinePropertyExports;
            }
            return JSDeclarationKind::ObjectDefinePropertyValue;
        }
        _ => {}
    }
    JSDeclarationKind::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambient_module_symbol_names_mirror_go() {
        // Plain quoted module names.
        assert_eq!(
            try_get_ambient_module_name_from_symbol_name("\"foo/bar\""),
            Some("foo/bar".to_string())
        );
        assert_eq!(
            try_get_ambient_module_name_from_symbol_name("foo"),
            None
        );
        // Pattern ambient modules: "\xFE\"<pattern>\"pattern@<n>".
        let name = format!("{}\"A.B\"pattern@3", crate::symbol::INTERNAL_SYMBOL_NAME_PREFIX);
        assert_eq!(try_get_ambient_module_name_from_symbol_name(&name), Some("A.B".to_string()));
        assert!(is_ambient_module_symbol_name("\"x\""));
        assert!(!is_ambient_module_symbol_name("x"));
    }

    #[test]
    fn optional_chain_check() {
        // is_optional_chain needs a Node; covered in the visitor tests through
        // the flag/kind checks. Spot-check the pure flag logic here.
        assert_eq!(NodeFlags::OPTIONAL_CHAIN, NodeFlags(1 << 5));
    }

    /// Go: NodeIsMissing — empty non-negative range (missing), zero-width
    /// EndOfFile (not missing), negative positions (synthesized, missing).
    #[test]
    fn node_missing_mirrors_go() {
        let make = |pos: i32, end: i32, kind: Kind| Node {
            kind,
            flags: NodeFlags::NONE,
            loc: crate::TextRange::new(pos, end),
            id: std::cell::Cell::new(0),
            parent: std::cell::Cell::new(NodeId::NONE),
            data: crate::NodeData::Token(Box::new(crate::ast_generated::Token {})),
        };
        assert!(node_is_missing(&make(3, 3, Kind::Identifier)));
        assert!(!node_is_missing(&make(3, 4, Kind::Identifier)));
        // Go: KindEndOfFile is exempt (a zero-width EOF token is present).
        assert!(!node_is_missing(&make(3, 3, Kind::EndOfFile)));
        // Go: NodeIsMissing requires Pos >= 0, so a zero-width node at
        // synthesized (negative) positions is NOT missing — NodeIsPresent
        // is true. (Synthesis is its own predicate: NodeIsSynthesized.)
        assert!(node_is_present(&make(-1, -1, Kind::Identifier)));
    }
}
