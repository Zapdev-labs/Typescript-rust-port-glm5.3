// Ported from tsc/internal/ast/visitor.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Also carries the `Visitor` helpers from tsc/internal/ast/ast.go @ ec47d33c
// and the `NodeStore` seam (Go's `*NodeFactory` fields on NodeVisitor, reduced
// to resolution + allocation).
//
// Go name mapping:
//   Visitor (type)               → type alias + visit helpers (ast.go)
//   NodeVisitor (struct)         → trait NodeVisitor (the struct's function
//                                  fields become trait methods; overriding a
//                                  hook_* method installs the corresponding
//                                  NodeVisitorHooks function field)
//   NodeVisitorHooks (struct)    → the hook_* default methods
//   NodeVisitor.Factory          → NodeVisitor::store() (NodeStore seam)
//   NodeVisitor.Visit (required) → NodeVisitor::visit (required)
//   private visit* dispatchers   → the visit_* trait methods
//   exported Visit* cores        → the *_core trait methods
//
// A Go struct-of-closures cannot express the recursive visitors of
// deepclone.go (the closure captures the visitor itself), hence the trait.

use std::cell::Cell;

use crate::{Kind, ModifierList, Node, NodeList, NodeId, TextRange};

// ────────────────────────────────────────────────────────────────────────────
// Visitor (ast.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type Visitor func(*Node) bool` — returns true to stop the traversal.
/// `&Node` parameters become `NodeId` handles (SPEC §5.1).
pub type Visitor<'a> = dyn FnMut(NodeId) -> bool + 'a;

/// Go: `func visit(v Visitor, node *Node) bool` — nil nodes are skipped
/// (`Option`).
#[inline]
pub fn visit(v: &mut Visitor, node: Option<NodeId>) -> bool {
    match node {
        Some(n) => v(n),
        None => false,
    }
}

/// Go: `func visitNodes(v Visitor, nodes []*Node) bool`.
pub fn visit_nodes(v: &mut Visitor, nodes: &[NodeId]) -> bool {
    for &node in nodes {
        if v(node) {
            return true;
        }
    }
    false
}

/// Go: `func visitNodeList(v Visitor, nodeList *NodeList) bool`.
pub fn visit_node_list(v: &mut Visitor, node_list: Option<&NodeList>) -> bool {
    match node_list {
        Some(list) => visit_nodes(v, &list.nodes),
        None => false,
    }
}

