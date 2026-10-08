// Ported from tsc/internal/vfs/walkdir_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::io;
use std::sync::{Arc, Mutex};

use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedPath};

use crate::sysfs::FileMode;
use crate::vfs::{DirEntry, Entries, Fs, WalkDirControl};
use crate::walkdir::walk_dir;
use crate::{vfstest, wrapvfs};

// Go TestWalkDir.
#[test]
fn walk_dir_traverses_and_reports_symlinks() {
    let base: Arc<dyn Fs> = vfstest::from_map(
        [
            ("/root/a.ts", ""),
            ("/root/dir/b.ts", ""),
            ("/root/link/hidden.ts", ""),
            ("/target/hidden.ts", ""),
        ],
        CaseSensitivity::CaseSensitive,
    );

    let base_for_entries = Arc::clone(&base);
    let file_system: Arc<dyn Fs> = wrapvfs::wrap(
        Arc::clone(&base),
        wrapvfs::Replacements {
            get_accessible_entries: Some(Box::new(
                move |path: &RootedDirectoryPath| -> Entries {
                    let mut entries = base_for_entries.get_accessible_entries(path);
                    entries.symlinks = None;
                    if path.as_string() == "/root" {
                        entries.files.push("C:/foreign.ts".to_string());
                    }
                    entries
                },
            )),
            realpath: Some(Box::new(|path: &RootedPath| -> RootedPath {
                match path.as_string() {
                    "/root/link" => RootedPath::from("/target"),
                    "/root/link/hidden.ts" => RootedPath::from("/target/hidden.ts"),
                    _ => path.clone(),
                }
            })),
            ..Default::default()
        },
    );

    let mut paths: Vec<String> = Vec::new();
    let mut modes: Vec<FileMode> = Vec::new();
    let result = walk_dir(file_system.as_ref(), &RootedPath::from("/root"),
        |path: &RootedPath,
         entry: Option<&dyn DirEntry>,
         err: Option<&io::Error>|
         -> Result<WalkDirControl, io::Error> {
            assert!(err.is_none());
            let entry = entry.expect("entry");
            paths.push(path.as_string().to_string());
            modes.push(entry.type_bits());
            if entry.type_bits() & FileMode::SYMLINK != FileMode::EMPTY {
                let info = entry.info().unwrap();
                assert_eq!(info.mode(), FileMode::SYMLINK);
                assert!(!info.is_dir());
            }
            Ok(WalkDirControl::Continue)
        },
    );
    assert!(result.is_ok());
    assert_eq!(
        paths,
        ["/root", "/root/a.ts", "/root/dir", "/root/dir/b.ts", "/root/link"]
    );
    assert_eq!(
        modes,
        [FileMode::DIR, FileMode::EMPTY, FileMode::DIR, FileMode::EMPTY, FileMode::SYMLINK]
    );
}

#[test]
fn walk_dir_does_not_follow_root_symlink() {
    let base: Arc<dyn Fs> = vfstest::from_map(
        [("/root/link/hidden.ts", ""), ("/target/hidden.ts", "")],
        CaseSensitivity::CaseSensitive,
    );

    let base_for_entries = Arc::clone(&base);
    let file_system: Arc<dyn Fs> = wrapvfs::wrap(
        Arc::clone(&base),
        wrapvfs::Replacements {
            get_accessible_entries: Some(Box::new(
                move |path: &RootedDirectoryPath| -> Entries {
                    let mut entries = base_for_entries.get_accessible_entries(path);
                    entries.symlinks = None;
                    entries
                },
            )),
            realpath: Some(Box::new(|path: &RootedPath| -> RootedPath {
                if path.as_string() == "/root/link" {
                    RootedPath::from("/target")
                } else {
                    path.clone()
                }
            })),
            ..Default::default()
        },
    );

    let mut paths: Vec<String> = Vec::new();
    let result = walk_dir(file_system.as_ref(), &RootedPath::from("/root/link"),
        |path: &RootedPath,
         entry: Option<&dyn DirEntry>,
         err: Option<&io::Error>|
         -> Result<WalkDirControl, io::Error> {
            assert!(err.is_none());
            paths.push(path.as_string().to_string());
            assert_eq!(entry.expect("entry").type_bits(), FileMode::SYMLINK);
            Ok(WalkDirControl::Continue)
        },
    );
    assert!(result.is_ok());
    assert_eq!(paths, ["/root/link"]);
}

#[test]
fn walk_dir_reports_root_file_symlink() {
    let base: Arc<dyn Fs> =
        vfstest::from_map([("/target/file.ts", "")], CaseSensitivity::CaseSensitive);

    let base_for_stat = Arc::clone(&base);
    let file_system: Arc<dyn Fs> = wrapvfs::wrap(
        Arc::clone(&base),
        wrapvfs::Replacements {
            stat: Some(Box::new(move |path: &RootedPath| {
                if path.as_string() == "/root/link.ts" {
                    base_for_stat.stat(&RootedPath::from("/target/file.ts"))
                } else {
                    base_for_stat.stat(path)
                }
            })),
            realpath: Some(Box::new(|path: &RootedPath| -> RootedPath {
                if path.as_string() == "/root/link.ts" {
                    RootedPath::from("/target/file.ts")
                } else {
                    path.clone()
                }
            })),
            ..Default::default()
        },
    );

    let result = walk_dir(file_system.as_ref(), &RootedPath::from("/root/link.ts"),
        |path: &RootedPath,
         entry: Option<&dyn DirEntry>,
         err: Option<&io::Error>|
         -> Result<WalkDirControl, io::Error> {
            assert!(err.is_none());
            assert_eq!(path.as_string(), "/root/link.ts");
            let entry = entry.expect("entry");
            assert_eq!(entry.name(), "link.ts");
            assert_eq!(entry.type_bits(), FileMode::SYMLINK);
            Ok(WalkDirControl::Continue)
        },
    );
    assert!(result.is_ok());
}

