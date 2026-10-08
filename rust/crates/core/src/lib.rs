// Ported from tsc/internal/core @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Not yet ported (separate tasks per the M1 plan): core.go (depends on
// further cross-package pieces; see SPEC M1 sequencing).

pub mod arena;
pub mod bfs;
pub mod binarysearch;
pub mod compileroptions;
pub mod context;
pub mod languagevariant;
pub mod languagevariant_stringer_generated;
pub mod linkstore;
pub mod nodemodules;
pub mod options_generated;
pub mod pattern;
pub mod scriptkind;
pub mod scriptkind_stringer_generated;
pub mod semaphore;
pub mod stack;
pub mod text;
pub mod textchange;
pub mod tristate;
pub mod tristate_stringer_generated;
pub mod version;
pub mod workgroup;

#[cfg(test)]
mod bfs_test;
#[cfg(test)]
mod pattern_test;
