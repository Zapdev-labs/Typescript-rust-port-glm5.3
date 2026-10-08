// Ported from tsc/internal/scanner/utilities.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   *ast.Node parameters → `store: &dyn NodeStore, node: NodeId` (SPEC §5.1:
//                          the arena replaces Go's `*Node`; the store is
//                          passed alongside since every accessor needs it)
//   *ast.SourceFile      → &SourceFile (implements NodeStore itself)
//   *ast.NodeList        → Option<&NodeList> where Go may pass nil
//   string return       → Cow<'_, str> where the value is usually a slice of
//                          the source text (Go strings are cheap slices; the
//                          normalize path allocates exactly where Go's
//                          strings.Builder does)
//
// `tokenIsIdentifierOrKeyword` (the first function in utilities.go) lives in
// [`crate::scanner`] alongside the keyword maps it belongs with; it is
// re-exported below so this module mirrors the Go file surface.

use std::borrow::Cow;

use tsc_ast::{
    compute_ecma_line_starts, get_source_file_of_node, is_identifier, is_jsdoc_type_expression,
    is_string_literal, node_text, Kind, NodeFlags, NodeList, NodeId, NodeStore, SourceFile,
};
use tsc_core::languagevariant::LanguageVariant;
use tsc_core::options_generated::NewLineKind;
use tsc_stringutil as stringutil;

use crate::go_shims::{is_type_node_kind, node_is_missing};
use crate::scanner::{is_identifier_part_ex, is_identifier_start, skip_trivia, text_to_keyword};

pub use crate::scanner::token_is_identifier_or_keyword;

/// Go: `func IdentifierToKeywordKind(node *ast.Identifier) ast.Kind`.
pub fn identifier_to_keyword_kind(store: &dyn NodeStore, node: NodeId) -> Kind {
    // Go's zero-value for a missing map entry is KindUnknown.
    text_to_keyword(node_text(store, node)).unwrap_or(Kind::Unknown)
}

/// Go: `func GetSourceTextOfNodeFromSourceFile(sourceFile *ast.SourceFile, node *ast.Node, includeTrivia bool) string`.
pub fn get_source_text_of_node_from_source_file<'f>(
    source_file: &'f SourceFile,
    node: NodeId,
    include_trivia: bool,
) -> Cow<'f, str> {
    get_text_of_node_from_source_text(source_file.text(), source_file, node, include_trivia)
}

/// Go: `func isJSDocTypeExpressionOrChild(node *ast.Node) bool`.
pub fn is_jsdoc_type_expression_or_child(store: &dyn NodeStore, node: NodeId) -> bool {
    if is_jsdoc_type_expression(store.node(node)) {
        return true;
    }
    let n = store.node(node);
    if !(n.flags.intersects(NodeFlags::JSDOC) || n.flags.intersects(NodeFlags::REPARSED)) {
        return false;
    }
    let mut current = node;
    loop {
        let n = store.node(current);
        if is_type_node_kind(n.kind) {
            return true;
        }
        let parent = n.parent.get();
        if parent.is_none() {
            return false;
        }
        current = parent;
    }
}

/// Go: `func normalizeJSDocTypeSourceText(text string) string`.
pub fn normalize_jsdoc_type_source_text(text: &str) -> String {
    let line_starts = compute_ecma_line_starts(text);
    if line_starts.len() == 1 {
        return strip_leading_jsdoc_comment(text).to_string();
    }

    let mut result = String::with_capacity(text.len());
    let new_line = NewLineKind::LF.get_new_line_character();
    for (i, line_start) in line_starts.iter().enumerate() {
        if i > 0 {
            result.push_str(new_line);
        }
        let line_end = if i + 1 < line_starts.len() {
            line_starts[i + 1] as usize
        } else {
            text.len()
        };
        let line = trim_right_line_breaks(&text[*line_start as usize..line_end]);
        result.push_str(strip_leading_jsdoc_comment(line));
    }
    result
}

/// Go: `strings.TrimRightFunc(line, stringutil.IsLineBreak)`.
fn trim_right_line_breaks(line: &str) -> &str {
    let mut end = line.len();
    while end > 0 {
        // Walk back one rune at a time (line slices are rune-aligned).
        let (ch, size) = crate::go_shims::decode_last_rune_in_string(&line.as_bytes()[..end]);
        if !stringutil::is_line_break(crate::as_char(ch)) {
            break;
        }
        end -= size;
    }
    &line[..end]
}

/// Go: `func stripLeadingJSDocComment(line string) string`.
fn strip_leading_jsdoc_comment(line: &str) -> &str {
    let line = trim_left_white_space_like(line);
    let line = if !line.is_empty() && line.as_bytes()[0] == b'*' {
        &line[1..]
    } else {
        line
    };
    trim_left_white_space_like(line)
}

