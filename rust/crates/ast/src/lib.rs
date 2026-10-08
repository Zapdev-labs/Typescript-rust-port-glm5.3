// Ported from tsc/internal/ast @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// tsc-ast — the M2 AST core. Generated pieces (from tools/scripts/tsc/ast.json,
// via rust/tools/gen-ast — SPEC §5.8, §6):
//   - kind_generated.rs   (Kind enum, ordinals 1:1 with Go; kind guards; name())
//   - ast_generated.rs    (node structs with flattened base composition,
//                          boxed NodeData, as_/is_ accessors, for_each_child,
//                          visit_each_child, Modifiers()/Name() dispatch)
//
// Hand-written surface (one module per Go file — PORT banners inside):
//   - ids.rs         (Go ids.go + the SPEC §5.1 packed NodeId)
//   - *flags.rs      (Go nodeflags/tokenflags/modifierflags/symbolflags/
//                     checkflags/functionflags.go — exact bit values)
//   - symbol.rs      (Go symbol.go)
//   - source_file.rs (Go ast.go SourceFile + parseoptions.rs/positionmap.rs)
//   - visitor.rs     (Go ast.go Visitor + visitor.go NodeVisitor)
//   - deepclone.rs   (Go deepclone.go)
//   - precedence.rs  (Go precedence.go)
//   - utilities.rs   (minimal subset of Go utilities.go — dedup on its port)
//   - diagnostic.rs  (minimal subset of Go diagnostic.go — full port pending)
//
// NodeFactory (the ~190 New*/Update* factory methods of ast.go/ast_generated.go),
// utilities.go wholesale, and subtreefacts.go remain separate M2 tasks.

/// Hand-rolled bitflag pattern shared by the flags modules (the `bitflags`
/// crate is not on the SPEC §5.11 dependency list).
///
/// PORT: Go defines these as bare integer types with `const X Type = 1 << n`
/// packages; the Rust port keeps the exact bit values (they appear in
/// baselines and tsbuildinfo diffs) as associated consts on a newtype, with
/// the usual bitset operators. Textually scoped, so declared before the
/// `mod` list below.
macro_rules! define_flags {
    ($name:ident, $repr:ty) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
        pub struct $name(pub $repr);

        impl $name {
            /// Go: the zero value (`XFlagsNone`).
            pub const NONE: Self = Self(0);
            #[inline]
            pub const fn from_bits(bits: $repr) -> Self {
                Self(bits)
            }
            #[inline]
            pub const fn bits(self) -> $repr {
                self.0
            }
            /// Go: `flags&other != 0`.
            #[inline]
            pub const fn intersects(self, other: Self) -> bool {
                (self.0 & other.0) != 0
            }
            /// Go: `flags&other == other`.
            #[inline]
            pub const fn contains(self, other: Self) -> bool {
                (self.0 & other.0) == other.0
            }
            /// Go: `flags |= other` as a value.
            #[inline]
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }
            /// Go: `flags == 0` / `flags == XFlagsNone`.
            #[inline]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }
        }

        impl std::ops::BitOr for $name {
            type Output = Self;
            #[inline]
            fn bitor(self, rhs: Self) -> Self {
                Self(self.0 | rhs.0)
            }
        }
        impl std::ops::BitOrAssign for $name {
            #[inline]
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }
        impl std::ops::BitAnd for $name {
            type Output = Self;
            #[inline]
            fn bitand(self, rhs: Self) -> Self {
                Self(self.0 & rhs.0)
            }
        }
        impl std::ops::BitAndAssign for $name {
            #[inline]
            fn bitand_assign(&mut self, rhs: Self) {
                self.0 &= rhs.0;
            }
        }
        impl std::ops::Not for $name {
            type Output = Self;
            #[inline]
            fn not(self) -> Self {
                Self(!self.0)
            }
        }
    };
}

pub mod ast_generated;
pub mod checkflags;
pub mod deepclone;
pub mod diagnostic;
pub mod functionflags;
pub mod kind_generated;
pub mod modifierflags;
pub mod nodeflags;
pub mod parseoptions;
pub mod positionmap;
pub mod precedence;
pub mod source_file;
pub mod symbol;
pub mod symbolflags;
pub mod tokenflags;
pub mod utilities;
pub mod visitor;

pub use ast_generated::*;
pub use checkflags::*;
pub use deepclone::*;
pub use diagnostic::*;
pub use functionflags::*;
pub use kind_generated::*;
pub use modifierflags::*;
pub use nodeflags::*;
pub use parseoptions::*;
pub use positionmap::*;
pub use precedence::*;
pub use source_file::*;
pub use symbol::*;
pub use symbolflags::*;
pub use tokenflags::*;
pub use utilities::*;
pub use visitor::*;

use std::cell::Cell;
pub use tsc_core::text::{TextPos, TextRange};

// ────────────────────────────────────────────────────────────────────────────
// Ids (ids.rs re-exports; the definitions live there to keep the Go
// file-per-file mapping)
// ────────────────────────────────────────────────────────────────────────────

pub use ids::{FlowNodeId, NodeId, SymbolId};

mod ids;

// ────────────────────────────────────────────────────────────────────────────
// Node lists (Go ast.go NodeList/ModifierList — factory methods land with the
// NodeFactory port)
// ────────────────────────────────────────────────────────────────────────────

/// SPEC §5.1: Go `type NodeList struct { Loc; Nodes []*Node }`. Lists are
/// inline values here (Go arena-allocates them separately); zero-length
/// lists are `None` where Go uses nil.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeList {
    pub loc: TextRange,
    pub nodes: Box<[NodeId]>,
}

