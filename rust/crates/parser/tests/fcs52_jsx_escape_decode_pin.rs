//! FCS52 parser pin (full-corpus check sweep, micro-tranche 52): the
//! unicodeEscapesInJsxtags cluster's parse-level half — the scanner and
//! parser ALREADY decode JSX identifier escapes exactly like Go
//! (`ScanJsxIdentifier` reuses `scanIdentifierParts` "to ensure Unicode
//! escapes are handled", scanner.go:1318-1332; the `\` arm of `Scan` routes
//! through `scanIdentifier`'s escape branch, scanner.go:1498-1529). The
//! raw-escape residual of the record lived in the CHECKER's
//! `get_text_of_node` misuse (pinned at the checker/record level); this pin
//! guards the decode so a scanner/parser regression can't silently revive
//! the raw text.
//!
//! ("FCS tranche 52" PORT notes in-tree.)

use tsc_ast::{NodeId, NodeStore, SourceFile, SourceFileParseOptions};
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::{CaseSensitivity, rooted_file_path_from_absolute};

fn parse_snippet(text: &str) -> SourceFile {
    let file_name = rooted_file_path_from_absolute("/probe.tsx");
    let path_key = CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    let opts = SourceFileParseOptions {
        file_name,
        path_key,
        external_module_indicator_options: Default::default(),
    };
    parse_source_file(opts, text, ScriptKind::TSX)
}

/// Every identifier of the parsed snippet, in walk order.
fn identifiers(sf: &SourceFile) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = vec![sf.as_node_id()];
    while let Some(id) = stack.pop() {
        if id.is_none() {
            continue;
        }
        let node = NodeStore::node(sf, id);
        if let Some(ident) = node.as_identifier() {
            out.push(ident.text.to_string());
        }
        node.for_each_child(&mut |child| {
            stack.push(child);
            false
        });
    }
    out
}

/// The corpus record's tag/attribute shapes (unicodeEscapesInJsxtags.tsx):
/// the identifier TEXT must be the DECODED name ("a", "a-b", "a-c",
/// "Compa", "data-video"), never the raw escape — Go's
/// `parseIdentifierNameErrorOnUnicodeEscapeSequence` reports the
/// 17021 escape error but still builds the identifier from the scanner's
/// decoded `tokenValue` (parser.go:5893-5898), and the checker then
/// resolves the decoded text against JSX.IntrinsicElements.
#[test]
fn fcs52_jsx_escape_decode_pin() {
    let cases: &[(&str, &[&str])] = &[
        // `<\u0061></a>` — decoded tag name "a" (closing "a" already raw).
        ("const x = <\\u0061></a>;", &["a", "a", "x"]),
        // `<\u0061-b></a-b>` — the escape plus the JSX `-` continuation.
        ("const x = <\\u0061-b></a-b>;", &["a-b", "a-b", "x"]),
        // `<a-\u0063></a-c>` — escape in the continuation part.
        ("const x = <a-\\u0063></a-c>;", &["a-c", "a-c", "x"]),
        // The extended `\u{...}` escape form.
        ("const x = <\\u{0061}-b></a-b>;", &["a-b", "a-b", "x"]),
        // Capitalized component names keep their case.
        ("const x = <Comp\\u0061 x={12} />;", &["x", "Compa", "x"]),
        // Attribute names decode the same way.
        ("const x = <video data-\\u0076ideo />;", &["data-video", "video", "x"]),
    ];
    for (src, expected) in cases {
        let sf = parse_snippet(src);
        let mut ids = identifiers(&sf);
        ids.sort();
        let mut expected: Vec<&str> = expected.to_vec();
        expected.sort();
        assert_eq!(
            ids, expected,
            "decoded identifier texts for {src:?}"
        );
    }
}
