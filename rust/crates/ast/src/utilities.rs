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
