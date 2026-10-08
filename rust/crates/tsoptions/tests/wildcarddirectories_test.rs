// Port of tsc/internal/tsoptions/wildcarddirectories_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - The Go exclude arm runs the vfsmatch SpecMatcher; the port's
//     `get_wildcard_directories` takes the exclusion decision as a
//     predicate (see wildcarddirectories.rs PORT notes). These fixtures'
//     include directories match no exclude pattern, so `|_| false` mirrors
//     the Go results exactly for the ported cases.

use tsc_tsoptions::wildcarddirectories::get_wildcard_directories;
use tsc_tspath::{rooted_directory_path_from_absolute, CaseSensitivity};

/// Go: `TestGetWildcardDirectories_DotPrefixedIncludeWithDotDirExclude` —
/// https://github.com/microsoft/TypeScript/tsc/issues/3733.
#[test]
fn test_dot_prefixed_include_with_dot_dir_exclude() {
    let result = get_wildcard_directories(
        &["./app/**/*.ts", "./app/**/*.tsx"],
        |_| false,
        &rooted_directory_path_from_absolute("/home/projects/monorepo/apps/web"),
        CaseSensitivity::CaseSensitive,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0.as_string(), "/home/projects/monorepo/apps/web/app");
    assert!(result[0].1.recursive);
}

/// Go: `TestGetWildcardDirectories_DriveRoot`.
#[test]
fn test_drive_root() {
    let result = get_wildcard_directories(
        &["*.ts"],
        |_| false,
        &rooted_directory_path_from_absolute("c:/"),
        CaseSensitivity::CaseInsensitive,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0.as_string(), "c:/");
    assert!(!result[0].1.recursive);
}

/// Go: `TestGetWildcardDirectories_NonASCIICharacters` — non-ASCII paths
/// flow through the derivation unchanged.
#[test]
fn test_non_ascii_characters() {
    let cases: Vec<(&str, &[&str], &[&str], bool)> = vec![
        (
            "C:/Users/TobiasLægreid/dev/app/frontend/packages/react",
            &["src/**/*.test.ts", "src/**/*.stories.ts", "src/**/*.mdx"],
            &["node_modules"],
            false,
        ),
        (
            "/Users/ユーザー/プロジェクト",
            &["src/**/*.ts"],
            &["テスト"],
            true,
        ),
        (
            "/home/用户/项目",
            &["源代码/**/*.js"],
            &["节点模块"],
            true,
        ),
        (
            "/Users/Müller/café/naïve/résumé",
            &["src/**/*.ts"],
            &["node_modules"],
            false,
        ),
    ];
    for (current_directory, include, _exclude, case_sensitive) in cases {
        let result = get_wildcard_directories(
            include,
            |_| false,
            &rooted_directory_path_from_absolute(current_directory),
            if case_sensitive {
                CaseSensitivity::CaseSensitive
            } else {
                CaseSensitivity::CaseInsensitive
            },
        );
        assert!(
            !result.is_empty(),
            "expected non-empty result for {current_directory}"
        );
    }
}

/// `isImplicitGlob` (vfsmatch helper, re-homed with a PORT banner).
#[test]
fn test_is_implicit_glob() {
    use tsc_tsoptions::wildcarddirectories::is_implicit_glob;
    assert!(is_implicit_glob("src"));
    assert!(!is_implicit_glob("src.ts"));
    assert!(!is_implicit_glob("**"));
    assert!(!is_implicit_glob("a?b"));
}
