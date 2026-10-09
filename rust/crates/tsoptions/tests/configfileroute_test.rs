// Config-file route tests (the M6 gap-5 wave): the port of Go's
// tsconfigparsing_test.go TestParseConfigFileTextToJson value cases +
// TestParseJsonSourceFileConfigFileContentReportsInvalidExtendedConfig /
// ...WithEmptyExtendedConfig /
// ...DoesNotDuplicateUnquotedKeyDiagnostics /
// ...ReportsQuestionTokenDiagnostics — the AST→value conversion and the
// route-split diagnostics discipline (the JSON PARSE diagnostics ride the
// TsConfigSourceFile and surface through GetConfigFileParsingDiagnostics;
// the conversion's diagnostics ride `errors`; neither duplicates the
// other's).
//
// PORT notes:
//   - Go's `tsoptionstest.GetParsedCommandLine` (NewTsconfigSourceFile-
//     FromFilePath + ParseJsonSourceFileConfigFileContent) maps onto
//     [`get_parsed_command_line_for_test`] below; the positional form
//     (tsoptionstest.GetParsedCommandLineWithPositions) is the same walk
//     with the config AST retained.
//   - Positions assert through
//     `get_ecma_line_and_utf16_character_of_position` exactly like Go.
//   - Go asserts each diagnostic's `File()`; the port pairs through
//     `ParsedCommandLine::error_files` (the file-pairing surface this wave
//     adds — the tsc_ast Diagnostic cannot resolve its own file).

use std::sync::Arc;

use tsc_tspath::{rooted_directory_path_from_absolute, rooted_file_path_from_absolute, CaseSensitivity};
use tsc_vfs::vfstest;

use tsc_tsoptions::tsconfigparsing::{
    new_tsconfig_source_file_from_file_path, parse_config_file_text_to_json,
    parse_json_source_file_config_file_content,
};
use tsc_tsoptions::{CompilerOptionsValue, ParsedCommandLine};

/// Go: `tsoptions.ParseConfigFileTextToJson("/apath/tsconfig.json", "/apath", jsonText)`
/// with the test's fixed name.
fn parse_for_test(text: &str) -> (CompilerOptionsValue, Vec<tsc_ast::Diagnostic>) {
    let file_name = rooted_file_path_from_absolute("/apath/tsconfig.json");
    let path_key = CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    parse_config_file_text_to_json(&file_name, path_key, text)
}

/// Go: `tsoptionstest.GetParsedCommandLine` — the config-file route over a
/// map FS (the tsconfig at `<current_directory>/tsconfig.json` with the
/// given text, plus the extra files).
fn get_parsed_command_line_for_test(
    json_text: &str,
    files: &[(&str, &str)],
    current_directory: &str,
) -> ParsedCommandLine {
    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect();
    let config_file_name_string = format!(
        "{}/tsconfig.json",
        current_directory.trim_end_matches('/')
    );
    entries.push((config_file_name_string.clone(), json_text.to_string()));
    let fs = vfstest::from_map(entries, CaseSensitivity::CaseSensitive);
    let config_file_name = rooted_file_path_from_absolute(&config_file_name_string);
    let config_file = new_tsconfig_source_file_from_file_path(
        &config_file_name,
        CaseSensitivity::CaseSensitive,
        json_text,
    );
    parse_json_source_file_config_file_content(
        Arc::new(config_file),
        fs.as_ref(),
        &rooted_directory_path_from_absolute(current_directory),
        None,
        &config_file_name,
        &[],
        None,
        None,
    )
}

/// The `(line, character)` a diagnostic renders at (Go:
/// `scanner.GetECMALineAndUTF16CharacterOfPosition(d.File(), d.Pos())`).
fn line_and_character(
    config_text: &str,
    config_name: &str,
    pos: i32,
) -> (usize, usize) {
    let config_file = new_tsconfig_source_file_from_file_path(
        &rooted_file_path_from_absolute(config_name),
        CaseSensitivity::CaseSensitive,
        config_text,
    );
    let source_file = config_file.lock_source_file();
    let (line, character) = tsc_scanner::get_ecma_line_and_utf16_character_of_position(
        &*source_file,
        pos,
    );
    (line, character as usize)
}

