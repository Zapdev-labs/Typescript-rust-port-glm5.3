// Port of tsc/internal/tsoptions/rawcompileroptions_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use tsc_core::options_generated::ScriptTarget;
use tsc_core::tristate::Tristate;
use tsc_tspath::{
    rooted_directory_path_from_absolute, to_source_map_location, RootedDirectoryPath,
    RootedFilePath,
};
use tsc_tsoptions::rawcompileroptions::RawCompilerOptions;

/// Go: `TestRawCompilerOptionsFinalizePaths`.
#[test]
fn test_raw_compiler_options_finalize_paths() {
    let mut raw = RawCompilerOptions::new();
    raw.unmarshal(
        r#"{
        "outDir": "dist",
        "rootDirs": ["src", "${configDir}/generated"],
        "tsBuildInfoFile": "cache/build.tsbuildinfo",
        "sourceRoot": "sources\\mapped",
        "paths": {
            "*a": ["first/*"],
            "*": ["fallback/*"]
        },
        "allowNonTsExtensions": true,
        "suppressOutputPathCheck": true,
        "configFilePath": "",
        "NoImplicitAny": true
    }"#,
    )
    .expect("unmarshal");

    let base_path: RootedDirectoryPath = rooted_directory_path_from_absolute("/project");
    let (options, diagnostics) = raw.finalize(&base_path);
    assert!(diagnostics.is_empty());
    assert_eq!(options.out_dir.as_string(), "/project/dist");
    assert_eq!(
        options
            .root_dirs
            .iter()
            .map(|d| d.as_string())
            .collect::<Vec<_>>(),
        vec!["/project/src", "/project/generated"]
    );
    assert_eq!(
        options.ts_build_info_file.as_string(),
        "/project/cache/build.tsbuildinfo"
    );
    assert_eq!(
        options.source_root,
        to_source_map_location("sources/mapped")
    );
    let paths = options.paths.as_ref().expect("paths");
    let paths_keys: Vec<&String> = paths.keys().collect();
    assert_eq!(paths_keys, ["*a", "*"]);
    let paths_a = paths.get("*a").expect("*a");
    assert_eq!(paths_a, &vec!["first/*".to_string()]);
    assert_eq!(options.allow_non_ts_extensions, Tristate::TSTrue);
    assert_eq!(options.suppress_output_path_check, Tristate::TSTrue);
    assert_eq!(options.config_file_path, RootedFilePath::default());
    // NoImplicitAny is case-insensitive-only (the declaration's name is
    // "noImplicitAny"), so it does not finalize.
    assert_eq!(options.no_implicit_any, Tristate::TSUnknown);
}

/// The marshal/unmarshal round trip preserves key order.
///
/// PORT: enum options stay out of the round trip — Go's
/// `floatOrInt32ToFlag` panics on a string exactly as the port does (the
/// raw form stores the JSON string; only the converted enum-map form
/// reaches the parse).
#[test]
fn test_raw_compiler_options_round_trip_order() {
    let mut raw = RawCompilerOptions::new();
    raw.unmarshal(r#"{ "strict": true, "outDir": "x", "types": ["node"] }"#)
        .expect("unmarshal");
    let text = raw.marshal();
    assert!(
        text.find("\"strict\"").unwrap() < text.find("\"outDir\"").unwrap(),
        "{text}"
    );
    assert!(
        text.find("\"outDir\"").unwrap() < text.find("\"types\"").unwrap(),
        "{text}"
    );

    // A second finalize after marshalling parses the same values.
    let mut raw2 = RawCompilerOptions::new();
    raw2.unmarshal(&text).expect("unmarshal");
    let (options, _) = raw2.finalize(&rooted_directory_path_from_absolute("/project"));
    assert!(options.strict.is_true());
    assert_eq!(options.out_dir.as_string(), "/project/x");
    assert_eq!(options.types, vec!["node".to_string()]);
}
