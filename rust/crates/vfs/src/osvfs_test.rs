// Ported from tsc/internal/vfs/osvfs/os_test.go, realpath_test.go, and the
// TestGetAccessibleEntries helper in tsc/internal/vfs/realpath_test.go @
// ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - The mklink Windows branch is not ported (unix-only coverage here; the
//     symlink creation itself uses std::os::unix::fs::symlink).
//   - Go's `t.TempDir`-style directories are created under std::env::temp_dir
//     with a per-process tag and removed at test end.
//   - BenchmarkRealpath (realpath_test.go) is a Go benchmark with no Rust
//     equivalent in this test suite; not ported.

use std::path::Path;

use tsc_tspath::{rooted_directory_path_from_absolute, rooted_file_path_from_absolute};

use crate::osvfs;

fn temp_dir(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tsc-vfs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn setup_symlinks() -> (String, String) {
    let tmp = temp_dir("osvfs-symlinks");
    let target_dir = format!("{tmp}/target");
    let target_file = format!("{tmp}/target/file.txt");
    let link_dir = format!("{tmp}/link");
    let link_file = format!("{tmp}/link/file.txt");

    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(&target_file, "hello").unwrap();
    std::os::unix::fs::symlink(&target_dir, &link_dir).unwrap();

    (target_file, link_file)
}

#[test]
#[cfg(unix)]
fn symlink_realpath() {
    let (target_file, link_file) = setup_symlinks();
    assert_eq!(std::fs::read_to_string(&link_file).unwrap(), "hello");

    let fs = osvfs::fs();
    let target_real_path = fs.realpath(&rooted_file_path_from_absolute(&target_file).as_path());
    let link_real_path = fs.realpath(&rooted_file_path_from_absolute(&link_file).as_path());
    assert_eq!(target_real_path, link_real_path);

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!(
        "tsc-vfs-osvfs-symlinks-{}",
        std::process::id()
    )));
}

#[test]
fn os_fs() {
    let fs = osvfs::fs();

    // ReadFile
    let go_mod = format!("{}/go.mod", tsc_repo::root_path());
    let expected = String::from_utf8(std::fs::read(&go_mod).unwrap()).unwrap();
    let contents = fs.read_file(&rooted_file_path_from_absolute(&go_mod));
    assert_eq!(contents, Some(expected));

    // Realpath
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().into_owned();
        if !home.is_empty() {
            let home_path = rooted_directory_path_from_absolute(&home);
            let realpath = fs.realpath(&home_path.as_path());
            assert_eq!(realpath.as_string(), home_path.as_string());
        }
    }

    // CaseSensitivity
    let case_sensitivity = fs.case_sensitivity();
    #[cfg(target_os = "linux")]
    assert!(case_sensitivity.is_case_sensitive());
    #[cfg(target_os = "windows")]
    assert!(case_sensitivity.is_case_insensitive());
}

#[test]
#[cfg(unix)]
fn get_accessible_entries() {
    let fs = osvfs::fs();

    let temp = temp_dir("osvfs-entries");
    let target = format!("{temp}/target");
    let link = format!("{temp}/link");
    let dir1 = format!("{target}/dir1");
    let dir2 = format!("{target}/dir2");
    std::fs::create_dir_all(&dir1).unwrap();
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::create_dir_all(&link).unwrap();
    std::fs::write(Path::new(&target).join("file1"), "hello").unwrap();
    std::fs::write(Path::new(&target).join("file2"), "world").unwrap();

    std::os::unix::fs::symlink(format!("{target}/file1"), Path::new(&link).join("file1")).unwrap();
    std::os::unix::fs::symlink(format!("{target}/file2"), Path::new(&link).join("file2")).unwrap();
    std::os::unix::fs::symlink(&dir1, Path::new(&link).join("dir1")).unwrap();
    std::os::unix::fs::symlink(&dir2, Path::new(&link).join("dir2")).unwrap();

    // On the target directory, no symlinks should be reported.
    let entries = fs.get_accessible_entries(&rooted_directory_path_from_absolute(&target));
    assert_eq!(entries.directories, ["dir1", "dir2"]);
    assert_eq!(entries.files, ["file1", "file2"]);
    assert!(entries.symlinks.is_some());
    assert_eq!(entries.symlinks.as_ref().unwrap().len(), 0);

    // On the link directory, all symlinks should be reported.
    let entries = fs.get_accessible_entries(&rooted_directory_path_from_absolute(&link));
    assert_eq!(entries.directories, ["dir1", "dir2"]);
    assert_eq!(entries.files, ["file1", "file2"]);
    assert!(entries.symlinks.is_some());
    let symlinks = entries.symlinks.as_ref().unwrap();
    assert_eq!(symlinks.len(), 4);
    for name in ["dir1", "dir2", "file1", "file2"] {
        assert!(symlinks.contains(name), "{name} missing from symlinks");
    }

    let _ = std::fs::remove_dir_all(&temp);
}
