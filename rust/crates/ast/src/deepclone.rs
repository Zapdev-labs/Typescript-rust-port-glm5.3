// Ported from tsc/internal/ast/deepclone.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   getDeepCloneVisitor(f, syntheticLocation) → DeepCloneVisitor (the
//       NodeVisitor is a trait here, since Go's struct-of-closures visitor
//       captures the visitor itself in its closures)
//   f.DeepCloneNode(node)                → deep_clone_node(store, node)
//   f.DeepCloneReparse(node)              → deep_clone_reparse(store, node)
//   f.DeepCloneReparseModifiers(modifiers) → deep_clone_reparse_modifiers
//
// Go compares `*NodeList`/`*ModifierList` pointers for the unchanged case;
// the port compares list values (`PartialEq`), which is the same observable
// behavior (an unchanged rebuild is value-identical to the input list).

use tsc_core::text::TextRange;

use crate::{ModifierList, NodeFlags, NodeId, NodeList};
use crate::visitor::{NodeStore, NodeVisitor};

/// Go: `getDeepCloneVisitor(f *NodeFactory, syntheticLocation bool) *NodeVisitor`.
struct DeepCloneVisitor<'s> {
    store: &'s mut dyn NodeStore,
    synthetic_location: bool,
}

impl<'s> DeepCloneVisitor<'s> {
    fn new(store: &'s mut dyn NodeStore, synthetic_location: bool) -> Self {
        DeepCloneVisitor {
            store,
            synthetic_location,
        }
    }
}

impl NodeVisitor for DeepCloneVisitor<'_> {
    /// Go: the visitor's `Visit` closure — children first (cascading new
    /// nodes/arrays upwards through the `update` calls), then forcibly clone
    /// the leaves when nothing below changed.
    fn visit(&mut self, node: NodeId) -> Option<NodeId> {
        let visited = NodeVisitor::visit_each_child(self, Some(node));
        match visited {
            Some(visited) if visited != node => {
                if self.synthetic_location {
                    self.store.node_mut(visited).loc = TextRange::new(-1, -1);
                }
                Some(visited)
            }
            Some(_) => {
                // Go: `c := node.Clone(f)` — forcibly clone leaf nodes. In
                // strada, `factory.cloneNode` was dynamic and did _not_ clone
                // positions for any "special cases"; Node.Clone in corsa
                // reliably uses `Update` calls for all nodes and so copies
                // locations by default. Deep clones are done to copy a node
                // across files, so with `syntheticLocation` the location range
                // is made synthetic on all cloned nodes.
                let c = self.store.clone_node(node);
                if self.synthetic_location {
                    self.store.node_mut(c).loc = TextRange::new(-1, -1);
                }
                Some(c)
            }
            // Go: `Visit` returned nil — the node was dropped upstream.
            None => None,
        }
    }

    fn store(&mut self) -> &mut dyn NodeStore {
        self.store
    }

    /// Go: `NodeVisitorHooks.VisitNodes`.
    fn hook_visit_nodes(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        let nodes = nodes?;
        // Go: `visited := v.VisitNodes(nodes)` (the exported core).
        let visited = self.visit_nodes_core(Some(nodes));
        let new_list = match visited.as_ref() {
            // Go: `visited != nodes` — pointer inequality; value inequality
            // here (an unchanged rebuild is value-identical).
            Some(v) if v != nodes => visited.expect("checked above"),
            _ => nodes.clone(),
        };
        if self.synthetic_location {
            let mut new_list = new_list;
            new_list.loc = TextRange::new(-1, -1);
            // Go: the trailing comma marker is the (-2, -2) loc on the last
            // element (`nodes.HasTrailingComma()` resolves through the store).
            if nodes.has_trailing_comma(self.store) {
                let last = *new_list.nodes.last().expect("trailing comma implies a last node");
                self.store.node_mut(last).loc = TextRange::new(-2, -2);
            }
            return Some(new_list);
        }
        Some(new_list)
    }

    /// Go: `NodeVisitorHooks.VisitModifiers`.
    fn hook_visit_modifiers(&mut self, nodes: Option<&ModifierList>) -> Option<ModifierList> {
        let nodes = nodes?;
        let visited = self.visit_modifiers_core(Some(nodes));
        let new_list = match visited.as_ref() {
            Some(v) if v != nodes => visited.expect("checked above"),
            _ => nodes.clone(),
        };
        if self.synthetic_location {
            let mut new_list = new_list;
            new_list.loc = TextRange::new(-1, -1);
            if nodes.has_trailing_comma(self.store) {
                let last = *new_list.nodes.last().expect("trailing comma implies a last node");
                self.store.node_mut(last).loc = TextRange::new(-2, -2);
            }
            return Some(new_list);
        }
        Some(new_list)
    }
}

