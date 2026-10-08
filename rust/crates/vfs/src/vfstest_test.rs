// Ported from tsc/internal/vfs/vfstest/vfstest_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes (Go tests not carried over):
//   - `fstest.TestFS` consistency checks are Go-stdlib test infrastructure
//     over fs.FS/fs.File plumbing the port does not have; the corresponding
//     assert lines are dropped everywhere they appear.
//   - TestFromMap/"InvalidFile" checks Go's `case default: panic("invalid
//     file type %T")` — unreachable with the port's typed MapFile inputs.
//   - TestStress replaces Go's math/rand shuffle with a per-thread rotation
//     (no external rand dependency; the exercised interleaving is the same
//     set of 8 ops).
//
// Go's `assert.Error(t, err, msg)` only checks err != nil (msg is a failure
// annotation); where the Go test also names the expected message the port
// additionally asserts the substring for clarity.

use std::any::Any;
use std::sync::Arc;

use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath};

use crate::sysfs::SubFs;
use crate::vfs::{Fs, WalkDirControl};
use crate::vfstest::{MapFile, convert_map_fs, from_map, symlink};

fn dir_entries_to_names(entries: &[crate::sysfs::SubDirEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.name.clone()).collect()
}

fn map_with_sys(data: &[u8]) -> MapFile {
    MapFile {
        data: data.to_vec(),
        sys: Some(Arc::new(1234i32) as Arc<dyn Any + Send + Sync>),
        ..Default::default()
    }
}

#[test]
fn insensitive() {
    let contents = b"bar".to_vec();

    let vfs = convert_map_fs(
        [
            ("foo/bar/baz", map_with_sys(&contents)),
            ("foo/bar2/baz2", map_with_sys(&contents)),
            ("foo/bar3/baz3", map_with_sys(&contents)),
        ],
        CaseSensitivity::CaseInsensitive,
        None,
    );

    let sensitive = vfs.read_file("foo/bar/baz").unwrap();
    assert_eq!(sensitive, contents);
    let sensitive_info = vfs.stat("foo/bar/baz").unwrap();
    assert_eq!(sensitive_info.sys().unwrap().downcast_ref::<i32>(), Some(&1234));
    let sensitive_real_path = vfs.realpath("foo/bar/baz").unwrap();
    assert_eq!(sensitive_real_path, "foo/bar/baz");
    let entries = dir_entries_to_names(&vfs.read_dir("foo").unwrap());
    assert_eq!(entries, ["bar", "bar2", "bar3"]);

    let err = vfs.realpath("does/not/exist").unwrap_err();
    assert!(err.to_string().contains("file does not exist"));
    let err = vfs
        .stat("does/not/exist")
        .err()
        .expect("stat should fail");
    assert!(err.to_string().contains("file does not exist"));

    // fstest.TestFS(vfs, "foo/bar/baz") — not ported (stdlib test harness).

    let insensitive = vfs.read_file("Foo/Bar/Baz").unwrap();
    assert_eq!(insensitive, contents);
    let insensitive_info = vfs.stat("Foo/Bar/Baz").unwrap();
    assert_eq!(insensitive_info.sys().unwrap().downcast_ref::<i32>(), Some(&1234));
    let insensitive_real_path = vfs.realpath("Foo/Bar/Baz").unwrap();
    assert_eq!(insensitive_real_path, "foo/bar/baz");
    let entries = dir_entries_to_names(&vfs.read_dir("Foo").unwrap());
    assert_eq!(entries, ["bar", "bar2", "bar3"]);

    let err = vfs.realpath("Does/Not/Exist").unwrap_err();
    assert!(err.to_string().contains("file does not exist"));
    let err = vfs
        .stat("Does/Not/Exist")
        .err()
        .expect("stat should fail");
    assert!(err.to_string().contains("file does not exist"));

    // TODO(port) fstest.TestFS doesn't understand case-insensitive file systems.
}

#[test]
fn insensitive_upper() {
    let contents = b"bar".to_vec();

    let vfs = convert_map_fs(
        [
            ("Foo/Bar/Baz", map_with_sys(&contents)),
            ("Foo/Bar2/Baz2", map_with_sys(&contents)),
            ("Foo/Bar3/Baz3", map_with_sys(&contents)),
        ],
        CaseSensitivity::CaseInsensitive,
        None,
    );

    let sensitive = vfs.read_file("foo/bar/baz").unwrap();
    assert_eq!(sensitive, contents);
    let sensitive_info = vfs.stat("foo/bar/baz").unwrap();
    assert_eq!(sensitive_info.sys().unwrap().downcast_ref::<i32>(), Some(&1234));
    let entries = dir_entries_to_names(&vfs.read_dir("foo").unwrap());
    assert_eq!(entries, ["Bar", "Bar2", "Bar3"]);

    let insensitive = vfs.read_file("Foo/Bar/Baz").unwrap();
    assert_eq!(insensitive, contents);
    let insensitive_info = vfs.stat("Foo/Bar/Baz").unwrap();
    assert_eq!(insensitive_info.sys().unwrap().downcast_ref::<i32>(), Some(&1234));
    let entries = dir_entries_to_names(&vfs.read_dir("Foo").unwrap());
    assert_eq!(entries, ["Bar", "Bar2", "Bar3"]);
}

