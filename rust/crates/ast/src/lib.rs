// Ported from tsc/internal/ast @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// tsc-ast — the M2 AST core. Generated pieces (from tools/scripts/tsc/ast.json,
// via rust/tools/gen-ast — SPEC §5.8, §6):
//   - kind_generated.rs   (Kind enum, ordinals 1:1 with Go; kind guards; name())
//   - ast_generated.rs    (node structs with flattened base composition,
//                          NodeData, as_/is_ accessors, for_each_child)
//
// This file carries ONLY the small hand-written core the generated code needs
// (Node/NodeId per SPEC §5.1). The rest of the Go ast package — utilities.go,
// ast.go helpers, symbol.go, visitor.go, NodeFactory — is ported in separate
// M2 tasks.

pub mod ast_generated;
pub mod kind_generated;

pub use ast_generated::*;
pub use kind_generated::*;

use std::cell::Cell;
pub use tsc_core::text::{TextPos, TextRange};

/// SPEC §5.1: arena-stored node handle — `file_id:20 | local_index:44`,
/// unique across the program. `Copy`; a `&SourceFile::node(id)` accessor
/// replaces Go's `*Node`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Sentinel for "no parent yet" (parent is assigned in a post-parse pass).
    pub const NONE: NodeId = NodeId(u64::MAX);
}

/// SPEC §5.2: index into the program-level symbol arena (`Vec<Symbol>` on
/// `Program`). The `Symbol` struct itself is a separate M2 port (symbol.go).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SymbolId(pub u32);

/// Index into the flow-graph `Arena<FlowNode>` (Go: `*FlowNode`; the flow
/// graph lives in bind results, SPEC §5.1/§5.10). The `FlowNode` struct is a
/// separate M2 port (ast/flow.go).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FlowNodeId(pub u32);

// TODO(port): NodeFlags/TokenFlags/ModifierFlags are bitflags newtypes with
// bit values identical to Go (ast/flags.go) — they appear in baselines and
// tsbuildinfo diffs, so the exact values matter. Opaque newtypes here until
// the flags.go port lands; the generated code only stores them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct NodeFlags(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct TokenFlags(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct ModifierFlags(pub u32);

// TODO(port): ast/symbol.go — `SymbolTable` is an insertion-ordered
// `Atom -> SymbolId` map (SPEC §5.2: IndexMap; iteration order is observable
// in baselines, so a plain HashMap is forbidden). Unit placeholder until that
// port lands; the generated code only stores the field.
#[derive(Default)]
pub struct SymbolTable;

/// SPEC §5.1: Go `type NodeList struct { Loc; Nodes []*Node }`. Lists are
/// inline values here (Go arena-allocates them separately); zero-length
/// lists are `None` where Go uses nil.
#[derive(Clone, Debug, Default)]
pub struct NodeList {
    pub loc: TextRange,
    pub nodes: Box<[NodeId]>,
}

/// Go `type ModifierList struct { NodeList; ModifierFlags }` (flattened).
#[derive(Clone, Debug, Default)]
pub struct ModifierList {
    pub loc: TextRange,
    pub nodes: Box<[NodeId]>,
    pub modifier_flags: ModifierFlags,
}

/// SPEC §5.1: the arena slot. Go: `type Node struct { Kind; Flags; Loc; id;
/// Parent *Node; data nodeData }`. `NodeData` is the generated enum
/// (one variant per concrete node struct). The `Cell` fields hold state set
/// after construction but before escape (id, parent), exactly as in Go.
pub struct Node {
    pub kind: Kind,
    pub flags: NodeFlags,
    pub loc: TextRange,
    /// Lazily-assigned node id (Go: `id atomic.Uint64`).
    pub id: Cell<u64>,
    /// Assigned in a post-parse pass (Go assigns eagerly; SPEC §5.1 defers).
    pub parent: Cell<NodeId>,
    pub data: NodeData,
}

// TODO(port): SourceFile is hand-written in tsc/internal/ast/ast.go; the full
// Rust port (text, line map, path/lib refs, bind results, parse diagnostics)
// is a separate M2 task. Only the schema-derived fields are here so the
// generated NodeData::SourceFile variant and accessors compile. Struct and
// factory are hand-written in Go, so this lives in the hand-written core,
// not in ast_generated.rs.
pub struct SourceFile {
    pub statements: Option<NodeList>,
    pub end_of_file_token: NodeId,
}

impl SourceFile {
    /// Go hand-writes SourceFile.ForEachChild in ast.go (it is one of the
    /// `handWritten` nodes in ast.json); port it with the ast.go M2 task.
    pub fn for_each_child(&self, _visit: &mut dyn FnMut(NodeId) -> bool) -> bool {
        todo!() // TODO(port): tsc/internal/ast/ast.go, SourceFile.ForEachChild
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2 gate item (SPEC §8): the Kind table must be 1:1 with the Go
    /// ordinals. Spot-checks here; the full table test lives with the kind
    /// table task.
    #[test]
    fn kind_ordinals_mirror_go() {
        assert_eq!(Kind::Unknown as u16, 0);
        assert_eq!(Kind::EndOfFile as u16, 1);
        assert_eq!(Kind::NotEmittedTypeElement as u16, 351);
        // Go: KindCount == iota after the last named kind.
        assert_eq!(Kind::Count as u16, 352);
        // Go marker consts.
        assert_eq!(Kind::FirstAssignment, Kind::EqualsToken);
        assert_eq!(Kind::LastKeyword, Kind::SourceKeyword);
        // Go String() text.
        assert_eq!(Kind::Unknown.name(), "KindUnknown");
        assert_eq!(Kind::JSDocTypeExpression.name(), "KindJSDocTypeExpression");
        assert_eq!(Kind::Count.to_string(), "KindCount");
    }

    #[test]
    fn kind_guards_mirror_go() {
        assert!(is_keyword_kind(Kind::BreakKeyword));
        assert!(!is_keyword_kind(Kind::Identifier));
        assert!(is_token_kind(Kind::Unknown));
        assert!(!is_token_kind(Kind::QualifiedName));
    }
}
