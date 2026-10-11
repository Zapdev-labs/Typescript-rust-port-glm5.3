// Ported from tsc/internal/checker @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// tsc-checker — the M5 checker spike (SPEC §8 M5): the crate skeleton with the
// Type/TypeData arena, proving the `t.checker` eradication strategy (SPEC §5.4)
// on checker/types.go + checker/utilities.go + the intrinsic/literal type
// constructors (checker.go's literal/intrinsic slice), with the M5 gate demo:
// `getTypeOfSymbol` on `let x = 1` infers the literal type `1`.
//
// File map (one Go file = one Rust file, SPEC §4.2):
//   - types.rs      ← checker/types.go (complete: kinds/flags, TypeData shapes,
//                     Signature, links structs)
//   - utilities.rs  ← checker/utilities.go (the self-contained utility family;
//                     PORT banners mark the seams into unported waves)
//   - links.rs      ← checker/links.go (nodeLinkStore/symbolArenaLinkStore)
//   - checker.rs    ← checker/checker.go — ONLY the slices the spike scope
//                     prescribes: the Checker skeleton, NewChecker's
//                     intrinsic/literal creation, and the getTypeOfSymbol path.
//                     The remaining checker.go surface (~28k LOC) is M6.
//   - ast_shims.rs  ← Go ast-package helpers the utilities family needs that no
//                     landed crate exports yet (dedup candidates — same pattern
//                     as the binder's ast_shims.rs; fold on the ast/utilities.go
//                     wholesale wave).
//
// PORT (SPEC §5.4 — the `t.checker` eradication strategy, proven here): Go's
// `Type { ...; checker *Checker; data TypeData }` is self-referential (TypeBase
// embeds Type; `t.data.AsType() == t`), which cannot port to an arena design.
// Instead (exactly the SPEC §5.4 prescription):
//   - `Type` stores `checker_shard: u16` — the index of the checker (Go: one of
//     the 4 pool checkers) that created the type. Types from different shards
//     are distinguishable (`CompareTypes` panics across shards, mirroring
//     Go's `t1.checker != t2.checker` check) and the M6 pool merge re-bases ids.
//   - Every code path that read `t.checker` (in the spike's file scope:
//     `CompareTypes`/`compareTypeNames` in utilities.go) takes the checker as
//     an explicit `&Checker` parameter instead. The types themselves are pure
//     data (handles into the per-checker `Vec<Type>` arena + `checker_shard`).

/// Hand-rolled bitflag pattern shared by this crate's flag modules — the same
/// macro as tsc-ast's lib.rs (kept crate-local there too; hoist into a shared
/// crate when a third copy appears).
///
/// PORT: Go defines these as bare integer types with `const X Type = 1 << n`;
/// the port keeps the exact bit values (they appear in baselines and type
/// ordering — `TypeFlags` values determine union constituent order) as
/// associated consts on a newtype with the usual bitset operators.
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

pub mod ast_shims;
pub mod checker;
pub mod contextual;
pub mod exports;
pub mod expressions;
pub mod inference;
pub mod jsdoc;
pub mod links;
pub mod mapper;
pub mod members;
pub mod relater;
pub mod resolution;
pub mod resolvecall;
pub mod spine;
pub mod statements;
pub mod stringer_generated;
pub mod symboltracker;
pub mod todo_shapes;
pub mod typenodes;
pub mod typeofsymbol;
pub mod uniontypes;
pub mod tracer;
pub mod tracing;
pub mod types;
pub mod utilities;

pub use checker::{is_fresh_literal_type, new_checker, Checker, Program, TypeSystemEntity};
pub use links::{NodeLinkStore, SymbolArenaLinkStore};
pub use types::*;
