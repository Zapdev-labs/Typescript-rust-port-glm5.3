//! Go: `scanner.GetLeadingCommentRanges` / `GetTrailingCommentRanges` /
//! `iterateCommentRanges` (tsc/internal/scanner/scanner.go @ ec47d33c23).
//!
//! PORT: Go keeps these in the scanner package with a `*ast.NodeFactory`
//! parameter (used only for `f.NewCommentRange`); the Rust scanner crate has
//! no factory dependency, so the parser crate owns this port and
//! `ast::new_comment_range` is a free function. They are public here because
//! `GetJSDocCommentRanges` (parser/utilities.go) consumes them.

use tsc_ast::{new_comment_range, CommentRange, Kind};
use tsc_stringutil as stringutil;

/// Go: `func isShebangTrivia(text string, pos int) bool`.
fn is_shebang_trivia(text: &str, pos: usize) -> bool {
    if text.len() < 2 {
        return false;
    }
    if pos != 0 {
        panic!("Shebangs check must only be done at the start of the file");
    }
    text.as_bytes()[0] == b'#' && text.as_bytes()[1] == b'!'
}

/// Go: `func scanShebangTrivia(text string, pos int) int`.
fn scan_shebang_trivia(text: &str, pos: usize) -> usize {
    let mut pos = pos + 2;
    let _bytes = text.as_bytes();
    while pos < text.len() {
        let (ch, size) = decode_rune(&text[pos..]);
        if stringutil::is_line_break(ch) {
            break;
        }
        pos += size;
    }
    pos
}

/// Go: `func iterateCommentRanges(f *ast.NodeFactory, text string, pos int,
/// trailing bool) iter.Seq[ast.CommentRange]` — Go's pull-iterator becomes a
/// `Vec` push loop (every parser call site drains the iterator fully; the
/// pending-range handoff between adjacent ranges is preserved exactly).
///
/// Returns an iterator over each comment range following the provided
/// position. Single-line comment ranges include the leading double-slash
/// characters but not the ending line break. Multi-line comment ranges
/// include the leading slash-asterisk and trailing asterisk-slash characters.
pub(crate) fn iterate_comment_ranges(text: &str, pos: usize, trailing: bool) -> Vec<CommentRange> {
    let mut result = Vec::new();
    let mut pending_pos = 0usize;
    let mut pending_end = 0usize;
    let mut pending_kind = Kind::Unknown;
    let mut pending_has_trailing_new_line = false;
    let mut has_pending_comment_range = false;
    let mut collecting = trailing;
    let mut pos = pos;
    if pos == 0 {
        collecting = true;
        if is_shebang_trivia(text, pos) {
            pos = scan_shebang_trivia(text, pos);
        }
    }
    let bytes = text.as_bytes();
    'scan: while pos as isize >= 0 && pos < text.len() {
        let (ch, size) = decode_rune(&text[pos..]);
        match ch {
            '\r' | '\n' => {
                if ch == '\r' && pos + 1 < text.len() && bytes[pos + 1] == b'\n' {
                    pos += 1;
                }
                pos += 1;
                if trailing {
                    break 'scan;
                }
                collecting = true;
                if has_pending_comment_range {
                    pending_has_trailing_new_line = true;
                }
                continue;
            }
            '\t' | '\u{0b}' | '\u{0c}' | ' ' => {
                pos += 1;
                continue;
            }
            '/' => {
                let next_char = if pos + 1 < text.len() {
                    bytes[pos + 1]
                } else {
                    0u8
                };
                let mut has_trailing_new_line = false;
                if next_char == b'/' || next_char == b'*' {
                    let kind = if next_char == b'/' {
                        Kind::SingleLineCommentTrivia
                    } else {
                        Kind::MultiLineCommentTrivia
                    };
                    let start_pos = pos;
                    pos += 2;
                    if next_char == b'/' {
                        while pos < text.len() {
                            let (c, s) = decode_rune(&text[pos..]);
                            if stringutil::is_line_break(c) {
                                has_trailing_new_line = true;
                                break;
                            }
                            pos += s;
                        }
                    } else if let Some(i) = text[pos..].find("*/") {
                        pos += i + 2;
                    } else {
                        pos = text.len();
                    }
                    if collecting {
                        if has_pending_comment_range {
                            result.push(new_comment_range(
                                pending_kind,
                                pending_pos as i32,
                                pending_end as i32,
                                pending_has_trailing_new_line,
                            ));
                        }
                        pending_pos = start_pos;
                        pending_end = pos;
                        pending_kind = kind;
                        pending_has_trailing_new_line = has_trailing_new_line;
                        has_pending_comment_range = true;
                    }
                    continue;
                }
                break 'scan;
            }
            _ => {
                if ch > '\u{7f}' && stringutil::is_white_space_like(ch) {
                    if has_pending_comment_range && stringutil::is_line_break(ch) {
                        pending_has_trailing_new_line = true;
                    }
                    pos += size;
                    continue;
                }
                break 'scan;
            }
        }
    }
    if has_pending_comment_range {
        result.push(new_comment_range(
            pending_kind,
            pending_pos as i32,
            pending_end as i32,
            pending_has_trailing_new_line,
        ));
    }
    result
}

/// Go: `func GetLeadingCommentRanges(f *ast.NodeFactory, text string, pos int) iter.Seq[ast.CommentRange]`.
pub(crate) fn get_leading_comment_ranges(text: &str, pos: usize) -> Vec<CommentRange> {
    iterate_comment_ranges(text, pos, false)
}

/// Go: `func GetTrailingCommentRanges(f *ast.NodeFactory, text string, pos int) iter.Seq[ast.CommentRange]`.
pub(crate) fn get_trailing_comment_ranges(text: &str, pos: usize) -> Vec<CommentRange> {
    iterate_comment_ranges(text, pos, true)
}

/// Go: `utf8.DecodeRuneInString(text[pos:])` over a well-formed `&str` suffix.
fn decode_rune(s: &str) -> (char, usize) {
    let ch = s.chars().next().unwrap_or('\u{fffd}');
    (ch, ch.len_utf8())
}