#[test]
fn walk_dir_skip_dir() {
    let file_system: Arc<dyn Fs> = vfstest::from_map(
        [("/root/a/hidden.ts", ""), ("/root/b.ts", "")],
        CaseSensitivity::CaseSensitive,
    );

    let mut paths: Vec<String> = Vec::new();
    let result = walk_dir(file_system.as_ref(), &RootedPath::from("/root"),
        |path: &RootedPath,
         _entry: Option<&dyn DirEntry>,
         err: Option<&io::Error>|
         -> Result<WalkDirControl, io::Error> {
            assert!(err.is_none());
            paths.push(path.as_string().to_string());
            if path.as_string() == "/root/a" {
                return Ok(WalkDirControl::SkipDir);
            }
            Ok(WalkDirControl::Continue)
        },
    );
    assert!(result.is_ok());
    assert_eq!(paths, ["/root", "/root/a", "/root/b.ts"]);
}

#[test]
fn walk_dir_skip_all() {
    let file_system: Arc<dyn Fs> = vfstest::from_map(
        [("/root/a.ts", ""), ("/root/b.ts", "")],
        CaseSensitivity::CaseSensitive,
    );

    let mut paths: Vec<String> = Vec::new();
    let result = walk_dir(file_system.as_ref(), &RootedPath::from("/root"),
        |path: &RootedPath,
         _entry: Option<&dyn DirEntry>,
         err: Option<&io::Error>|
         -> Result<WalkDirControl, io::Error> {
            assert!(err.is_none());
            paths.push(path.as_string().to_string());
            if path.as_string() == "/root/a.ts" {
                return Ok(WalkDirControl::SkipAll);
            }
            Ok(WalkDirControl::Continue)
        },
    );
    assert!(result.is_ok());
    assert_eq!(paths, ["/root", "/root/a.ts"]);
}

#[test]
fn walk_dir_consumes_skip_dir_for_root_file() {
    let file_system: Arc<dyn Fs> =
        vfstest::from_map([("/root.ts", "")], CaseSensitivity::CaseSensitive);
    let result = walk_dir(file_system.as_ref(), &RootedPath::from("/root.ts"),
        |_path: &RootedPath,
         _entry: Option<&dyn DirEntry>,
         err: Option<&io::Error>|
         -> Result<WalkDirControl, io::Error> {
            assert!(err.is_none());
            Ok(WalkDirControl::SkipDir)
        },
    );
    assert!(result.is_ok());
}

#[test]
fn walk_dir_consumes_skip_for_missing_root() {
    let file_system: Arc<dyn Fs> =
        vfstest::from_map(Vec::<(&str, &str)>::new(), CaseSensitivity::CaseSensitive);
    for sentinel in [WalkDirControl::SkipDir, WalkDirControl::SkipAll] {
        let result = walk_dir(file_system.as_ref(), &RootedPath::from("/missing"),
            |_path: &RootedPath,
             entry: Option<&dyn DirEntry>,
             err: Option<&io::Error>|
             -> Result<WalkDirControl, io::Error> {
                assert!(entry.is_none());
                let err = err.expect("err must be ErrNotExist");
                assert_eq!(err.kind(), io::ErrorKind::NotFound);
                Ok(sentinel)
            },
        );
        assert!(result.is_ok());
    }
}

#[test]
fn walk_dir_uses_symlink_metadata_without_realpath_calls() {
    let base: Arc<dyn Fs> =
        vfstest::from_map([("/root/dir/file.ts", "")], CaseSensitivity::CaseSensitive);

    let realpath_calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_closure = Arc::clone(&realpath_calls);
    let file_system: Arc<dyn Fs> = wrapvfs::wrap(
        Arc::clone(&base),
        wrapvfs::Replacements {
            realpath: Some(Box::new(move |path: &RootedPath| -> RootedPath {
                calls_for_closure
                    .lock()
                    .unwrap()
                    .push(path.as_string().to_string());
                path.clone()
            })),
            ..Default::default()
        },
    );

    let result = walk_dir(file_system.as_ref(), &RootedPath::from("/root"),
        |_path: &RootedPath,
         _entry: Option<&dyn DirEntry>,
         err: Option<&io::Error>|
         -> Result<WalkDirControl, io::Error> {
            match err {
                // PORT: the Go walkFn returns the incoming err; None is Continue.
                None => Ok(WalkDirControl::Continue),
                Some(err) => Err(io::Error::new(err.kind(), err.to_string())),
            }
        },
    );
    assert!(result.is_ok());
    let calls = realpath_calls.lock().unwrap();
    assert!(!calls.iter().any(|call| call == "/root/dir"));
}
