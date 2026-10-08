// Package bundled provides access to files bundled with TypeScript.
//
// Ported from tsc/internal/bundled/bundled.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// File map:
//   bundled.go         → lib.rs (this file)
//   embed.go           → embed.rs (always-embedded variant)
//   embed_generated.go → embed_generated.rs (generate.sh output; the Go file
//                        is `//go:embed`-driven, the Rust one include_str!-driven)
//   libs_generated.go  → libs_generated.rs (generate.sh output)
//   generate.go        → generate.sh (the generator; `//go:build ignore` → a
//                        shell script, same validations)
//   noembed.go         → not ported (no build-tag equivalent; the port always
//                        embeds — PORTING-NOTES)
//
// Generated outputs are reproducible: `sh generate.sh` is idempotent.

pub mod embed;
pub mod embed_generated;
pub mod libs_generated;

use std::sync::Arc;

use tsc_tspath::RootedDirectoryPath;
use tsc_vfs::Fs;

pub use embed::is_bundled;

/// Embedded is true if the bundled files are implemented through an embedded
/// FS. Go: `const Embedded = embedded`; the port always embeds (noembed.go is
/// not ported).
pub const EMBEDDED: bool = embed::EMBEDDED;

/// WrapFS returns an FS which redirects embedded paths to the embedded file
/// system. If the embedded file system is not available, it returns the
/// original FS.
///
/// PORT: the port always embeds, so the second sentence is dead (Go's noembed
/// build); the function wraps unconditionally like Go's embed variant.
pub fn wrap_fs(fs: Arc<dyn Fs>) -> Arc<dyn Fs> {
    Arc::new(embed::wrap_fs(fs))
}

/// LibPath returns the path to the directory containing the bundled lib.d.ts
/// files. If embedding is not enabled, this is a path on disk, and must be
/// accessed through a real OS filesystem.
///
/// Go: `tspath.RootedDirectoryPathFromNormalized(libPath())`.
pub fn lib_path() -> RootedDirectoryPath {
    tsc_tspath::rooted_directory_path_from_normalized(&embed::lib_path())
}

/// TestingLibPath returns the path to the source bundled libs directory.
/// It's only valid to use in tests where the source code is available.
///
/// PORT: Go derives the source dir from `runtime.Caller(0)` and panics outside
/// `testing.Testing()`; Rust has no runtime caller introspection, so the path
/// is the compile-time location of the Go sources relative to this crate
/// (valid in any checkout that contains tsc/, which the workspace requires —
/// the generated embed table reads the same directory).
pub fn testing_lib_path() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../tsc/internal/bundled/libs").to_string()
}

/// Re-export of the generated `LibNames` list under its Go name.
pub use libs_generated::LIB_NAMES as LibNames;
