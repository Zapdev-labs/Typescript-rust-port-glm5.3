// Ported from tsc/internal/bundled/bundled_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::path::Path;

use tsc_bundled::{wrap_fs, LibNames, EMBEDDED};
use tsc_vfs::osvfs;

#[test]
fn testing_lib_path() {
    let p = tsc_bundled::testing_lib_path();

    assert!(Path::new(&p).is_dir(), "TestingLibPath {p} must exist");

    let libdts = format!("{p}/lib.d.ts");

    assert!(
        Path::new(&libdts).is_file(),
        "lib.d.ts must exist under TestingLibPath"
    );
}

#[test]
fn embedded_libs() {
    if !EMBEDDED {
        // Go skips when the noembed build tag is set; the port always embeds.
        return;
    }

    let fs = wrap_fs(osvfs::fs());

    let mut files = fs.get_accessible_entries(&tsc_bundled::lib_path()).files;
    files.sort();
    assert_eq!(files, LibNames.to_vec());
}
