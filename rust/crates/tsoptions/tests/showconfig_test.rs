// Port of tsc/internal/tsoptions/showconfig_test.go (the portable surface) +
// the M4 exit criterion: the deterministic showconfig runner.
//
// PORT notes:
//   - Go's `reflectedSerializeCompilerOptions` cross-check relies on
//     reflection; the port instead locks the deterministic runner: parse a
//     fixture tsconfig through the full pipeline (text → JSON → config
//     content → ConvertToTSConfig → marshal) and assert the marshalled
//     output — including running it twice for byte-for-byte determinism.
//   - `TestShowConfigEnumSerialization` ports exactly (first-alias rule).

use tsc_tspath::{
    path_key_from_canonical, rooted_directory_path_from_absolute, rooted_file_path_from_absolute,
    CaseSensitivity,
};
use tsc_vfs::vfstest::from_map;

use tsc_tsoptions::showconfig::convert_to_tsconfig;
use tsc_tsoptions::tsconfigparsing::{
    parse_config_file_text_to_json, parse_json_config_file_content,
};
use tsc_tsoptions::{CompilerOptionsValue, ParsedCommandLine};

/// The deterministic showconfig runner: given a tsconfig text (and its
/// directory), produce the `--showConfig` output.
pub fn run_showconfig(config_text: &str, config_directory: &str) -> String {
    let fs = from_map(
        vec![(format!("{config_directory}/tsconfig.json"), config_text.to_string())],
        CaseSensitivity::CaseSensitive,
    );
    let config_file_path = rooted_file_path_from_absolute(&format!(
        "{config_directory}/tsconfig.json"
    ));
    let base_path = rooted_directory_path_from_absolute(config_directory);

    let (json, mut errors) = parse_config_file_text_to_json(config_text);
    assert!(errors.is_empty(), "parse errors: {errors:?}");
    let json = json.expect("parse result");
    let parsed: ParsedCommandLine = parse_json_config_file_content(
        &json,
        fs.as_ref(),
        &base_path,
        None,
        &config_file_path,
        &[],
        None,
        None,
    );
    errors.extend(parsed.errors.iter().cloned());
    let tsconfig = convert_to_tsconfig(&parsed, &config_file_path);
    tsconfig.marshal()
}

fn parse_map(text: &str) -> tsc_collections::OrderedMap<String, CompilerOptionsValue> {
    let (json, errors) = parse_config_file_text_to_json(text);
    assert!(errors.is_empty());
    let CompilerOptionsValue::Object(map) = json.expect("parse result") else {
        panic!("not an object");
    };
    map
}

/// The marshalled runner output is deterministic across runs and stable for
/// key option kinds (paths, tristates, enums, lists, implied options).
#[test]
fn test_showconfig_runner_is_deterministic() {
    let config_text = r#"{
    "compilerOptions": {
        "target": "es2020",
        "module": "commonjs",
        "strict": true,
        "noImplicitAny": false,
        "outDir": "dist",
        "rootDirs": ["src", "gen"],
        "types": ["node", "jest"],
        "paths": { "@/*": ["src/*"] },
        "incremental": true
    },
    "include": ["src/**/*"],
    "exclude": ["node_modules", "dist"]
}"#;
    let first = run_showconfig(config_text, "/project");
    let second = run_showconfig(config_text, "/project");
    assert_eq!(first, second, "showconfig output must be deterministic");
    assert!(first.starts_with("{\n  \"compilerOptions\": {"), "{first}");
    // Paths serialize relative to the config file.
    assert!(first.contains("\"outDir\": \"dist\""), "{first}");
    assert!(first.contains("\"rootDirs\": [\n      \"src\",\n      \"gen\"\n    ]"), "{first}");
    // Strict implies its constituent checks (implied options); an explicitly
    // provided value (noImplicitAny: false) is not overridden.
    assert!(first.contains("\"strict\": true"), "{first}");
    assert!(first.contains("\"noImplicitAny\": false"), "{first}");
    assert!(first.contains("\"noImplicitThis\": true"), "{first}");
    assert!(first.contains("\"target\": \"es2020\""), "{first}");
    assert!(first.contains("\"module\": \"commonjs\""), "{first}");
    assert!(first.contains("\"types\": [\n      \"node\",\n      \"jest\"\n    ]"), "{first}");
    assert!(first.contains("\"paths\": {\n      \"@/*\": [\n        \"src/*\"\n      ]\n    }"), "{first}");
    // incremental implies tsBuildInfoFile (implied-options table).
    assert!(first.contains("\"tsBuildInfoFile\""), "{first}");
    // The spec arrays pass through.
    assert!(first.contains("\"include\": [\n    \"src/**/*\"\n  ]"), "{first}");
    assert!(first.contains("\"exclude\": [\n    \"node_modules\",\n    \"dist\"\n  ]"), "{first}");
    assert!(first.ends_with("}"), "{first}");
}

