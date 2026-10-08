// Crate-local AST-utility shims for the M6 config-node diagnostic surface
// (Go sources: tsc/internal/ast/utilities.go `TryGetTextOfPropertyName`,
// ast/ast.go `Node.Text()` / `NodeData.Expression()`).
//
// The ast crate ports only what the M2 waves needed, so the config-syntax
// walks keep crate-local copies of the three accessors they use — the
// compiler/binder `ast_shims` precedent. Dedup candidates when the ast
// crate grows the full accessor surface (PORTING-NOTES).
//
// Everything resolves nodes through the per-file arena (`tsc_ast::NodeStore`),
// Go's `*Node` pointer-graph equivalent.

use tsc_ast::{Kind, Node, NodeData, NodeId, NodeStore};

/// Go: the `nodeData.Expression()` dispatch (ast.go) — the arm the config
/// walks reach (`ExpressionStatement.Expression`); `NodeDefault` returning
/// nil is mirrored by `None`.
pub fn expression_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::ExpressionStatement(d) => Some(d.expression),
        _ => None,
    }
}

/// Go: `func (n *Node) Text() string` — the literal-carrying arms the
/// property-name walk reads (the full dispatch's JSDoc/template/meta arms
/// are unreachable for config JSON; omitted — dedup candidate).
fn node_text(n: &Node) -> String {
    match n.kind {
        Kind::Identifier => n.as_identifier().expect("Identifier data").text.to_string(),
        Kind::PrivateIdentifier => {
            n.as_private_identifier().expect("PrivateIdentifier data").text.to_string()
        }
        Kind::StringLiteral => n.as_string_literal().expect("StringLiteral data").text.to_string(),
        Kind::NumericLiteral => n.as_numeric_literal().expect("NumericLiteral data").text.to_string(),
        Kind::BigIntLiteral => n.as_big_int_literal().expect("BigIntLiteral data").text.to_string(),
        Kind::NoSubstitutionTemplateLiteral => {
            n.as_no_substitution_template_literal()
                .expect("NoSubstitutionTemplateLiteral data")
                .text
                .to_string()
        }
        kind => panic!("Unhandled case in Node.Text(): {kind:?}"),
    }
}

/// Go: `func TryGetTextOfPropertyName(name *Node) (string, bool)`.
///
/// Returns the property-name text when the name is a literal-like name
/// (identifier/string/numeric/bigint/no-substitution template), a computed
/// property whose expression is a string-or-numeric literal, or a JSX
/// namespaced name; `None` mirrors Go's `("", false)`.
pub fn try_get_text_of_property_name(store: &dyn NodeStore, name: NodeId) -> Option<String> {
    let n = store.node(name);
    match n.kind {
        Kind::Identifier
        | Kind::PrivateIdentifier
        | Kind::StringLiteral
        | Kind::NumericLiteral
        | Kind::BigIntLiteral
        | Kind::NoSubstitutionTemplateLiteral => Some(node_text(n)),
        Kind::ComputedPropertyName => {
            let expression = n
                .as_computed_property_name()
                .expect("ComputedPropertyName data")
                .expression;
            let expression_node = store.node(expression);
            if tsc_ast::is_string_literal_like(expression_node)
                || tsc_ast::is_numeric_literal(expression_node)
            {
                // Go: `name.Expression().Text()` — the literal's own text.
                Some(node_text(expression_node))
            } else {
                None
            }
        }
        Kind::JsxNamespacedName => {
            let data = n.as_jsx_namespaced_name().expect("JsxNamespacedName data");
            let namespace = node_text(store.node(data.namespace));
            let name_text = node_text(store.node(data.name));
            Some(format!("{namespace}:{name_text}"))
        }
        _ => None,
    }
}