/// Go: `func (f *NodeFactory) DeepCloneNode(node *Node) *Node`.
pub fn deep_clone_node(store: &mut dyn NodeStore, node: NodeId) -> NodeId {
    let mut visitor = DeepCloneVisitor::new(store, /*syntheticLocation:*/ true);
    visitor
        .visit_node_core(Some(node))
        .expect("DeepCloneNode preserves the node (it forcibly clones leaves)")
}

/// Go: `func (f *NodeFactory) DeepCloneReparse(node *Node) *Node`.
pub fn deep_clone_reparse(store: &mut dyn NodeStore, node: Option<NodeId>) -> Option<NodeId> {
    let node = node?;
    // The visitor holds the &mut store borrow; scope it so the post-walk work
    // (SetParentInChildren, the Reparsed flag) can use the store again.
    let cloned = {
        let mut visitor = DeepCloneVisitor::new(store, /*syntheticLocation:*/ false);
        visitor
            .visit_node_core(Some(node))
            .expect("DeepCloneReparse preserves the node (it forcibly clones leaves)")
    };
    // Go: SetParentInChildren(node); node.Flags |= NodeFlagsReparsed.
    crate::utilities::set_parent_in_children(store, cloned);
    store.node_mut(cloned).flags |= NodeFlags::REPARSED;
    Some(cloned)
}

