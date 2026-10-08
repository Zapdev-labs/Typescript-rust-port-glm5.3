// Ported from tsc/internal/scanner/scanner_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   &ast.Node{...} literals → nodes allocated into a SourceFile arena
//                             (SPEC §5.1); `Parent` becomes a `Cell<NodeId>`
//                             set explicitly where the Go literal sets it
//   gotest assert.Equal     → assert_eq!
//   t.Parallel()/t.Run      → dropped (cargo runs #[test]s on a thread pool)

use std::borrow::Cow;
use std::cell::Cell;

use tsc_ast::{
    Identifier, JSDocAllType, JSDocParameterOrPropertyTag, JSDocTypeExpression, Kind, Node,
    NodeData, NodeFlags, NodeId, SourceFile, SourceFileParseOptions, TypeLiteralNode,
    TypeReferenceNode,
};
use tsc_core::text::TextRange;
use tsc_stringutil as stringutil;

use crate::scanner::{string_to_token, token_to_text, Scanner};
use crate::utilities::{
    get_text_of_node_from_source_text, is_jsdoc_type_expression_or_child,
    normalize_jsdoc_type_source_text,
};

fn make_node(kind: Kind, flags: NodeFlags, loc: TextRange, data: NodeData) -> Node {
    Node {
        kind,
        flags,
        loc,
        id: Cell::new(0),
        parent: Cell::new(NodeId::NONE),
        data,
    }
}

fn new_source_file(text: &str) -> SourceFile {
    // file_id 0 is reserved for the SourceFile node itself (nodes[0]);
    // tests use a distinct id so the arena asserts stay honest.
    SourceFile::new(
        7,
        SourceFileParseOptions::default(),
        text,
        None,
        None,
    )
}

fn node_id(source_file: &SourceFile, index: u64) -> NodeId {
    NodeId::new(source_file.file_id(), index)
}

/// Go: `func TestScanStringPreservesLoneSurrogates(t *testing.T)`.
#[test]
fn test_scan_string_preserves_lone_surrogates() {
    let mut s = Scanner::new();
    s.set_text("\"🦀\\ud7ff\\ud800\\ud801\\uD83E\\uDD80\"");
    assert_eq!(s.scan(), Kind::StringLiteral);
    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice("🦀".as_bytes());
    expected.extend_from_slice(&stringutil::encode_js_string_rune(0xD7FF));
    expected.extend_from_slice(&stringutil::encode_js_string_rune(0xD800));
    expected.extend_from_slice(&stringutil::encode_js_string_rune(0xD801));
    expected.extend_from_slice("🦀".as_bytes());
    assert_eq!(s.token_value(), expected.as_slice());
}

/// Go: `func TestNormalizeJSDocTypeSourceText(t *testing.T)`.
#[test]
fn test_normalize_jsdoc_type_source_text() {
    struct Case {
        name: &'static str,
        text: &'static str,
        expected_lines: Vec<&'static str>,
    }
    let tests = [
        Case {
            name: "single line",
            text: " \t* \tFoo",
            expected_lines: vec!["Foo"],
        },
        Case {
            name: "ECMAScript line breaks",
            text: "Foo\r\n * Bar\r\t* Baz\u{2028} * Qux\u{2029}* Quux",
            expected_lines: vec!["Foo", "Bar", "Baz", "Qux", "Quux"],
        },
        Case {
            name: "blank and trailing lines",
            text: "Foo\r\n *\r\n",
            expected_lines: vec!["Foo", "", ""],
        },
        Case {
            name: "line without marker",
            text: "Foo\n  Bar",
            expected_lines: vec!["Foo", "Bar"],
        },
        Case {
            name: "only leading marker",
            text: "**Foo",
            expected_lines: vec!["*Foo"],
        },
    ];

    for test in tests {
        let expected = test.expected_lines.join("\n");
        assert_eq!(
            normalize_jsdoc_type_source_text(test.text),
            expected,
            "case {}",
            test.name
        );
    }
}