fn exclude_list(value: &CompilerOptionsValue) -> Vec<String> {
    let CompilerOptionsValue::Object(map) = value else {
        panic!("not an object: {value:?}");
    };
    match map.get("exclude").expect("exclude") {
        CompilerOptionsValue::Array(list) => list
            .iter()
            .map(|value| match value {
                CompilerOptionsValue::Str(spec) => spec.clone(),
                other => panic!("non-string spec: {other:?}"),
            })
            .collect(),
        other => panic!("exclude is not an array: {other:?}"),
    }
}

/// Go: `TestParseConfigFileTextToJson` — "returns empty config for file
/// with only whitespaces" + "with comments only" (the `struct{}{}` root
/// marker — a non-map value; the callers' empty-map fallback) + "when
/// config is empty object".
#[test]
fn test_parse_config_file_text_to_json_empty_roots() {
    for text in ["", " ", "// Comment", "/* Comment*/"] {
        let (json, errors) = parse_for_test(text);
        assert!(errors.is_empty(), "no diagnostics for {text:?}: {errors:?}");
        // Go: the `struct{}{}` marker (not an OrderedMap) — every caller
        // normalizes to the empty config.
        assert!(
            !matches!(json, CompilerOptionsValue::Object(_)),
            "not an object for {text:?}"
        );
    }
    let (json, errors) = parse_for_test("{}");
    assert!(errors.is_empty());
    assert!(matches!(json, CompilerOptionsValue::Object(_)));
}

/// Go: `TestParseConfigFileTextToJson` — "returns config object without
/// comments" (line + multiline + inline comments) and "keeps string content
/// untouched" (`//`/`/*` inside string literals stay literal).
#[test]
fn test_parse_config_file_text_to_json_comments() {
    let (json, errors) = parse_for_test(
        r#"{ // Excluded files
            "exclude": [
                // Exclude d.ts
                "file.d.ts"
            ]
        }"#,
    );
    assert!(errors.is_empty());
    assert_eq!(exclude_list(&json), ["file.d.ts"]);

    let (json, errors) = parse_for_test(
        r#"{
            /* Excluded
                    Files
            */
            "exclude": [
                /* multiline comments can be in the middle of a line */"file.d.ts"
            ]
        }"#,
    );
    assert!(errors.is_empty());
    assert_eq!(exclude_list(&json), ["file.d.ts"]);

    // String content is untouched — the `//`/`/*` are part of the string.
    let (json, errors) = parse_for_test(
        r#"{
            "exclude": [
                "xx//file.d.ts"
            ]
        }"#,
    );
    assert!(errors.is_empty());
    assert_eq!(exclude_list(&json), ["xx//file.d.ts"]);

    let (json, errors) = parse_for_test(
        r#"{
            "exclude": [
                "xx/*file.d.ts*/"
            ]
        }"#,
    );
    assert!(errors.is_empty());
    assert_eq!(exclude_list(&json), ["xx/*file.d.ts*/"]);
}

/// Go: `TestParseConfigFileTextToJson` — "handles escaped characters in
/// strings correctly".
#[test]
fn test_parse_config_file_text_to_json_escapes() {
    let (json, errors) = parse_for_test(
        r#"{
            "exclude": [
                "xx\"//files"
            ]
        }"#,
    );
    assert!(errors.is_empty());
    assert_eq!(exclude_list(&json), ["xx\"//files"]);

    // The escaped backslash ENDS the string; the rest is a comment.
    let (json, errors) = parse_for_test(
        r#"{
            "exclude": [
                "xx\\" // end of line comment
            ]
        }"#,
    );
    assert!(errors.is_empty());
    assert_eq!(exclude_list(&json), ["xx\\"]);
}

/// Go: `TestParseConfigFileTextToJson` — "returns object when users
/// correctly specify library".
#[test]
fn test_parse_config_file_text_to_json_lib_arrays() {
    for (text, libs) in [
        (
            r#"{
            "compilerOptions": {
                "lib": ["es5"]
            }
        }"#,
            vec!["es5".to_string()],
        ),
        (
            r#"{
            "compilerOptions": {
                "lib": ["es5", "es6"]
            }
        }"#,
            vec!["es5".to_string(), "es6".to_string()],
        ),
    ] {
        let (json, errors) = parse_for_test(text);
        assert!(errors.is_empty());
        let CompilerOptionsValue::Object(map) = &json else {
            panic!("not an object");
        };
        let CompilerOptionsValue::Object(compiler_options) = map.get("compilerOptions").unwrap()
        else {
            panic!("no compilerOptions");
        };
        assert_eq!(
            compiler_options.get("lib").unwrap(),
            &CompilerOptionsValue::Array(
                libs.iter().map(|s| CompilerOptionsValue::Str(s.clone())).collect()
            )
        );
    }
}