#[test]
fn sensitive() {
    let contents = b"bar".to_vec();

    let vfs = convert_map_fs(
        [
            ("foo/bar/baz", map_with_sys(&contents)),
            ("foo/bar2/baz2", map_with_sys(&contents)),
            ("foo/bar3/baz3", map_with_sys(&contents)),
        ],
        CaseSensitivity::CaseSensitive,
        None,
    );

    let sensitive = vfs.read_file("foo/bar/baz").unwrap();
    assert_eq!(sensitive, contents);
    let sensitive_info = vfs.stat("foo/bar/baz").unwrap();
    assert_eq!(sensitive_info.sys().unwrap().downcast_ref::<i32>(), Some(&1234));

    let err = vfs.read_file("Foo/Bar/Baz").unwrap_err();
    assert!(err.to_string().contains("file does not exist"));
}

#[test]
#[should_panic(expected = "duplicate path: \"Foo\" and \"foo\" have the same canonical path")]
fn sensitive_duplicate_path() {
    let testfs = [
        ("foo", MapFile::from("bar")),
        ("Foo", MapFile::from("baz")),
    ];
    convert_map_fs(testfs, CaseSensitivity::CaseInsensitive, None);
}

#[test]
fn insensitive_duplicate_path() {
    let testfs = [
        ("foo", MapFile::from("bar")),
        ("Foo", MapFile::from("baz")),
    ];
    convert_map_fs(testfs, CaseSensitivity::CaseSensitive, None);
}

