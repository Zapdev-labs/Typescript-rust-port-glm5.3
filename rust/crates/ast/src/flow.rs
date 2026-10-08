// Ported from tsc/internal/ast/flow.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   FlowFlags / FlowFlags* consts → FlowFlags newtype + associated consts
//                  (exact bit values; const names drop the `FlowFlags`
//                  prefix per the nodeflags.rs/symbolflags.rs convention)
//   FlowNode       → FlowNode (`*Node`/`*FlowNode`/`*FlowList` pointers become
//                  NodeId/FlowNodeId/FlowListId handles, SPEC §5.1/§5.2)
//   FlowList       → FlowList + FlowListId (Go arena-allocates each cons cell
//                  in `core.Arena[FlowList]`; the port's arena is the binder's
//                  `Vec<FlowList>`, so the cells need an id newtype — ids.rs
//                  pre-declares only FlowNodeId, hence the new type lives here)
//   FlowLabel      → type alias to FlowNode (Go: `type FlowLabel = FlowNode`)
//   NewFlowSwitchClauseData / NewFlowReduceLabelData → the two data structs
//                  carried as [`FlowNodeData`] variants instead of synthetic
//                  KindUnknown AST nodes (see the PORT note on FlowNodeData)
//
// PORT (flow node id scheme, per the M4 wave-A brief): Go allocates flow
// nodes in `core.Arena[ast.FlowNode]` inside the Binder and hands out
// `*FlowNode` pointers; nodes' `flow_node` payload fields and the checker's
// flow graph then reference those pointers. The port reuses the ids.rs
// packed-id scheme: `FlowNodeId(u32)` indexes the binder's flow-node arena
// (per-file, SPEC §5.10 "bind writes the flow graph into bind results");
// the arena travels with [`crate::BindResult`]-style side data owned by the
// binder crate, to be re-based into the program-level store at the M5
// program merge.

use crate::{NodeId, FlowNodeId};

define_flags!(FlowFlags, u32);

impl FlowFlags {
    pub const UNREACHABLE: FlowFlags = FlowFlags(1 << 0); // Unreachable code
    pub const START: FlowFlags = FlowFlags(1 << 1); // Start of flow graph
    pub const BRANCH_LABEL: FlowFlags = FlowFlags(1 << 2); // Non-looping junction
    pub const LOOP_LABEL: FlowFlags = FlowFlags(1 << 3); // Looping junction
    pub const ASSIGNMENT: FlowFlags = FlowFlags(1 << 4); // Assignment
    pub const TRUE_CONDITION: FlowFlags = FlowFlags(1 << 5); // Condition known to be true
    pub const FALSE_CONDITION: FlowFlags = FlowFlags(1 << 6); // Condition known to be false
    pub const SWITCH_CLAUSE: FlowFlags = FlowFlags(1 << 7); // Switch statement clause
    pub const ARRAY_MUTATION: FlowFlags = FlowFlags(1 << 8); // Potential array mutation
    pub const CALL: FlowFlags = FlowFlags(1 << 9); // Potential assertion call
    pub const REDUCE_LABEL: FlowFlags = FlowFlags(1 << 10); // Temporarily reduce antecedents of label
    pub const REFERENCED: FlowFlags = FlowFlags(1 << 11); // Referenced as antecedent once
    pub const SHARED: FlowFlags = FlowFlags(1 << 12); // Referenced as antecedent more than once

    pub const LABEL: FlowFlags = FlowFlags(Self::BRANCH_LABEL.0 | Self::LOOP_LABEL.0);
    pub const CONDITION: FlowFlags = FlowFlags(Self::TRUE_CONDITION.0 | Self::FALSE_CONDITION.0);
}

/// ids.rs declares `FlowNodeId` (the FlowNode arena index) with a `Default`
/// of `u32::MAX` but no `NONE` sentinel; the sentinel lives here beside the
/// flow types that consume it (Go's nil `*FlowNode`).
impl FlowNodeId {
    /// Sentinel for "no flow node" (Go nil `*FlowNode`).
    pub const NONE: FlowNodeId = FlowNodeId(u32::MAX);

    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
}

/// Index into the flow-list cons-cell arena (Go: `*FlowList`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FlowListId(pub u32);

impl FlowListId {
    /// Sentinel for "no list cell" (Go nil `*FlowList`).
    pub const NONE: FlowListId = FlowListId(u32::MAX);

    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
}

impl Default for FlowListId {
    fn default() -> Self {
        FlowListId::NONE
    }
}