/// Go: convertConfigFileToObject's root check + array recovery — a
/// non-object root reports `The_root_value_of_a_0_file_must_be_an_object`
/// positioned at the root and falls back to the FIRST object-literal array
/// element (Go DROPS the root diagnostic on that arm); with no object
/// element the empty config + the root diagnostic return.
///
/// PORT: Go exercises this through the config/tsconfigParsing baselines;
/// the port asserts the value/diagnostic shapes directly.
#[test]
fn test_parse_config_file_text_to_json_root_not_object() {
    // Array with an object element: the recovery converts it, the root
    // diagnostic is dropped (Go's exact arm).
    let (json, errors) = parse_for_test(r#"[{ "exclude": ["a"] }]"#);
    assert!(errors.is_empty());
    assert_eq!(exclude_list(&json), ["a"]);

    // Array with no object element: the root diagnostic + the empty map.
    let (json, errors) = parse_for_test(r#"[42]"#);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code(), 5092 /* The_root_value_of_a_0_file_must_be_an_object */);
    assert_eq!(errors[0].message_args(), ["tsconfig.json"]);
    assert_eq!(
        &"[42]"[errors[0].pos() as usize..errors[0].end() as usize],
        "[42]"
    );
    assert!(matches!(json, CompilerOptionsValue::Object(_)));

    // A jsconfig names itself in the message.
    let file_name = rooted_file_path_from_absolute("/apath/jsconfig.json");
    let path_key = CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    let (_json, errors) = parse_config_file_text_to_json(&file_name, path_key, r#"[42]"#);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message_args(), ["jsconfig.json"]);
}

/// Go: `TestParseJsonSourceFileConfigFileContentDoesNotDuplicateUnquotedKeyDiagnostics`
/// — the unquoted keys produce the JSON parse's
/// String_literal_with_double_quotes_expected diagnostics (riding the
/// SourceFile) EXACTLY once each; the conversion reports nothing for them
/// (errors stays empty).
#[test]
fn test_does_not_duplicate_unquoted_key_diagnostics() {
    let text = r#"{
  compilerOptions: {
    strict: true
  }
}"#;
    let parsed = get_parsed_command_line_for_test(
        text,
        &[("/main.ts", "export const x = 1;")],
        "/",
    );
    let diags = parsed.get_config_file_parsing_diagnostics();
    assert_eq!(diags.len(), 2, "exactly the two parse diagnostics: {diags:?}");
    let expected = [(1, 2), (2, 4)];
    for (index, (file_name, diagnostic)) in diags.iter().enumerate() {
        assert_eq!(diagnostic.code(), 1327 /* String_literal_with_double_quotes_expected */);
        assert_eq!(file_name.as_ref().map(|n| n.as_string().to_string()), Some("/tsconfig.json".to_string()));
        assert_eq!(line_and_character(text, "/tsconfig.json", diagnostic.pos()), expected[index]);
    }
}

/// Go: `TestParseJsonSourceFileConfigFileContentReportsQuestionTokenDiagnostics`
/// — the conversion reports `The_0_modifier_can_only_be_used_in_TypeScript_files`
/// ("?") at each `?` token (riding `errors`, paired with the config file).
#[test]
fn test_reports_question_token_diagnostics() {
    let text = r#"{
  compilerOptions?: {
    strict?: true
  }
}"#;
    let parsed = get_parsed_command_line_for_test(
        text,
        &[("/main.ts", "export const x = 1;")],
        "/",
    );
    let question_token_diagnostics: Vec<_> = parsed
        .get_config_file_parsing_diagnostics()
        .into_iter()
        .filter(|(_, diagnostic)| {
            diagnostic.message_key() == "The_0_modifier_can_only_be_used_in_TypeScript_files_8009"
        })
        .collect();
    assert_eq!(
        question_token_diagnostics.len(),
        2,
        "exactly the two question-token diagnostics"
    );
    let expected = [(1, 17), (2, 10)];
    for (index, (file_name, diagnostic)) in question_token_diagnostics.iter().enumerate() {
        assert_eq!(file_name.as_ref().map(|n| n.as_string().to_string()), Some("/tsconfig.json".to_string()));
        assert_eq!(line_and_character(text, "/tsconfig.json", diagnostic.pos()), expected[index]);
        assert_eq!(diagnostic.message_args(), ["?"]);
    }
}

/// Go: `TestParseJsonSourceFileConfigFileContentReportsInvalidExtendedConfig`
/// — the extended config's JSON parse diagnostics ride the PARENT's
/// `errors` (ALL of them — the conversion is skipped), each paired with the
/// EXTENDED file's identity.
#[test]
fn test_reports_invalid_extended_config() {
    let files = [
        (
            "/project/tsconfig.json",
            r#"{
  "extends": "./bad.json"
}"#,
        ),
        // The parser recovers from this as object-like JSON, producing
        // expected-token errors for ':', ',', ',', and '}'.
        ("/project/bad.json", "{ this is not json"),
        ("/project/main.ts", "export const x = 1;"),
    ];
    let entries: Vec<(String, String)> = files
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect();
    let fs = vfstest::from_map(entries, CaseSensitivity::CaseSensitive);
    let config_file_name = rooted_file_path_from_absolute("/project/tsconfig.json");
    let config_file = new_tsconfig_source_file_from_file_path(
        &config_file_name,
        CaseSensitivity::CaseSensitive,
        files[0].1,
    );
    let parsed = parse_json_source_file_config_file_content(
        Arc::new(config_file),
        fs.as_ref(),
        &rooted_directory_path_from_absolute("/project"),
        None,
        &config_file_name,
        &[],
        None,
        None,
    );

    let parse_errors: Vec<_> = parsed
        .get_config_file_parsing_diagnostics()
        .into_iter()
        .filter(|(_, diagnostic)| diagnostic.message_key() == "_0_expected_1005")
        .collect();
    let expected_messages = [":", ",", ",", "}"];
    let expected_positions = [7, 10, 14, 18];
    assert_eq!(parse_errors.len(), expected_messages.len());
    for (index, (file_name, diagnostic)) in parse_errors.iter().enumerate() {
        assert_eq!(diagnostic.message_args()[0], expected_messages[index]);
        assert_eq!(diagnostic.pos(), expected_positions[index]);
        // Each extended-file diagnostic pairs with the EXTENDED file.
        assert_eq!(
            file_name.as_ref().map(|n| n.as_string().to_string()),
            Some("/project/bad.json".to_string())
        );
    }
}

