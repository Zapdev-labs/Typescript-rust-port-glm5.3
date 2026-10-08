//! Rust port of the Go recursive-descent parser
//! (`tsc/internal/parser` @ ec47d33c23e464a17cdf2475632cba629bee8763):
//! parser.go, jsdoc.go, references.go, utilities.go, types.go — plus
//! `scanner.GetLeading/GetTrailingCommentRanges` and `GetJSDocCommentRanges`
//! (scanner-owned but factory-dependent; see comment_ranges.rs).
//!
//! Go name mapping:
//!   *ast.Node            → `NodeId` handles into the per-`SourceFile` arena
//!                         (SPEC §5.1); nil-able Go pointers → `Option`
//!   Parser (sync.Pool'd) → plain-constructed `Parser` per parse (PORT:
//!                         the pool saves the scanner + closure slots in Go;
//!         the arena-owner design makes reuse a wash, see parser.rs)
//!   parser state slices  → plain `Vec`s (Go's `core.Arena` slice/string
//!                         arenas are GC-era space optimizations with no
//!                         observable semantics; dropped, see PORTING-NOTES)
//!
//! PORT: `reparser.go` (752 LOC: reparseTags, the JSDoc @typedef/@import →
//! statement reparsing machinery, reparsed-clones bookkeeping) is explicitly
//! OUT OF SCOPE for the M3 parser wave and will land in a follow-up wave.
//! Consequences carried through the port:
//!   * `withJSDoc` calls a stubbed `reparse_tags` (no-op) — `reparse_list`
//!     therefore stays empty and `IsJSTypeAliasDeclaration`/`IsJSImportDeclaration`
//!     statements are not produced by lazy JSDoc reparsing yet.
//!   * `finishSourceFile`'s `reparsedClones` sorting/clone is dropped
//!     (always empty).
//!   * `ParserState.reparsedClonesLen` is dropped from mark/rewind.

pub mod comment_ranges;
pub mod parser;
#[cfg(test)]
mod parser_test;

mod expressions;
mod jsdoc;
mod jsx;
mod references;
mod types;

use tsc_ast::{CommentRange, Kind, NodeId, NodeStore, SourceFile, SourceFileParseOptions};
use tsc_core::languagevariant::LanguageVariant;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::text::TextPos;

// ────────────────────────────────────────────────────────────────────────────
// types.go — ParseFlags
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ParseFlags uint32` + consts (parser/types.go).
pub type ParseFlags = u32;
pub const PARSE_FLAGS_NONE: ParseFlags = 0;
pub const PARSE_FLAGS_YIELD: ParseFlags = 1 << 0;
pub const PARSE_FLAGS_AWAIT: ParseFlags = 1 << 1;
pub const PARSE_FLAGS_TYPE: ParseFlags = 1 << 2;
pub const PARSE_FLAGS_IGNORE_MISSING_OPEN_BRACE: ParseFlags = 1 << 4;
pub const PARSE_FLAGS_JSDOC: ParseFlags = 1 << 5;

// ────────────────────────────────────────────────────────────────────────────
// parser.go — ParsingContext
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ParsingContext int` + iota consts (parser.go) — kept as an
/// `i32` (Go int) so `parsingContexts` bitmasks work identically.
pub type ParsingContext = i32;