/// Go: `strings.TrimLeftFunc(line, stringutil.IsWhiteSpaceLike)`.
fn trim_left_white_space_like(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let (ch, size) = crate::go_shims::decode_rune_in_string(&bytes[start..]);
        if !stringutil::is_white_space_like(crate::as_char(ch)) {
            break;
        }
        start += size;
    }
    &line[start..]
}

/// Go: `func GetTextOfNodeFromSourceText(sourceText string, node *ast.Node, includeTrivia bool) string`.
pub fn get_text_of_node_from_source_text<'t>(
    source_text: &'t str,
    store: &dyn NodeStore,
    node: NodeId,
    include_trivia: bool,
) -> Cow<'t, str> {
    let n = store.node(node);
    if node_is_missing(n) {
        return Cow::Borrowed("");
    }
    let mut pos = n.pos();
    if !include_trivia {
        pos = skip_trivia(source_text, pos);
    }
    let mut text: Cow<str> = Cow::Borrowed(&source_text[pos as usize..n.end() as usize]);
    if is_jsdoc_type_expression_or_child(store, node) {
        text = Cow::Owned(normalize_jsdoc_type_source_text(&text));
    }
    if n.flags.intersects(NodeFlags::REPARSER_TRANSFORMED_LITERAL) {
        // This is similar to `getLiteralTextOfNode` in the printer, but
        // without the context of an `emitContext` to provide overrides.
        if is_string_literal(n) {
            let token_flags = n.as_string_literal().expect("StringLiteral data").token_flags;
            if token_flags.intersects(tsc_ast::TokenFlags::SINGLE_QUOTE) {
                return Cow::Owned(format!("'{}'", text));
            }
            return Cow::Owned(format!("\"{}\"", text));
        } else if is_identifier(n) {
            return Cow::Owned(node_text(store, node).to_string());
        }
        // Only the above node kinds are currently transformed into one
        // another by the reparser, requiring the textual remapping. (Any
        // remappings done by emit transforms are handled by
        // `getLiteralTextOfNode` in the printer.) Fail on any other kinds.
        tsc_debug::fail_bad_syntax_kind(
            n.kind_string().to_string(),
            Some("Unexpected reparser-transformed node kind".to_string()),
        );
    }
    text
}

/// Go: `func GetTextOfNode(node *ast.Node) string`.
pub fn get_text_of_node<'s>(store: &'s dyn NodeStore, node: NodeId) -> Cow<'s, str> {
    let source_file =
        get_source_file_of_node(store, node).expect("GetTextOfNode: node has no source file");
    get_source_text_of_node_from_source_file(source_file, node, false /* include_trivia */)
}

/// Go: `func GetTextOfJSDocComment(comment *ast.NodeList) string`.
pub fn get_text_of_jsdoc_comment(store: &dyn NodeStore, comment: Option<&NodeList>) -> String {
    let Some(comment) = comment else {
        return String::new();
    };
    let mut b = String::new();
    for &n in comment.nodes.iter() {
        match store.node(n).kind {
            Kind::JSDocText => b.push_str(node_text(store, n)),
            Kind::JSDocLink | Kind::JSDocLinkCode | Kind::JSDocLinkPlain => {
                b.push_str(&get_text_of_node(store, n));
            }
            _ => {}
        }
    }
    // Go: strings.TrimRightFunc(b.String(), unicode.IsSpace) — Rust's
    // char::is_whitespace is the same White_Space property.
    b.trim_end().to_string()
}

/// Go: `func DeclarationNameToString(name *ast.Node) string`.
pub fn declaration_name_to_string(store: &dyn NodeStore, name: Option<NodeId>) -> String {
    let Some(name) = name else {
        return "(Missing)".to_string();
    };
    let n = store.node(name);
    if n.pos() == n.end() {
        return "(Missing)".to_string();
    }
    get_text_of_node(store, name).into_owned()
}

/// Go: `func IsIdentifierText(name string, languageVariant core.LanguageVariant) bool`.
pub fn is_identifier_text(name: &str, language_variant: LanguageVariant) -> bool {
    let mut chars = name.chars();
    // Go: an empty string decodes to (RuneError, 0), which fails
    // IsIdentifierStart — the `None` case below mirrors that.
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_identifier_start(first as i32) {
        return false;
    }
    for ch in chars {
        if !is_identifier_part_ex(ch as i32, language_variant) {
            return false;
        }
    }
    true
}

/// Go: `func IsIntrinsicJsxName(name string) bool`.
pub fn is_intrinsic_jsx_name(name: &str) -> bool {
    !name.is_empty() && (name.as_bytes()[0].is_ascii_lowercase() || name.contains('-'))
}
