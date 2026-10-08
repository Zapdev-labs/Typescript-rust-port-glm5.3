//! Ported subset of `tsc/internal/parser/parser_test.go` plus a smoke test.
//!
//! Ported:
//!   * TestHeritageClauseElementKinds
//!   * TestParseStaticSourcePhaseImport
//!   * TestParseSourceAsImportEqualsBinding
//!   * TestParseInvalidStaticSourcePhaseImports
//!   * TestParseDynamicSourcePhaseImport (minus the SubtreeFacts assertion —
//!     the `subtreefacts.go` subsystem is deferred in the port, so there is
//!     no `SubtreeContainsDynamicImport` to check; the file-flag assertion
//!     covering the same information is kept)
//!   * TestParseEscapedDynamicImportPhase
//!   * TestSourceFilePositionMapWithNonASCIIStringLiteral
//!   * smoke test (parser-internal): small program → statement count/kinds,
//!     binary-expression child order
//!   * references smoke test: collectExternalModuleReferences import order
//!     and node:core Tristate precedence
//!
//! Not ported (with reasons):
//!   * TestJSDocImportTypeParentChain / TestJSDocTypeSourceSurvivesReparse /
//!     TestJSDocTypeSourcePropagatesToConstructedReparse — depend on
//!     `reparser.go` (ReparsedClones, GetReparsedNodeForNode,
//!     NodeFlagsReparsed), which is explicitly out of scope for this wave
//!     (see lib.rs PORT banner).
//!   * BenchmarkParse / FuzzParser — Go test-harness infrastructure
//!     (testrunner corpus walk, fuzzing) with no Rust counterpart here.

use tsc_ast::{
    is_call_expression, is_meta_property, node_text, Kind, NodeFlags, NodeId, NodeStore,
    SourceFile, SourceFileParseOptions,
};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_tspath::{rooted_file_path_from_normalized, CaseSensitivity};

use crate::parse_source_file;
use tsc_diagnostics::KEYWORDS_CANNOT_CONTAIN_ESCAPE_CHARACTERS;

// ────────────────────────────────────────────────────────────────────────────
// parsetestutil.go ports
// ────────────────────────────────────────────────────────────────────────────

/// Go: `parsetestutil.ParseTypeScript(text string, jsx bool)` with jsx=false.
fn parse_typescript(text: &str) -> SourceFile {
    let file_name = rooted_file_path_from_normalized("/main.ts");
    let path_key = CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    parse_source_file(
        SourceFileParseOptions {
            file_name,
            path_key,
            ..Default::default()
        },
        text,
        ScriptKind::TS,
    )
}

/// Go: `parsetestutil.CheckDiagnostics(t, file)`.
fn check_diagnostics(file: &SourceFile) {
    let diags = file.diagnostics();
    let rendered: Vec<String> = diags
        .iter()
        .map(|d| format!("{}: {}", d.message_key(), d.message_text()))
        .collect();
    assert!(
        diags.is_empty(),
        "unexpected parse diagnostics:\n{}",
        rendered.join("\n")
    );
}

fn statements_of(file: &SourceFile) -> Vec<NodeId> {
    file.payload()
        .statements
        .as_ref()
        .map(|l| l.nodes.to_vec())
        .unwrap_or_default()
}