impl NodeList {
    /// Go: `func (list *NodeList) Pos() int`.
    pub fn pos(&self) -> TextPos {
        self.loc.pos()
    }

    /// Go: `func (list *NodeList) End() int`.
    pub fn end(&self) -> TextPos {
        self.loc.end()
    }

    /// Go: `func (list *NodeList) HasTrailingComma() bool` — the last node is
    /// resolved through `store` (Go dereferences the stored `*Node` directly;
    /// the port stores `NodeId` handles, SPEC §5.1).
    pub fn has_trailing_comma(&self, store: &dyn NodeStore) -> bool {
        match self.nodes.as_ref().last() {
            Some(&last) => store.node(last).end() < self.end(),
            None => false,
        }
    }
}

/// Go `type ModifierList struct { NodeList; ModifierFlags }` (flattened).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModifierList {
    pub loc: TextRange,
    pub nodes: Box<[NodeId]>,
    pub modifier_flags: ModifierFlags,
}

impl ModifierList {
    pub fn pos(&self) -> TextPos {
        self.loc.pos()
    }

    pub fn end(&self) -> TextPos {
        self.loc.end()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Node (SPEC §5.1)
// ────────────────────────────────────────────────────────────────────────────

/// SPEC §5.1: the arena slot. Go: `type Node struct { Kind; Flags; Loc; id;
/// Parent *Node; data nodeData }`. `NodeData` is the generated enum (one
/// variant per concrete node struct, every payload boxed so the slot is a
/// fixed 48 bytes — see the `node_size` test and PORTING-NOTES). The `Cell`
/// fields hold state set after construction but before escape (id, parent),
/// exactly as in Go (`atomic.Uint64` id, `*Node` parent).
pub struct Node {
    pub kind: Kind,
    pub flags: NodeFlags,
    pub loc: TextRange,
    /// Lazily-assigned node id (Go: `id atomic.Uint64`, 0 = unassigned).
    pub id: Cell<u64>,
    /// Assigned in a post-parse pass (Go assigns eagerly; SPEC §5.1 defers).
    pub parent: Cell<NodeId>,
    pub data: NodeData,
}

impl Node {
    /// Go: `func (n *Node) AsNode() *Node` — identity in the Rust port
    /// (handles resolve through the arena instead).
    pub fn as_node(&self) -> &Node {
        self
    }

    /// Go: `func (n *Node) Pos() int`.
    pub fn pos(&self) -> TextPos {
        self.loc.pos()
    }

    /// Go: `func (n *Node) End() int`.
    pub fn end(&self) -> TextPos {
        self.loc.end()
    }

    /// Go: `func (n *Node) KindString() string`.
    pub fn kind_string(&self) -> &'static str {
        self.kind.name()
    }

    /// Go: `func (n *Node) KindValue() int16`.
    pub fn kind_value(&self) -> i16 {
        self.kind as i16
    }
}

/// `Node` is cloneable so visitor traversal can snapshot a node out of the
/// arena before walking (the walk may allocate into the same arena, and a
/// `Vec`-backed arena invalidates borrows on growth — see
/// `NodeVisitor::visit_each_child` and PORTING-NOTES).
impl Clone for Node {
    fn clone(&self) -> Self {
        Node {
            kind: self.kind,
            flags: self.flags,
            loc: self.loc,
            id: Cell::new(self.id.get()),
            parent: Cell::new(self.parent.get()),
            data: self.data.clone(),
        }
    }
}

/// Go: `func (n *Node) Text() string` (ast.go) — the text of a text-bearing
/// node whose text is a single stored string. Go's `Text()` additionally
/// concatenates for `KindJsxNamespacedName` and the four JSDoc comment kinds;
/// those allocate and are TODO(port) with the full Text surface (nothing in
/// the M2 surface reads them; Go panics on other kinds and so does this).
pub fn node_text(store: &dyn NodeStore, node: NodeId) -> &str {
    let n = store.node(node);
    match &n.data {
        NodeData::Identifier(d) => &d.text,
        NodeData::PrivateIdentifier(d) => &d.text,
        NodeData::StringLiteral(d) => &d.text,
        NodeData::NumericLiteral(d) => &d.text,
        NodeData::BigIntLiteral(d) => &d.text,
        NodeData::MetaProperty(d) => {
            // Go: n.AsMetaProperty().Name().Text()
            let name = d.name;
            node_text(store, name)
        }
        NodeData::NoSubstitutionTemplateLiteral(d) => &d.text,
        NodeData::TemplateHead(d) => &d.text,
        NodeData::TemplateMiddle(d) => &d.text,
        NodeData::TemplateTail(d) => &d.text,
        NodeData::RegularExpressionLiteral(d) => &d.text,
        _ => panic!("Unhandled case in Node.Text: {}", n.kind_string()),
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

    /// SPEC §12.1 boxing decision (PORTING-NOTES "NodeData boxing"): every
    /// NodeData payload is boxed, so the enum is a fixed 16 bytes and `Node`
    /// a fixed 48-byte arena slot — inside the §12 window ("≤64B without box,
    /// accept ≤96B boxed"), and a single indirection like Go's `nodeData`
    /// interface. Update this number only together with that decision.
    #[test]
    fn node_size_is_48_bytes() {
        assert_eq!(std::mem::size_of::<NodeData>(), 16);
        assert_eq!(std::mem::size_of::<Node>(), 48);
        // The SPEC §5.1 target window.
        assert!(std::mem::size_of::<Node>() <= 96);
    }
}
