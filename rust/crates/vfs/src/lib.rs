// Ported from tsc/internal/vfs @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Layout decision (PORT): the Go tree has one package per directory
// (tsc/internal/vfs, vfs/osvfs, vfs/wrapvfs, vfs/vfstest, vfs/iovfs and the
// unexported vfs/internal). The Rust port keeps them as modules inside ONE
// crate (`tsc-vfs`) so the whole layer ships as a single dependency, matching
// the Go packages' import graph (all of them import tsc/internal/vfs):
//
//   - `sysfs`   : the subset of Go's io/fs the vfs packages build on
//                 (fs.FileMode + the fs.FS/RealpathFS/WritableFS role, named
//                 `SubFs`). No Go file — it stands in for the stdlib.
//   - `vfs`     : vfs.go — the `Fs` trait, `Entries`, `FileInfo`/`DirEntry`,
//                 error sentinels, walk controls.
//   - `walkdir` : walkdir.go — `walk_dir` over `&dyn Fs`.
//   - `internal`: vfs/internal/internal.go — `Common`, the shared
//                 implementation over `SubFs` (pub(crate)).
//   - `iovfs`   : iovfs/iofs.go — adapts a `SubFs` into an `Fs`.
//   - `osvfs`   : osvfs/os.go — the real-filesystem `Fs`.
//   - `vfstest` : vfstest/vfstest.go — the in-memory `MapFS` (the Go package
//                 leans on testing/fstest.MapFS; the port folds the required
//                 fstest.MapFS behavior into `MapFile`/`MapFS` directly).
//   - `wrapvfs` : wrapvfs/wrapvfs.go — per-method delegation overrides.
//
// Go packages NOT ported (out of scope for this crate's consumers; none are
// imported by the packages above): cachedvfs, trackingvfs, vfsmatch, vfsmock.

pub mod sysfs;
pub mod walkdir;

mod vfs;
pub use vfs::*;

pub(crate) mod internal;

pub mod iovfs;
pub mod osvfs;
pub mod vfstest;
pub mod wrapvfs;

#[cfg(test)]
mod iovfs_test;
#[cfg(test)]
mod osvfs_test;
#[cfg(test)]
mod vfstest_test;
#[cfg(test)]
mod walkdir_test;
