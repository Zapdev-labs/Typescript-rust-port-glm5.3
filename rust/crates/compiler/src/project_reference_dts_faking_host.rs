// Ported from tsc/internal/compiler/projectreferencedtsfakinghost.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// NOT PORTED (seam): the dts-faking host wraps the resolution host's FS in a
// `cachedvfs.From(&projectReferenceDtsFakingVfs{...})` that fakes
// project-reference .d.ts outputs as their source files and tracks
// node_modules symlinks via `symlinks.KnownSymlinks`. Neither cachedvfs nor
// the symlinks package is ported (see crates/vfs/src/lib.rs — cachedvfs is in
// its unported list), and the mapper's output-dts tables that feed it stay
// empty in this wave anyway (tsoptions ParseInputOutputNames /
// SourceToProjectReference are the AST-config wave). The port's
// `ProjectReferenceFileMapper::resolution_host` therefore returns the host
// unchanged, which is exactly Go's behavior with no resolved project
// references in source mode.
//
// TODO(porting-vfs): port `projectReferenceDtsFakingVfs` (FileExists/
// DirectoryExists over the output-dts table, symlink bookkeeping) when
// cachedvfs + symlinks land; swap it into
// `ProjectReferenceFileMapper::resolution_host` (PORTING-NOTES).
