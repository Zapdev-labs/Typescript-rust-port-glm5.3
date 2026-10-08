// Ported from tsc/internal/vfs/vfs.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::any::Any;
use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::time::SystemTime;

use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath};

use crate::sysfs::FileMode;

// FS is a file system abstraction over rooted, normalized paths. Operations
// declare whether they require a file, directory, or either kind of path.
//
// PORT (trait design): Go composes `vfs.FS` as a flat interface; the port
// keeps the same flat surface as a single object-safe trait `Fs` so callers
// hold `Arc<dyn Fs>` / `&dyn Fs` exactly where Go holds a `vfs.FS` interface
// value. The Go extension points that live in *other* packages' interfaces
// (iovfs.RealpathFS / iovfs.WritableFS over fs.FS) become the default
// methods of `sysfs::SubFs` (see sysfs.rs) — there are no RealFS /
// SortedReadDirFS-style embedded interfaces in vfs.go at this revision, so
// no extension traits are needed. `Fs: Send + Sync` matches Go interface
// values being freely shared across goroutines.
pub trait Fs: Send + Sync {
    /// CaseSensitivity returns whether path comparison is case-sensitive.
    fn case_sensitivity(&self) -> CaseSensitivity;

    /// FileExists returns true if the file exists.
    fn file_exists(&self, path: &RootedFilePath) -> bool;

    /// ReadFile reads the file specified by path and returns the content.
    /// If the file fails to be read, returns None.
    ///
    /// PORT: Go returns (contents string, ok bool); `Option<String>` carries
    /// the same contract (None means ok == false).
    fn read_file(&self, path: &RootedFilePath) -> Option<String>;

    fn write_file(&self, path: &RootedFilePath, data: &str) -> io::Result<()>;

    /// AppendFile appends data to the file at path, creating it if it does not exist.
    fn append_file(&self, path: &RootedFilePath, data: &str) -> io::Result<()>;

    /// Removes `path` and all its contents. Will return the first error it encounters.
    fn remove(&self, path: &RootedPath) -> io::Result<()>;

    /// Chtimes changes the access and modification times of the named file.
    fn chtimes(&self, path: &RootedPath, atime: SystemTime, mtime: SystemTime) -> io::Result<()>;

    /// DirectoryExists returns true if the path is a directory.
    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool;

    /// GetAccessibleEntries returns the files/directories in the specified directory.
    /// If any entry is a symlink, it will be followed.
    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries;

    /// Stat returns the file metadata for path, or None when it does not exist
    /// (PORT: Go's nil FileInfo).
    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>>;

    /// Realpath returns the "real path" of the specified path, following
    /// symlinks and correcting filename casing. On failure the input path is
    /// returned (all Go implementations fall back to the original).
    fn realpath(&self, path: &RootedPath) -> RootedPath;
}

/// Entries is the result of GetAccessibleEntries.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Entries {
    pub files: Vec<String>,
    pub directories: Vec<String>,
    // Symlinks contains the names of entries in Files or Directories that were
    // originally symbolic links (or reparse points) on disk. The names are the
    // same as those in Files/Directories (i.e., the link name, not the target).
    // None means symlink information is not available and the entries may need
    // to be re-checked for symlinks.
    //
    // PORT: Go's `Symlinks map[string]struct{}` nil-vs-empty distinction
    // becomes Option (nil -> None).
    pub symlinks: Option<HashSet<String>>,
}

/// FileInfo mirrors Go's `fs.FileInfo` (aliased as vfs.FileInfo).
pub trait FileInfo: Send + Sync {
    fn name(&self) -> &str;
    fn size(&self) -> i64;
    fn mode(&self) -> FileMode;
    fn mod_time(&self) -> SystemTime;
    fn is_dir(&self) -> bool;
    /// Go's `Sys() any` attachment point (raw underlying data).
    fn sys(&self) -> Option<&(dyn Any + Send + Sync)>;
}

/// DirEntry mirrors Go's `fs.DirEntry` (aliased as vfs.DirEntry).
pub trait DirEntry {
    fn name(&self) -> &str;
    fn is_dir(&self) -> bool;
    /// Type returns the type bits for the entry (Go DirEntry.Type).
    fn type_bits(&self) -> FileMode;
    /// Info returns the FileInfo for the entry (Go DirEntry.Info).
    fn info(&self) -> io::Result<Arc<dyn FileInfo>>;
}

// PORT: Go aliases the stdlib fs error values (fs.ErrInvalid, ...). Rust's
// io::Error carries a kind plus message; these constructors reproduce the
// kinds and the exact Go message strings so `ErrorContains`-style checks and
// ErrorKind comparisons keep working.
pub fn err_invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid argument")
}

pub fn err_permission() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "permission denied")
}

pub fn err_exist() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "file already exists")
}

pub fn err_not_exist() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "file does not exist")
}

pub fn err_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "file already closed")
}

// WalkDirFunc is like [fs.WalkDirFunc], but reports rooted, normalized
// [tspath.RootedPath] paths.
//
// PORT: Go's walkFn returns error, where fs.SkipDir / fs.SkipAll are sentinel
// errors and anything else aborts the walk. The port splits the return into
// `Result<WalkDirControl, io::Error>` so the sentinels become enum variants
// (SkipDir, SkipAll) and `Err` is the "other error" case.
pub type WalkDirFunc<'a> = dyn FnMut(&RootedPath, Option<&dyn DirEntry>, Option<&io::Error>) -> Result<WalkDirControl, io::Error> + 'a;

/// WalkDirControl carries Go's fs.SkipDir / fs.SkipAll walk sentinels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WalkDirControl {
    /// Keep walking (Go's nil return).
    Continue,
    /// SkipDir: skip the current directory's remaining entries (or this file).
    SkipDir,
    /// SkipAll: skip all remaining files and directories.
    SkipAll,
}

/// FileInfoToDirEntry mirrors Go's fs.FileInfoToDirEntry.
pub fn file_info_to_dir_entry(info: Arc<dyn FileInfo>) -> Box<dyn DirEntry> {
    Box::new(FileInfoDirEntry { info })
}

struct FileInfoDirEntry {
    info: Arc<dyn FileInfo>,
}

impl DirEntry for FileInfoDirEntry {
    fn name(&self) -> &str {
        self.info.name()
    }

    fn is_dir(&self) -> bool {
        self.info.is_dir()
    }

    fn type_bits(&self) -> FileMode {
        self.info.mode().type_bits()
    }

    fn info(&self) -> io::Result<Arc<dyn FileInfo>> {
        Ok(Arc::clone(&self.info))
    }
}
