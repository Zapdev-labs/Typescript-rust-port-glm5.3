// Ported from tsc/internal/vfs/wrapvfs/wrapvfs.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::io;
use std::sync::Arc;
use std::time::SystemTime;

use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath};

use crate::vfs::{Entries, Fs, FileInfo};

// Type aliases for the per-method override closures (Go stores bare funcs).
type CaseSensitivityFn = Box<dyn Fn() -> CaseSensitivity + Send + Sync>;
type FileExistsFn = Box<dyn Fn(&RootedFilePath) -> bool + Send + Sync>;
type ReadFileFn = Box<dyn Fn(&RootedFilePath) -> Option<String> + Send + Sync>;
type WriteFileFn = Box<dyn Fn(&RootedFilePath, &str) -> io::Result<()> + Send + Sync>;
type AppendFileFn = Box<dyn Fn(&RootedFilePath, &str) -> io::Result<()> + Send + Sync>;
type RemoveFn = Box<dyn Fn(&RootedPath) -> io::Result<()> + Send + Sync>;
type ChtimesFn = Box<dyn Fn(&RootedPath, SystemTime, SystemTime) -> io::Result<()> + Send + Sync>;
type DirectoryExistsFn = Box<dyn Fn(&RootedDirectoryPath) -> bool + Send + Sync>;
type GetAccessibleEntriesFn = Box<dyn Fn(&RootedDirectoryPath) -> Entries + Send + Sync>;
type StatFn = Box<dyn Fn(&RootedPath) -> Option<Arc<dyn FileInfo>> + Send + Sync>;
type RealpathFn = Box<dyn Fn(&RootedPath) -> RootedPath + Send + Sync>;

// Replacements mirrors Go's `Replacements` struct: every field is an optional
// method override; None delegates to the wrapped FS (the Go nil function
// fields). Go stores bare funcs; the port boxes closures.
#[derive(Default)]
pub struct Replacements {
    pub case_sensitivity: Option<CaseSensitivityFn>,
    pub file_exists: Option<FileExistsFn>,
    pub read_file: Option<ReadFileFn>,
    pub write_file: Option<WriteFileFn>,
    pub append_file: Option<AppendFileFn>,
    pub remove: Option<RemoveFn>,
    pub chtimes: Option<ChtimesFn>,
    pub directory_exists: Option<DirectoryExistsFn>,
    pub get_accessible_entries: Option<GetAccessibleEntriesFn>,
    pub stat: Option<StatFn>,
    pub realpath: Option<RealpathFn>,
}

/// Wrap returns an FS that delegates each method to `fs` unless the same
/// method is provided in `replacements`.
pub fn wrap(fs: Arc<dyn Fs>, replacements: Replacements) -> Arc<dyn Fs> {
    Arc::new(WrappedFs { fs, replacements })
}

struct WrappedFs {
    fs: Arc<dyn Fs>,
    replacements: Replacements,
}

impl Fs for WrappedFs {
    // CaseSensitivity implements [Fs].
    fn case_sensitivity(&self) -> CaseSensitivity {
        match &self.replacements.case_sensitivity {
            Some(case_sensitivity) => case_sensitivity(),
            None => self.fs.case_sensitivity(),
        }
    }

    // FileExists implements [Fs].
    fn file_exists(&self, path: &RootedFilePath) -> bool {
        match &self.replacements.file_exists {
            Some(file_exists) => file_exists(path),
            None => self.fs.file_exists(path),
        }
    }

    // ReadFile implements [Fs].
    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        match &self.replacements.read_file {
            Some(read_file) => read_file(path),
            None => self.fs.read_file(path),
        }
    }

    // WriteFile implements [Fs].
    fn write_file(&self, path: &RootedFilePath, data: &str) -> io::Result<()> {
        match &self.replacements.write_file {
            Some(write_file) => write_file(path, data),
            None => self.fs.write_file(path, data),
        }
    }

    // AppendFile implements [Fs].
    fn append_file(&self, path: &RootedFilePath, data: &str) -> io::Result<()> {
        match &self.replacements.append_file {
            Some(append_file) => append_file(path, data),
            None => self.fs.append_file(path, data),
        }
    }

    // Remove implements [Fs].
    fn remove(&self, path: &RootedPath) -> io::Result<()> {
        match &self.replacements.remove {
            Some(remove) => remove(path),
            None => self.fs.remove(path),
        }
    }

    // Chtimes implements [Fs].
    fn chtimes(&self, path: &RootedPath, atime: SystemTime, mtime: SystemTime) -> io::Result<()> {
        match &self.replacements.chtimes {
            Some(chtimes) => chtimes(path, atime, mtime),
            None => self.fs.chtimes(path, atime, mtime),
        }
    }

    // DirectoryExists implements [Fs].
    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        match &self.replacements.directory_exists {
            Some(directory_exists) => directory_exists(path),
            None => self.fs.directory_exists(path),
        }
    }

    // GetAccessibleEntries implements [Fs].
    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        match &self.replacements.get_accessible_entries {
            Some(get_accessible_entries) => get_accessible_entries(path),
            None => self.fs.get_accessible_entries(path),
        }
    }

    // Stat implements [Fs].
    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        match &self.replacements.stat {
            Some(stat) => stat(path),
            None => self.fs.stat(path),
        }
    }

    // Realpath implements [Fs].
    fn realpath(&self, path: &RootedPath) -> RootedPath {
        match &self.replacements.realpath {
            Some(realpath) => realpath(path),
            None => self.fs.realpath(path),
        }
    }
}