/// Go: `TestParseJsonSourceFileConfigFileContentWithEmptyExtendedConfig`
/// — extending an empty config file must not panic (#4265); the
/// conversion's `struct{}{}` root normalizes to the empty config.
#[test]
fn test_with_empty_extended_config() {
    let parsed = get_parsed_command_line_for_test(
        r#"{
  "extends": "./base.json"
}"#,
        &[
            ("/project/base.json", ""),
            ("/project/main.ts", "export const x = 1;"),
        ],
        "/project",
    );
    assert_eq!(
        parsed
            .file_names()
            .iter()
            .map(|name| name.as_string().to_string())
            .collect::<Vec<_>>(),
        ["/project/main.ts".to_string()]
    );
}

/// PORT (no direct Go counterpart): the worker sets
/// `ConfigFilePath = configFileName` on the config-file route (Go's
/// parseJsonConfigFileContentWorker line — feeds verifyCompilerOptions'
/// tsBuildInfoFile arm + the rootDir-inferred check).
#[test]
fn test_config_file_route_sets_config_file_path() {
    let parsed = get_parsed_command_line_for_test(
        r#"{ "compilerOptions": { "incremental": true } }"#,
        &[("/main.ts", "export const x = 1;")],
        "/project",
    );
    assert_eq!(
        parsed.compiler_options().config_file_path.as_string(),
        "/project/tsconfig.json"
    );
}