/// Go: `func (f *NodeFactory) DeepCloneReparseModifiers(modifiers *ModifierList) *ModifierList`.
pub fn deep_clone_reparse_modifiers(
    store: &mut dyn NodeStore,
    modifiers: Option<&ModifierList>,
) -> Option<ModifierList> {
    let mut visitor = DeepCloneVisitor::new(store, /*syntheticLocation:*/ false);
    visitor.visit_modifiers_core(modifiers)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use tsc_core::text::TextRange;

    use crate::source_file::SourceFile;
    use crate::visitor::{NodeStore, NodeVisitor};
    use crate::{Kind, Node, NodeData, NodeId, NodeList};
    use super::DeepCloneVisitor;

    fn identifier(file: &mut SourceFile, text: &str, pos: i32, end: i32) -> NodeId {
        let d = crate::ast_generated::Identifier {
            flow_node: None,
            text: text.into(),
        };
        NodeStore::alloc(
            file,
            Node {
                kind: Kind::Identifier,
                flags: crate::NodeFlags::NONE,
                loc: TextRange::new(pos, end),
                id: Cell::new(0),
                parent: Cell::new(NodeId::NONE),
                data: NodeData::Identifier(Box::new(d)),
            },
        )
    }

    fn return_statement(file: &mut SourceFile, expression: NodeId) -> NodeId {
        let d = crate::ast_generated::ReturnStatement {
            flow_node: None,
            facts: 0,
            expression: Some(expression),
        };
        NodeStore::alloc(
            file,
            Node {
                kind: Kind::ReturnStatement,
                flags: crate::NodeFlags::NONE,
                loc: TextRange::new(0, 5),
                id: Cell::new(0),
                parent: Cell::new(NodeId::NONE),
                data: NodeData::ReturnStatement(Box::new(d)),
            },
        )
    }

    /// The Go deep-clone contract: every node in the subtree is a fresh
    /// allocation (leaf clones cascade upwards), locations go synthetic
    /// (-1, -1), and the structure/children are preserved.
    #[test]
    fn deep_clone_node_clones_leaves_and_synthesizes_locations() {
        let mut file = SourceFile::new(1, Default::default(), "return x;", None, None);
        let x = identifier(&mut file, "x", 7, 8);
        let stmt = return_statement(&mut file, x);

        let store: &mut dyn NodeStore = &mut file;
        let cloned = crate::deepclone::deep_clone_node(store, stmt);

        assert_ne!(cloned, stmt, "the subtree is reallocated");
        let (kind, loc, cloned_expr) = {
            let n = NodeStore::node(&file, cloned);
            (
                n.kind,
                n.loc,
                n.as_return_statement()
                    .expect("data")
                    .expression
                    .expect("the clone kept the expression"),
            )
        };
        assert_eq!(kind, Kind::ReturnStatement);
        assert_eq!(loc, TextRange::new(-1, -1), "synthetic location");
        assert_ne!(cloned_expr, x, "the leaf was forcibly cloned");
        let (leaf_kind, leaf_loc, leaf_text) = {
            let n = NodeStore::node(&file, cloned_expr);
            (
                n.kind,
                n.loc,
                n.as_identifier().expect("data").text.clone(),
            )
        };
        assert_eq!(leaf_kind, Kind::Identifier);
        assert_eq!(leaf_loc, TextRange::new(-1, -1));
        assert_eq!(leaf_text.as_ref(), "x");
    }

    /// Go: DeepCloneReparse — keeps locations, re-parents the cloned subtree,
    /// and sets NodeFlagsReparsed.
    #[test]
    fn deep_clone_reparse_reparents_and_flags() {
        let mut file = SourceFile::new(1, Default::default(), "return x;", None, None);
        let x = identifier(&mut file, "x", 7, 8);
        let stmt = return_statement(&mut file, x);

        let store: &mut dyn NodeStore = &mut file;
        let cloned = crate::deepclone::deep_clone_reparse(store, Some(stmt)).expect("cloned");
        // nil passthrough (the last use of `store`).
        assert_eq!(crate::deepclone::deep_clone_reparse(store, None), None);

        assert_ne!(cloned, stmt);
        let n = NodeStore::node(&file, cloned);
        assert_eq!(n.loc, TextRange::new(0, 5), "reparse keeps locations");
        assert!(n.flags.intersects(crate::NodeFlags::REPARSED));
        let expr = n
            .as_return_statement()
            .expect("data")
            .expression
            .expect("the clone kept the expression");
        assert_ne!(expr, x);
        // SetParentInChildren re-parented the cloned leaf.
        assert_eq!(NodeStore::node(&file, expr).parent.get(), cloned);
        assert_eq!(
            NodeStore::node(&file, expr).as_identifier().expect("data").text.as_ref(),
            "x"
        );
    }

    /// Go: the VisitNodes hook — unchanged empty lists still round-trip, and
    /// the synthetic (-2, -2) trailing-comma marker survives a clone.
    #[test]
    fn deep_clone_trailing_comma_marker() {
        let mut file = SourceFile::new(1, Default::default(), "", None, None);
        let a = identifier(&mut file, "a", 0, 1);
        // A list whose end extends past the last node = trailing comma.
        let list = NodeList {
            loc: TextRange::new(0, 2),
            nodes: vec![a].into_boxed_slice(),
        };

        let store: &mut dyn NodeStore = &mut file;
        let mut visitor = DeepCloneVisitor::new(store, true);
        // Go: the hook sits on the visitNodes dispatcher.
        let out = visitor.visit_nodes(Some(&list)).expect("list");
        // The single leaf was forcibly cloned (fresh id).
        assert_ne!(out.nodes[0], a);
        assert_eq!(out.loc, TextRange::new(-1, -1));
        // HasTrailingComma held on the original list → last node loc (-2, -2).
        let last_loc = NodeStore::node(&file, out.nodes[0]).loc;
        assert_eq!(last_loc, TextRange::new(-2, -2));
    }
}
