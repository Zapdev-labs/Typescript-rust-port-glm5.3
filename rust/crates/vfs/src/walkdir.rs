// Ported from tsc/internal/vfs/walkdir.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::collections::HashSet;
use std::io;

use tsc_tspath::{PathKey, RootedPath, rooted_directory_path_from_path};

use crate::sysfs::FileMode;
use crate::vfs::{DirEntry, Fs, FileInfo, WalkDirControl, err_not_exist, file_info_to_dir_entry};

/// WalkDir calls walkFn for root and each accessible descendant in lexical order.
/// Symbolic links are reported but not followed. Directory read failures are
/// indistinguishable from empty directories because [Fs::get_accessible_entries]
/// does not return errors. The FileInfo returned by DirEntry::info for a symbolic
/// link contains only its name and mode because [Fs] does not provide lstat.
/// Using DirEntry::info() requires the FS to implement stat().
///
/// PORT: Go's walkFn error return is split into `Result<WalkDirControl,
/// io::Error>` (see vfs::WalkDirFunc); WalkDir itself still returns
/// io::Result<()> like Go's `error` return, with SkipDir/SkipAll normalized
/// to Ok exactly where Go's normalizeWalkDirError does.
pub fn walk_dir<F>(file_system: &dyn Fs, root: &RootedPath, mut walk_fn: F) -> io::Result<()>
where
    F: FnMut(&RootedPath, Option<&dyn DirEntry>, Option<&io::Error>) -> Result<WalkDirControl, io::Error>,
{
    let root_path = root.clone();
    let root_info = file_system.stat(&root_path);
    let Some(root_info) = root_info else {
        let not_exist = err_not_exist();
        return match walk_fn(&root_path, None, Some(&not_exist)) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
    };

    let mut ctx = WalkCtx {
        fs: file_system,
        case_sensitivity: file_system.case_sensitivity(),
        root_prefix: root.root_and_relative_path().0,
        visited: HashSet::new(),
    };

    let root_realpath = file_system.realpath(&root_path);
    let mut root_entry = file_info_to_dir_entry(root_info);
    // PORT: Go compares `rootPath != rootPrefix.AsPath()`; both types are
    // normalized path strings so as_string comparison is exact.
    if root_path.as_string() != ctx.root_prefix.as_string() {
        let parent = root_path.directory();
        let parent_realpath = file_system.realpath(&parent.as_path());
        let expected_realpath = rooted_directory_path_from_path(parent_realpath)
            .resolve_directory(root_path.base_name())
            .as_path();
        if !ctx.equivalent(&root_realpath, &expected_realpath) {
            root_entry = Box::new(WalkDirEntry {
                fs: file_system,
                path: root_path.clone(),
                name: root_path.base_name().to_string(),
                mode: FileMode::SYMLINK,
            });
        }
    }
    normalize_walk_dir_error(ctx.visit(&root_path, root_entry.as_ref(), &root_realpath, &mut walk_fn))
}

/// normalizeWalkDirError maps the internal abort kinds to the Go behavior:
/// SkipDir / SkipAll (and success) become Ok, other errors propagate.
fn normalize_walk_dir_error(err: Result<(), WalkAbort>) -> io::Result<()> {
    match err {
        Ok(()) => Ok(()),
        Err(WalkAbort::SkipDir) | Err(WalkAbort::SkipAll) => Ok(()),
        Err(WalkAbort::Error(e)) => Err(e),
    }
}

// PORT: Go's recursive `visit` closure returns error, with fs.SkipDir /
// fs.SkipAll traveling through it alongside real errors; the enum keeps the
// three channels distinguishable at each consumption point.
enum WalkAbort {
    SkipDir,
    SkipAll,
    Error(io::Error),
}

struct WalkCtx<'a> {
    fs: &'a dyn Fs,
    case_sensitivity: tsc_tspath::CaseSensitivity,
    root_prefix: tsc_tspath::RootedDirectoryPath,
    visited: HashSet<PathKey>,
}