/// Go: `func TestIsJSDocTypeExpressionOrChild(t *testing.T)`.
#[test]
fn test_is_jsdoc_type_expression_or_child() {
    let mut sf = new_source_file("");

    let alloc = |sf: &mut SourceFile, node: Node| -> NodeId {
        let index = sf.nodes.len() as u64;
        sf.nodes.push(node);
        node_id(sf, index)
    };

    let js_doc_type = alloc(
        &mut sf,
        make_node(
            Kind::TypeReference,
            NodeFlags::JSDOC,
            TextRange::undefined(),
            NodeData::TypeReferenceNode(Box::new(TypeReferenceNode {
                type_arguments: None,
                type_name: NodeId::NONE,
            })),
        ),
    );
    let js_doc_type_child = alloc(
        &mut sf,
        make_node(
            Kind::Identifier,
            NodeFlags::JSDOC,
            TextRange::undefined(),
            NodeData::Identifier(Box::new(Identifier {
                flow_node: None,
                text: "T".into(),
            })),
        ),
    );
    sf.nodes[js_doc_type_child.local_index() as usize]
        .parent
        .set(js_doc_type);

    let reparsed_type = alloc(
        &mut sf,
        make_node(
            Kind::TypeLiteral,
            NodeFlags::REPARSED,
            TextRange::undefined(),
            NodeData::TypeLiteralNode(Box::new(TypeLiteralNode {
                symbol: None,
                members: None,
            })),
        ),
    );
    let reparsed_type_child = alloc(
        &mut sf,
        make_node(
            Kind::Identifier,
            NodeFlags::REPARSED,
            TextRange::undefined(),
            NodeData::Identifier(Box::new(Identifier {
                flow_node: None,
                text: "T".into(),
            })),
        ),
    );
    sf.nodes[reparsed_type_child.local_index() as usize]
        .parent
        .set(reparsed_type);

    let ordinary_type = alloc(
        &mut sf,
        make_node(
            Kind::TypeReference,
            NodeFlags::NONE,
            TextRange::undefined(),
            NodeData::TypeReferenceNode(Box::new(TypeReferenceNode {
                type_arguments: None,
                type_name: NodeId::NONE,
            })),
        ),
    );
    let js_doc_tag = alloc(
        &mut sf,
        make_node(
            Kind::JSDocParameterTag,
            NodeFlags::JSDOC,
            TextRange::undefined(),
            NodeData::JSDocParameterOrPropertyTag(Box::new(JSDocParameterOrPropertyTag {
                tag_name: NodeId::NONE,
                comment: None,
                name: NodeId::NONE,
                is_bracketed: false,
                type_expression: None,
                is_name_first: false,
            })),
        ),
    );
    let js_doc_tag_child = alloc(
        &mut sf,
        make_node(
            Kind::Identifier,
            NodeFlags::JSDOC,
            TextRange::undefined(),
            NodeData::Identifier(Box::new(Identifier {
                flow_node: None,
                text: "param".into(),
            })),
        ),
    );
    sf.nodes[js_doc_tag_child.local_index() as usize]
        .parent
        .set(js_doc_tag);

    let jsdoc_type_expression = alloc(
        &mut sf,
        make_node(
            Kind::JSDocTypeExpression,
            NodeFlags::NONE,
            TextRange::undefined(),
            NodeData::JSDocTypeExpression(Box::new(JSDocTypeExpression {
                type_: NodeId::NONE,
            })),
        ),
    );

    let cases: Vec<(String, NodeId, bool)> = vec![
        ("type expression".to_string(), jsdoc_type_expression, true),
        ("JSDoc type".to_string(), js_doc_type, true),
        ("JSDoc type child".to_string(), js_doc_type_child, true),
        ("reparsed type".to_string(), reparsed_type, true),
        ("reparsed type child".to_string(), reparsed_type_child, true),
        ("ordinary type".to_string(), ordinary_type, false),
        ("other JSDoc child".to_string(), js_doc_tag_child, false),
    ];

    for (name, node, expected) in cases {
        assert_eq!(is_jsdoc_type_expression_or_child(&sf, node), expected, "case {}", name);
    }
    // The JSDoc tag itself is in the Go table as a non-type case via its child
    // above; the tag node is also directly expected-false (it has no parent
    // chain containing a type node).
    assert!(!is_jsdoc_type_expression_or_child(&sf, js_doc_tag));
}

/// Go: `func TestGetTextOfNodeFromJSDocTypePreservesAsteriskType(t *testing.T)`.
#[test]
fn test_get_text_of_node_from_jsdoc_type_preserves_asterisk_type() {
    let source_text = "\n * *";
    let mut sf = new_source_file(source_text);

    let index = sf.nodes.len() as u64;
    sf.nodes.push(make_node(
        Kind::JSDocAllType,
        NodeFlags::JSDOC,
        TextRange::new(0, source_text.len() as i32),
        NodeData::JSDocAllType(Box::new(JSDocAllType {})),
    ));
    let node = node_id(&sf, index);

    let text: Cow<'_, str> =
        get_text_of_node_from_source_text(source_text, &sf, node, false /* include_trivia */);
    assert_eq!(&*text, "*");
}

/// Go: `func TestScanSourceKeyword(t *testing.T)`.
#[test]
fn test_scan_source_keyword() {
    let mut s = Scanner::new();
    s.set_text("source sourceValue");

    assert_eq!(s.scan(), Kind::SourceKeyword);
    assert_eq!(token_to_text(Kind::SourceKeyword), "source");
    assert_eq!(string_to_token("source"), Kind::SourceKeyword);
    assert_eq!(s.scan(), Kind::Identifier);
    assert_eq!(s.token_value(), b"sourceValue".as_ref());
}
