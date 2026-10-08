// Config-node surface tests (the M6 removed-option wave): the port of Go's
// parsedcommandline_test.go "Config file specs are owned by the parsed
// command line" (GetMatchedFileSpec) + the tsconfigparsing.go config-syntax
// walks the wave ports (ForEachTsConfigPropArray / ForEachPropertyAssignment /
// GetTsConfigPropArrayElementValue / CreateDiagnosticForNodeInSourceFile).
//
// PORT notes:
//   - Go's test parses through `ParseJsonSourceFileConfigFileContent`; the
//     Rust tsoptions crate parses values from text (the AST→value conversion
//     is the deferred full AST-config wave), so the tests go through
//     `get_parsed_command_line_of_config_file_path` — the route that rides
//     the config AST alongside (same dataflow the program twin uses).
//   - Positions assert the config-text slice a diagnostic covers (Go's
//     `scanner.SkipTrivia(text, node.Loc.Pos())`..`node.End()` range), not
//     raw indexes — byte-stable against the JSON layout.

use tsc_ast::{NodeId, NodeStore};
use tsc_tspath::{rooted_file_path_from_absolute, CaseSensitivity};
use tsc_vfs::vfstest;

use tsc_tsoptions::tsconfigparsing::{
    for_each_property_assignment, for_each_ts_config_prop_array,
    get_ts_config_object_literal_expression, get_ts_config_prop_array_element_value,
    new_tsconfig_source_file_from_file_path,
};
use tsc_tsoptions::{create_diagnostic_for_node_in_source_file, get_parsed_command_line_of_config_file_path};

/// Go: `t.Run("Config file specs are owned by the parsed command line")` —
/// `GetMatchedFileSpec` returns the spec AS WRITTEN (the
/// before-${configDir}-substitution form), and the specs are owned by the
/// parsed command line (per-config-file specs, not shared on the source
/// file).
#[test]
fn test_config_file_specs_owned_by_parsed_command_line() {
    let config_file_name = rooted_file_path_from_absolute("/dev/tsconfig.json");
    let fs = vfstest::from_map(
        vec![(
            "/dev/tsconfig.json".to_string(),
            r#"{ "files": ["SRC/a.ts"] }"#.to_string(),
        )],
        CaseSensitivity::CaseSensitive,
    );
    let parsed = get_parsed_command_line_of_config_file_path(&config_file_name, fs.as_ref(), None);
    assert!(parsed.config_file.is_some(), "the config AST rides the result");
    assert_eq!(
        parsed.get_matched_file_spec(&rooted_file_path_from_absolute("/dev/SRC/a.ts")).as_string(),
        "SRC/a.ts"
    );
    // The non-listed file has no matched spec ("" — Go's zero FileSpec).
    assert_eq!(
        parsed.get_matched_file_spec(&rooted_file_path_from_absolute("/dev/other.ts")).as_string(),
        ""
    );
}