/// Go: `func visitModifiers(v Visitor, modifiers *ModifierList) bool`.
pub fn visit_modifiers(v: &mut Visitor, modifiers: Option<&ModifierList>) -> bool {
    match modifiers {
        Some(list) => visit_nodes(v, &list.nodes),
        None => false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// NodeStore — the arena seam (Go: NodeVisitor's `Factory *NodeFactory` and
// every Go `*Node` dereference)
// ────────────────────────────────────────────────────────────────────────────

/// Resolution + allocation seam over the per-file node arena (SPEC §5.1:
/// arena-per-`SourceFile`; `SourceFile::node(id)` replaces Go's `*Node`).
/// [`crate::SourceFile`] implements it; the program-level file table (M4)
/// implements the cross-file case.
///
/// The transformer-era factory hooks (Go `NodeFactoryHooks` OnCreate/OnUpdate/
/// OnClone) are NOT part of this seam — they land with the NodeFactory port.
pub trait NodeStore {
    /// Resolves a node handle. Panics on a foreign file id (per-file arena).
    fn node(&self, id: NodeId) -> &Node;

    /// Resolves a node handle mutably (binder-era `Cell` field writes).
    fn node_mut(&mut self, id: NodeId) -> &mut Node;

    /// The file owning `file_id` (Go: `GetSourceFileOfNode` resolved through
    /// the pointer graph; here the packed NodeId carries the file id).
    fn file(&self, file_id: u32) -> &crate::SourceFile;

    /// Appends a node to the arena (Go: the factory's `newNode`).
    fn alloc(&mut self, node: Node) -> NodeId;

    /// Go: `func (f *NodeFactory) NewBlock(f.NewNodeList(nodes), multiLine)`.
    fn new_block(&mut self, nodes: Box<[NodeId]>, multi_line: bool) -> NodeId {
        let block = crate::ast_generated::Block {
            flow_node: None,
            locals: Default::default(),
            next_container: None,
            facts: 0,
            statements: Some(NodeList {
                loc: TextRange::undefined(),
                nodes,
            }),
            multi_line,
        };
        self.alloc(Node {
            kind: Kind::Block,
            flags: crate::NodeFlags::NONE,
            loc: TextRange::undefined(),
            id: Cell::new(0),
            parent: Cell::new(NodeId::NONE),
            data: crate::NodeData::Block(Box::new(block)),
        })
    }

    /// Go: `func (n *Node) Clone(f NodeFactoryCoercible) *Node` + the generated
    /// per-struct `Clone` methods (ast_generated.go) — a shallow value copy
    /// sharing all child handles, with a fresh id/parent (Go's `New*` leave
    /// them zero/nil; `cloneNode` copies Flags/Loc from the original).
    fn clone_node(&mut self, id: NodeId) -> NodeId {
        let (kind, flags, loc, data) = {
            let n = self.node(id);
            (n.kind, n.flags, n.loc, n.data.clone())
        };
        self.alloc(Node {
            kind,
            flags,
            loc,
            id: Cell::new(0),
            parent: Cell::new(NodeId::NONE),
            data,
        })
    }

    /// Go: `func (f *NodeFactory) NewModifierList(nodes []*Node)` (utilities'
    /// `ModifiersToFlags` folded in; dropped children — `NodeId::NONE` — are
    /// skipped, Go skips nil modifiers).
    fn new_modifier_list(&mut self, nodes: Box<[NodeId]>) -> ModifierList {
        let mut modifier_flags = crate::ModifierFlags::NONE;
        for &n in &nodes {
            if n == NodeId::NONE {
                continue;
            }
            modifier_flags |= modifier_to_flag(self.node(n).kind);
        }
        ModifierList {
            loc: TextRange::undefined(),
            nodes,
            modifier_flags,
        }
    }
}

/// Go: `func ModifierToFlag(token Kind) ModifierFlags` (utilities.go —
/// included here because `NodeStore::new_modifier_list` needs it; dedup into
/// the utilities.go port).
pub fn modifier_to_flag(token: Kind) -> crate::ModifierFlags {
    use crate::ModifierFlags as M;
    match token {
        Kind::StaticKeyword => M::STATIC,
        Kind::PublicKeyword => M::PUBLIC,
        Kind::ProtectedKeyword => M::PROTECTED,
        Kind::PrivateKeyword => M::PRIVATE,
        Kind::AbstractKeyword => M::ABSTRACT,
        Kind::AccessorKeyword => M::ACCESSOR,
        Kind::ExportKeyword => M::EXPORT,
        Kind::DeclareKeyword => M::AMBIENT,
        Kind::ConstKeyword => M::CONST,
        Kind::DefaultKeyword => M::DEFAULT,
        Kind::AsyncKeyword => M::ASYNC,
        Kind::ReadonlyKeyword => M::READONLY,
        Kind::OverrideKeyword => M::OVERRIDE,
        Kind::InKeyword => M::IN,
        Kind::OutKeyword => M::OUT,
        Kind::Decorator => M::DECORATOR,
        _ => M::NONE,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// NodeVisitor (visitor.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type NodeVisitor struct { Visit; Factory; Hooks }` — a trait, since
/// Go's visitors (deepclone.go) build recursive closures that capture the
/// visitor itself. `None` from [`NodeVisitor::visit`] means "dropped node"
/// (Go: `Visit` returning nil); `None` from [`NodeVisitor::visit_each_child`]
/// means "no child changed" (Go: `Update*` returning the original node).
pub trait NodeVisitor {
    /// Go: `Visit func(node *Node) *Node` (required).
    fn visit(&mut self, node: NodeId) -> Option<NodeId>;

    /// Go: `Factory *NodeFactory` (required) — the arena seam.
    fn store(&mut self) -> &mut dyn NodeStore;

    // ── Hook points (Go NodeVisitorHooks function fields; the defaults
    // implement Go's "unset hook" fallback chain) ──────────────────────────

    /// Go: `Hooks.VisitNode`.
    fn hook_visit_node(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        self.visit_node_core(node)
    }
    /// Go: `Hooks.VisitToken`.
    fn hook_visit_token(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        // Go default: `v.VisitNode(node)`.
        self.visit_node_core(node)
    }
    /// Go: `Hooks.VisitNodes`.
    fn hook_visit_nodes(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        self.visit_nodes_core(nodes)
    }
    /// Go: `Hooks.VisitModifiers`.
    fn hook_visit_modifiers(&mut self, nodes: Option<&ModifierList>) -> Option<ModifierList> {
        self.visit_modifiers_core(nodes)
    }
    /// Go: `Hooks.VisitEmbeddedStatement`.
    fn hook_visit_embedded_statement(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        // Go default: `v.VisitEmbeddedStatement(node)` — Visit + liftToBlock.
        self.visit_embedded_statement_core(node)
    }
    /// Go: `Hooks.VisitIterationBody`.
    fn hook_visit_iteration_body(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        // Go default: `v.visitEmbeddedStatement(node)`.
        self.visit_embedded_statement_core(node)
    }
    /// Go: `Hooks.VisitFunctionBody`.
    fn hook_visit_function_body(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        // Go default: `v.visitNode(node)`.
        self.visit_node_core(node)
    }
    /// Go: `Hooks.VisitParameters`.
    fn hook_visit_parameters(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        // Go default: `v.visitNodes(nodes)`.
        self.visit_nodes_core(nodes)
    }
    /// Go: `Hooks.VisitTopLevelStatements`.
    fn hook_visit_top_level_statements(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        // Go default: `v.visitNodes(nodes)`.
        self.visit_nodes_core(nodes)
    }

    // ── Private Go dispatchers (visitNode etc.) — what the generated
    // per-struct visit_each_child calls. Overriding is NOT the Go hook
    // mechanism (override the hook_* methods for that); these route through
    // the hooks by default. ────────────────────────────────────────────────

    /// Go: `func (v *NodeVisitor) visitNode(node *Node) *Node`.
    fn visit_node(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        self.hook_visit_node(node)
    }
    /// Go: `func (v *NodeVisitor) visitToken(node *TokenNode) *Node`.
    fn visit_token(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        self.hook_visit_token(node)
    }
    /// Go: `func (v *NodeVisitor) visitNodes(nodes *NodeList) *NodeList`.
    fn visit_nodes(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        self.hook_visit_nodes(nodes)
    }
    /// Go: `func (v *NodeVisitor) visitModifiers(nodes *ModifierList) *ModifierList`.
    fn visit_modifiers(&mut self, nodes: Option<&ModifierList>) -> Option<ModifierList> {
        self.hook_visit_modifiers(nodes)
    }
    /// Go: `func (v *NodeVisitor) visitEmbeddedStatement(node *Statement) *Statement`.
    fn visit_embedded_statement(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        self.hook_visit_embedded_statement(node)
    }
    /// Go: `func (v *NodeVisitor) visitIterationBody(node *Statement) *Statement`.
    fn visit_iteration_body(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        self.hook_visit_iteration_body(node)
    }
    /// Go: `func (v *NodeVisitor) visitFunctionBody(node *BlockOrExpression) *BlockOrExpression`.
    fn visit_function_body(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        self.hook_visit_function_body(node)
    }
    /// Go: `func (v *NodeVisitor) visitParameters(nodes *ParameterList) *ParameterList`.
    fn visit_parameters(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        self.hook_visit_parameters(nodes)
    }
    /// Go: `func (v *NodeVisitor) visitTopLevelStatements(nodes *StatementList) *StatementList`.
    fn visit_top_level_statements(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        self.hook_visit_top_level_statements(nodes)
    }

    // ── Core algorithms (Go's exported NodeVisitor methods) ────────────────

    /// Go: `func (v *NodeVisitor) VisitSourceFile(node *SourceFile) *SourceFile`.
    fn visit_source_file(&mut self, node: NodeId) -> Option<NodeId> {
        self.visit_node(Some(node))
    }

    /// Go: `func (v *NodeVisitor) VisitNode(node *Node) *Node` — SyntaxList
    /// results with exactly one child are lifted to that child; both Go
    /// panics are preserved.
    fn visit_node_core(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        let id = node?;
        let visited = self.visit(id)?;
        if self.store().node(visited).kind == Kind::SyntaxList {
            let children = {
                let n = self.store().node(visited);
                let d = n.as_syntax_list().expect("SyntaxList data for a SyntaxList kind");
                d.children.clone()
            };
            if children.len() != 1 {
                panic!("Expected only a single node to be written to output");
            }
            let lifted = children[0];
            if self.store().node(lifted).kind == Kind::SyntaxList {
                panic!("The result of visiting and lifting a Node may not be SyntaxList");
            }
            return Some(lifted);
        }
        Some(visited)
    }

    /// Go: `func (v *NodeVisitor) VisitEmbeddedStatement(node *Statement) *Statement`.
    fn visit_embedded_statement_core(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        let id = node?;
        let visited = self.visit(id)?;
        self.lift_to_block(Some(visited))
    }

    /// Go: `func (v *NodeVisitor) VisitNodes(nodes *NodeList) *NodeList`.
    ///
    /// PORT: the unchanged case returns a clone of the input list (Go returns
    /// the same `*NodeList` pointer; Rust lists are values, so "unchanged" is
    /// value equality — the generated rebuilds compare with `==`).
    fn visit_nodes_core(&mut self, nodes: Option<&NodeList>) -> Option<NodeList> {
        let list = nodes?;
        let (result, changed) = self.visit_slice(&list.nodes);
        if changed {
            Some(NodeList {
                loc: list.loc,
                nodes: result.unwrap_or_default().into_boxed_slice(),
            })
        } else {
            Some(list.clone())
        }
    }

    /// Go: `func (v *NodeVisitor) VisitModifiers(nodes *ModifierList) *ModifierList`.
    fn visit_modifiers_core(&mut self, nodes: Option<&ModifierList>) -> Option<ModifierList> {
        let list = nodes?;
        let (result, changed) = self.visit_slice(&list.nodes);
        if changed {
            let new_nodes: Box<[NodeId]> = result.unwrap_or_default().into_boxed_slice();
            // Go: Factory.NewModifierList(nodes) recomputes ModifiersToFlags.
            let mut modifier_flags = crate::ModifierFlags::NONE;
            for &n in &new_nodes {
                if n == NodeId::NONE {
                    continue;
                }
                modifier_flags |= modifier_to_flag(self.store().node(n).kind);
            }
            Some(ModifierList {
                loc: list.loc,
                nodes: new_nodes,
                modifier_flags,
            })
        } else {
            Some(list.clone())
        }
    }

    /// Go: `func (v *NodeVisitor) VisitSlice(nodes []*Node) (result []*Node, changed bool)`
    /// — the incremental rebuild: the prefix is cloned only once a node is
    /// dropped or replaced, and SyntaxList results are spliced in.
    fn visit_slice(&mut self, nodes: &[NodeId]) -> (Option<Vec<NodeId>>, bool) {
        let mut i = 0usize;
        while i < nodes.len() {
            let mut visited = self.visit(nodes[i]);
            if visited != Some(nodes[i]) {
                let mut updated: Vec<NodeId> = nodes[..i].to_vec();
                loop {
                    match visited {
                        None => {}
                        Some(v) if self.store().node(v).kind == Kind::SyntaxList => {
                            let children = {
                                let n = self.store().node(v);
                                let d = n.as_syntax_list().expect("SyntaxList data");
                                d.children.clone()
                            };
                            updated.extend_from_slice(&children);
                        }
                        Some(v) => updated.push(v),
                    }
                    i += 1;
                    if i >= nodes.len() {
                        break;
                    }
                    visited = self.visit(nodes[i]);
                }
                return (Some(updated), true);
            }
            i += 1;
        }
        (None, false)
    }

    /// Go: `func (v *NodeVisitor) VisitEachChild(node *Node) *Node`.
    ///
    /// PORT: the node is cloned out of the arena before the walk — a walk can
    /// allocate into the same `Vec`-backed arena (Go's GC tolerates this; a
    /// growing `Vec` would invalidate the borrow), so the snapshot pays one
    /// payload copy. The transformer's separate target arenas (SPEC §5.6)
    /// remove the cost.
    fn visit_each_child(&mut self, node: Option<NodeId>) -> Option<NodeId>
    where
        Self: Sized,
    {
        let id = node?;
        let snapshot = self.store().node(id).clone();
        match snapshot.visit_each_child(self) {
            Some(new_node) => Some(self.store().alloc(new_node)),
            // Unchanged → the same id (Go returns the same *Node).
            None => Some(id),
        }
    }

    /// Go: `func (v *NodeVisitor) liftToBlock(node *Statement) *Statement`.
    fn lift_to_block(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        let nodes: Box<[NodeId]> = match node {
            None => Box::default(),
            Some(id) if self.store().node(id).kind == Kind::SyntaxList => {
                let n = self.store().node(id);
                let d = n.as_syntax_list().expect("SyntaxList data");
                d.children.clone().into_boxed_slice()
            }
            Some(id) => Box::new([id]),
        };
        let node = if nodes.len() == 1 {
            nodes[0]
        } else {
            // Go: v.Factory.NewBlock(v.Factory.NewNodeList(nodes), true)
            self.store().new_block(nodes, true)
        };
        if self.store().node(node).kind == Kind::SyntaxList {
            panic!("The result of visiting and lifting a Node may not be SyntaxList");
        }
        Some(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal in-memory store for exercising the visitor core against the
    /// same Go semantics (drop/replace/SyntaxList lifting).
    struct TestStore {
        nodes: Vec<Node>,
    }

    impl TestStore {
        fn add(&mut self, node: Node) -> NodeId {
            let idx = self.nodes.len() as u64;
            self.nodes.push(node);
            NodeId::new(0, idx)
        }

        fn syntax_list(&mut self, children: Vec<NodeId>) -> NodeId {
            let d = crate::ast_generated::SyntaxList { children };
            self.add(Node {
                kind: Kind::SyntaxList,
                flags: crate::NodeFlags::NONE,
                loc: TextRange::undefined(),
                id: Cell::new(0),
                parent: Cell::new(NodeId::NONE),
                data: crate::NodeData::SyntaxList(Box::new(d)),
            })
        }

        fn empty(&mut self, kind: Kind) -> NodeId {
            // A node kind with no children — Token data is as good as any for
            // identity tests.
            let d = crate::ast_generated::Token {};
            self.add(Node {
                kind,
                flags: crate::NodeFlags::NONE,
                loc: TextRange::undefined(),
                id: Cell::new(0),
                parent: Cell::new(NodeId::NONE),
                data: crate::NodeData::Token(Box::new(d)),
            })
        }
    }

    impl NodeStore for TestStore {
        fn node(&self, id: NodeId) -> &Node {
            &self.nodes[id.local_index() as usize]
        }
        fn node_mut(&mut self, id: NodeId) -> &mut Node {
            &mut self.nodes[id.local_index() as usize]
        }
        fn file(&self, file_id: u32) -> &crate::SourceFile {
            panic!("test store has no files (file_id {file_id})")
        }
        fn alloc(&mut self, node: Node) -> NodeId {
            self.add(node)
        }
    }

    /// Go: VisitNode — nil passthrough, SyntaxList lifting, single-child rule.
    struct Identity(TestStore);

    impl NodeVisitor for Identity {
        fn visit(&mut self, node: NodeId) -> Option<NodeId> {
            Some(node)
        }
        fn store(&mut self) -> &mut dyn NodeStore {
            &mut self.0
        }
    }

    #[test]
    fn visit_node_lifts_syntax_lists() {
        let mut v = Identity(TestStore { nodes: Vec::new() });
        let target = v.0.empty(Kind::Identifier);
        let list = v.0.syntax_list(vec![target]);
        assert_eq!(v.visit_node_core(Some(list)), Some(target));
        assert_eq!(v.visit_node_core(Some(target)), Some(target));
        assert_eq!(v.visit_node_core(None), None);
        // Two children → Go panics with exactly this message.
        let a = v.0.empty(Kind::Identifier);
        let b = v.0.empty(Kind::Identifier);
        let two = v.0.syntax_list(vec![a, b]);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            v.visit_node_core(Some(two))
        }));
        assert!(result.is_err());
    }

    /// Go: VisitSlice — prefix cloned once, drops and replacements preserved.
    struct DropSecond(TestStore);

    impl NodeVisitor for DropSecond {
        fn visit(&mut self, node: NodeId) -> Option<NodeId> {
            // Drops the node with local index 1 (the second node allocated in
            // the tests below — "b").
            if node.local_index() == 1 {
                None
            } else {
                Some(node)
            }
        }
        fn store(&mut self) -> &mut dyn NodeStore {
            &mut self.0
        }
    }

    #[test]
    fn visit_slice_drops_and_replaces() {
        let mut v = DropSecond(TestStore { nodes: Vec::new() });
        let a = v.0.empty(Kind::Identifier);
        let b = v.0.empty(Kind::Identifier);
        let c = v.0.empty(Kind::Identifier);
        // [a, b, c] — b is dropped by the visitor.
        let (result, changed) = v.visit_slice(&[a, b, c]);
        assert!(changed);
        assert_eq!(result, Some(vec![a, c]));

        // [a, c, b] — the drop may come last; the prefix survives unchanged.
        let (result, changed) = v.visit_slice(&[a, c, b]);
        assert!(changed);
        assert_eq!(result, Some(vec![a, c]));

        // Unchanged when nothing is dropped or replaced.
        let (result, changed) = v.visit_slice(&[a, c]);
        assert!(!changed);
        assert_eq!(result, None);
    }

    /// Go: VisitNodes — same Loc on a changed list; None passthrough.
    #[test]
    fn visit_nodes_keeps_loc() {
        let mut v = DropSecond(TestStore { nodes: Vec::new() });
        let a = v.0.empty(Kind::Identifier);
        let b = v.0.empty(Kind::Identifier);
        let c = v.0.empty(Kind::Identifier);
        let list = NodeList {
            loc: TextRange::new(3, 9),
            nodes: Box::new([a, b, c]),
        };
        let out = v.visit_nodes_core(Some(&list)).expect("list in, list out");
        assert_eq!(out.loc, TextRange::new(3, 9));
        assert_eq!(out.nodes.as_ref(), [a, c].as_ref());
        assert!(v.visit_nodes_core(None).is_none());
    }

    /// Go: liftToBlock — single child passes through, multiple children (or a
    /// dropped statement) become a Block.
    #[test]
    fn lift_to_block_mirrors_go() {
        let mut v = Identity(TestStore { nodes: Vec::new() });
        let a = v.0.empty(Kind::Identifier);
        let b = v.0.empty(Kind::Identifier);
        let list = v.0.syntax_list(vec![a]);
        assert_eq!(v.lift_to_block(Some(list)), Some(a));
        let two = v.0.syntax_list(vec![a, b]);
        let block = v.lift_to_block(Some(two)).expect("lifted to a block");
        assert_eq!(v.0.node(block).kind, Kind::Block);
        let dropped = v.lift_to_block(None).expect("Go lifts nil to an empty Block");
        assert_eq!(v.0.node(dropped).kind, Kind::Block);
        let block_data = v.0.node(dropped).as_block().expect("Block data");
        assert!(block_data.statements.as_ref().is_none_or(|l| l.nodes.is_empty()));
    }
}