#[test]
fn writable_fs() {
    let fs = from_map(Vec::<(String, MapFile)>::new(), CaseSensitivity::CaseInsensitive);

    fs.write_file(&RootedFilePath::from("/foo/bar/baz"), "hello, world")
        .unwrap();

    let content = fs.read_file(&RootedFilePath::from("/foo/bar/baz"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    fs.write_file(&RootedFilePath::from("/foo/bar/baz"), "goodbye, world")
        .unwrap();

    let content = fs.read_file(&RootedFilePath::from("/foo/bar/baz"));
    assert_eq!(content.as_deref(), Some("goodbye, world"));

    let err = fs
        .write_file(&RootedFilePath::from("/foo/bar/baz/oops"), "goodbye, world")
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("mkdir \"foo/bar/baz\": path exists but is not a directory"),
        "unexpected error: {err}"
    );
}

#[test]
fn writable_fs_delete() {
    let fs = from_map(Vec::<(String, MapFile)>::new(), CaseSensitivity::CaseInsensitive);

    fs.write_file(&RootedFilePath::from("/foo/bar/file.ts"), "remove")
        .unwrap();
    assert!(fs.file_exists(&RootedFilePath::from("/foo/bar/file.ts")));
    fs.remove(&RootedPath::from("/foo/bar/file.ts")).unwrap();
    assert!(!fs.file_exists(&RootedFilePath::from("/foo/bar/file.ts")));

    fs.write_file(&RootedFilePath::from("/foo/bar/test/remove2.ts"), "remove2")
        .unwrap();
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/foo/bar/test")));
    fs.remove(&RootedPath::from("/foo/bar/test")).unwrap();
    assert!(!fs.file_exists(&RootedFilePath::from("/foo/bar/test/remove2.ts")));
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/foo/bar/test")));

    // no errors when removing file/dir that does not exist
    fs.remove(&RootedPath::from("/foo/bar/test")).unwrap();
    fs.remove(&RootedPath::from("/foo/bar/file.ts")).unwrap();

    fs.write_file(&RootedFilePath::from("/foo/barbar"), "remove2").unwrap();
    fs.remove(&RootedPath::from("/foo/bar")).unwrap();
    assert!(fs.file_exists(&RootedFilePath::from("/foo/barbar")));
}

#[test]
fn stress() {
    // PORT: Go runs GOMAXPROCS workers each performing 10_000 randomly
    // shuffled ops; the port rotates the 8 ops by thread index.
    let fs = from_map(Vec::<(String, MapFile)>::new(), CaseSensitivity::CaseInsensitive);

    let thread_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let mut threads = Vec::with_capacity(thread_count);
    for t in 0..thread_count {
        let fs = Arc::clone(&fs);
        threads.push(std::thread::spawn(move || {
            for i in 0..10_000 {
                match (i + t) % 8 {
                    0 => {
                        let _ = fs.write_file(
                            &RootedFilePath::from("/foo/bar/baz.txt"),
                            "hello, world",
                        );
                    }
                    1 => {
                        let _ = fs.read_file(&RootedFilePath::from("/foo/bar/baz.txt"));
                    }
                    2 => {
                        let _ = fs.directory_exists(&RootedDirectoryPath::from("/foo/bar"));
                    }
                    3 => {
                        let _ = fs.file_exists(&RootedFilePath::from("/foo/bar"));
                    }
                    4 => {
                        let _ = fs.file_exists(&RootedFilePath::from("/foo/bar/baz.txt"));
                    }
                    5 => {
                        let _ = fs.get_accessible_entries(&RootedDirectoryPath::from("/foo/bar"));
                    }
                    6 => {
                        let _ = fs.realpath(&RootedPath::from("/foo/bar/baz.txt"));
                    }
                    _ => {
                        let _ = fs.stat(&RootedPath::from("/foo/bar/baz.txt"));
                    }
                }
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
#[should_panic(
    expected = "failed to create intermediate directories for \"foo/oops\": mkdir \"foo\": path exists but is not a directory"
)]
fn parent_dir_file() {
    let testfs = [
        ("foo", MapFile::from("bar")),
        ("foo/oops", MapFile::from("baz")),
    ];
    convert_map_fs(testfs, CaseSensitivity::CaseInsensitive, None);
}

#[test]
fn from_map_posix() {
    let fs = from_map(
        [
            ("/string", MapFile::from("hello, world")),
            ("/bytes", MapFile::from(b"hello, world".to_vec())),
            ("/mapfile", MapFile::from_data(b"hello, world".to_vec())),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    let content = fs.read_file(&RootedFilePath::from("/string"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/bytes"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/mapfile"));
    assert_eq!(content.as_deref(), Some("hello, world"));
}

#[test]
fn from_map_windows() {
    let fs = from_map(
        [
            ("c:/string", MapFile::from("hello, world")),
            ("d:/bytes", MapFile::from(b"hello, world".to_vec())),
            ("e:/mapfile", MapFile::from_data(b"hello, world".to_vec())),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    let content = fs.read_file(&RootedFilePath::from("c:/string"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("d:/bytes"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("e:/mapfile"));
    assert_eq!(content.as_deref(), Some("hello, world"));
}

#[test]
#[should_panic(expected = "mixed posix and windows paths")]
fn from_map_mixed() {
    from_map(
        [
            ("/string", MapFile::from("hello, world")),
            ("c:/bytes", MapFile::from(b"hello, world".to_vec())),
        ],
        CaseSensitivity::CaseInsensitive,
    );
}

#[test]
#[should_panic(expected = "non-rooted path \"string\"")]
fn from_map_non_rooted() {
    from_map([("string", MapFile::from("hello, world"))], CaseSensitivity::CaseInsensitive);
}

#[test]
#[should_panic(expected = "non-normalized path \"/string/\"")]
fn from_map_non_normalized() {
    from_map([("/string/", MapFile::from("hello, world"))], CaseSensitivity::CaseInsensitive);
}

#[test]
#[should_panic(expected = "non-normalized path \"/string/../foo\"")]
fn from_map_non_normalized_2() {
    from_map(
        [("/string/../foo", MapFile::from("hello, world"))],
        CaseSensitivity::CaseInsensitive,
    );
}

#[test]
fn vfstest_map_fs() {
    let fs = from_map(
        [
            ("/foo.ts", "hello, world"),
            ("/dir1/file1.ts", "export const foo = 42;"),
            ("/dir1/file2.ts", "export const foo = 42;"),
            ("/dir2/file1.ts", "export const foo = 42;"),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    // ReadFile
    let content = fs.read_file(&RootedFilePath::from("/foo.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/does/not/exist.ts"));
    assert_eq!(content, None);

    // Realpath
    let realpath = fs.realpath(&RootedPath::from("/foo.ts"));
    assert_eq!(realpath.as_string(), "/foo.ts");

    let realpath = fs.realpath(&RootedPath::from("/Foo.ts"));
    assert_eq!(realpath.as_string(), "/foo.ts");

    let realpath = fs.realpath(&RootedPath::from("/does/not/exist.ts"));
    assert_eq!(realpath.as_string(), "/does/not/exist.ts");

    // CaseSensitivity
    assert!(fs.case_sensitivity().is_case_insensitive());
}

#[test]
fn vfstest_map_fs_windows() {
    let fs = from_map(
        [
            ("c:/foo.ts", "hello, world"),
            ("c:/dir1/file1.ts", "export const foo = 42;"),
            ("c:/dir1/file2.ts", "export const foo = 42;"),
            ("c:/dir2/file1.ts", "export const foo = 42;"),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    // ReadFile
    let content = fs.read_file(&RootedFilePath::from("c:/foo.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("c:/does/not/exist.ts"));
    assert_eq!(content, None);

    // Realpath
    let realpath = fs.realpath(&RootedPath::from("c:/foo.ts"));
    assert_eq!(realpath.as_string(), "c:/foo.ts");

    let realpath = fs.realpath(&RootedPath::from("c:/Foo.ts"));
    assert_eq!(realpath.as_string(), "c:/foo.ts");

    let realpath = fs.realpath(&RootedPath::from("c:/does/not/exist.ts"));
    assert_eq!(realpath.as_string(), "c:/does/not/exist.ts");
}

#[test]
fn bom() {
    let expected = "hello, world";

    for (name, bom, little) in [
        ("BigEndian", [0xFE, 0xFF], false),
        ("LittleEndian", [0xFF, 0xFE], true),
    ] {
        let code_points: Vec<u16> = expected.encode_utf16().collect();

        let mut buf = bom.to_vec();
        for r in code_points {
            if little {
                buf.extend_from_slice(&r.to_le_bytes());
            } else {
                buf.extend_from_slice(&r.to_be_bytes());
            }
        }

        let fs = from_map([("/foo.ts", buf)], CaseSensitivity::CaseSensitive);
        let content = fs.read_file(&RootedFilePath::from("/foo.ts"));
        assert_eq!(content.as_deref(), Some(expected), "{name}");
    }

    // UTF8
    let fs = from_map(
        [( "/foo.ts", b"\xEF\xBB\xBFhello, world".to_vec())],
        CaseSensitivity::CaseSensitive,
    );
    let content = fs.read_file(&RootedFilePath::from("/foo.ts"));
    assert_eq!(content.as_deref(), Some(expected));
}

#[test]
fn symlink_fs() {
    let fs: Arc<dyn Fs> = from_map(
        [
            ("/foo.ts", MapFile::from("hello, world")),
            ("/symlink.ts", symlink("/foo.ts")),
            ("/some/dir/file.ts", MapFile::from("hello, world")),
            ("/some/dirlink", symlink("/some/dir")),
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("this is existing.ts")),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    // ReadFile
    let content = fs.read_file(&RootedFilePath::from("/symlink.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/some/dirlink/file.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/a/existing.ts"));
    assert_eq!(content.as_deref(), Some("this is existing.ts"));

    // Realpath
    let realpath = fs.realpath(&RootedPath::from("/symlink.ts"));
    assert_eq!(realpath.as_string(), "/foo.ts");

    let realpath = fs.realpath(&RootedPath::from("/some/dirlink"));
    assert_eq!(realpath.as_string(), "/some/dir");

    let realpath = fs.realpath(&RootedPath::from("/some/dirlink/file.ts"));
    assert_eq!(realpath.as_string(), "/some/dir/file.ts");

    // FileExists
    assert!(fs.file_exists(&RootedFilePath::from("/symlink.ts")));
    assert!(fs.file_exists(&RootedFilePath::from("/some/dirlink/file.ts")));
    assert!(fs.file_exists(&RootedFilePath::from("/a/existing.ts")));

    // DirectoryExists
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/some/dirlink")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/d")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/c")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/b")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/a")));
}

#[test]
fn writable_fs_symlink() {
    let fs = from_map(
        [
            ("/some/dir/other.ts", MapFile::from("NOTHING")),
            ("/other.ts", symlink("/some/dir/other.ts")),
            ("/some/dirlink", symlink("/some/dir")),
            ("/brokenlink", symlink("/does/not/exist")),
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("hello, world")),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    fs.write_file(&RootedFilePath::from("/some/dirlink/file.ts"), "hello, world")
        .unwrap();

    let content = fs.read_file(&RootedFilePath::from("/some/dirlink/file.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/some/dir/file.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    fs.write_file(&RootedFilePath::from("/some/dirlink/file.ts"), "goodbye, world")
        .unwrap();

    let content = fs.read_file(&RootedFilePath::from("/some/dirlink/file.ts"));
    assert_eq!(content.as_deref(), Some("goodbye, world"));

    fs.write_file(&RootedFilePath::from("/other.ts"), "hello, world")
        .unwrap();

    let content = fs.read_file(&RootedFilePath::from("/other.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let content = fs.read_file(&RootedFilePath::from("/some/dir/other.ts"));
    assert_eq!(content.as_deref(), Some("hello, world"));

    let err = fs
        .write_file(&RootedFilePath::from("/some/dirlink"), "hello, world")
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("write \"some/dirlink\": path exists but is not a regular file"),
        "unexpected error: {err}"
    );

    // Can't write inside a broken dir symlink
    let err = fs
        .write_file(&RootedFilePath::from("/brokenlink/file.ts"), "hello, world")
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("broken symlink \"brokenlink\" -> \"does/not/exist\""),
        "unexpected error: {err}"
    );

    let err = fs
        .write_file(&RootedFilePath::from("/brokenlink/also/wrong/file.ts"), "hello, world")
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("broken symlink \"brokenlink\" -> \"does/not/exist\""),
        "unexpected error: {err}"
    );

    // But we can write to a broken file symlink
    fs.write_file(&RootedFilePath::from("/brokenlink"), "hello, world")
        .unwrap();
    let content = fs.read_file(&RootedFilePath::from("/brokenlink"));
    assert_eq!(content.as_deref(), Some("hello, world"));
    let content = fs.read_file(&RootedFilePath::from("/does/not/exist"));
    assert_eq!(content.as_deref(), Some("hello, world"));
}

#[test]
fn writable_fs_symlink_chain() {
    let fs = from_map(
        [
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("hello, world")),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    fs.write_file(&RootedFilePath::from("/a/foo/bar/new.ts"), "this is new.ts")
        .unwrap();
    let content = fs.read_file(&RootedFilePath::from("/a/foo/bar/new.ts"));
    assert_eq!(content.as_deref(), Some("this is new.ts"));
    let content = fs.read_file(&RootedFilePath::from("/b/foo/bar/new.ts"));
    assert_eq!(content.as_deref(), Some("this is new.ts"));
    let content = fs.read_file(&RootedFilePath::from("/d/foo/bar/new.ts"));
    assert_eq!(content.as_deref(), Some("this is new.ts"));
}

#[test]
fn writable_fs_symlink_chain_not_dir() {
    let fs = from_map(
        [
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d", MapFile::from("hello, world")),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    let err = fs
        .write_file(&RootedFilePath::from("/a/foo/bar/new.ts"), "this is new.ts")
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("mkdir \"d\": path exists but is not a directory"),
        "unexpected error: {err}"
    );
}

#[test]
fn writable_fs_symlink_delete() {
    let fs = from_map(
        [
            ("/some/dir/other.ts", MapFile::from("NOTHING")),
            ("/other.ts", symlink("/some/dir/other.ts")),
            ("/some/dirlink", symlink("/some/dir")),
            ("/brokenlink", symlink("/does/not/exist")),
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("hello, world")),
        ],
        CaseSensitivity::CaseInsensitive,
    );

    fs.remove(&RootedPath::from("/a")).unwrap();
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/a")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/b")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/c")));
    assert!(fs.file_exists(&RootedFilePath::from("/d/existing.ts")));

    // symlinks should still exist even if underlying file/dir is deleted
    fs.remove(&RootedPath::from("/d")).unwrap();
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/b")));
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/c")));
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/d")));
    assert!(!fs.file_exists(&RootedFilePath::from("/d/again.ts")));
    fs.write_file(&RootedFilePath::from("/d/again.ts"), "d exists again")
        .unwrap();
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/b")));
    assert!(fs.directory_exists(&RootedDirectoryPath::from("/c")));
    let content = fs.read_file(&RootedFilePath::from("/b/again.ts"));
    assert_eq!(content.as_deref(), Some("d exists again"));

    assert!(!fs.file_exists(&RootedFilePath::from("/brokenlink")));
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/brokenlink")));
    fs.remove(&RootedPath::from("/does/not/exist")).unwrap(); // should do nothing
    assert!(!fs.file_exists(&RootedFilePath::from("/brokenlink")));
    assert!(!fs.directory_exists(&RootedDirectoryPath::from("/brokenlink")));
    fs.write_file(&RootedFilePath::from("/does/not/exist"), "hello, world")
        .unwrap();
    assert!(fs.file_exists(&RootedFilePath::from("/brokenlink")));

    // The walk controls are unused in this file but re-exported for parity.
    let _ = WalkDirControl::Continue;
}