impl WalkCtx<'_> {
    fn same_root(&self, path: &RootedPath) -> bool {
        let (path_root, _) = path.root_and_relative_path();
        self.case_sensitivity
            .compare_paths(&path_root.as_path(), &self.root_prefix.as_path())
            == 0
    }

    fn equivalent(&self, left: &RootedPath, right: &RootedPath) -> bool {
        self.case_sensitivity.compare_paths(left, right) == 0
    }

    fn canonicalize(&self, path: &RootedPath) -> PathKey {
        self.case_sensitivity.path_key(path)
    }

    fn visit<F>(
        &mut self,
        path: &RootedPath,
        entry: &dyn DirEntry,
        realpath: &RootedPath,
        walk_fn: &mut F,
    ) -> Result<(), WalkAbort>
    where
        F: FnMut(&RootedPath, Option<&dyn DirEntry>, Option<&io::Error>) -> Result<WalkDirControl, io::Error>,
    {
        if entry.is_dir() {
            let canonical_realpath = self.canonicalize(realpath);
            if !self.visited.insert(canonical_realpath) {
                return Ok(());
            }
        }

        match walk_fn(path, Some(entry), None) {
            Ok(WalkDirControl::Continue) => {}
            Ok(WalkDirControl::SkipDir) if entry.is_dir() => return Ok(()),
            Ok(WalkDirControl::SkipDir) => return Err(WalkAbort::SkipDir),
            Ok(WalkDirControl::SkipAll) => return Err(WalkAbort::SkipAll),
            Err(e) => return Err(WalkAbort::Error(e)),
        }
        if !entry.is_dir() {
            return Ok(());
        }

        let directory = rooted_directory_path_from_path(path.clone());
        let entries = self.fs.get_accessible_entries(&directory);
        let directories: HashSet<&String> = entries.directories.iter().collect();
        let mut names = entries.directories.clone();
        names.extend(entries.files.iter().cloned());
        names.sort();
        for name in names {
            let mut mode = FileMode::EMPTY;
            if directories.contains(&name) {
                mode = FileMode::DIR;
            }
            let child_path = if mode.is_dir() {
                directory.resolve_directory(&name).as_path()
            } else {
                directory.resolve_file(&name).as_path()
            };
            if !self.same_root(&child_path) {
                continue;
            }

            let mut child_realpath = RootedPath::default();
            let is_symlink;
            if let Some(symlinks) = &entries.symlinks {
                is_symlink = symlinks.contains(&name);
                if !is_symlink && mode.is_dir() {
                    child_realpath = rooted_directory_path_from_path(realpath.clone())
                        .resolve_directory(&name)
                        .as_path();
                }
            } else {
                child_realpath = self.fs.realpath(&child_path);
                let real_directory = rooted_directory_path_from_path(realpath.clone());
                let mut expected_realpath = real_directory.resolve_file(&name).as_path();
                if mode.is_dir() {
                    expected_realpath = real_directory.resolve_directory(&name).as_path();
                }
                is_symlink = !self.equivalent(&child_realpath, &expected_realpath);
            }
            if is_symlink {
                mode = FileMode::SYMLINK;
            }
            let child_entry = WalkDirEntry {
                fs: self.fs,
                path: child_path.clone(),
                name: name.clone(),
                mode,
            };
            if !mode.is_dir() {
                let empty = RootedPath::default();
                match self.visit(&child_path, &child_entry, &empty, walk_fn) {
                    Err(WalkAbort::SkipDir) => return Ok(()),
                    Err(e) => return Err(e),
                    Ok(()) => continue,
                }
            }

            if child_realpath.as_string().is_empty() {
                child_realpath = self.fs.realpath(&child_path);
            }
            match self.visit(&child_path, &child_entry, &child_realpath, walk_fn) {
                Err(WalkAbort::SkipDir) => return Ok(()),
                Err(e) => return Err(e),
                Ok(()) => {}
            }
        }
        Ok(())
    }
}

struct WalkDirEntry<'a> {
    fs: &'a dyn Fs,
    path: RootedPath,
    name: String,
    mode: FileMode,
}

impl DirEntry for WalkDirEntry<'_> {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_dir(&self) -> bool {
        self.mode.is_dir()
    }

    fn type_bits(&self) -> FileMode {
        self.mode.type_bits()
    }

    fn info(&self) -> io::Result<std::sync::Arc<dyn FileInfo>> {
        if self.mode & FileMode::SYMLINK != FileMode::EMPTY {
            return Ok(std::sync::Arc::new(WalkDirFileInfo {
                name: self.name.clone(),
                mode: self.mode,
            }));
        }
        match self.fs.stat(&self.path) {
            Some(info) => Ok(info),
            None => Err(err_not_exist()),
        }
    }
}

struct WalkDirFileInfo {
    name: String,
    mode: FileMode,
}

impl FileInfo for WalkDirFileInfo {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> i64 {
        0
    }

    fn mode(&self) -> FileMode {
        self.mode
    }

    fn mod_time(&self) -> std::time::SystemTime {
        // PORT: Go returns the zero time.Time{}.
        std::time::SystemTime::UNIX_EPOCH
    }

    fn is_dir(&self) -> bool {
        self.mode.is_dir()
    }

    fn sys(&self) -> Option<&(dyn std::any::Any + Send + Sync)> {
        None
    }
}
