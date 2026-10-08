// Ported from tsc/internal/ast/ids.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go ids.go is a two-line type block:
//
//	type (
//		NodeId   uint64
//		SymbolId uint64
//	)
//
// In the Go port these are the *lazily assigned* id counters backing the
// `id atomic.Uint64` fields on Node/Symbol (used as LinkStore keys). The Rust
// port keeps that role for the `Cell<u64>` fields but REPURPOSES the names per
// SPEC §5.1/§5.2:
//
//   - `NodeId` is the packed arena handle — `file_id:20 | local_index:44`,
//     unique across the program — that replaces every Go `*Node` field.
//   - `SymbolId` is the index into the program-level `Vec<Symbol>` arena.
//
// Go's lazily-assigned ids remain the `id: Cell<u64>` fields on
// [`crate::Node`]/[`crate::Symbol`] (0 = unassigned), exactly as in Go.

/// SPEC §5.1: arena-stored node handle — `file_id:20 | local_index:44`,
/// unique across the program. `Copy`; a `&SourceFile::node(id)` accessor
/// (the [`crate::NodeStore`] trait) replaces Go's `*Node`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u64);

/// SPEC §5.2: index into the program-level symbol arena (`Vec<Symbol>` on
/// `Program`); replaces Go's `*Symbol` back-references.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SymbolId(pub u32);

/// Index into the flow-graph `Arena<FlowNode>` (Go: `*FlowNode`; the flow
/// graph lives in bind results, SPEC §5.1/§5.10). The `FlowNode` struct is a
/// separate M2 port (ast/flow.go).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FlowNodeId(pub u32);

const LOCAL_INDEX_BITS: u32 = 44;
const LOCAL_INDEX_MASK: u64 = (1 << LOCAL_INDEX_BITS) - 1;

impl NodeId {
    /// Packs `file_id:20 | local_index:44` (SPEC §5.1).
    #[inline]
    pub const fn new(file_id: u32, local_index: u64) -> NodeId {
        NodeId(((file_id as u64) << LOCAL_INDEX_BITS) | (local_index & LOCAL_INDEX_MASK))
    }

    /// The owning file's id (top 20 bits).
    #[inline]
    pub const fn file_id(self) -> u32 {
        (self.0 >> LOCAL_INDEX_BITS) as u32
    }

    /// The index into the owning file's node arena (low 44 bits).
    #[inline]
    pub const fn local_index(self) -> u64 {
        self.0 & LOCAL_INDEX_MASK
    }

    /// Sentinel for "no node" (Go nil `*Node`); also used for parentless nodes
    /// before the post-parse parent pass.
    pub const NONE: NodeId = NodeId(u64::MAX);

    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == u64::MAX
    }
}

impl Default for NodeId {
    fn default() -> Self {
        NodeId::NONE
    }
}

impl SymbolId {
    pub const NONE: SymbolId = SymbolId(u32::MAX);

    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
}

impl Default for SymbolId {
    fn default() -> Self {
        SymbolId::NONE
    }
}

impl Default for FlowNodeId {
    fn default() -> Self {
        FlowNodeId(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_packing_roundtrips() {
        let id = NodeId::new(0xABCDE, 0x12345);
        assert_eq!(id.file_id(), 0xABCDE);
        assert_eq!(id.local_index(), 0x12345);
        // 20-bit file id, 44-bit local index (SPEC §5.1).
        let max = NodeId::new((1 << 20) - 1, (1 << 44) - 1);
        assert_eq!(max.file_id(), (1 << 20) - 1);
        assert_eq!(max.local_index(), (1 << 44) - 1);
        assert!(NodeId::NONE.is_none());
        assert!(NodeId::default().is_none());
    }
}
