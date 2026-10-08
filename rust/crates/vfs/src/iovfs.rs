// Ported from tsc/internal/vfs/iovfs/iofs.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go's From(fsys fs.FS, caseSensitivity) detects the RealpathFS /
// WritableFS capability interfaces via type assertion and stores the resolved
// closures. In the port those capabilities are default methods on
// sysfs::SubFs (identity realpath / panicking writes), so IoFs simply calls
// through the trait — dynamic dispatch performs the "detection". FsWithSys'
// FSys() accessor is not exposed on the returned Arc<dyn Fs>; callers that
// need the underlying SubFs can keep their own Arc before calling from().

use std::io;
use std::sync::Arc;
use std::time::SystemTime;

use tsc_tspath::{
    CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath, is_url,
    remove_trailing_directory_separator, rooted_path_from_absolute,
};

use crate::internal::Common;
use crate::sysfs::{PrefixSubFs, SubFs};
use crate::vfs::{Entries, Fs, FileInfo};

/// From creates a new FS from a [SubFs].
///
/// For paths like `c:/foo/bar`, fsys will be used as though it's rooted at `/` and the path is `/c:/foo/bar`.
///
/// If the provided [SubFs] implements the RealpathFS capability (its
/// realpath override), it will be used to implement the Realpath method; the
/// same holds for the WritableFS capability methods (write_file, append_file,
/// mkdir_all, remove, chtimes).
///
/// From does not actually handle case-insensitivity; ensure the passed in [SubFs]
/// respects case-insensitive file names if needed. Consider using
/// [crate::vfstest::from_map] for testing.
pub fn from(fsys: Arc<dyn SubFs>, case_sensitivity: CaseSensitivity) -> Arc<dyn Fs> {
    let root_fsys = Arc::clone(&fsys);
    let io_fs = IoFs {
        common: Common {
            root_for: Box::new(move |root: &str| -> Option<Arc<dyn SubFs>> {
                if root == "/" {
                    return Some(Arc::clone(&root_fsys));
                }
                // PORT: Go calls fs.Sub and returns nil when it errors and the
                // root is a URL (tspath.IsUrl); the prefix-based SubFs never
                // fails on valid tspath roots, so the URL check is hoisted.
                if is_url(root) {
                    return None;
                }
                let p = remove_trailing_directory_separator(root);
                Some(Arc::new(PrefixSubFs::new(Arc::clone(&root_fsys), p)))
            }),
            is_reparse_point: None,
        },
        case_sensitivity,
        fsys,
    };
    Arc::new(io_fs)
}

struct IoFs {
    common: Common,
    case_sensitivity: CaseSensitivity,
    fsys: Arc<dyn SubFs>,
}

// PORT: Go's write closures strip the leading "/" via strings.CutPrefix
// before calling into the wrapped fs.FS; `cut_slash` is that helper.
fn cut_slash(path: &str) -> &str {
    path.strip_prefix('/').unwrap_or(path)
}

impl IoFs {
    fn write_file_ensuring_dir(
        &self,
        path: &RootedFilePath,
        content: &str,
        op: fn(&dyn SubFs, &str, &str) -> io::Result<()>,
    ) -> io::Result<()> {
        let path_string = path.as_string();
        let rest = cut_slash(path_string);
        if op(self.fsys.as_ref(), rest, content).is_ok() {
            return Ok(());
        }
        let directory = cut_slash(path.directory().as_string());
        self.fsys.mkdir_all(directory)?;
        op(self.fsys.as_ref(), rest, content)
    }
}

impl Fs for IoFs {
    fn case_sensitivity(&self) -> CaseSensitivity {
        self.case_sensitivity
    }

    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        self.common.directory_exists(path)
    }

    fn file_exists(&self, path: &RootedFilePath) -> bool {
        self.common.file_exists(path)
    }

    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        self.common.get_accessible_entries(path)
    }

    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        self.common.stat(path)
    }

    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        self.common.read_file(path)
    }

    fn remove(&self, path: &RootedPath) -> io::Result<()> {
        self.fsys.remove(cut_slash(path.as_string()))
    }

    fn chtimes(&self, path: &RootedPath, atime: SystemTime, mtime: SystemTime) -> io::Result<()> {
        self.fsys.chtimes(cut_slash(path.as_string()), atime, mtime)
    }

    fn realpath(&self, path: &RootedPath) -> RootedPath {
        let (root, rest) = path.root_and_relative_path();
        let full = format!("{}{}", root.as_string(), rest);
        let had_slash = full.starts_with('/');
        match self.fsys.realpath(cut_slash(&full)) {
            Err(_) => path.clone(),
            Ok(realpath) => {
                if had_slash {
                    rooted_path_from_absolute(&format!("/{realpath}"))
                } else {
                    rooted_path_from_absolute(&realpath)
                }
            }
        }
    }

    fn write_file(&self, path: &RootedFilePath, content: &str) -> io::Result<()> {
        // PORT: perm 0o666 matches iovfs.From's closure calls into WritableFS.
        self.write_file_ensuring_dir(path, content, |fsys, rest, content| {
            fsys.write_file(rest, content)
        })
    }

    fn append_file(&self, path: &RootedFilePath, content: &str) -> io::Result<()> {
        self.write_file_ensuring_dir(path, content, |fsys, rest, content| {
            fsys.append_file(rest, content)
        })
    }
}
