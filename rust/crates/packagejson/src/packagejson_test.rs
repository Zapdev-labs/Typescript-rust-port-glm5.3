// Ported from tsc/internal/packagejson/packagejson_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Test parity:
//   TestPackageDirectory
//     → package_directory
//   TestPackageDirectoryCacheIdentityPreservesPresentation
//     → package_directory_cache_identity_preserves_presentation
//   TestForEachAncestorDirectoryStoppingAtGlobalCache
//     → for_each_ancestor_directory_stopping_at_global_cache
//   TestParse (duplicate names / content mapper / invalid typescript field)
//     → parse_duplicate_names / parse_content_mapper / parse_invalid_typescript_field_is_ignored
//   BenchmarkPackageJSON/UnmarshalJSON + UnmarshalJSONV2 (package.json,
//     date-fns.json) → covered on the same fixtures by `parse_file_fixtures`
//     below (both Go cases exercise the identical `json.Unmarshal` code path;
//     the workspace has no Rust bench harness yet).
//   BenchmarkPackageJSON/ParseJSONText (package.json, date-fns.json) —
//     PORT: deferred — requires tsc-parser (M3 wave).

use std::sync::Arc;

use tsc_tspath::{
    CaseSensitivity, rooted_directory_path_from_absolute, rooted_directory_path_from_normalized,
};

use crate::{
    ContentMapperFields, Fields, HeaderFields, InfoCacheEntry, PackageJson, expected_of,
    new_info_cache, new_package_directory, parse,
};

// Go: TestPackageDirectory.
#[test]
fn package_directory() {
    let directory = new_package_directory(
        rooted_directory_path_from_absolute("/package/"),
        CaseSensitivity::CaseSensitive,
    );

    assert_eq!(directory.to_string(), "/package");
    assert_eq!(directory.as_directory_path().as_string(), "/package");
    assert_eq!(directory.path_key().as_string(), "/package");
    assert_eq!(directory.parent().to_string(), "/");
    assert_eq!(directory.parent().path_key().as_string(), "/");
}

// Go: TestPackageDirectoryCacheIdentityPreservesPresentation.
#[test]
fn package_directory_cache_identity_preserves_presentation() {
    let cache = new_info_cache(CaseSensitivity::CaseInsensitive);
    let upper = cache.package_directory(rooted_directory_path_from_normalized("/Repo/Package"));
    let lower = cache.package_directory(rooted_directory_path_from_normalized("/repo/package"));
    assert_eq!(upper.path_key(), lower.path_key());

    let entry = Arc::new(InfoCacheEntry {
        package_directory: upper.clone(),
        directory_exists: true,
        contents: Some(Arc::new(PackageJson::default())),
    });
    // First-writer-wins: Set returns the stored entry (pointer-identical here).
    let stored = cache.set(&upper, entry.clone());
    assert!(Arc::ptr_eq(&stored, &entry));
    let got = cache.get(&lower).expect("entry found under the lower-case key");
    assert!(Arc::ptr_eq(&got, &entry));

    let corrected = entry.clone().with_package_directory(lower.clone());
    assert_eq!(
        corrected.package_directory.as_directory_path(),
        lower.as_directory_path()
    );
    assert_eq!(corrected.package_directory.path_key(), upper.path_key());
    assert_eq!(
        entry.package_directory.as_directory_path(),
        upper.as_directory_path()
    );
}

// Go: TestForEachAncestorDirectoryStoppingAtGlobalCache.
#[test]
fn for_each_ancestor_directory_stopping_at_global_cache() {
    let cache = new_info_cache(CaseSensitivity::CaseInsensitive);
    let start = cache.package_directory(rooted_directory_path_from_normalized("/Repo/Project/src"));
    let mut names = Vec::new();
    let mut keys = Vec::new();
    let _ = crate::for_each_ancestor_directory_stopping_at_global_cache(
        &rooted_directory_path_from_normalized("/Repo"),
        start,
        |directory| {
            names.push(directory.to_string());
            keys.push(directory.path_key().as_string().to_string());
            ((), false)
        },
    );
    assert_eq!(names, ["/Repo/Project/src", "/Repo/Project", "/Repo"]);
    assert_eq!(keys, ["/repo/project/src", "/repo/project", "/repo"]);
}

