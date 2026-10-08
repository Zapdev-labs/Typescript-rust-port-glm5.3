//! Ported subset of the `tsc/internal/parser` reparser tests:
//! `tsc/internal/parser/parser_test.go`
//!   * TestJSDocImportTypeParentChain
//!   * TestJSDocTypeSourceSurvivesReparse
//!   * TestJSDocTypeSourcePropagatesToConstructedReparse
//!     (minus the `GetTokenPosOfNode` assertion — that helper is
//!     scanner-package surface (`scanner.go:2526`) not yet ported; the
//!     flags/text assertions covering the same reparsed node are kept)
//!
//! Plus end-to-end coverage for `ast.GetAssignmentDeclarationKind` (Go has
//! no direct unit test for it — it is exercised via the binder; the port
//! classifies parsed JS assignment expressions directly).

use tsc_ast::{
    get_assignment_declaration_kind, get_reparsed_node_for_node, get_source_file_of_node,
    is_function_declaration, is_js_type_alias_declaration, JSDeclarationKind, Kind, NodeFlags,
    NodeId, NodeStore, SourceFile, SourceFileParseOptions,
};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::options_generated::NewLineKind;
use tsc_tspath::{rooted_file_path_from_normalized, CaseSensitivity};

use crate::parse_source_file;

/// Go: `ast.SourceFileParseOptions{FileName: "/index.js", PathKey: "/index.js"}`
/// + `parser.ParseSourceFile(opts, sourceText, core.ScriptKindJS)`.
fn parse_javascript(text: &str) -> SourceFile {
    let file_name = rooted_file_path_from_normalized("/index.js");
    let path_key = CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    parse_source_file(
        SourceFileParseOptions {
            file_name,
            path_key,
            ..Default::default()
        },
        text,
        ScriptKind::JS,
    )
}

fn statements_of(file: &SourceFile) -> Vec<NodeId> {
    file.payload()
        .statements
        .as_ref()
        .map(|l| l.nodes.to_vec())
        .unwrap_or_default()
}

fn lf_join(parts: &[&str]) -> String {
    // Go: strings.Join(parts, core.NewLineKindLF.GetNewLineCharacter()).
    parts.join(NewLineKind::LF.get_new_line_character())
}

// ────────────────────────────────────────────────────────────────────────────
// parser_test.go ports
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func TestJSDocImportTypeParentChain(t *testing.T)`.
#[test]
fn jsdoc_import_type_parent_chain() {
    let source_text = r#"test("", async function () {
  ;(/** @type {typeof import("a")} */ ({}))
})

test("", async function () {
  ;(/** @type {typeof import("a")} */ a)
})

test("", async function () {
  (/** @type {typeof import("a")} */ ({}))
  ;(/** @type {typeof import("a")} */ ({}))
})

test("", async function () {
  (/** @type {typeof import("a")} */ a)
  ;(/** @type {typeof import("a")} */ a)
})

test("", async function () {
  (/** @type {typeof import("a")} */ ({}))
  ;(/** @type {typeof import("a")} */ ({}))
})
"#;
    let file = parse_javascript(source_text);
    let sf: &dyn NodeStore = &file;

    let reparsed_clones = file.reparsed_clones();
    // PORT (not in Go): sanity-guard that the reparse machinery actually ran
    // — the Go test would pass vacuously on an empty clone list, which in
    // this port would signal a wiring regression rather than a pass.
    assert!(
        !reparsed_clones.is_empty(),
        "expected the JSDoc @type reparses to register reparsed clones"
    );

    for i in 1..reparsed_clones.len() {
        let (a, b) = (reparsed_clones[i - 1], reparsed_clones[i]);
        let (pos_a, end_a, kind_a) = {
            let n = sf.node(a);
            (n.pos(), n.end(), n.kind)
        };
        let (pos_b, end_b, kind_b) = {
            let n = sf.node(b);
            (n.pos(), n.end(), n.kind)
        };
        assert!(
            !(pos_a == pos_b && end_a == end_b && kind_a == kind_b),
            "duplicate ReparsedClones at [{i}] and [{i}]: {} pos={pos_a} end={end_a}",
            kind_a.name()
        );
    }

    for &imp in file.imports() {
        let reparsed = get_reparsed_node_for_node(sf, imp);
        assert!(
            get_source_file_of_node(sf, reparsed).is_some(),
            "reparsed import at pos={} has broken parent chain",
            sf.node(imp).pos()
        );
    }
}

/// Go: `func TestJSDocTypeSourceSurvivesReparse(t *testing.T)`.
#[test]
fn jsdoc_type_source_survives_reparse() {
    let source_text = r#"/**
 * @typedef {(
 *   "a" |
 *   "b"
 * )[]} T
 */
const value = 0;"#;
    let file = parse_javascript(source_text);
    let sf: &dyn NodeStore = &file;

    let mut type_alias: Option<NodeId> = None;
    for statement in statements_of(&file) {
        if is_js_type_alias_declaration(sf.node(statement)) {
            type_alias = Some(statement);
            break;
        }
    }
    let type_alias = type_alias.expect("expected a JSTypeAliasDeclaration statement");

    // Go: `typeAlias.JSDoc(file)` — the eager (non-lazy) JSDoc cache lookup.
    let js_docs = file
        .payload()
        .jsdoc_cache
        .as_ref()
        .and_then(|cache| cache.get(&type_alias))
        .cloned()
        .expect("expected the type alias to carry its JSDoc");
    assert_eq!(js_docs.len(), 1);
    let tags = sf
        .node(js_docs[0])
        .as_jsdoc()
        .expect("JSDoc data")
        .tags
        .as_ref()
        .expect("JSDoc tags");
    assert_eq!(tags.nodes.len(), 1);

    let type_expression = sf
        .node(tags.nodes[0])
        .as_jsdoc_typedef_tag()
        .expect("JSDocTypedefTag data")
        .type_expression
        .expect("typedef type expression");

    let expected = lf_join(&["(", "\"a\" |", "\"b\"", ")[]"]);
    // {name: "original", node: typeExpression.Type()}
    let original = sf
        .node(type_expression)
        .as_jsdoc_type_expression()
        .expect("JSDocTypeExpression data")
        .type_;
    assert_eq!(tsc_scanner::utilities::get_text_of_node(sf, original), expected);
    // {name: "reparsed", node: typeAlias.Type()}
    let reparsed = sf
        .node(type_alias)
        .as_type_alias_declaration()
        .expect("TypeAliasDeclaration data")
        .type_;
    assert_eq!(tsc_scanner::utilities::get_text_of_node(sf, reparsed), expected);
}

/// Go: `func TestJSDocTypeSourcePropagatesToConstructedReparse(t *testing.T)`.
#[test]
fn jsdoc_type_source_propagates_to_constructed_reparse() {
    let source_text = r#"/**
 * @param {{
 *   value: string
 * }} options
 */
function foo(options) {}"#;
    let file = parse_javascript(source_text);
    let sf: &dyn NodeStore = &file;

    let function = statements_of(&file)[0];
    assert!(is_function_declaration(sf.node(function)));

    let parameters = sf
        .node(function)
        .as_function_declaration()
        .expect("FunctionDeclaration data")
        .parameters
        .as_ref()
        .expect("parameter list");
    assert_eq!(parameters.nodes.len(), 1);

    let type_node = sf
        .node(parameters.nodes[0])
        .as_parameter_declaration()
        .expect("ParameterDeclaration data")
        .type_
        .expect("parameter type");
    assert!(
        sf.node(type_node).flags.intersects(NodeFlags::REPARSED),
        "expected the constructed parameter type to carry NodeFlagsReparsed"
    );

    let expected = lf_join(&["{", "value: string", "}"]);
    assert_eq!(tsc_scanner::utilities::get_text_of_node(sf, type_node), expected);
    // PORT: Go additionally asserts
    // `scanner.GetTokenPosOfNode(typeNode, file, false) ==
    // strings.Index(sourceText, "{{")+1` — GetTokenPosOfNode
    // (scanner/scanner.go:2526) is scanner-package surface not yet ported;
    // deferred with it.
}

