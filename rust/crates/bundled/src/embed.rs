// Ported from tsc/internal/bundled/embed.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The `noembed` build-tag variant (noembed.go) is not ported: the Rust build
// always embeds (there is no -tags noembed equivalent in the port; see
// PORTING-NOTES).
//
// Go: `wrappedFS` is implemented directly over `embeddedContents` rather than
// going through io/fs; the port keeps the same direct dispatch over the
// generated `embedded_contents` (embed_generated.rs).

use std::sync::Arc;
use std::time::SystemTime;

use tsc_tspath::{RootedDirectoryPath, RootedFilePath, RootedPath};
use tsc_vfs::sysfs::FileMode;
use tsc_vfs::{Entries, Fs, FileInfo};

pub(crate) const EMBEDDED: bool = true;

const SCHEME: &str = "bundled:///";

fn split_path(path: &str) -> Option<&str> {
    path.strip_prefix(SCHEME)
}

pub(crate) fn lib_path() -> String {
    format!("{SCHEME}libs")
}

/// Go: `func IsBundled(path string) bool`.
pub fn is_bundled(path: &str) -> bool {
    split_path(path).is_some()
}

pub(crate) struct WrappedFs {
    fs: Arc<dyn Fs>,
}

/// Go: `func wrapFS(fs vfs.FS) vfs.FS` (exported as `WrapFS` in bundled.go).
pub(crate) fn wrap_fs(fs: Arc<dyn Fs>) -> WrappedFs {
    WrappedFs { fs }
}

impl Fs for WrappedFs {
    fn case_sensitivity(&self) -> tsc_tspath::CaseSensitivity {
        self.fs.case_sensitivity()
    }

    fn file_exists(&self, path: &RootedFilePath) -> bool {
        if let Some(rest) = split_path(path.as_string()) {
            return crate::embed_generated::embedded_contents(rest).is_some();
        }
        self.fs.file_exists(path)
    }

    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        if let Some(rest) = split_path(path.as_string()) {
            return crate::embed_generated::embedded_contents(rest).map(str::to_string);
        }
        self.fs.read_file(path)
    }

    fn write_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        if split_path(path.as_string()).is_some() {
            panic!("cannot write to embedded file system");
        }
        self.fs.write_file(path, data)
    }

    fn append_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        if split_path(path.as_string()).is_some() {
            panic!("cannot write to embedded file system");
        }
        self.fs.append_file(path, data)
    }

    fn remove(&self, path: &RootedPath) -> std::io::Result<()> {
        if split_path(path.as_string()).is_some() {
            panic!("cannot remove from embedded file system");
        }
        self.fs.remove(path)
    }

    fn chtimes(
        &self,
        path: &RootedPath,
        atime: SystemTime,
        mtime: SystemTime,
    ) -> std::io::Result<()> {
        if split_path(path.as_string()).is_some() {
            panic!("cannot change times on embedded file system");
        }
        self.fs.chtimes(path, atime, mtime)
    }

    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        if let Some(rest) = split_path(path.as_string()) {
            return rest == "libs";
        }
        self.fs.directory_exists(path)
    }

    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        if let Some(rest) = split_path(path.as_string()) {
            let mut result = Entries::default();
            if rest.is_empty() {
                result.directories = vec!["libs".to_string()];
            } else if rest == "libs" {
                result.files = crate::libs_generated::LIB_NAMES
                    .iter()
                    .map(|name| name.to_string())
                    .collect();
            }
            return result;
        }
        self.fs.get_accessible_entries(path)
    }

    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        if let Some(rest) = split_path(path.as_string()) {
            if rest.is_empty() || rest == "libs" {
                return Some(Arc::new(FileInfoImpl {
                    mode: FileMode::DIR,
                    name: rest.to_string(),
                    size: 0,
                }));
            }
            if let Some(lib) = crate::embed_generated::embedded_contents(rest) {
                let lib_name = rest.strip_prefix("libs/").unwrap_or(rest);
                return Some(Arc::new(FileInfoImpl {
                    mode: FileMode::EMPTY,
                    name: lib_name.to_string(),
                    size: lib.len() as i64,
                }));
            }
            return None;
        }
        self.fs.stat(path)
    }

    fn realpath(&self, path: &RootedPath) -> RootedPath {
        if split_path(path.as_string()).is_some() {
            return path.clone();
        }
        self.fs.realpath(path)
    }
}

/// Go: the unexported `fileInfo` struct (`fs.FileInfo` + `fs.DirEntry`).
struct FileInfoImpl {
    mode: FileMode,
    name: String,
    size: i64,
}

impl FileInfo for FileInfoImpl {
    fn name(&self) -> &str {
        &self.name
    }
    fn size(&self) -> i64 {
        self.size
    }
    fn mode(&self) -> FileMode {
        self.mode
    }
    fn mod_time(&self) -> SystemTime {
        // Go: `return time.Time{}` — the zero time.
        SystemTime::UNIX_EPOCH
    }
    fn is_dir(&self) -> bool {
        self.mode.is_dir()
    }
    fn sys(&self) -> Option<&(dyn std::any::Any + Send + Sync)> {
        None
    }
}