// Go: TestParse, case "duplicate names".
#[test]
fn parse_duplicate_names() {
    let content = r#"{
        "name": "test-package",
        "name": "test-package",
        "version": "1.0.0"
    }"#;

    let got = parse(content.as_bytes()).expect("parse");
    let want = Fields {
        header_fields: HeaderFields {
            name: expected_of("test-package".to_string()),
            version: expected_of("1.0.0".to_string()),
            ..HeaderFields::default()
        },
        ..Fields::default()
    };
    assert_eq!(got, want);
}

// Go: TestParse, case "content mapper".
#[test]
fn parse_content_mapper() {
    let content = r#"{
        "name": "test-package",
        "typescript": {
            "contentMapper": { "exec": ["mapper"], "dynamicConfig": true }
        }
    }"#;

    let got = parse(content.as_bytes()).expect("parse");
    let want = Fields {
        header_fields: HeaderFields {
            name: expected_of("test-package".to_string()),
            ..HeaderFields::default()
        },
        content_mapper: expected_of(ContentMapperFields {
            exec: expected_of(vec!["mapper".to_string()]),
            dynamic_config: expected_of(true),
            ..ContentMapperFields::default()
        }),
        ..Fields::default()
    };
    assert_eq!(got, want);
}

// Go: TestParse, case "invalid typescript field is ignored".
#[test]
fn parse_invalid_typescript_field_is_ignored() {
    let content = r#"{ "name": "test-package", "typescript": "invalid" }"#;

    let got = parse(content.as_bytes()).expect("parse");
    let want = Fields {
        header_fields: HeaderFields {
            name: expected_of("test-package".to_string()),
            ..HeaderFields::default()
        },
        ..Fields::default()
    };
    assert_eq!(got, want);
}

// Replaces the non-parser halves of BenchmarkPackageJSON: UnmarshalJSON and
// UnmarshalJSONV2 both benchmark `json.Unmarshal(content, &Fields{})` over
// filefixture.FromFile(...) inputs. The workspace has no bench harness yet,
// so the same code path runs once over the same fixtures here.
//
// BenchmarkPackageJSON/ParseJSONText (package.json, date-fns.json) —
// PORT: deferred — requires tsc-parser (M3 wave)
// (parser.ParseSourceFile with ast.SourceFileParseOptions).
#[test]
fn parse_file_fixtures() {
    // filefixture.FromFile("package.json", repo.RootPath()/package.json) with
    // SkipIfNotExist: the fixture is absent from this checkout, so this leg
    // is skipped the same way the Go benchmark skips it.
    let package_json = std::path::Path::new(tsc_repo::root_path()).join("package.json");
    if let Ok(content) = std::fs::read(&package_json) {
        parse(&content).expect("unmarshal package.json fixture");
    }

    // filefixture.FromFile("date-fns.json",
    // repo.TestDataPath()/fixtures/packagejson/date-fns.json).
    let date_fns = std::path::Path::new(tsc_repo::test_data_path())
        .join("fixtures")
        .join("packagejson")
        .join("date-fns.json");
    let content = std::fs::read(&date_fns).expect("date-fns.json fixture");
    let got: Fields = tsc_json::unmarshal(&content, &[]).expect("unmarshal date-fns.json");
    assert!(got.header_fields.name.is_valid());
}

// ---------------------------------------------------------------------------
// PORT: no Go counterpart — cache.go has no in-package test file in Go;
// GetVersionPaths is exercised only by the (deferred) module-resolution
// suites. These focused tests pin the trace/state-machine behavior.
// ---------------------------------------------------------------------------

#[test]
fn get_version_paths_traces_missing_types_versions() {
    let p = PackageJson::default();
    let mut traced: Vec<(&str, Vec<String>)> = Vec::new();
    let vp = p.get_version_paths(Some(&mut |message, args| {
        traced.push((message.key(), args.to_vec()));
    }));
    assert!(!vp.exists());
    assert!(vp.get_paths().is_none());
    assert_eq!(
        traced,
        [(
            "package_json_does_not_have_a_0_field_6100",
            vec!["typesVersions".to_string()],
        )]
    );
}