/// Extends merging: the base file's options are inherited and overridden.
#[test]
fn test_showconfig_extends_merge() {
    let base_text = r#"{
    "compilerOptions": {
        "target": "es2019",
        "strict": true,
        "outDir": "base-dist"
    }
}"#;
    let config_text = r#"{
    "extends": "./base.json",
    "compilerOptions": {
        "outDir": "dist"
    }
}"#;
    let fs = from_map(
        vec![
            ("/project/tsconfig.json".to_string(), config_text.to_string()),
            ("/project/base.json".to_string(), base_text.to_string()),
        ],
        CaseSensitivity::CaseSensitive,
    );
    let config_file_path = rooted_file_path_from_absolute("/project/tsconfig.json");
    let base_path = rooted_directory_path_from_absolute("/project");
    let (json, errors) = parse_config_file_text_to_json(config_text);
    assert!(errors.is_empty());
    let parsed = parse_json_config_file_content(
        &json.expect("parse result"),
        fs.as_ref(),
        &base_path,
        None,
        &config_file_path,
        &[],
        None,
        None,
    );
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors.len());
    let options = parsed.compiler_options();
    // The extends target is inherited; outDir is overridden; strict carries.
    assert_eq!(options.target, tsc_core::options_generated::ScriptTarget::ES2019);
    assert!(options.strict.is_true());
    assert_eq!(options.out_dir.as_string(), "/project/dist");
}

/// Default include when neither files nor include are present: `["**/*"]`.
#[test]
fn test_showconfig_default_include() {
    let config_text = r#"{ "compilerOptions": { "strict": true } }"#;
    let output = run_showconfig(config_text, "/project");
    assert!(output.contains("\"include\": [\n    \"**/*\"\n  ]"), "{output}");
}

/// Go: `TestShowConfigEnumSerialization` — every enum map value serializes
/// back to its first alias.
#[test]
fn test_showconfig_enum_serialization() {
    for option in tsc_tsoptions::declarations_generated::options_declarations() {
        let Some(enum_map) = option.enum_map() else {
            continue;
        };
        // value → the first key that produced it (Go: `seen` map).
        let mut seen: Vec<(CompilerOptionsValue, String)> = Vec::new();
        for (name, value) in enum_map.entries() {
            let first_alias = match seen.iter().find(|(seen_value, _)| seen_value == value) {
                Some((_, first_alias)) => first_alias.clone(),
                None => {
                    seen.push((value.clone(), name.clone()));
                    name.clone()
                }
            };
            let got = tsc_tsoptions::serialize_compiler_option_enum(value);
            assert_eq!(
                got, first_alias,
                "{}: got {got:?}, want first alias {first_alias:?}",
                option.name
            );
        }
    }
}

/// PathKeyFromCanonical round-trips for the fixture paths the runner emits.
#[test]
fn test_path_key_from_canonical_fixtures() {
    let key = path_key_from_canonical("/project/tsconfig.json");
    assert_eq!(key.as_string(), "/project/tsconfig.json");
}

/// Raw object values flow through `marshal_value` in Go key order.
#[test]
fn test_marshal_preserves_key_order() {
    let map = parse_map(r#"{ "zeta": 1, "alpha": "x", "mid": [1, 2] }"#);
    let text = tsc_tsoptions::showconfig::marshal_value(&CompilerOptionsValue::Object(map), 0);
    assert!(text.contains("\"zeta\""), "{text}");
    assert!(text.find("\"zeta\"").unwrap() < text.find("\"alpha\"").unwrap(), "{text}");
    assert!(text.find("\"alpha\"").unwrap() < text.find("\"mid\"").unwrap(), "{text}");
}