/// `GetMatchedIncludeSpec` — the written pattern for an include match, the
/// default-include flag for the `["**/*"]` default, "" without include.
#[test]
fn test_get_matched_include_spec() {
    let config_file_name = rooted_file_path_from_absolute("/dev/tsconfig.json");
    let fs = vfstest::from_map(
        vec![
            (
                "/dev/tsconfig.json".to_string(),
                r#"{ "include": ["src/*.ts"] }"#.to_string(),
            ),
            ("/dev/src/a.ts".to_string(), String::new()),
        ],
        CaseSensitivity::CaseSensitive,
    );
    let parsed = get_parsed_command_line_of_config_file_path(&config_file_name, fs.as_ref(), None);
    let (spec, is_default) = parsed.get_matched_include_spec(&rooted_file_path_from_absolute("/dev/src/a.ts"));
    assert_eq!(spec.as_string(), "src/*.ts");
    assert!(!is_default);
    let (spec, is_default) = parsed.get_matched_include_spec(&rooted_file_path_from_absolute("/dev/b.ts"));
    assert_eq!(spec.as_string(), "");
    assert!(!is_default);

    // The default-include arm: no files/include → ["**/*"] + isDefault.
    let fs = vfstest::from_map(
        vec![("/dev/tsconfig.json".to_string(), r#"{}"#.to_string())],
        CaseSensitivity::CaseSensitive,
    );
    let parsed = get_parsed_command_line_of_config_file_path(&config_file_name, fs.as_ref(), None);
    let (spec, is_default) = parsed.get_matched_include_spec(&rooted_file_path_from_absolute("/dev/src/a.ts"));
    assert_eq!(spec.as_string(), "**/*");
    assert!(is_default);
}

/// Go: the config-syntax walks — the root object literal, the
/// `compilerOptions` property (ForEachTsConfigPropArray + core.Identity),
/// the per-option property (ForEachPropertyAssignment), and the
/// files-array element (GetTsConfigPropArrayElementValue).
#[test]
fn test_config_syntax_walks() {
    let text = r#"{ "compilerOptions": { "target": "ES5" }, "files": ["a.ts"] }"#;
    let tsconfig = new_tsconfig_source_file_from_file_path(
        &rooted_file_path_from_absolute("/dev/tsconfig.json"),
        CaseSensitivity::CaseSensitive,
        text,
    );
    let source_file = tsconfig.lock_source_file();
    let source_file: &tsc_ast::SourceFile = &source_file;

    let root = get_ts_config_object_literal_expression(source_file).expect("root object literal");
    let compiler_options = for_each_ts_config_prop_array(
        Some(source_file),
        "compilerOptions",
        |_store, property| Some(property),
    )
    .expect("compilerOptions property");
    assert_eq!(
        for_each_property_assignment(source_file, Some(root), "compilerOptions", "", |_store, p| Some(p)),
        Some(compiler_options)
    );
    // A key that is absent yields None (Go's "no match" nil).
    assert_eq!(
        for_each_property_assignment(source_file, Some(root), "compilerOptions", "", |_store, p| {
            let _ = p;
            None::<NodeId>
        }),
        None
    );

    // The compilerOptions object literal + the per-option property.
    let initializer = source_file
        .node(compiler_options)
        .as_property_assignment()
        .expect("PropertyAssignment")
        .initializer;
    let object_literal = get_ts_config_object_literal_expression(source_file);
    let _ = object_literal;
    assert!(tsc_ast::is_object_literal_expression(source_file.node(initializer)));
    let target_property = for_each_property_assignment(
        source_file,
        Some(initializer),
        "target",
        "",
        |_store, p| Some(p),
    )
    .expect("target property");

    // The key2 alias arm matches either name (Go's variadic key2).
    assert_eq!(
        for_each_property_assignment(source_file, Some(initializer), "noMatch", "target", |_store, p| {
            Some(p)
        }),
        Some(target_property)
    );

    // The files-array element whose text equals the value.
    let element = get_ts_config_prop_array_element_value(Some(source_file), "files", "a.ts")
        .expect("files element");
    assert!(tsc_ast::is_string_literal(source_file.node(element)));
    assert_eq!(
        get_ts_config_prop_array_element_value(Some(source_file), "files", "b.ts"),
        None
    );
}

/// Go: `CreateDiagnosticForNodeInSourceFile` — SkipTrivia(pos)..end over the
/// config text, the exact range the removed-option diagnostics carry
/// (the removed-option arm positions TS5108 on the property INITIALIZER,
/// TS5102 on the NAME).
#[test]
fn test_create_diagnostic_for_node_in_source_file_positions() {
    let text = r#"{ "compilerOptions": { "target": "ES5", "downlevelIteration": true } }"#;
    let tsconfig = new_tsconfig_source_file_from_file_path(
        &rooted_file_path_from_absolute("/dev/tsconfig.json"),
        CaseSensitivity::CaseSensitive,
        text,
    );
    let source_file = tsconfig.lock_source_file();
    let source_file: &tsc_ast::SourceFile = &source_file;
    let compiler_options_object_literal = for_each_ts_config_prop_array(
        Some(source_file),
        "compilerOptions",
        |_store, property| {
            let initializer = source_file
                .node(property)
                .as_property_assignment()
                .expect("PropertyAssignment")
                .initializer;
            tsc_ast::is_object_literal_expression(source_file.node(initializer))
                .then_some(initializer)
        },
    )
    .expect("compilerOptions object literal");

    // TS5108 shape: positioned on the "ES5" initializer (onKey = false).
    let target_initializer = source_file
        .node(
            for_each_property_assignment(
                source_file,
                Some(compiler_options_object_literal),
                "target",
                "",
                |_store, p| Some(p),
            )
            .expect("target property"),
        )
        .as_property_assignment()
        .expect("PropertyAssignment")
        .initializer;
    let diagnostic = create_diagnostic_for_node_in_source_file(
        source_file,
        target_initializer,
        &tsc_diagnostics::OPTION_0_1_HAS_BEEN_REMOVED_PLEASE_REMOVE_IT_FROM_YOUR_CONFIGURATION,
        &["target".to_string(), "ES5".to_string()],
    );
    assert_eq!(diagnostic.code(), 5108);
    assert_eq!(diagnostic.message_args(), ["target", "ES5"]);
    assert_eq!(
        &text[diagnostic.pos() as usize..diagnostic.end() as usize],
        "\"ES5\""
    );

    // TS5102 shape: positioned on the "downlevelIteration" NAME (onKey = true).
    let downlevel_name = source_file
        .node(
            for_each_property_assignment(
                source_file,
                Some(compiler_options_object_literal),
                "downlevelIteration",
                "",
                |_store, p| Some(p),
            )
            .expect("downlevelIteration property"),
        )
        .name()
        .expect("PropertyAssignment name");
    let diagnostic = create_diagnostic_for_node_in_source_file(
        source_file,
        downlevel_name,
        &tsc_diagnostics::OPTION_0_HAS_BEEN_REMOVED_PLEASE_REMOVE_IT_FROM_YOUR_CONFIGURATION,
        &["downlevelIteration".to_string()],
    );
    assert_eq!(diagnostic.code(), 5102);
    assert_eq!(diagnostic.message_args(), ["downlevelIteration"]);
    assert_eq!(
        &text[diagnostic.pos() as usize..diagnostic.end() as usize],
        "\"downlevelIteration\""
    );
    // The diagnostic's file is the config source file (Go: the *ast.SourceFile).
    assert_eq!(diagnostic.file(), Some(source_file.as_node_id()));
}
