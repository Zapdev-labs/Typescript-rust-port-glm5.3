// Port of tsc/internal/tsoptions/tsconfigparsing_test.go (the M4-visible
// surface) + contentmappers_test.go's parseContentMapper scenarios.
//
// PORT notes:
//   - The Go suite's baseline-file scenarios (golden diagnostics text) are
//     replaced by direct value/error-count assertions, exactly as in
//     commandlineparser_test.rs.
//   - Node-module `extends` resolution needs the module crate seam
//     (`ResolveConfigHost`) — covered by the seam test below.

use tsc_tspath::{
    rooted_directory_path_from_absolute, rooted_file_path_from_absolute, CaseSensitivity,
};
use tsc_vfs::vfstest::from_map;

use tsc_tsoptions::tsconfigparsing::{
    parse_config_file_text_to_json, parse_json_config_file_content, ResolveConfigHost,
};
use tsc_tsoptions::CompilerOptionsValue;

/// Go: the parse-worker spec validation (validateSpecs) — trailing
/// recursion and dot-dot-after-recursive-wildcard are rejected with the
/// 5065/5010-family diagnostics.
#[test]
fn test_spec_validation_errors() {
    let fs = from_map(Vec::<(String, String)>::new(), CaseSensitivity::CaseSensitive);
    let base_path = rooted_directory_path_from_absolute("/project");
    let config_file_path = rooted_file_path_from_absolute("/project/tsconfig.json");

    let text = r#"{ "include": ["src/**/**/a.ts"] }"#;
    let (json, errors) = parse_config_file_text_to_json(text);
    assert!(errors.is_empty());
    let parsed = parse_json_config_file_content(
        &json.expect("parse"),
        fs.as_ref(),
        &base_path,
        None,
        &config_file_path,
        &[],
        None,
        None,
    );
    assert!(!parsed.errors.is_empty(), "trailing recursion must error");

    let text = r#"{ "include": ["src/**/../a.ts"] }"#;
    let (json, errors) = parse_config_file_text_to_json(text);
    assert!(errors.is_empty());
    let parsed = parse_json_config_file_content(
        &json.expect("parse"),
        fs.as_ref(),
        &base_path,
        None,
        &config_file_path,
        &[],
        None,
        None,
    );
    assert!(!parsed.errors.is_empty(), "dot-dot after wildcard must error");
}

