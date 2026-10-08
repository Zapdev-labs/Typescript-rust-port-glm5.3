// Port of tsc/internal/tsoptions/parsinghelpers_test.go (portable surface)
// + compileroptions_test.go's parse-value spot checks.
//
// PORT notes:
//   - `TestMergeCompilerOptions` and `TestParseCompilerOptionNoMissingFields`
//     reflect over core.CompilerOptions; the generator's parity check locks
//     the parse-table coverage instead. `TestMergeCompilerOptionsRawNulls`
//     (explicit-null semantics) ports exactly — it needs no reflection.

use tsc_core::options_generated::{CompilerOptions, ModuleDetectionKind};
use tsc_core::tristate::Tristate;
use tsc_tsoptions::parsinghelpers::merge_compiler_options;
use tsc_tsoptions::tsconfigparsing::parse_json_text;
use tsc_tsoptions::{parse_tristate, CompilerOptionsValue};

/// Go: `TestMergeCompilerOptionsRawNulls`.
#[test]
fn test_merge_compiler_options_raw_nulls() {
    for name in ["moduleDetection", "ModuleDetection", "moduleDetectionKind", "unknownOption"] {
        let raw_text = format!(r#"{{ "compilerOptions": {{ "{name}": null }} }}"#);
        let raw = parse_json_text(&raw_text).expect("parse");
        let mut target = CompilerOptions {
            module_detection: ModuleDetectionKind::Force,
            ..CompilerOptions::default()
        };
        let source = CompilerOptions {
            module_detection: ModuleDetectionKind::Auto,
            ..CompilerOptions::default()
        };
        merge_compiler_options(&mut target, Some(&source), Some(&raw));
        // Go: the explicit `moduleDetection: null` resets the merged value;
        // case variants and unknown keys do not.
        let expected = if name == "moduleDetection" {
            ModuleDetectionKind::None
        } else {
            ModuleDetectionKind::Auto
        };
        assert_eq!(target.module_detection, expected, "{name}");

        // Merging an object with itself must still apply explicit nulls.
        let mut source = source;
        let source_snapshot = source.clone();
        merge_compiler_options(&mut source, Some(&source_snapshot), Some(&raw));
        assert_eq!(source.module_detection, expected, "{name}");
    }
}

/// `parseTristate` over the JSON value shapes (compileroptions_test spot
/// checks).
#[test]
fn test_parse_tristate() {
    assert_eq!(parse_tristate(&CompilerOptionsValue::Bool(true)), Tristate::TSTrue);
    assert_eq!(parse_tristate(&CompilerOptionsValue::Bool(false)), Tristate::TSFalse);
    assert_eq!(parse_tristate(&CompilerOptionsValue::Null), Tristate::TSUnknown);
}

/// `parseStringArray` accepts both array shapes and rejects scalars.
#[test]
fn test_parse_string_array() {
    assert_eq!(
        tsc_tsoptions::parse_string_array(&CompilerOptionsValue::StrArray(vec![
            "a".to_string(),
            "b".to_string()
        ])),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        tsc_tsoptions::parse_string_array(&CompilerOptionsValue::Array(vec![
            CompilerOptionsValue::Str("a".to_string()),
        ])),
        vec!["a".to_string()]
    );
    // A scalar surfaces as an empty array (Go's type assertion zero value).
    assert!(tsc_tsoptions::parse_string_array(&CompilerOptionsValue::Str("a".to_string())).is_empty());
}

/// `parseProjectReference` keeps Go's has*/valid* flag shape.
#[test]
fn test_parse_project_reference() {
    let value = parse_json_text(r#"{ "path": "./src" }"#).expect("parse");
    let result = tsc_tsoptions::parse_project_reference(&value).expect("reference");
    assert_eq!(result.path, "./src");
    assert!(result.has_path);
    assert!(result.path_valid);

    let value = parse_json_text(r#"{ "circular": true }"#).expect("parse");
    let result = tsc_tsoptions::parse_project_reference(&value).expect("reference");
    assert!(result.has_circular);
    assert!(result.reference.circular);
    assert!(!result.has_path);
}

/// The ${configDir} template trio (tsconfigparsing.go helpers, re-homed in
/// parsinghelpers.rs).
#[test]
fn test_config_dir_template() {
    use tsc_tsoptions::parsinghelpers::{
        get_substituted_path_with_config_dir_template, starts_with_config_dir_template,
    };
    assert!(starts_with_config_dir_template("${configDir}/src"));
    assert!(starts_with_config_dir_template("${CONFIGDIR}/src"));
    assert!(!starts_with_config_dir_template("src/${configDir}"));

    let base = tsc_tspath::rooted_directory_path_from_absolute("/project");
    assert_eq!(
        get_substituted_path_with_config_dir_template("${configDir}/src", &base),
        "/project/src"
    );
    assert_eq!(
        get_substituted_path_with_config_dir_template("plain", &base),
        "/project/plain"
    );
}

/// ProjectReferenceParseResult flag surface (parsinghelpers.rs).
#[test]
fn test_parse_project_reference_flags() {
    let value = parse_json_text(
        r#"{ "path": "a", "circular": "not-a-bool" }"#,
    )
    .expect("parse");
    let result = tsc_tsoptions::parse_project_reference(&value).expect("reference");
    assert!(result.has_circular);
    // Go: a non-bool `circular` leaves the flag present but invalid.
    assert!(!result.circular_valid);
}