// ────────────────────────────────────────────────────────────────────────────
// parser_test.go ports
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func TestHeritageClauseElementKinds`.
#[test]
fn heritage_clause_element_kinds() {
    let source_text = "
class C extends Base<number> implements Contract<string> {}
interface I extends Parent<boolean> {}
interface Invalid implements Recovery {}
interface MissingExtends extends A. {}
class MissingImplements implements B. {}
";
    let file = parse_typescript(source_text);
    let sf: &dyn NodeStore = &file;
    let statements = statements_of(&file);

    let class_decl = sf
        .node(statements[0])
        .as_class_declaration()
        .expect("ClassDeclaration data");
    let clauses = class_decl
        .heritage_clauses
        .as_ref()
        .expect("heritage clauses");
    let kind = |i: usize, j: usize| {
        let clause = sf
            .node(clauses.nodes[i])
            .as_heritage_clause()
            .expect("HeritageClause data");
        let types = clause.types.as_ref().expect("clause types");
        sf.node(types.nodes[j]).kind
    };
    assert_eq!(kind(0, 0), Kind::ExpressionWithTypeArguments);
    assert_eq!(kind(1, 0), Kind::TypeReference);

    let interface_decl = sf
        .node(statements[1])
        .as_interface_declaration()
        .expect("InterfaceDeclaration data");
    let clauses = interface_decl
        .heritage_clauses
        .as_ref()
        .expect("heritage clauses");
    let kind = |i: usize, j: usize| {
        let clause = sf
            .node(clauses.nodes[i])
            .as_heritage_clause()
            .expect("HeritageClause data");
        let types = clause.types.as_ref().expect("clause types");
        sf.node(types.nodes[j]).kind
    };
    assert_eq!(kind(0, 0), Kind::TypeReference);

    let invalid_interface_decl = sf
        .node(statements[2])
        .as_interface_declaration()
        .expect("InterfaceDeclaration data");
    let clauses = invalid_interface_decl
        .heritage_clauses
        .as_ref()
        .expect("heritage clauses");
    let kind = |i: usize, j: usize| {
        let clause = sf
            .node(clauses.nodes[i])
            .as_heritage_clause()
            .expect("HeritageClause data");
        let types = clause.types.as_ref().expect("clause types");
        sf.node(types.nodes[j]).kind
    };
    assert_eq!(kind(0, 0), Kind::ExpressionWithTypeArguments);

    let missing_extends_decl = sf
        .node(statements[3])
        .as_interface_declaration()
        .expect("InterfaceDeclaration data");
    let clauses = missing_extends_decl
        .heritage_clauses
        .as_ref()
        .expect("heritage clauses");
    let kind = |i: usize, j: usize| {
        let clause = sf
            .node(clauses.nodes[i])
            .as_heritage_clause()
            .expect("HeritageClause data");
        let types = clause.types.as_ref().expect("clause types");
        sf.node(types.nodes[j]).kind
    };
    assert_eq!(kind(0, 0), Kind::ExpressionWithTypeArguments);

    let missing_implements_decl = sf
        .node(statements[4])
        .as_class_declaration()
        .expect("ClassDeclaration data");
    let clauses = missing_implements_decl
        .heritage_clauses
        .as_ref()
        .expect("heritage clauses");
    let kind = |i: usize, j: usize| {
        let clause = sf
            .node(clauses.nodes[i])
            .as_heritage_clause()
            .expect("HeritageClause data");
        let types = clause.types.as_ref().expect("clause types");
        sf.node(types.nodes[j]).kind
    };
    assert_eq!(kind(0, 0), Kind::ExpressionWithTypeArguments);
}

/// Go: `func TestParseStaticSourcePhaseImport`.
#[test]
fn parse_static_source_phase_import() {
    struct Case {
        name: &'static str,
        source: &'static str,
        phase_modifier: Option<Kind>,
        binding_name: &'static str,
        has_attributes: bool,
    }
    let tests = [
        Case {
            name: "source phase",
            source: r#"import source a from "./a.wasm";"#,
            phase_modifier: Some(Kind::SourceKeyword),
            binding_name: "a",
            has_attributes: false,
        },
        Case {
            name: "source phase with import attributes",
            source: r#"import source a from "./a.wasm" with { type: "webassembly" };"#,
            phase_modifier: Some(Kind::SourceKeyword),
            binding_name: "a",
            has_attributes: true,
        },
        Case {
            name: "from as source phase binding",
            source: r#"import source from from "./module.js";"#,
            phase_modifier: Some(Kind::SourceKeyword),
            binding_name: "from",
            has_attributes: false,
        },
        Case {
            name: "source as ordinary default binding",
            source: r#"import source from "./module.js";"#,
            phase_modifier: None, // Go: ast.KindUnknown
            binding_name: "source",
            has_attributes: false,
        },
        Case {
            name: "source as ordinary default binding with named imports",
            source: r#"import source, { value } from "./module.js";"#,
            phase_modifier: None,
            binding_name: "source",
            has_attributes: false,
        },
        Case {
            name: "escaped source as ordinary default binding",
            source: r#"import s\u006furce from "./module.js";"#,
            phase_modifier: None,
            binding_name: "source",
            has_attributes: false,
        },
        Case {
            name: "escaped defer as ordinary default binding",
            source: r#"import d\u0065fer from "./module.js";"#,
            phase_modifier: None,
            binding_name: "defer",
            has_attributes: false,
        },
    ];

    for test in &tests {
        let file = parse_typescript(test.source);
        check_diagnostics(&file);
        let sf: &dyn NodeStore = &file;
        let statements = statements_of(&file);
        assert_eq!(statements.len(), 1, "{}", test.name);

        let statement = sf.node(statements[0]);
        assert!(statement.kind == Kind::ImportDeclaration, "{}", test.name);

        let declaration = statement
            .as_import_declaration()
            .expect("ImportDeclaration data");
        assert!(declaration.import_clause.is_some(), "{}", test.name);

        let clause_id = declaration.import_clause.unwrap();
        let clause = sf
            .node(clause_id)
            .as_import_clause()
            .expect("ImportClause data");
        assert_eq!(clause.phase_modifier, test.phase_modifier, "{}", test.name);
        let name = clause.name.expect(test.name);
        assert_eq!(node_text(sf, name), test.binding_name, "{}", test.name);
        assert_eq!(
            declaration.attributes.is_some(),
            test.has_attributes,
            "{}",
            test.name
        );
    }
}

/// Go: `func TestParseSourceAsImportEqualsBinding`.
#[test]
fn parse_source_as_import_equals_binding() {
    let file = parse_typescript(r#"import source = require("./module.js");"#);
    check_diagnostics(&file);
    let sf: &dyn NodeStore = &file;
    let statements = statements_of(&file);
    assert_eq!(statements.len(), 1);

    let statement = sf.node(statements[0]);
    assert_eq!(statement.kind, Kind::ImportEqualsDeclaration);

    let name = statement
        .as_import_equals_declaration()
        .expect("ImportEqualsDeclaration data")
        .name;
    assert_eq!(node_text(sf, name), "source");
}

/// Go: `func TestParseInvalidStaticSourcePhaseImports`.
#[test]
fn parse_invalid_static_source_phase_imports() {
    struct Case {
        source: &'static str,
        has_name: bool,
        has_named_bindings: bool,
    }
    let tests = [
        Case {
            source: r#"import source "./a.js";"#,
            has_name: false,
            has_named_bindings: false,
        },
        Case {
            source: r#"import source * as a from "./a.js";"#,
            has_name: false,
            has_named_bindings: true,
        },
        Case {
            source: r#"import source { a } from "./a.js";"#,
            has_name: false,
            has_named_bindings: true,
        },
        Case {
            source: r#"import source a, { b } from "./a.js";"#,
            has_name: true,
            has_named_bindings: true,
        },
    ];

    for test in &tests {
        let file = parse_typescript(test.source);
        check_diagnostics(&file);
        let sf: &dyn NodeStore = &file;
        let statements = statements_of(&file);
        assert_eq!(statements.len(), 1);

        let statement = sf.node(statements[0]);
        assert_eq!(statement.kind, Kind::ImportDeclaration);

        let declaration = statement
            .as_import_declaration()
            .expect("ImportDeclaration data");
        let clause_id = declaration.import_clause.expect("ImportClause");
        let clause = sf
            .node(clause_id)
            .as_import_clause()
            .expect("ImportClause data");
        assert_eq!(clause.phase_modifier, Some(Kind::SourceKeyword));
        assert_eq!(clause.name.is_some(), test.has_name);
        assert_eq!(clause.named_bindings.is_some(), test.has_named_bindings);
    }
}

/// Go: `func TestParseDynamicSourcePhaseImport`.
#[test]
fn parse_dynamic_source_phase_import() {
    let file = parse_typescript(r#"import.source("./a.wasm", { with: { type: "webassembly" } });"#);
    check_diagnostics(&file);
    let sf: &dyn NodeStore = &file;
    let statements = statements_of(&file);
    assert_eq!(statements.len(), 1);

    let statement = sf.node(statements[0]);
    assert_eq!(statement.kind, Kind::ExpressionStatement);

    let call_id = statement
        .as_expression_statement()
        .expect("ExpressionStatement data")
        .expression;
    let call = sf.node(call_id);
    assert!(is_call_expression(call));
    // Go: ast.IsImportCall — the callee is `import.source`.
    let meta_property_id = call
        .as_call_expression()
        .expect("CallExpression data")
        .expression;
    assert!(is_meta_property(sf.node(meta_property_id)));

    let meta_property = sf
        .node(meta_property_id)
        .as_meta_property()
        .expect("MetaProperty data");
    assert_eq!(meta_property.keyword_token, Kind::ImportKeyword);
    assert_eq!(node_text(sf, meta_property_id), "source");
    let arguments = call
        .as_call_expression()
        .expect("CallExpression data")
        .arguments
        .as_ref()
        .expect("arguments")
        .nodes
        .len();
    assert_eq!(arguments, 2);

    let file_flags = sf.node(file.as_node_id()).flags;
    assert!(file_flags.intersects(NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT));
    assert!(!file_flags.intersects(NodeFlags::POSSIBLY_CONTAINS_IMPORT_META));
    assert!(file.payload().external_module_indicator.get().is_none());

    // Go additionally asserts `call.SubtreeFacts()&SubtreeContainsDynamicImport != 0`;
    // the subtreefacts.go subsystem is deferred in the port (see banner).
}

/// Go: `func TestParseEscapedDynamicImportPhase`.
#[test]
fn parse_escaped_dynamic_import_phase() {
    let tests = [
        (r#"import.d\u0065fer("./a.js");"#, "defer"),
        (r#"import.s\u006furce("./a.wasm");"#, "source"),
    ];

    for &(source, phase_name) in &tests {
        let file = parse_typescript(source);
        let sf: &dyn NodeStore = &file;
        let diagnostics = file.diagnostics();
        assert_eq!(diagnostics.len(), 1, "{phase_name}");
        assert_eq!(
            diagnostics[0].code(),
            KEYWORDS_CANNOT_CONTAIN_ESCAPE_CHARACTERS.code()
        );

        let statements = statements_of(&file);
        assert_eq!(statements.len(), 1);

        let statement = sf.node(statements[0]);
        assert_eq!(statement.kind, Kind::ExpressionStatement);

        let call_id = statement
            .as_expression_statement()
            .expect("ExpressionStatement data")
            .expression;
        let call = sf.node(call_id);
        assert!(is_call_expression(call));
        let meta_property_id = call
            .as_call_expression()
            .expect("CallExpression data")
            .expression;
        assert!(is_meta_property(sf.node(meta_property_id)));
        assert_eq!(node_text(sf, meta_property_id), phase_name);

        let file_flags = sf.node(file.as_node_id()).flags;
        assert!(file_flags.intersects(NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT));
        assert!(!file_flags.intersects(NodeFlags::POSSIBLY_CONTAINS_IMPORT_META));
    }
}

/// Go: `func TestSourceFilePositionMapWithNonASCIIStringLiteral`.
#[test]
fn source_file_position_map_with_non_ascii_string_literal() {
    let source_text = "const x = \"─\";

namespace N {
  export const y = x;
}
";
    let file = parse_typescript(source_text);

    let position_map = file.get_position_map();
    assert!(!position_map.is_ascii_only());
    let after_box_drawing_character = source_text.find('─').unwrap() + '─'.len_utf8();
    assert_eq!(
        position_map.utf8_to_utf16(after_box_drawing_character as i32),
        (after_box_drawing_character - 2) as i32
    );
    assert_eq!(
        position_map.utf8_to_utf16(source_text.len() as i32),
        (source_text.len() - 2) as i32
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Smoke tests (parser-internal)
// ────────────────────────────────────────────────────────────────────────────

/// Parses a small program end-to-end and checks the statement inventory and
/// binary-expression associativity (`1 + 2 * 3` nests `*` under `+`).
#[test]
fn smoke_test_parses_small_program() {
    let file = parse_typescript(
        "const x = 1 + 2 * 3;\nfunction f(a: number): string { return `${a}`; }\n",
    );
    check_diagnostics(&file);
    let sf: &dyn NodeStore = &file;
    let statements = statements_of(&file);
    assert_eq!(statements.len(), 2);
    assert_eq!(sf.node(statements[0]).kind, Kind::VariableStatement);
    assert_eq!(sf.node(statements[1]).kind, Kind::FunctionDeclaration);

    // VariableStatement → declarations[0] → initializer = BinaryExpression(+)
    let declaration_list_id = sf
        .node(statements[0])
        .as_variable_statement()
        .expect("VariableStatement data")
        .declaration_list;
    let declarations = sf
        .node(declaration_list_id)
        .as_variable_declaration_list()
        .expect("VariableDeclarationList data")
        .declarations
        .as_ref()
        .expect("declarations")
        .nodes
        .to_vec();
    assert_eq!(declarations.len(), 1);
    let declaration = sf
        .node(declarations[0])
        .as_variable_declaration()
        .expect("VariableDeclaration data");
    assert_eq!(node_text(sf, declaration.name), "x");
    let initializer = declaration.initializer.expect("initializer");

    let plus = sf.node(initializer);
    assert_eq!(plus.kind, Kind::BinaryExpression);
    let plus_data = plus.as_binary_expression().expect("BinaryExpression data");
    assert_eq!(sf.node(plus_data.operator_token).kind, Kind::PlusToken);
    assert_eq!(sf.node(plus_data.left).kind, Kind::NumericLiteral);

    // `2 * 3` nests under the `+` (precedence).
    let times = sf.node(plus_data.right);
    assert_eq!(times.kind, Kind::BinaryExpression);
    let times_data = times.as_binary_expression().expect("BinaryExpression data");
    assert_eq!(sf.node(times_data.operator_token).kind, Kind::AsteriskToken);
    assert_eq!(sf.node(times_data.left).kind, Kind::NumericLiteral);
    assert_eq!(sf.node(times_data.right).kind, Kind::NumericLiteral);

    // Node/text accounting ran.
    let payload = file.payload();
    assert!(payload.node_count > 0);
    assert!(payload.text_count > 0);
}

/// `collectExternalModuleReferences` port: import ordering (statement walk,
/// then the dynamic-import text scan) and `node:core` Tristate precedence.
#[test]
fn collect_references_import_order_and_node_core_precedence() {
    let file = parse_typescript(
        "import a from \"./m.js\";\nimport b from \"node:fs\";\nconst p = import(\"./dyn.js\");\n",
    );
    check_diagnostics(&file);
    let sf: &dyn NodeStore = &file;

    let payload = file.payload();
    assert_eq!(payload.imports.len(), 3);
    // Statement walk order first…
    assert_eq!(node_text(sf, payload.imports[0]), "./m.js");
    assert_eq!(node_text(sf, payload.imports[1]), "node:fs");
    // …then the dynamic import found by the text scan.
    assert_eq!(node_text(sf, payload.imports[2]), "./dyn.js");

    // `node:fs` is not an exclusively-prefixed module, so the presence of the
    // `node:` prefix wins immediately (Go: core.TSTrue).
    assert_eq!(payload.uses_uri_style_node_core_modules, Tristate::TSTrue);
}

/// `collectExternalModuleReferences` port (ambient modules): an unprefixed
/// node core module resolves UsesUriStyleNodeCoreModules to TSFalse.
#[test]
fn collect_references_unprefixed_node_core_module() {
    let file = parse_typescript("import a from \"fs\";\n");
    check_diagnostics(&file);
    let payload = file.payload();
    assert_eq!(payload.uses_uri_style_node_core_modules, Tristate::TSFalse);
    assert_eq!(payload.imports.len(), 1);
}