// ────────────────────────────────────────────────────────────────────────────
// ast.GetAssignmentDeclarationKind coverage (utilities.go:1547 — Go has no
// direct unit test; the binder exercises it. The port classifies parsed JS
// assignment expressions directly, one per JSDeclarationKind variant.)
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn assignment_declaration_kind_mirrors_go() {
    let source_text = r#"module.exports = x;
module.exports = exports;
exports.name = 1;
this.count = 1;
F.member = 1;
F["member"] = 1;
Object.defineProperty(x, "name", { value: 1 });
Object.defineProperty(exports, "name", { value: 1 });
a + b;
plain = 1;
"#;
    let file = parse_javascript(source_text);
    let sf: &dyn NodeStore = &file;

    let expected = [
        JSDeclarationKind::ModuleExports,   // module.exports = x
        JSDeclarationKind::Property,        // module.exports = exports (not ModuleExports: RHS is `exports`)
        JSDeclarationKind::ExportsProperty, // exports.name = 1
        JSDeclarationKind::ThisProperty,    // this.count = 1
        JSDeclarationKind::Property,        // F.member = 1
        JSDeclarationKind::Property,        // F["member"] = 1
        JSDeclarationKind::ObjectDefinePropertyValue, // Object.defineProperty(x, ...)
        JSDeclarationKind::ObjectDefinePropertyExports, // Object.defineProperty(exports, ...)
        JSDeclarationKind::None,            // a + b (not an assignment)
        JSDeclarationKind::None,            // plain = 1 (target is not an access expression)
    ];
    let statements = statements_of(&file);
    let expression_statements: Vec<NodeId> = statements
        .iter()
        .copied()
        .filter(|&s| sf.node(s).kind == Kind::ExpressionStatement)
        .collect();
    assert_eq!(expression_statements.len(), expected.len());
    for (&statement, &want) in expression_statements.iter().zip(expected.iter()) {
        let expression = sf
            .node(statement)
            .as_expression_statement()
            .expect("ExpressionStatement data")
            .expression;
        let kind = get_assignment_declaration_kind(sf, expression);
        assert_eq!(kind, want, "statement at pos {}", sf.node(statement).pos());
    }
}

/// Go: `type JSDeclarationKind int` iota order — the ordinals are load
/// bearing (they appear in binder switch tables).
#[test]
fn assignment_declaration_kind_ordinals_mirror_go() {
    assert_eq!(JSDeclarationKind::None as i32, 0);
    assert_eq!(JSDeclarationKind::ModuleExports as i32, 1);
    assert_eq!(JSDeclarationKind::ExportsProperty as i32, 2);
    assert_eq!(JSDeclarationKind::ThisProperty as i32, 3);
    assert_eq!(JSDeclarationKind::Property as i32, 4);
    assert_eq!(JSDeclarationKind::ObjectDefinePropertyValue as i32, 5);
    assert_eq!(JSDeclarationKind::ObjectDefinePropertyExports as i32, 6);
    assert_eq!(
        JSDeclarationKind::default(),
        JSDeclarationKind::None,
        "Go's zero value is JSDeclarationKindNone"
    );
}