#[test]
fn get_version_paths_traces_non_object_types_versions() {
    let fields = parse(br#"{"typesVersions": "1.0.0"}"#).expect("parse");
    let mut p = PackageJson::default();
    p.fields = fields;
    let mut traced: Vec<(&str, Vec<String>)> = Vec::new();
    let vp = p.get_version_paths(Some(&mut |message, args| {
        traced.push((message.key(), args.to_vec()));
    }));
    assert!(!vp.exists());
    assert!(vp.get_paths().is_none());
    assert_eq!(
        traced,
        [(
            "Expected_type_of_0_field_in_package_json_to_be_1_got_2_6105",
            vec![
                "typesVersions".to_string(),
                "object".to_string(),
                "string".to_string(),
            ],
        )]
    );
}

#[test]
fn get_version_paths_traces_invalid_range_entries() {
    let fields = parse(br#"{"typesVersions": {"not a range": {"*": []}}}"#).expect("parse");
    let mut p = PackageJson::default();
    p.fields = fields;
    let mut traced: Vec<(&str, Vec<String>)> = Vec::new();
    let vp = p.get_version_paths(Some(&mut |message, args| {
        traced.push((message.key(), args.to_vec()));
    }));
    assert!(!vp.exists());
    assert!(vp.get_paths().is_none());
    assert_eq!(
        traced,
        [
            (
                "package_json_has_a_typesVersions_field_with_version_specific_path_mappings_6206",
                vec!["typesVersions".to_string()],
            ),
            (
                "package_json_has_a_typesVersions_entry_0_that_is_not_a_valid_semver_range_6209",
                vec!["not a range".to_string()],
            ),
            (
                "package_json_does_not_have_a_typesVersions_entry_that_matches_version_0_6207",
                vec!["7.1".to_string()],
            ),
        ]
    );
}

#[test]
fn get_version_paths_rejects_non_object_version_entry() {
    let fields = parse(br#"{"typesVersions": {"*": "oops"}}"#).expect("parse");
    let mut p = PackageJson::default();
    p.fields = fields;
    let mut traced: Vec<(&str, Vec<String>)> = Vec::new();
    let vp = p.get_version_paths(Some(&mut |message, args| {
        traced.push((message.key(), args.to_vec()));
    }));
    assert!(!vp.exists());
    assert!(vp.get_paths().is_none());
    assert_eq!(
        traced,
        [
            (
                "package_json_has_a_typesVersions_field_with_version_specific_path_mappings_6206",
                vec!["typesVersions".to_string()],
            ),
            (
                "Expected_type_of_0_field_in_package_json_to_be_1_got_2_6105",
                vec![
                    "typesVersions['*']".to_string(),
                    "object".to_string(),
                    "string".to_string(),
                ],
            ),
        ]
    );
}

#[test]
fn get_version_paths_matches_wildcard_range() {
    let fields = parse(br#"{"typesVersions": {"*": {"*": ["./v3/index.d.ts"]}}}"#).expect("parse");
    let mut p = PackageJson::default();
    p.fields = fields;
    let mut traced: Vec<(&str, Vec<String>)> = Vec::new();
    let vp = p.get_version_paths(Some(&mut |message, args| {
        traced.push((message.key(), args.to_vec()));
    }));
    assert!(vp.exists());
    assert_eq!(vp.version, "*");
    let paths = vp.get_paths().expect("paths");
    assert_eq!(paths.get_or_zero("*"), ["./v3/index.d.ts".to_string()]);
    assert_eq!(
        traced,
        [(
            "package_json_has_a_typesVersions_field_with_version_specific_path_mappings_6206",
            vec!["typesVersions".to_string()],
        )]
    );
    // Second call returns the same computed state (Go sync.Once).
    let again = p.get_version_paths(None);
    assert!(again.exists());
    assert_eq!(again.version, vp.version);
}