/// Extends cycles are detected via the resolution stack.
#[test]
fn test_extends_cycle_detection() {
    let fs = from_map(
        vec![
            (
                "/project/tsconfig.json".to_string(),
                r#"{ "extends": "./base.json" }"#.to_string(),
            ),
            (
                "/project/base.json".to_string(),
                r#"{ "extends": "./tsconfig.json" }"#.to_string(),
            ),
        ],
        CaseSensitivity::CaseSensitive,
    );
    let base_path = rooted_directory_path_from_absolute("/project");
    let config_file_path = rooted_file_path_from_absolute("/project/tsconfig.json");
    let (json, errors) = parse_config_file_text_to_json(r#"{ "extends": "./base.json" }"#);
    assert!(errors.is_empty());
    let parsed = parse_json_config_file_content(
        &json.expect("parse"),
        fs.as_ref(),
        &base_path,
        None,
        &config_file_path,
        &[],
        None,
        None,
    );
    assert!(
        !parsed.errors.is_empty(),
        "extends cycle must produce an error"
    );
}

/// The module-specifier `extends` arm consults the ResolveConfigHost seam.
#[test]
fn test_extends_module_specifier_seam() {
    struct ResolvingHost;
    impl ResolveConfigHost for ResolvingHost {
        fn resolve(
            &self,
            specifier: &str,
            _containing_file: &tsc_tspath::RootedFilePath,
        ) -> Option<tsc_tspath::RootedFilePath> {
            if specifier == "@tsconfig/spec" {
                Some(rooted_file_path_from_absolute(
                    "/project/node_modules/@tsconfig/spec/tsconfig.json",
                ))
            } else {
                None
            }
        }
    }

    let fs = from_map(
        vec![
            (
                "/project/tsconfig.json".to_string(),
                r#"{ "extends": "@tsconfig/spec" }"#.to_string(),
            ),
            (
                "/project/node_modules/@tsconfig/spec/tsconfig.json".to_string(),
                r#"{ "compilerOptions": { "strict": true } }"#.to_string(),
            ),
        ],
        CaseSensitivity::CaseSensitive,
    );
    let base_path = rooted_directory_path_from_absolute("/project");
    let config_file_path = rooted_file_path_from_absolute("/project/tsconfig.json");
    let host = ResolvingHost;
    let (json, errors) = parse_config_file_text_to_json(r#"{ "extends": "@tsconfig/spec" }"#);
    assert!(errors.is_empty());
    let parsed = parse_json_config_file_content(
        &json.expect("parse"),
        fs.as_ref(),
        &base_path,
        None,
        &config_file_path,
        &[],
        None,
        Some(&host),
    );
    assert!(parsed.errors.is_empty(), "errors: {}", parsed.errors.len());
    assert!(parsed.compiler_options().strict.is_true());
}

/// Go: contentmappers_test.go — parseContentMapper accepts the declared
/// shape and rejects malformed ones with the requires-a-value diagnostic.
#[test]
fn test_parse_content_mapper() {
    use tsc_tsoptions::tsconfigparsing::parse_json_text;
    use tsc_tsoptions::parse_content_mapper;

    let value = parse_json_text(
        r#"{ "package": "@scope/mapper", "extensions": [".astro"], "options": { "a": 1 } }"#,
    )
    .expect("parse");
    let (mapper, errors) = parse_content_mapper(&value);
    assert!(errors.is_empty());
    let mapper = mapper.expect("mapper");
    assert_eq!(mapper.definition.package, "@scope/mapper");
    assert_eq!(mapper.definition.extensions, vec![".astro".to_string()]);
    assert!(mapper.definition.options.is_some());

    // Missing package → the requires-a-value diagnostic.
    let value = parse_json_text(r#"{ "extensions": [".astro"] }"#).expect("parse");
    let (mapper, errors) = parse_content_mapper(&value);
    assert!(mapper.is_none());
    assert_eq!(errors.len(), 1);

    // Non-array extensions.
    let value = parse_json_text(r#"{ "package": "p", "extensions": ".astro" }"#).expect("parse");
    let (mapper, errors) = parse_content_mapper(&value);
    assert!(mapper.is_none());
    assert_eq!(errors.len(), 1);
}

/// The config-file entry point reads and parses the tsconfig from the FS.
#[test]
fn test_get_parsed_command_line_of_config_file_path() {
    use tsc_tsoptions::tsconfigparsing::get_parsed_command_line_of_config_file_path;

    let fs = from_map(
        vec![(
            "/project/tsconfig.json".to_string(),
            r#"{ "compilerOptions": { "strict": true, "outDir": "dist" } }"#.to_string(),
        )],
        CaseSensitivity::CaseSensitive,
    );
    let config_file_path = rooted_file_path_from_absolute("/project/tsconfig.json");
    let parsed = get_parsed_command_line_of_config_file_path(&config_file_path, fs.as_ref(), None);
    assert!(parsed.errors.is_empty(), "errors: {}", parsed.errors.len());
    assert!(parsed.compiler_options().strict.is_true());
    assert_eq!(parsed.compiler_options().out_dir.as_string(), "/project/dist");
    assert_eq!(parsed.config_file_name, config_file_path);
}

/// Raw values flow through the value model with key order preserved.
#[test]
fn test_parse_json_text_key_order() {
    use tsc_tsoptions::tsconfigparsing::parse_json_text;

    let value = parse_json_text(r#"{ "b": 1, "a": 2, "c": [1, "x"] }"#).expect("parse");
    let CompilerOptionsValue::Object(map) = value else {
        panic!("object");
    };
    let keys: Vec<&String> = map.keys().collect();
    assert_eq!(keys, ["b", "a", "c"]);
}
