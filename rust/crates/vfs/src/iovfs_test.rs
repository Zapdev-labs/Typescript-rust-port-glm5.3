// Ported from tsc/internal/vfs/iovfs/iofs_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::sync::Arc;

use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath, rooted_directory_path_from_absolute};

use crate::iovfs;
use crate::sysfs::SubFs;
use crate::vfstest::{MapFile, convert_map_fs};

#[test]
fn io_fs() {
    // PORT: the Go test builds an fstest.MapFS directly; the port's
    // equivalent map-filesystem is vfstest::convert_map_fs.
    let testfs: Arc<dyn SubFs> = convert_map_fs(
        [
            ("foo.ts", MapFile::from("hello, world")),
            ("dir1/file1.ts", MapFile::from("export const foo = 42;")),
            ("dir1/file2.ts", MapFile::from("export const foo = 42;")),
            ("dir2/file1.ts", MapFile::from("export const foo = 42;")),
        ],
        CaseSensitivity::CaseSensitive,
        None,
    );

    let fs = iovfs::from(testfs, CaseSensitivity::CaseSensitive);

    // ReadFile
    let content = fs.read_file(&RootedFilePath::from("/foo.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/does/not/exist.ts"));
    assert_eq!(content, None);

    // FileExists
    assert!(fs.file_exists(&RootedFilePath::from("/foo.ts")));
    assert!(!fs.file_exists(&RootedFilePath::from("/bar")));

    // DirectoryExists
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/dir1")));
    assert!(fs.directory_exists(&rooted_directory_path_from_absolute("/dir1/")));
    assert!(fs.directory_exists(&rooted_directory_path_from_absolute("/dir1/./")));
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/bar")));

    // GetAccessibleEntries
    let entries = fs.get_accessible_entries(&RootedDirectoryPath::from("/"));
    assert_eq!(entries.directories, ["dir1", "dir2"]);
    assert_eq!(entries.files, ["foo.ts"]);

    // Realpath
    let realpath = fs.realpath(&RootedPath::from("/foo.ts"));
    assert_eq!(realpath.as_string(), "/foo.ts");

    // CaseSensitivity
    assert!(fs.case_sensitivity().is_case_sensitive());
}
