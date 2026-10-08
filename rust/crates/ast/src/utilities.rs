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

/// Go: `func IsJsxOpeningLikeElement(node *Node) bool`.
pub fn is_jsx_opening_like_element(node: &Node) -> bool {
    matches!(node.kind, Kind::JsxOpeningElement | Kind::JsxSelfClosingElement)
}

/// Go: `func IsJsxFragment(node *Node) bool`.
pub fn is_jsx_fragment(node: &Node) -> bool {
    node.kind == Kind::JsxFragment
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
    let Some(rest) = s.strip_prefix(&pattern_prefix) else {
        return None;
    };
    // Go: strings.LastIndex(rest, "\"pattern@")
    let Some(marker_index) = rest.rfind("\"pattern@") else {
        return None;
    };
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
            && n.name().is_some_and(|name| crate::node_text(store, store.node(name)) == "meta");
    }
    false
}

/// Go: `func IsAmbientModuleSymbolName(s string) bool`.
pub fn is_ambient_module_symbol_name(s: &str) -> bool {
    try_get_ambient_module_name_from_symbol_name(s).is_some()
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
}