pub const PC_SOURCE_ELEMENTS: ParsingContext = 0; // Elements in source file
pub const PC_BLOCK_STATEMENTS: ParsingContext = 1; // Statements in block
pub const PC_SWITCH_CLAUSES: ParsingContext = 2; // Clauses in switch statement
pub const PC_SWITCH_CLAUSE_STATEMENTS: ParsingContext = 3; // Statements in switch clause
pub const PC_TYPE_MEMBERS: ParsingContext = 4; // Members in interface or type literal
pub const PC_CLASS_MEMBERS: ParsingContext = 5; // Members in class declaration
pub const PC_ENUM_MEMBERS: ParsingContext = 6; // Members in enum declaration
pub const PC_HERITAGE_CLAUSE_ELEMENT: ParsingContext = 7; // Elements in a heritage clause
pub const PC_VARIABLE_DECLARATIONS: ParsingContext = 8; // Variable declarations in variable statement
pub const PC_OBJECT_BINDING_ELEMENTS: ParsingContext = 9; // Binding elements in object binding list
pub const PC_ARRAY_BINDING_ELEMENTS: ParsingContext = 10; // Binding elements in array binding list
pub const PC_ARGUMENT_EXPRESSIONS: ParsingContext = 11; // Expressions in argument list
pub const PC_OBJECT_LITERAL_MEMBERS: ParsingContext = 12; // Members in object literal
pub const PC_JSX_ATTRIBUTES: ParsingContext = 13; // Attributes in jsx element
pub const PC_JSX_CHILDREN: ParsingContext = 14; // Things between opening and closing JSX tags
pub const PC_ARRAY_LITERAL_MEMBERS: ParsingContext = 15; // Members in array literal
pub const PC_PARAMETERS: ParsingContext = 16; // Parameters in parameter list
pub const PC_JSDOC_PARAMETERS: ParsingContext = 17; // JSDoc parameters in parameter list of JSDoc function type
pub const PC_REST_PROPERTIES: ParsingContext = 18; // Property names in a rest type list
pub const PC_TYPE_PARAMETERS: ParsingContext = 19; // Type parameters in type parameter list
pub const PC_TYPE_ARGUMENTS: ParsingContext = 20; // Type arguments in type argument list
pub const PC_TUPLE_ELEMENT_TYPES: ParsingContext = 21; // Element types in tuple element type list
pub const PC_HERITAGE_CLAUSES: ParsingContext = 22; // Heritage clauses for a class or interface declaration.
pub const PC_IMPORT_OR_EXPORT_SPECIFIERS: ParsingContext = 23; // Named import clause's import specifier list
pub const PC_IMPORT_ATTRIBUTES: ParsingContext = 24; // Import attributes
pub const PC_JSDOC_COMMENT: ParsingContext = 25; // Parsing via JSDocParser
pub const PC_COUNT: ParsingContext = 26; // Number of parsing contexts

/// Go: `type ParsingContexts int` — the bitmask over [`ParsingContext`].
pub type ParsingContexts = i32;

