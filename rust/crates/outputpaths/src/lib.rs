// Ported from tsc/internal/outputpaths @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// tsc-outputpaths — output path computation for js/d.ts/map/tsbuildinfo emit.
//
// One Go file = one Rust file:
//   outputpaths.go          → outputpaths.rs
//   commonsourcedirectory.go → commonsourcedirectory.rs
//
// Go's `outputpaths.OutputPathsHost` interface becomes the object-safe trait
// `OutputPathsHost` (the vfs-crate `Arc<dyn Fs>` precedent for Go interface
// values; Go's `*tsoptions.ParsedCommandLine` and `*compiler.emitHost` both
// implement it, so the trait must be implementable from other crates).

pub mod commonsourcedirectory;
pub mod outputpaths;

pub use commonsourcedirectory::{
    get_common_source_directory, get_computed_common_source_directory,
};
pub use outputpaths::*;