// ────────────────────────────────────────────────────────────────────────────
// FlowNode
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type FlowNode struct { Flags FlowFlags; Node *Node; Antecedent *FlowNode;
/// Antecedents *FlowList }`.
///
/// PORT (synthetic flow-data nodes): Go points `Node` at synthetic
/// KindUnknown AST arena nodes created by `NewFlowSwitchClauseData` /
/// `NewFlowReduceLabelData` (flow.go). The port's generated `NodeData` enum
/// (frozen for this wave) has no variants for those two payloads, and
/// allocating real AST nodes for them would require the factory hook surface
/// — so the two synthetic payloads ride directly on the flow node as
/// dedicated [`FlowNodeData`] variants. The checker (M6 flow.go) reads them
/// via the enum instead of `node.AsFlowSwitchClauseData()`; the data
/// structs themselves are ported verbatim below.
#[derive(Clone, Debug)]
pub struct FlowNode {
    pub flags: FlowFlags,
    /// Go: `Node *Node` — the associated AST node, or one of the two
    /// synthetic flow-data payloads.
    pub node: FlowNodeData,
    /// Antecedent for all but FlowLabel (Go: nil-able `*FlowNode`).
    pub antecedent: Option<FlowNodeId>,
    /// Linked list of antecedents for FlowLabel (Go: `Antecedents *FlowList`).
    pub antecedents: Option<FlowListId>,
}

impl FlowNode {
    /// Go: the zero flow node (`newFlowNode` fills Flags first).
    pub fn new(flags: FlowFlags) -> Self {
        FlowNode {
            flags,
            node: FlowNodeData::None,
            antecedent: None,
            antecedents: None,
        }
    }
}

/// Go: the `Node *Node` field of [`FlowNode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowNodeData {
    /// Go: nil `Node`.
    None,
    /// A regular associated AST node.
    Node(NodeId),
    /// Go: a synthetic KindUnknown node wrapping `FlowSwitchClauseData`
    /// (created by `NewFlowSwitchClauseData`).
    SwitchClause(FlowSwitchClauseData),
    /// Go: a synthetic KindUnknown node wrapping `FlowReduceLabelData`
    /// (created by `NewFlowReduceLabelData`).
    ReduceLabel(FlowReduceLabelData),
}

/// Go: `type FlowList struct { Flow *FlowNode; Next *FlowList }` — a
/// persistent cons cell. Sharing is load-bearing (the try/finally
/// `combineFlowLists` reuses tails), so the cells are arena-allocated and
/// referenced by [`FlowListId`] rather than inlined into a Vec.
#[derive(Clone, Debug)]
pub struct FlowList {
    pub flow: FlowNodeId,
    pub next: Option<FlowListId>,
}

/// Go: `type FlowLabel = FlowNode`.
pub type FlowLabel = FlowNode;

// ────────────────────────────────────────────────────────────────────────────
// FlowSwitchClauseData (synthetic AST node for FlowFlagsSwitchClause)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type FlowSwitchClauseData struct { NodeBase; SwitchStatement *Node;
/// ClauseStart int32; ClauseEnd int32 }` (NodeBase dropped — the port does
/// not allocate a synthetic AST node; see the FlowNode PORT note).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowSwitchClauseData {
    pub switch_statement: NodeId,
    /// Start index of case/default clause range.
    pub clause_start: i32,
    /// End index of case/default clause range.
    pub clause_end: i32,
}

impl FlowSwitchClauseData {
    /// Go: `func (node *FlowSwitchClauseData) IsEmpty() bool`.
    pub fn is_empty(&self) -> bool {
        self.clause_start == self.clause_end
    }
}

// ────────────────────────────────────────────────────────────────────────────
// FlowReduceLabelData (synthetic AST node for FlowFlagsReduceLabel)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type FlowReduceLabelData struct { NodeBase; Target *FlowLabel;
/// Antecedents *FlowList }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowReduceLabelData {
    pub target: FlowNodeId,
    /// Temporary antecedent list.
    pub antecedents: Option<FlowListId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go: the FlowFlags bit layout (flow.go const block) — exact values.
    #[test]
    fn flow_flags_bit_values_mirror_go() {
        assert_eq!(FlowFlags::UNREACHABLE, FlowFlags(1 << 0));
        assert_eq!(FlowFlags::START, FlowFlags(1 << 1));
        assert_eq!(FlowFlags::BRANCH_LABEL, FlowFlags(1 << 2));
        assert_eq!(FlowFlags::LOOP_LABEL, FlowFlags(1 << 3));
        assert_eq!(FlowFlags::ASSIGNMENT, FlowFlags(1 << 4));
        assert_eq!(FlowFlags::TRUE_CONDITION, FlowFlags(1 << 5));
        assert_eq!(FlowFlags::FALSE_CONDITION, FlowFlags(1 << 6));
        assert_eq!(FlowFlags::SWITCH_CLAUSE, FlowFlags(1 << 7));
        assert_eq!(FlowFlags::ARRAY_MUTATION, FlowFlags(1 << 8));
        assert_eq!(FlowFlags::CALL, FlowFlags(1 << 9));
        assert_eq!(FlowFlags::REDUCE_LABEL, FlowFlags(1 << 10));
        assert_eq!(FlowFlags::REFERENCED, FlowFlags(1 << 11));
        assert_eq!(FlowFlags::SHARED, FlowFlags(1 << 12));
        assert_eq!(FlowFlags::LABEL, FlowFlags::BRANCH_LABEL | FlowFlags::LOOP_LABEL);
        assert_eq!(FlowFlags::CONDITION, FlowFlags::TRUE_CONDITION | FlowFlags::FALSE_CONDITION);
    }

    #[test]
    fn flow_node_data_shapes() {
        let mut flow = FlowNode::new(FlowFlags::START);
        assert_eq!(flow.node, FlowNodeData::None);
        assert!(flow.antecedent.is_none());
        assert!(flow.antecedents.is_none());
        flow.node = FlowNodeData::SwitchClause(FlowSwitchClauseData {
            switch_statement: NodeId::new(0, 1),
            clause_start: 0,
            clause_end: 0,
        });
        match &flow.node {
            FlowNodeData::SwitchClause(d) => assert!(d.is_empty()),
            _ => panic!("unreachable"),
        }
        assert!(FlowNodeId::NONE.is_none());
        assert!(FlowListId::NONE.is_none());
    }
}