// ────────────────────────────────────────────────────────────────────────────
// utilities.go
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getLanguageVariant(scriptKind core.ScriptKind) core.LanguageVariant`.
pub(crate) fn get_language_variant(script_kind: ScriptKind) -> LanguageVariant {
    match script_kind {
        ScriptKind::TSX | ScriptKind::JSX | ScriptKind::JS | ScriptKind::JSON => {
            // .tsx and .jsx files are treated as jsx language variant.
            LanguageVariant::JSX
        }
        _ => LanguageVariant::Standard,
    }
}

/// Go: `func tokenIsIdentifierOrKeyword(token ast.Kind) bool` — the Go parser
/// package's copy is the same predicate the scanner package exports; the
/// port reuses the scanner's (dedup, PORTING-NOTES).
pub(crate) use tsc_scanner::token_is_identifier_or_keyword;

/// Go: `func tokenIsIdentifierOrKeywordOrGreaterThan(token ast.Kind) bool`.
pub(crate) fn token_is_identifier_or_keyword_or_greater_than(token: Kind) -> bool {
    token == Kind::GreaterThanToken || token_is_identifier_or_keyword(token)
}

/// Go: `func isKeywordOrPunctuation(token ast.Kind) bool`.
pub(crate) fn is_keyword_or_punctuation(token: Kind) -> bool {
    tsc_ast::is_keyword_kind(token) || tsc_ast::is_punctuation_kind(token)
}

/// Go: `func isJSDocLikeText(text string) bool`.
pub(crate) fn is_jsdoc_like_text(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 4 && b[1] == b'*' && b[2] == b'*' && b[3] != b'/'
}

/// Go: `func GetJSDocCommentRanges(f *ast.NodeFactory, commentRanges []ast.CommentRange,
/// node *ast.Node, text string) []ast.CommentRange`.
///
/// PORT: Go's `commentRanges` in/out slice parameter is a space-reuse
/// optimization (`jsdocCommentRangesSpace`); the port allocates a fresh Vec.
/// The Go `*NodeFactory` parameter exists only because the underlying comment
/// range iteration needs it; the Rust free-function `new_comment_range` makes
/// it unnecessary.
pub(crate) fn get_jsdoc_comment_ranges(
    store: &dyn NodeStore,
    node: NodeId,
    text: &str,
) -> Vec<CommentRange> {
    let n = store.node(node);
    let kind = n.kind;
    let pos = n.pos();
    let end = n.end();
    let mut comment_ranges = Vec::new();
    match kind {
        Kind::Parameter
        | Kind::TypeParameter
        | Kind::FunctionExpression
        | Kind::ArrowFunction
        | Kind::ParenthesizedExpression
        | Kind::VariableDeclaration
        | Kind::ExportSpecifier => {
            for comment_range in comment_ranges::get_trailing_comment_ranges(text, pos as usize) {
                comment_ranges.push(comment_range);
            }
            for comment_range in comment_ranges::get_leading_comment_ranges(text, pos as usize) {
                comment_ranges.push(comment_range);
            }
        }
        _ => {
            for comment_range in comment_ranges::get_leading_comment_ranges(text, pos as usize) {
                comment_ranges.push(comment_range);
            }
        }
    }
    // Keep if the comment starts with '/**' but not if it is '/**/'
    comment_ranges.retain(|comment| {
        let comment_start = comment.loc.pos() as usize;
        let comment_len = (comment.loc.end() - comment_start as TextPos) as usize;
        let b = text.as_bytes();
        comment.loc.end() <= end
            && comment_len >= 4
            && b[comment_start + 1] == b'*'
            && b[comment_start + 2] == b'*'
            && b[comment_start + 3] != b'/'
    });
    comment_ranges
}

// ────────────────────────────────────────────────────────────────────────────
// Package entry points (parser.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func ParseSourceFile(opts ast.SourceFileParseOptions, sourceText string,
/// scriptKind core.ScriptKind) *ast.SourceFile`.
///
/// PORT: the bench-harness seam pins this non-optional return. Go can return
/// nil only through `parseSourceFileWorker` panicking or scriptKind Unknown
/// (initializeState panics there too, so the nil branch is dead in practice).
pub fn parse_source_file(
    opts: SourceFileParseOptions,
    source_text: &str,
    script_kind: ScriptKind,
) -> SourceFile {
    // Go: p := getParser(); defer putParser(p) — the pool is dropped (PORT,
    // see parser.rs).
    let mut file = SourceFile::new(0, opts.clone(), source_text, None, None);
    {
        let mut p = parser::Parser::new(&mut file, opts, source_text, script_kind);
        p.next_token();
        if script_kind == ScriptKind::JSON {
            p.parse_json_text();
        } else {
            p.parse_source_file_worker();
        }
    }
    file
}

/// Go: `func ParseIsolatedEntityName(text string) *ast.EntityName`.
///
/// PORT: Go returns a GC-owned `*ast.EntityName`; the port's nodes live in
/// the per-file arena, so the owning `SourceFile` is returned alongside the
/// handle.
pub fn parse_isolated_entity_name(text: &str) -> Option<(SourceFile, NodeId)> {
    let mut file = SourceFile::new(0, SourceFileParseOptions::default(), text, None, None);
    let entity_name;
    let at_eof;
    let diagnostics_len;
    {
        let mut p = parser::Parser::new(
            &mut file,
            SourceFileParseOptions::default(),
            text,
            ScriptKind::JS,
        );
        p.next_token();
        entity_name = p.parse_entity_name(true, false, None);
        at_eof = p.token == Kind::EndOfFile;
        diagnostics_len = p.sink_diagnostics_len();
    }
    if at_eof && diagnostics_len == 0 {
        Some((file, entity_name))
    } else {
        None
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Local re-exports used by the internal modules (Go keeps these package-scope).
// ────────────────────────────────────────────────────────────────────────────
