// Ported from tsc/internal/scanner/scanner.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   Scanner (struct)          → Scanner<'a> (lifetime = borrowed source text;
//                              Go strings are (ptr,len) pairs, the Rust port
//                              borrows the buffer instead)
//   ScannerState (embed)      → Scanner.state field (Rust has no embedding)
//   TokenValue string         → token_value: Cow<'a, [u8]> — the fast path is
//                              a borrowed slice of the source text; Go's
//                              strings.Builder sites become Cow::Owned. Byte
//                              slices, not &str: lone surrogates are stored
//                              as the CESU-8 sentinel from
//                              stringutil::encode_js_string_rune (invalid
//                              UTF-8 — see ast::positionmap).
//   defaultScanner()          → Scanner::new() / Default (plain zeroing; Go's
//                              function-constructed struct dodges write
//                              barriers, which Rust does not need)
//   int positions             → usize inside the scanner; i32 at
//                              possibly-synthesized boundaries (skip_trivia*)
//   rune                      → i32 (EOF sentinel -1), matching stringutil's
//                              rune-typed surrogate helpers
//
// PORT (dropped caches, per the M3 dispatch perf notes): Go carries
// hexNumberCache and hexDigitCache memo maps. The Rust paths are already
// allocation-free on their fast paths (borrowed &str slices; "0x"+digits is
// built only when separators/uppercase force it, and hex digits are borrowed
// when already lowercase and separator-free), so the caches only re-do work
// that is now trivial slice comparisons — dropped with no observable change.
// numberCache IS kept: it memoizes jsnum::from_string + shortes-round-trip
// formatting of repeated numeric literals ("0", "1", ...), which is real CPU
// work the Go cache avoids. In Go the memo maps are never observable: the
// cached values are byte-identical to recomputation.

use std::borrow::Cow;

use tsc_ast::{CommentDirective, CommentDirectiveKind, Kind, SourceFile, TokenFlags};
use tsc_core::languagevariant::LanguageVariant;
use tsc_core::options_generated::ScriptTarget;
use tsc_core::text::{TextPos, TextRange};
use tsc_diagnostics::{Message, ASTERISK_SLASH_EXPECTED, BINARY_DIGIT_EXPECTED,
    DECIMALS_WITH_LEADING_ZEROS_ARE_NOT_ALLOWED, DIGIT_EXPECTED,
    A_BIGINT_LITERAL_CANNOT_USE_EXPONENTIAL_NOTATION, A_BIGINT_LITERAL_MUST_BE_AN_INTEGER,
    AN_IDENTIFIER_OR_KEYWORD_CANNOT_IMMEDIATELY_FOLLOW_A_NUMERIC_LITERAL,
    FILE_APPEARS_TO_BE_BINARY, HEXADECIMAL_DIGIT_EXPECTED, INVALID_CHARACTER,
    MERGE_CONFLICT_MARKER_ENCOUNTERED, MULTIPLE_CONSECUTIVE_NUMERIC_SEPARATORS_ARE_NOT_PERMITTED,
    NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, OCTAL_DIGIT_EXPECTED,
    OCTAL_LITERALS_ARE_NOT_ALLOWED_USE_THE_SYNTAX_0, UNEXPECTED_END_OF_TEXT,
    UNEXPECTED_TOKEN_DID_YOU_MEAN_OR_GT, UNEXPECTED_TOKEN_DID_YOU_MEAN_OR_RBRACE,
    UNTERMINATED_REGULAR_EXPRESSION_LITERAL, UNTERMINATED_STRING_LITERAL,
    UNTERMINATED_TEMPLATE_LITERAL, UNTERMINATED_UNICODE_ESCAPE_SEQUENCE,
    UNDETERMINED_CHARACTER_ESCAPE, AN_EXTENDED_UNICODE_ESCAPE_VALUE_MUST_BE_BETWEEN_0X0_AND_0X10FFFF_INCLUSIVE,
    UNICODE_ESCAPE_SEQUENCES_ARE_ONLY_AVAILABLE_WHEN_THE_UNICODE_U_FLAG_OR_THE_UNICODE_SETS_V_FLAG_IS_SET,
    OCTAL_ESCAPE_SEQUENCES_AND_BACKREFERENCES_ARE_NOT_ALLOWED_IN_A_CHARACTER_CLASS_IF_THIS_WAS_INTENDED_AS_AN_ESCAPE_SEQUENCE_USE_THE_SYNTAX_0_INSTEAD,
    OCTAL_ESCAPE_SEQUENCES_ARE_NOT_ALLOWED_USE_THE_SYNTAX_0,
    DECIMAL_ESCAPE_SEQUENCES_AND_BACKREFERENCES_ARE_NOT_ALLOWED_IN_A_CHARACTER_CLASS,
    ESCAPE_SEQUENCE_0_IS_NOT_ALLOWED, X_CAN_ONLY_BE_USED_AT_THE_START_OF_A_FILE};

use crate::go_shims::{decode_last_rune_in_string, is_type_node_kind};
use crate::regexp::RegExpParser;
use crate::{as_char, unicodeproperties};

// ────────────────────────────────────────────────────────────────────────────
// Flags / variants
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type EscapeSequenceScanningFlags int32` + consts (bit 0–5 and the two
/// composites).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct EscapeSequenceScanningFlags(pub i32);

impl EscapeSequenceScanningFlags {
    pub const STRING: Self = Self(1 << 0);
    pub const REPORT_ERRORS: Self = Self(1 << 1);
    pub const REGULAR_EXPRESSION: Self = Self(1 << 2);
    pub const ANNEX_B: Self = Self(1 << 3);
    pub const ANY_UNICODE_MODE: Self = Self(1 << 4);
    pub const ATOM_ESCAPE: Self = Self(1 << 5);
    pub const REPORT_INVALID_ESCAPE_ERRORS: Self =
        Self(Self::REGULAR_EXPRESSION.0 | Self::REPORT_ERRORS.0);
    pub const ALLOW_EXTENDED_UNICODE_ESCAPE: Self =
        Self(Self::STRING.0 | Self::ANY_UNICODE_MODE.0);

    /// Go: `flags&other != 0`.
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }
}

impl std::ops::BitOr for EscapeSequenceScanningFlags {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for EscapeSequenceScanningFlags {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for EscapeSequenceScanningFlags {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

/// Go: `type identifierVariant int32` + iota consts (unexported).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentifierVariant {
    Standard,
    Jsx,
    RegExpGroupName,
}

/// Go: `type ErrorCallback func(diagnostic *diagnostics.Message, start, length int, args ...any)`.
/// The variadic `args ...any` becomes `&[String]` (every Go call site passes
/// strings).
pub type ErrorCallback<'c> = Box<dyn FnMut(&'static Message, i32, i32, &[String]) + 'c>;

// ────────────────────────────────────────────────────────────────────────────
// Keyword maps (Go: textToKeyword / textToToken / tokenToText)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `var textToKeyword = map[string]ast.Kind{...}` — a `match` compiles to
/// a length+prefix dispatch (no allocation, no hash map).
fn text_to_keyword(s: &str) -> Option<Kind> {
    Some(match s {
        "abstract" => Kind::AbstractKeyword,
        "accessor" => Kind::AccessorKeyword,
        "any" => Kind::AnyKeyword,
        "as" => Kind::AsKeyword,
        "asserts" => Kind::AssertsKeyword,
        "assert" => Kind::AssertKeyword,
        "bigint" => Kind::BigIntKeyword,
        "boolean" => Kind::BooleanKeyword,
        "break" => Kind::BreakKeyword,
        "case" => Kind::CaseKeyword,
        "catch" => Kind::CatchKeyword,
        "class" => Kind::ClassKeyword,
        "continue" => Kind::ContinueKeyword,
        "const" => Kind::ConstKeyword,
        "constructor" => Kind::ConstructorKeyword,
        "debugger" => Kind::DebuggerKeyword,
        "declare" => Kind::DeclareKeyword,
        "default" => Kind::DefaultKeyword,
        "defer" => Kind::DeferKeyword,
        "delete" => Kind::DeleteKeyword,
        "do" => Kind::DoKeyword,
        "else" => Kind::ElseKeyword,
        "enum" => Kind::EnumKeyword,
        "export" => Kind::ExportKeyword,
        "extends" => Kind::ExtendsKeyword,
        "false" => Kind::FalseKeyword,
        "finally" => Kind::FinallyKeyword,
        "for" => Kind::ForKeyword,
        "from" => Kind::FromKeyword,
        "function" => Kind::FunctionKeyword,
        "get" => Kind::GetKeyword,
        "if" => Kind::IfKeyword,
        "immediate" => Kind::ImmediateKeyword,
        "implements" => Kind::ImplementsKeyword,
        "import" => Kind::ImportKeyword,
        "in" => Kind::InKeyword,
        "infer" => Kind::InferKeyword,
        "instanceof" => Kind::InstanceOfKeyword,
        "interface" => Kind::InterfaceKeyword,
        "intrinsic" => Kind::IntrinsicKeyword,
        "is" => Kind::IsKeyword,
        "keyof" => Kind::KeyOfKeyword,
        "let" => Kind::LetKeyword,
        "module" => Kind::ModuleKeyword,
        "namespace" => Kind::NamespaceKeyword,
        "never" => Kind::NeverKeyword,
        "new" => Kind::NewKeyword,
        "null" => Kind::NullKeyword,
        "number" => Kind::NumberKeyword,
        "object" => Kind::ObjectKeyword,
        "package" => Kind::PackageKeyword,
        "private" => Kind::PrivateKeyword,
        "protected" => Kind::ProtectedKeyword,
        "public" => Kind::PublicKeyword,
        "override" => Kind::OverrideKeyword,
        "out" => Kind::OutKeyword,
        "readonly" => Kind::ReadonlyKeyword,
        "require" => Kind::RequireKeyword,
        "global" => Kind::GlobalKeyword,
        "return" => Kind::ReturnKeyword,
        "satisfies" => Kind::SatisfiesKeyword,
        "set" => Kind::SetKeyword,
        "source" => Kind::SourceKeyword,
        "static" => Kind::StaticKeyword,
        "string" => Kind::StringKeyword,
        "super" => Kind::SuperKeyword,
        "switch" => Kind::SwitchKeyword,
        "symbol" => Kind::SymbolKeyword,
        "this" => Kind::ThisKeyword,
        "throw" => Kind::ThrowKeyword,
        "true" => Kind::TrueKeyword,
        "try" => Kind::TryKeyword,
        "type" => Kind::TypeKeyword,
        "typeof" => Kind::TypeOfKeyword,
        "undefined" => Kind::UndefinedKeyword,
        "unique" => Kind::UniqueKeyword,
        "unknown" => Kind::UnknownKeyword,
        "using" => Kind::UsingKeyword,
        "var" => Kind::VarKeyword,
        "void" => Kind::VoidKeyword,
        "while" => Kind::WhileKeyword,
        "with" => Kind::WithKeyword,
        "yield" => Kind::YieldKeyword,
        "async" => Kind::AsyncKeyword,
        "await" => Kind::AwaitKeyword,
        "of" => Kind::OfKeyword,
        _ => return None,
    })
}

/// Go: `var textToToken = func() ...` — keywords overlaid on punctuation.
fn text_to_token(s: &str) -> Option<Kind> {
    Some(match s {
        "{" => Kind::OpenBraceToken,
        "}" => Kind::CloseBraceToken,
        "(" => Kind::OpenParenToken,
        ")" => Kind::CloseParenToken,
        "[" => Kind::OpenBracketToken,
        "]" => Kind::CloseBracketToken,
        "." => Kind::DotToken,
        "..." => Kind::DotDotDotToken,
        ";" => Kind::SemicolonToken,
        "," => Kind::CommaToken,
        "<" => Kind::LessThanToken,
        ">" => Kind::GreaterThanToken,
        "<=" => Kind::LessThanEqualsToken,
        ">=" => Kind::GreaterThanEqualsToken,
        "==" => Kind::EqualsEqualsToken,
        "!=" => Kind::ExclamationEqualsToken,
        "===" => Kind::EqualsEqualsEqualsToken,
        "!==" => Kind::ExclamationEqualsEqualsToken,
        "=>" => Kind::EqualsGreaterThanToken,
        "+" => Kind::PlusToken,
        "-" => Kind::MinusToken,
        "**" => Kind::AsteriskAsteriskToken,
        "*" => Kind::AsteriskToken,
        "/" => Kind::SlashToken,
        "%" => Kind::PercentToken,
        "++" => Kind::PlusPlusToken,
        "--" => Kind::MinusMinusToken,
        "<<" => Kind::LessThanLessThanToken,
        "</" => Kind::LessThanSlashToken,
        ">>" => Kind::GreaterThanGreaterThanToken,
        ">>>" => Kind::GreaterThanGreaterThanGreaterThanToken,
        "&" => Kind::AmpersandToken,
        "|" => Kind::BarToken,
        "^" => Kind::CaretToken,
        "!" => Kind::ExclamationToken,
        "~" => Kind::TildeToken,
        "&&" => Kind::AmpersandAmpersandToken,
        "||" => Kind::BarBarToken,
        "?" => Kind::QuestionToken,
        "??" => Kind::QuestionQuestionToken,
        "?." => Kind::QuestionDotToken,
        ":" => Kind::ColonToken,
        "=" => Kind::EqualsToken,
        "+=" => Kind::PlusEqualsToken,
        "-=" => Kind::MinusEqualsToken,
        "*=" => Kind::AsteriskEqualsToken,
        "**=" => Kind::AsteriskAsteriskEqualsToken,
        "/=" => Kind::SlashEqualsToken,
        "%=" => Kind::PercentEqualsToken,
        "<<=" => Kind::LessThanLessThanEqualsToken,
        ">>=" => Kind::GreaterThanGreaterThanEqualsToken,
        ">>>=" => Kind::GreaterThanGreaterThanGreaterThanEqualsToken,
        "&=" => Kind::AmpersandEqualsToken,
        "|=" => Kind::BarEqualsToken,
        "^=" => Kind::CaretEqualsToken,
        "||=" => Kind::BarBarEqualsToken,
        "&&=" => Kind::AmpersandAmpersandEqualsToken,
        "??=" => Kind::QuestionQuestionEqualsToken,
        "@" => Kind::AtToken,
        "#" => Kind::HashToken,
        "`" => Kind::BacktickToken,
        _ => return text_to_keyword(s),
    })
}

// ────────────────────────────────────────────────────────────────────────────
// ScannerState / Scanner
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ScannerState struct` — the Mark/Rewind snapshot of the scanner
/// (Go embeds it in Scanner; Rust uses a `state` field).
#[derive(Clone, Debug)]
pub struct ScannerState<'a> {
    /// Current position in text (and ending position of current token).
    pub(crate) pos: usize,
    /// Starting position of current token including preceding whitespace.
    pub(crate) full_start_pos: usize,
    /// Starting position of non-whitespace part of current token.
    pub(crate) token_start: usize,
    /// Kind of current token.
    pub(crate) token: Kind,
    /// Parsed value of current token.
    pub(crate) token_value: Cow<'a, [u8]>,
    /// Flags for current token.
    pub(crate) token_flags: TokenFlags,
    pub(crate) comment_directives: Vec<CommentDirective>,
    /// Leading asterisks to skip when scanning types inside JSDoc. Should be 0
    /// outside JSDoc.
    pub(crate) skip_jsdoc_leading_asterisks: i32,
}

impl Default for ScannerState<'_> {
    fn default() -> Self {
        ScannerState {
            pos: 0,
            full_start_pos: 0,
            token_start: 0,
            token: Kind::Unknown,
            token_value: Cow::Borrowed(&[]),
            token_flags: TokenFlags::NONE,
            comment_directives: Vec::new(),
            skip_jsdoc_leading_asterisks: 0,
        }
    }
}

/// Go: `type Scanner struct`.
pub struct Scanner<'a> {
    pub(crate) text: &'a str,
    pub(crate) end: usize,
    language_variant: LanguageVariant,
    script_target: ScriptTarget,
    pub(crate) on_error: Option<ErrorCallback<'a>>,
    skip_trivia: bool,
    pub(crate) state: ScannerState<'a>,

    // Go: numberCache map[string]string (memo of canonicalized numeric
    // literal values; hexNumberCache/hexDigitCache dropped — see header).
    number_cache: Option<rustc_hash::FxHashMap<Vec<u8>, Cow<'a, [u8]>>>,
}

impl<'a> Scanner<'a> {
    /// Go: `func NewScanner() *Scanner` (via `defaultScanner()`; skipTrivia is
    /// true by default).
    pub fn new() -> Self {
        Scanner {
            text: "",
            end: 0,
            language_variant: LanguageVariant::Standard,
            script_target: ScriptTarget::None,
            on_error: None,
            skip_trivia: true,
            state: ScannerState::default(),
            number_cache: None,
        }
    }

    /// Go: `func (s *Scanner) Reset()` — Go clears the caches in place and
    /// re-defaults the struct (including `onError`; the defaultScanner zero
    /// value has no callback).
    pub fn reset(&mut self) {
        if let Some(cache) = self.number_cache.as_mut() {
            cache.clear();
        }
        let number_cache = self.number_cache.take();
        *self = Scanner::new();
        self.number_cache = number_cache;
    }

    /// Go: `func (s *Scanner) Text() string`.
    pub fn text(&self) -> &'a str {
        self.text
    }

    /// Go: `func (s *Scanner) Token() ast.Kind`.
    pub fn token(&self) -> Kind {
        self.state.token
    }

    /// Go: `func (s *Scanner) TokenFlags() ast.TokenFlags`.
    pub fn token_flags(&self) -> TokenFlags {
        self.state.token_flags
    }

    /// Go: `func (s *Scanner) TokenFullStart() int`.
    pub fn token_full_start(&self) -> usize {
        self.state.full_start_pos
    }

    /// Go: `func (s *Scanner) TokenStart() int`.
    pub fn token_start(&self) -> usize {
        self.state.token_start
    }

    /// Go: `func (s *Scanner) TokenEnd() int`.
    pub fn token_end(&self) -> usize {
        self.state.pos
    }

    /// Go: `func (s *Scanner) TokenText() string`.
    pub fn token_text(&self) -> &'a str {
        &self.text[self.state.token_start..self.state.pos]
    }

    /// Go: `func (s *Scanner) TokenValue() string` — bytes, not &str: values
    /// can contain the lone-surrogate CESU-8 sentinel (see header).
    pub fn token_value(&self) -> &[u8] {
        &self.state.token_value
    }

    /// Go: `func (s *Scanner) TokenRange() core.TextRange`.
    pub fn token_range(&self) -> TextRange {
        TextRange::new(self.state.token_start as i32, self.state.pos as i32)
    }

    /// Go: `func (s *Scanner) CommentDirectives() []ast.CommentDirective`.
    pub fn comment_directives(&self) -> &[CommentDirective] {
        &self.state.comment_directives
    }

    /// Go: `func (s *Scanner) Mark() ScannerState`.
    pub fn mark(&self) -> ScannerState<'a> {
        self.state.clone()
    }

    /// Go: `func (s *Scanner) Rewind(state ScannerState)`.
    pub fn rewind(&mut self, state: ScannerState<'a>) {
        self.state = state;
    }

    /// Go: `func (s *Scanner) ResetPos(pos int)`.
    pub fn reset_pos(&mut self, pos: usize) {
        self.state.pos = pos;
        self.state.full_start_pos = pos;
        self.state.token_start = pos;
    }

    /// Go: `func (s *Scanner) ResetTokenState(pos int)`.
    pub fn reset_token_state(&mut self, pos: usize) {
        self.reset_pos(pos);
        self.state.token = Kind::Unknown;
        self.state.token_value = Cow::Borrowed(&[]);
        self.state.token_flags = TokenFlags::NONE;
    }

    /// Go: `func (scanner *Scanner) SetSkipJSDocLeadingAsterisks(skip bool)`.
    pub fn set_skip_jsdoc_leading_asterisks(&mut self, skip: bool) {
        if skip {
            self.state.skip_jsdoc_leading_asterisks += 1;
        } else {
            self.state.skip_jsdoc_leading_asterisks -= 1;
        }
    }

    /// Go: `func (scanner *Scanner) SetSkipTrivia(skip bool)`.
    pub fn set_skip_trivia(&mut self, skip: bool) {
        self.skip_trivia = skip;
    }

    /// Go: `func (s *Scanner) HasUnicodeEscape() bool`.
    pub fn has_unicode_escape(&self) -> bool {
        self.state.token_flags.intersects(TokenFlags::UNICODE_ESCAPE)
    }

    /// Go: `func (s *Scanner) HasExtendedUnicodeEscape() bool`.
    pub fn has_extended_unicode_escape(&self) -> bool {
        self.state
            .token_flags
            .intersects(TokenFlags::EXTENDED_UNICODE_ESCAPE)
    }

    /// Go: `func (s *Scanner) HasPrecedingLineBreak() bool`.
    pub fn has_preceding_line_break(&self) -> bool {
        self.state.token_flags.intersects(TokenFlags::PRECEDING_LINE_BREAK)
    }

    /// Go: `func (s *Scanner) HasPrecedingJSDocComment() bool`.
    pub fn has_preceding_jsdoc_comment(&self) -> bool {
        self.state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JSDOC_COMMENT)
    }

    /// Go: `func (s *Scanner) HasPrecedingJSDocLeadingAsterisks() bool`.
    pub fn has_preceding_jsdoc_leading_asterisks(&self) -> bool {
        self.state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JSDOC_LEADING_ASTERISKS)
    }

    /// Go: `func (s *Scanner) HasPrecedingJSDocWithDeprecatedTag() bool`.
    pub fn has_preceding_jsdoc_with_deprecated_tag(&self) -> bool {
        self.state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JSDOC_WITH_DEPRECATED)
    }

    /// Go: `func (s *Scanner) HasPrecedingJSDocWithSeeOrLink() bool`.
    pub fn has_preceding_jsdoc_with_see_or_link(&self) -> bool {
        self.state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JSDOC_WITH_SEE_OR_LINK)
    }

    /// Go: `func (s *Scanner) SetText(text string)`.
    pub fn set_text(&mut self, text: &'a str) {
        self.text = text;
        self.end = text.len();
        self.state = ScannerState::default();
    }

    /// Go: `func (s *Scanner) SetOnError(errorCallback ErrorCallback)`.
    pub fn set_on_error(
        &mut self,
        error_callback: impl FnMut(&'static Message, i32, i32, &[String]) + 'a,
    ) {
        self.on_error = Some(Box::new(error_callback));
    }

    /// Go: `func (s *Scanner) SetLanguageVariant(languageVariant core.LanguageVariant)`.
    pub fn set_language_variant(&mut self, language_variant: LanguageVariant) {
        self.language_variant = language_variant;
    }

    /// Go: `func (s *Scanner) SetScriptTarget(scriptTarget core.ScriptTarget)`.
    pub fn set_script_target(&mut self, script_target: ScriptTarget) {
        self.script_target = script_target;
    }

    /// Go: `func (s *Scanner) languageVersion() core.ScriptTarget` (unexported).
    pub(crate) fn language_version(&self) -> ScriptTarget {
        if self.script_target == ScriptTarget::None {
            ScriptTarget::Latest
        } else {
            self.script_target
        }
    }

    /// Go: `func (s *Scanner) error(diagnostic *diagnostics.Message)`.
    pub(crate) fn error(&mut self, diagnostic: &'static Message) {
        self.error_at(diagnostic, self.state.pos, 0, &[]);
    }

    /// Go: `func (s *Scanner) errorAt(diagnostic *diagnostics.Message, pos int, length int, args ...any)`.
    pub(crate) fn error_at(
        &mut self,
        diagnostic: &'static Message,
        pos: usize,
        length: usize,
        args: &[String],
    ) {
        if let Some(on_error) = self.on_error.as_mut() {
            on_error(diagnostic, pos as i32, length as i32, args);
        }
    }

    /// The raw source bytes (positions are byte offsets, identical to Go).
    #[inline]
    pub(crate) fn bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    // ────────────────────────────────────────────────────────────────────────
    // Character access (Go: char / charAt / charAndSize)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (s *Scanner) char() rune` — NOTE: even though this returns a
    /// rune, it only decodes the current byte. It must be checked against
    /// utf8.RuneSelf (0x80) to verify that a call to char_and_size is not
    /// needed.
    #[inline]
    pub(crate) fn char_(&self) -> i32 {
        if self.state.pos < self.end {
            self.bytes()[self.state.pos] as i32
        } else {
            -1
        }
    }

    /// Go: `func (s *Scanner) charAt(offset int) rune` — decodes the byte at
    /// the offset only.
    #[inline]
    pub(crate) fn char_at(&self, offset: usize) -> i32 {
        if self.state.pos + offset < self.end {
            self.bytes()[self.state.pos + offset] as i32
        } else {
            -1
        }
    }

    /// Go: `func (s *Scanner) charAndSize() (rune, int)` — fast path: a single
    /// ASCII byte (the vast majority of source bytes), then full UTF-8
    /// decoding. Returns (0xFFFD, 0) at end-of-text, exactly as Go's
    /// utf8.DecodeRuneInString does for the empty string.
    #[inline]
    pub(crate) fn char_and_size(&self) -> (i32, usize) {
        if self.state.pos < self.end {
            let b = self.bytes()[self.state.pos];
            if b < 0x80 {
                return (b as i32, 1);
            }
        }
        crate::go_shims::decode_rune_in_string(&self.bytes()[self.state.pos..self.end])
    }

    /// Go: `func (s *Scanner) scanASCIIWhile(pred func(byte) bool)`.
    #[inline]
    pub(crate) fn scan_ascii_while(&mut self, pred: impl Fn(u8) -> bool) {
        let text = &self.bytes()[self.state.pos..self.end];
        let mut i = 0;
        while i < text.len() {
            let b = text[i];
            if b >= 0x80 || !pred(b) {
                break;
            }
            i += 1;
        }
        self.state.pos += i;
    }

    /// Go: `s.tokenValue = s.text[start:s.pos]`.
    #[inline]
    pub(crate) fn set_token_value_slice(&mut self, start: usize) {
        let value = &self.bytes()[start..self.state.pos];
        self.state.token_value = Cow::Borrowed(value);
    }

    /// Go: `s.tokenValue += s.scanIdentifierParts(variant)` — the fast path
    /// re-borrows the combined slice from the source text when both halves are
    /// borrowed (the common, escape-free case).
    pub(crate) fn append_token_value(&mut self, parts: Cow<'a, [u8]>) {
        let previous = std::mem::replace(&mut self.state.token_value, Cow::Borrowed(&[]));
        self.state.token_value = match (previous, parts) {
            (Cow::Borrowed(b), Cow::Borrowed(p)) => {
                let start = self.state.pos - b.len() - p.len();
                Cow::Borrowed(&self.bytes()[start..self.state.pos])
            }
            (Cow::Borrowed(b), Cow::Owned(p)) => {
                let mut v = Vec::with_capacity(b.len() + p.len());
                v.extend_from_slice(b);
                v.extend_from_slice(&p);
                Cow::Owned(v)
            }
            (Cow::Owned(mut v), p) => {
                v.extend_from_slice(&p);
                Cow::Owned(v)
            }
        };
    }

    // ────────────────────────────────────────────────────────────────────────
    // JSDoc tag flags
    // ────────────────────────────────────────────────────────────────────────

    /// scanJSDocCommentForTags scans a JSDoc comment for @deprecated, @see,
    /// and @link tags, setting the appropriate token flags. Called during
    /// scanning when a JSDoc comment is detected.
    pub(crate) fn scan_jsdoc_comment_for_tags(&mut self, mut comment_text: &[u8]) {
        loop {
            let Some(i) = memchr::memchr(b'@', comment_text) else {
                return;
            };
            comment_text = &comment_text[i + 1..];
            if !self
                .state
                .token_flags
                .intersects(TokenFlags::PRECEDING_JSDOC_WITH_DEPRECATED)
                && has_jsdoc_tag(comment_text, &["deprecated"])
            {
                self.state.token_flags |= TokenFlags::PRECEDING_JSDOC_WITH_DEPRECATED;
            }
            if !self
                .state
                .token_flags
                .intersects(TokenFlags::PRECEDING_JSDOC_WITH_SEE_OR_LINK)
                && has_jsdoc_tag(comment_text, &["see", "link", "linkcode", "linkplain"])
            {
                self.state.token_flags |= TokenFlags::PRECEDING_JSDOC_WITH_SEE_OR_LINK;
            }
            if self
                .state
                .token_flags
                .contains(TokenFlags::PRECEDING_JSDOC_WITH_DEPRECATED
                    | TokenFlags::PRECEDING_JSDOC_WITH_SEE_OR_LINK)
            {
                return;
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Scan() — the hot loop
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (s *Scanner) Scan() ast.Kind`.
    pub fn scan(&mut self) -> Kind {
        self.state.full_start_pos = self.state.pos;
        self.state.token_flags = TokenFlags::NONE;
        loop {
            let ch = self.char_();
            self.state.token_start = self.state.pos;

            match u8::try_from(ch) {
                Ok(b'\t' | 0x0B | 0x0C | b' ') => {
                    self.state.pos += 1;
                    if self.skip_trivia {
                        continue;
                    }
                    loop {
                        let (ch, size) = self.char_and_size();
                        if !stringutil::is_white_space_single_line(as_char(ch)) {
                            break;
                        }
                        self.state.pos += size;
                    }
                    self.state.token = Kind::WhitespaceTrivia;
                }
                Ok(b'\n' | b'\r') => {
                    self.state.token_flags |= TokenFlags::PRECEDING_LINE_BREAK;
                    if self.skip_trivia {
                        self.state.pos += 1;
                        self.scan_ascii_while(|b| b == b' ' || (0x09..=0x0D).contains(&b));
                        continue;
                    }
                    if ch == b'\r' as i32 && self.char_at(1) == b'\n' as i32 {
                        self.state.pos += 2;
                    } else {
                        self.state.pos += 1;
                    }
                    self.state.token = Kind::NewLineTrivia;
                }
                Ok(b'!') => {
                    if self.char_at(1) == b'=' as i32 {
                        if self.char_at(2) == b'=' as i32 {
                            self.state.pos += 3;
                            self.state.token = Kind::ExclamationEqualsEqualsToken;
                        } else {
                            self.state.pos += 2;
                            self.state.token = Kind::ExclamationEqualsToken;
                        }
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::ExclamationToken;
                    }
                }
                Ok(b'"' | b'\'') => {
                    let value = self.scan_string(false /*jsx_attribute_string*/);
                    self.state.token_value = value;
                    self.state.token = Kind::StringLiteral;
                }
                Ok(b'`') => {
                    self.state.token =
                        self.scan_template_and_set_token_value(false /*should_emit_invalid_escape_error*/);
                }
                Ok(b'%') => {
                    if self.char_at(1) == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::PercentEqualsToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::PercentToken;
                    }
                }
                Ok(b'&') => {
                    let next = self.char_at(1);
                    if next == b'&' as i32 {
                        if self.char_at(2) == b'=' as i32 {
                            self.state.pos += 3;
                            self.state.token = Kind::AmpersandAmpersandEqualsToken;
                        } else {
                            self.state.pos += 2;
                            self.state.token = Kind::AmpersandAmpersandToken;
                        }
                    } else if next == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::AmpersandEqualsToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::AmpersandToken;
                    }
                }
                Ok(b'(') => {
                    self.state.pos += 1;
                    self.state.token = Kind::OpenParenToken;
                }
                Ok(b')') => {
                    self.state.pos += 1;
                    self.state.token = Kind::CloseParenToken;
                }
                Ok(b'*') => {
                    let next = self.char_at(1);
                    if next == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::AsteriskEqualsToken;
                    } else if next == b'*' as i32 {
                        if self.char_at(2) == b'=' as i32 {
                            self.state.pos += 3;
                            self.state.token = Kind::AsteriskAsteriskEqualsToken;
                        } else {
                            self.state.pos += 2;
                            self.state.token = Kind::AsteriskAsteriskToken;
                        }
                    } else {
                        self.state.pos += 1;
                        if self.state.skip_jsdoc_leading_asterisks != 0
                            && !self
                                .state
                                .token_flags
                                .intersects(TokenFlags::PRECEDING_JSDOC_LEADING_ASTERISKS)
                            && self
                                .state
                                .token_flags
                                .intersects(TokenFlags::PRECEDING_LINE_BREAK)
                        {
                            self.state.token_flags |= TokenFlags::PRECEDING_JSDOC_LEADING_ASTERISKS;
                            continue;
                        }
                        self.state.token = Kind::AsteriskToken;
                    }
                }
                Ok(b'+') => {
                    let next = self.char_at(1);
                    if next == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::PlusEqualsToken;
                    } else if next == b'+' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::PlusPlusToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::PlusToken;
                    }
                }
                Ok(b',') => {
                    self.state.pos += 1;
                    self.state.token = Kind::CommaToken;
                }
                Ok(b'-') => {
                    let next = self.char_at(1);
                    if next == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::MinusEqualsToken;
                    } else if next == b'-' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::MinusMinusToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::MinusToken;
                    }
                }
                Ok(b'.') => {
                    let next = self.char_at(1);
                    if stringutil::is_digit(as_char(next)) {
                        self.state.token = self.scan_number();
                    } else if next == b'.' as i32 && self.char_at(2) == b'.' as i32 {
                        self.state.pos += 3;
                        self.state.token = Kind::DotDotDotToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::DotToken;
                    }
                }
                Ok(b'/') => {
                    // Single-line comment
                    if self.char_at(1) == b'/' as i32 {
                        self.state.pos += 2;

                        loop {
                            self.scan_ascii_while(|b| b != b'\n' && b != b'\r');
                            let (ch, size) = self.char_and_size();
                            if size == 0 || stringutil::is_line_break(as_char(ch)) {
                                break;
                            }
                            self.state.pos += size;
                        }

                        let token_start = self.state.token_start;
                        let pos = self.state.pos;
                        self.process_comment_directive(token_start, pos, false);

                        if self.skip_trivia {
                            continue;
                        }
                        self.state.token = Kind::SingleLineCommentTrivia;
                        return self.state.token;
                    }
                    // Multi-line comment
                    if self.char_at(1) == b'*' as i32 {
                        self.state.pos += 2;
                        let is_jsdoc =
                            self.char_() == b'*' as i32 && self.char_at(1) != b'/' as i32;

                        let mut comment_closed = false;
                        let mut last_line_start = self.state.token_start;
                        loop {
                            self.scan_ascii_while(|b| b != b'*' && b != b'\n' && b != b'\r');
                            let (ch, size) = self.char_and_size();
                            if size == 0 {
                                break;
                            }

                            if ch == b'*' as i32 && self.char_at(1) == b'/' as i32 {
                                self.state.pos += 2;
                                comment_closed = true;
                                break;
                            }

                            self.state.pos += size;

                            if stringutil::is_line_break(as_char(ch)) {
                                last_line_start = self.state.pos;
                                self.state.token_flags |= TokenFlags::PRECEDING_LINE_BREAK;
                            }
                        }

                        if is_jsdoc {
                            self.state.token_flags |= TokenFlags::PRECEDING_JSDOC_COMMENT;
                            let comment_bytes =
                                &self.bytes()[self.state.token_start..self.state.pos];
                            self.scan_jsdoc_comment_for_tags(comment_bytes);
                        }

                        self.process_comment_directive(last_line_start, self.state.pos, true);

                        if !comment_closed {
                            self.error(&ASTERISK_SLASH_EXPECTED);
                        }

                        if self.skip_trivia {
                            continue;
                        }

                        if !comment_closed {
                            self.state.token_flags |= TokenFlags::UNTERMINATED;
                        }
                        self.state.token = Kind::MultiLineCommentTrivia;
                        return self.state.token;
                    }
                    if self.char_at(1) == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::SlashEqualsToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::SlashToken;
                    }
                }
                Ok(b'0') => {
                    let next = self.char_at(1);
                    if next == b'X' as i32 || next == b'x' as i32 {
                        let start = self.state.pos;
                        self.state.pos += 2;
                        let digits = self.scan_hex_digits(1, true, true);
                        let digits = if digits.is_empty() {
                            self.error(&HEXADECIMAL_DIGIT_EXPECTED);
                            Cow::Borrowed(&b"0"[..])
                        } else {
                            digits
                        };
                        // Go: hexNumberCache memo (dropped — see header). The
                        // raw-text reuse below is the allocation-free path.
                        let raw_text = &self.bytes()[start..self.state.pos];
                        if raw_text.starts_with(b"0x") && &raw_text[2..] == digits.as_ref() {
                            self.state.token_value = Cow::Borrowed(raw_text);
                        } else {
                            let mut v = Vec::with_capacity(2 + digits.len());
                            v.extend_from_slice(b"0x");
                            v.extend_from_slice(&digits);
                            self.state.token_value = Cow::Owned(v);
                        }
                        self.state.token_flags |= TokenFlags::HEX_SPECIFIER;
                        self.state.token = self.scan_big_int_suffix();
                        return self.state.token;
                    }
                    if next == b'B' as i32 || next == b'b' as i32 {
                        self.state.pos += 2;
                        let digits = self.scan_binary_or_octal_digits(2);
                        let digits = if digits.is_empty() {
                            self.error(&BINARY_DIGIT_EXPECTED);
                            Cow::Owned(b"0".to_vec())
                        } else {
                            digits
                        };
                        self.set_base_prefixed_number_value(b"0b", digits);
                        self.state.token_flags |= TokenFlags::BINARY_SPECIFIER;
                        self.state.token = self.scan_big_int_suffix();
                        return self.state.token;
                    }
                    if next == b'O' as i32 || next == b'o' as i32 {
                        self.state.pos += 2;
                        let digits = self.scan_binary_or_octal_digits(8);
                        let digits = if digits.is_empty() {
                            self.error(&OCTAL_DIGIT_EXPECTED);
                            Cow::Owned(b"0".to_vec())
                        } else {
                            digits
                        };
                        self.set_base_prefixed_number_value(b"0o", digits);
                        self.state.token_flags |= TokenFlags::OCTAL_SPECIFIER;
                        self.state.token = self.scan_big_int_suffix();
                        return self.state.token;
                    }
                    // Go: fallthrough to the digit case.
                    self.state.token = self.scan_number();
                }
                Ok(b'1'..=b'9') => {
                    self.state.token = self.scan_number();
                }
                Ok(b':') => {
                    self.state.pos += 1;
                    self.state.token = Kind::ColonToken;
                }
                Ok(b';') => {
                    self.state.pos += 1;
                    self.state.token = Kind::SemicolonToken;
                }
                Ok(b'<') => {
                    if self.char_at(1) == b'<' as i32 && is_conflict_marker_trivia(self.text, self.state.pos) {
                        let new_pos = self.scan_conflict_marker_at(self.state.pos);
                        self.state.pos = new_pos;
                        if self.skip_trivia {
                            continue;
                        }
                        self.state.token = Kind::ConflictMarkerTrivia;
                        return self.state.token;
                    }
                    if self.char_at(1) == b'<' as i32 {
                        if self.char_at(2) == b'=' as i32 {
                            self.state.pos += 3;
                            self.state.token = Kind::LessThanLessThanEqualsToken;
                        } else {
                            self.state.pos += 2;
                            self.state.token = Kind::LessThanLessThanToken;
                        }
                    } else if self.char_at(1) == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::LessThanEqualsToken;
                    } else if self.language_variant == LanguageVariant::JSX
                        && self.char_at(1) == b'/' as i32
                        && self.char_at(2) != b'*' as i32
                    {
                        self.state.pos += 2;
                        self.state.token = Kind::LessThanSlashToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::LessThanToken;
                    }
                }
                Ok(b'=') => {
                    if self.char_at(1) == b'=' as i32 && is_conflict_marker_trivia(self.text, self.state.pos) {
                        let new_pos = self.scan_conflict_marker_at(self.state.pos);
                        self.state.pos = new_pos;
                        if self.skip_trivia {
                            continue;
                        }
                        self.state.token = Kind::ConflictMarkerTrivia;
                        return self.state.token;
                    }
                    if self.char_at(1) == b'=' as i32 {
                        if self.char_at(2) == b'=' as i32 {
                            self.state.pos += 3;
                            self.state.token = Kind::EqualsEqualsEqualsToken;
                        } else {
                            self.state.pos += 2;
                            self.state.token = Kind::EqualsEqualsToken;
                        }
                    } else if self.char_at(1) == b'>' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::EqualsGreaterThanToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::EqualsToken;
                    }
                }
                Ok(b'>') => {
                    if self.char_at(1) == b'>' as i32 && is_conflict_marker_trivia(self.text, self.state.pos) {
                        let new_pos = self.scan_conflict_marker_at(self.state.pos);
                        self.state.pos = new_pos;
                        if self.skip_trivia {
                            continue;
                        }
                        self.state.token = Kind::ConflictMarkerTrivia;
                        return self.state.token;
                    }
                    self.state.pos += 1;
                    self.state.token = Kind::GreaterThanToken;
                }
                Ok(b'?') => {
                    if self.char_at(1) == b'.' as i32 && !stringutil::is_digit(as_char(self.char_at(2))) {
                        self.state.pos += 2;
                        self.state.token = Kind::QuestionDotToken;
                    } else if self.char_at(1) == b'?' as i32 {
                        if self.char_at(2) == b'=' as i32 {
                            self.state.pos += 3;
                            self.state.token = Kind::QuestionQuestionEqualsToken;
                        } else {
                            self.state.pos += 2;
                            self.state.token = Kind::QuestionQuestionToken;
                        }
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::QuestionToken;
                    }
                }
                Ok(b'[') => {
                    self.state.pos += 1;
                    self.state.token = Kind::OpenBracketToken;
                }
                Ok(b']') => {
                    self.state.pos += 1;
                    self.state.token = Kind::CloseBracketToken;
                }
                Ok(b'^') => {
                    if self.char_at(1) == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::CaretEqualsToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::CaretToken;
                    }
                }
                Ok(b'{') => {
                    self.state.pos += 1;
                    self.state.token = Kind::OpenBraceToken;
                }
                Ok(b'|') => {
                    if self.char_at(1) == b'|' as i32 && is_conflict_marker_trivia(self.text, self.state.pos) {
                        let new_pos = self.scan_conflict_marker_at(self.state.pos);
                        self.state.pos = new_pos;
                        if self.skip_trivia {
                            continue;
                        }
                        self.state.token = Kind::ConflictMarkerTrivia;
                        return self.state.token;
                    }
                    if self.char_at(1) == b'|' as i32 {
                        if self.char_at(2) == b'=' as i32 {
                            self.state.pos += 3;
                            self.state.token = Kind::BarBarEqualsToken;
                        } else {
                            self.state.pos += 2;
                            self.state.token = Kind::BarBarToken;
                        }
                    } else if self.char_at(1) == b'=' as i32 {
                        self.state.pos += 2;
                        self.state.token = Kind::BarEqualsToken;
                    } else {
                        self.state.pos += 1;
                        self.state.token = Kind::BarToken;
                    }
                }
                Ok(b'}') => {
                    self.state.pos += 1;
                    self.state.token = Kind::CloseBraceToken;
                }
                Ok(b'~') => {
                    self.state.pos += 1;
                    self.state.token = Kind::TildeToken;
                }
                Ok(b'@') => {
                    self.state.pos += 1;
                    self.state.token = Kind::AtToken;
                }
                Ok(b'\\') => {
                    if self.scan_identifier(0, IdentifierVariant::Standard) {
                        self.state.token = self.token_value_kind();
                    } else {
                        self.scan_invalid_character();
                    }
                }
                Ok(b'#') => {
                    if self.char_at(1) == b'!' as i32 {
                        if self.state.pos == 0 {
                            self.state.pos += 2;
                            loop {
                                let (ch, size) = self.char_and_size();
                                if !(size > 0 && !stringutil::is_line_break(as_char(ch))) {
                                    break;
                                }
                                self.state.pos += size;
                            }
                            continue;
                        }
                        self.error_at(&X_CAN_ONLY_BE_USED_AT_THE_START_OF_A_FILE, self.state.pos, 2, &[]);
                        self.state.pos += 2;
                        self.state.token = Kind::Unknown;
                        return self.state.token;
                    }
                    if !self.scan_identifier(1, IdentifierVariant::Standard) {
                        self.error_at(&INVALID_CHARACTER, self.state.pos - 1, 1, &[]);
                        self.state.token_value = Cow::Borrowed(b"#");
                    }
                    self.state.token = Kind::PrivateIdentifier;
                }
                Ok(_) | Err(_) => {
                    // Go: default
                    if ch < 0 {
                        self.state.token = Kind::EndOfFile;
                        return self.state.token;
                    }
                    if self.scan_identifier(0, IdentifierVariant::Standard) {
                        self.state.token = self.token_value_kind();
                        return self.state.token;
                    }
                    let (ch, size) = self.char_and_size();
                    if ch == 0xFFFD {
                        // Go: utf8.RuneError. With a valid-UTF-8 `text` this
                        // fires for a real U+FFFD in the source, exactly as in
                        // Go (DecodeRuneInString returns RuneError for it);
                        // invalid source bytes cannot be represented by `&str`.
                        self.error_at(&FILE_APPEARS_TO_BE_BINARY, 0, 0, &[]);
                        self.state.pos = self.text.len();
                        self.state.token = Kind::NonTextFileMarkerTrivia;
                        return self.state.token;
                    }
                    if stringutil::is_white_space_single_line(as_char(ch)) {
                        self.state.pos += size;

                        // If we get here and it's not 0x0085 (nextLine), then we're handling non-ASCII whitespace.
                        // Handle skipTrivia like we do in the space case above.
                        if ch == 0x0085 || self.skip_trivia {
                            continue;
                        }

                        loop {
                            let (ch, size) = self.char_and_size();
                            if !stringutil::is_white_space_single_line(as_char(ch)) {
                                break;
                            }
                            self.state.pos += size;
                        }
                        self.state.token = Kind::WhitespaceTrivia;
                        return self.state.token;
                    }
                    if stringutil::is_line_break(as_char(ch)) {
                        self.state.token_flags |= TokenFlags::PRECEDING_LINE_BREAK;
                        self.state.pos += size;
                        continue;
                    }
                    self.scan_invalid_character();
                    return self.state.token;
                }
            }
            return self.state.token;
        }
    }

    /// Go: `func (s *Scanner) processCommentDirective(start int, end int, multiline bool)`.
    pub(crate) fn process_comment_directive(&mut self, start: usize, end: usize, multiline: bool) {
        let bytes = self.bytes();
        // Skip starting slashes and whitespace
        let mut pos = start;
        if multiline {
            // Skip whitespace
            while pos < end && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
                pos += 1;
            }
            // Skip combinations of / and *
            while pos < end && (bytes[pos] == b'/' || bytes[pos] == b'*') {
                pos += 1;
            }
        } else {
            // Skip opening //
            pos += 2;
            // Skip another / if present
            while pos < end && bytes[pos] == b'/' {
                pos += 1;
            }
        }
        // Skip whitespace
        while pos < end && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
            pos += 1;
        }
        // Directive must start with '@'
        if !(pos < end && bytes[pos] == b'@') {
            return;
        }
        pos += 1;
        let kind = if bytes[pos..].starts_with(b"ts-expect-error") {
            CommentDirectiveKind::ExpectError
        } else if bytes[pos..].starts_with(b"ts-ignore") {
            CommentDirectiveKind::Ignore
        } else {
            return;
        };
        self.state.comment_directives.push(CommentDirective {
            loc: TextRange::new(start as i32, end as i32),
            kind,
        });
    }

    // ────────────────────────────────────────────────────────────────────────
    // ReScan family
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (s *Scanner) ReScanLessThanToken() ast.Kind`.
    pub fn re_scan_less_than_token(&mut self) -> Kind {
        if self.state.token == Kind::LessThanLessThanToken {
            self.state.pos = self.state.token_start + 1;
            self.state.token = Kind::LessThanToken;
        }
        self.state.token
    }

    /// Go: `func (s *Scanner) ReScanGreaterThanToken() ast.Kind`.
    pub fn re_scan_greater_than_token(&mut self) -> Kind {
        if self.state.token == Kind::GreaterThanToken {
            self.re_scan_greater_than_token_inner();
        }
        self.state.token
    }

    /// Go: `func (s *Scanner) reScanGreaterThanTokenInner()`.
    fn re_scan_greater_than_token_inner(&mut self) {
        self.state.pos = self.state.token_start + 1;
        if self.char_() == b'>' as i32 {
            if self.char_at(1) == b'>' as i32 {
                if self.char_at(2) == b'=' as i32 {
                    self.state.pos += 3;
                    self.state.token = Kind::GreaterThanGreaterThanGreaterThanEqualsToken;
                } else {
                    self.state.pos += 2;
                    self.state.token = Kind::GreaterThanGreaterThanGreaterThanToken;
                }
            } else if self.char_at(1) == b'=' as i32 {
                self.state.pos += 2;
                self.state.token = Kind::GreaterThanGreaterThanEqualsToken;
            } else {
                self.state.pos += 1;
                self.state.token = Kind::GreaterThanGreaterThanToken;
            }
        } else if self.char_() == b'=' as i32 {
            self.state.pos += 1;
            self.state.token = Kind::GreaterThanEqualsToken;
        }
    }

    /// Go: `func (s *Scanner) ReScanTemplateToken(isTaggedTemplate bool) ast.Kind`.
    pub fn re_scan_template_token(&mut self, is_tagged_template: bool) -> Kind {
        self.state.pos = self.state.token_start;
        self.state.token = self.scan_template_and_set_token_value(!is_tagged_template);
        self.state.token
    }

    /// Go: `func (s *Scanner) ReScanAsteriskEqualsToken() ast.Kind`.
    pub fn re_scan_asterisk_equals_token(&mut self) -> Kind {
        if self.state.token != Kind::AsteriskEqualsToken {
            panic!("'ReScanAsteriskEqualsToken' should only be called on a '*='");
        }
        self.state.pos = self.state.token_start + 1;
        self.state.token = Kind::EqualsToken;
        self.state.token
    }

    /// Go: `func (s *Scanner) ReScanSlashToken(reportErrors ...bool) ast.Kind` —
    /// Go's optional variadic bool collapses to a plain parameter.
    pub fn re_scan_slash_token(&mut self, should_report_errors: bool) -> Kind {
        if self.state.token == Kind::SlashToken || self.state.token == Kind::SlashEqualsToken {
            // Quickly get to the end of regex such that we know the flags
            let start_of_reg_exp_body = self.state.token_start + 1;
            let mut p = start_of_reg_exp_body;
            let mut in_escape = false;
            let mut named_capture_groups = false;
            // Although nested character classes are allowed in Unicode Sets mode,
            // an unescaped slash is nevertheless invalid even in a character class in any Unicode mode.
            // This is indicated by Section 12.9.5 Regular Expression Literals of the specification,
            // where nested character classes are not considered at all. (A `[` RegularExpressionClassChar
            // does nothing in a RegularExpressionClass, and a `]` always closes the class.)
            // Additionally, parsing nested character classes will misinterpret regexes like `/[[]/`
            // as unterminated, consuming characters beyond the slash. (This even applies to `/[[]/v`,
            // which should be parsed as a well-terminated regex with an incomplete character class.)
            // Thus we must not handle nested character classes in the first pass.
            let mut in_character_class = false;
            let bytes = self.bytes();
            let text_len = self.end;
            loop {
                // If we reach the end of a file, or hit a newline, then this is an unterminated
                // regex. Report error and return what we have so far.
                if p >= text_len {
                    self.state.token_flags |= TokenFlags::UNTERMINATED;
                    break;
                }
                let ch = bytes[p] as i32;
                if stringutil::is_line_break(as_char(ch)) {
                    self.state.token_flags |= TokenFlags::UNTERMINATED;
                    break;
                }
                if in_escape {
                    // Parsing an escape character;
                    // reset the flag and just advance to the next char.
                    in_escape = false;
                } else if ch == b'/' as i32 && !in_character_class {
                    // A slash within a character class is permissible,
                    // but in general it signals the end of the regexp literal.
                    break;
                } else if ch == b'[' as i32 {
                    in_character_class = true;
                } else if ch == b'\\' as i32 {
                    in_escape = true;
                } else if ch == b']' as i32 {
                    in_character_class = false;
                } else if !in_character_class
                    && ch == b'(' as i32
                    && p + 1 < text_len
                    && bytes[p + 1] == b'?'
                    && p + 2 < text_len
                    && bytes[p + 2] == b'<'
                    && (p + 3 >= text_len || (bytes[p + 3] != b'=' && bytes[p + 3] != b'!'))
                {
                    named_capture_groups = true;
                }
                p += 1;
            }

            let end_of_reg_exp_body = p;
            if self.state.token_flags.intersects(TokenFlags::UNTERMINATED) {
                // Search for the nearest unbalanced bracket for better recovery. Since the expression is
                // invalid anyways, we take nested square brackets into consideration for the best guess.
                p = start_of_reg_exp_body;
                in_escape = false;
                let mut character_class_depth = 0;
                let mut in_decimal_quantifier = false;
                let mut group_depth = 0;
                while p < end_of_reg_exp_body {
                    let ch = bytes[p] as i32;
                    if in_escape {
                        in_escape = false;
                    } else if ch == b'\\' as i32 {
                        in_escape = true;
                    } else if ch == b'[' as i32 {
                        character_class_depth += 1;
                    } else if ch == b']' as i32 && character_class_depth != 0 {
                        character_class_depth -= 1;
                    } else if character_class_depth == 0 {
                        if ch == b'{' as i32 {
                            in_decimal_quantifier = true;
                        } else if ch == b'}' as i32 && in_decimal_quantifier {
                            in_decimal_quantifier = false;
                        } else if !in_decimal_quantifier {
                            if ch == b'(' as i32 {
                                group_depth += 1;
                            } else if ch == b')' as i32 && group_depth != 0 {
                                group_depth -= 1;
                            } else if ch == b')' as i32 || ch == b']' as i32 || ch == b'}' as i32 {
                                // We encountered an unbalanced bracket outside a character class. Treat this position as the end of regex.
                                break;
                            }
                        }
                    }
                    p += 1;
                }
                // Whitespaces and semicolons at the end are not likely to be part of the regex
                while p > start_of_reg_exp_body {
                    let (ch, size) = decode_last_rune_in_string(&bytes[..p]);
                    if !stringutil::is_white_space_like(as_char(ch)) && ch != b';' as i32 {
                        break;
                    }
                    p -= size;
                }
                self.error_at(
                    &UNTERMINATED_REGULAR_EXPRESSION_LITERAL,
                    self.state.token_start,
                    p - self.state.token_start,
                    &[],
                );
            } else {
                // Consume the slash character
                p += 1;
                let mut reg_exp_flags = 0i32;
                while p < text_len {
                    let (ch, size) = crate::go_shims::decode_rune_in_string(&bytes[p..]);
                    if ch == 0xFFFD || !is_identifier_part(ch) {
                        break;
                    }
                    if should_report_errors {
                        match crate::regexp::char_code_to_reg_exp_flag(ch) {
                            None => {
                                self.error_at(
                                    &tsc_diagnostics::UNKNOWN_REGULAR_EXPRESSION_FLAG,
                                    p,
                                    size,
                                    &[],
                                );
                            }
                            Some(flag) => {
                                if reg_exp_flags & flag != 0 {
                                    self.error_at(
                                        &tsc_diagnostics::DUPLICATE_REGULAR_EXPRESSION_FLAG,
                                        p,
                                        size,
                                        &[],
                                    );
                                } else if (reg_exp_flags | flag)
                                    & crate::regexp::REG_EXP_FLAGS_ANY_UNICODE_MODE
                                    == crate::regexp::REG_EXP_FLAGS_ANY_UNICODE_MODE
                                {
                                    self.error_at(
                                        &tsc_diagnostics::THE_UNICODE_U_FLAG_AND_THE_UNICODE_SETS_V_FLAG_CANNOT_BE_SET_SIMULTANEOUSLY,
                                        p,
                                        size,
                                        &[],
                                    );
                                } else {
                                    reg_exp_flags |= flag;
                                    self.check_regular_expression_flag_availability(flag, p, size);
                                }
                            }
                        }
                    }
                    p += size;
                }
                if should_report_errors {
                    self.state.pos = start_of_reg_exp_body;
                    let save_end = self.end;
                    let save_token_pos = self.state.token_start;
                    let save_token_flags = self.state.token_flags;
                    self.end = end_of_reg_exp_body;
                    let mut parser = RegExpParser {
                        scanner: self,
                        end: end_of_reg_exp_body,
                        reg_exp_flags,
                        any_unicode_mode: reg_exp_flags & crate::regexp::REG_EXP_FLAGS_ANY_UNICODE_MODE != 0,
                        unicode_sets_mode: reg_exp_flags & crate::regexp::REG_EXP_FLAGS_UNICODE_SETS != 0,
                        annex_b: true,
                        named_capture_groups,
                        ..RegExpParser::new_state()
                    };
                    parser.run();
                    self.end = save_end;
                    self.state.pos = p;
                    self.state.token_start = save_token_pos;
                    self.state.token_flags = save_token_flags;
                } else {
                    self.state.pos = p;
                }
            }

            self.state.pos = p;
            let value = &self.bytes()[self.state.token_start..self.state.pos];
            self.state.token_value = Cow::Borrowed(value);
            self.state.token = Kind::RegularExpressionLiteral;
        }
        self.state.token
    }

    /// Go: `func (s *Scanner) checkRegularExpressionFlagAvailability(flag regularExpressionFlags, pos int, size int)`
    /// (regexp.go).
    pub(crate) fn check_regular_expression_flag_availability(
        &mut self,
        flag: i32,
        pos: usize,
        size: usize,
    ) {
        if let Some(available_from) = crate::regexp::reg_exp_flag_first_available_language_version(flag) {
            if self.language_version() < available_from {
                self.error_at(
                    &tsc_diagnostics::THIS_REGULAR_EXPRESSION_FLAG_IS_ONLY_AVAILABLE_WHEN_TARGETING_0_OR_LATER,
                    pos,
                    size,
                    &[crate::go_shims::script_target_string(available_from).to_lowercase()],
                );
            }
        }
    }

    /// Go: `func (s *Scanner) ReScanJsxToken(allowMultilineJsxText bool) ast.Kind`.
    pub fn re_scan_jsx_token(&mut self, allow_multiline_jsx_text: bool) -> Kind {
        self.state.pos = self.state.full_start_pos;
        self.state.token_start = self.state.full_start_pos;
        self.state.token = self.scan_jsx_token_ex(allow_multiline_jsx_text);
        self.state.token
    }

    /// Go: `func (s *Scanner) ReScanHashToken() ast.Kind`.
    pub fn re_scan_hash_token(&mut self) -> Kind {
        if self.state.token == Kind::PrivateIdentifier {
            self.state.pos = self.state.token_start + 1;
            self.state.token = Kind::HashToken;
        }
        self.state.token
    }

    /// Go: `func (s *Scanner) ReScanQuestionToken() ast.Kind`.
    pub fn re_scan_question_token(&mut self) -> Kind {
        if self.state.token != Kind::QuestionQuestionToken {
            panic!("'reScanQuestionToken' should only be called on a '??'");
        }
        self.state.pos = self.state.token_start + 1;
        self.state.token = Kind::QuestionToken;
        self.state.token
    }

    // ────────────────────────────────────────────────────────────────────────
    // JSX scanning
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (s *Scanner) ScanJsxToken() ast.Kind`.
    pub fn scan_jsx_token(&mut self) -> Kind {
        self.scan_jsx_token_ex(true /*allow_multiline_jsx_text*/)
    }

    /// Go: `func (s *Scanner) ScanJsxTokenEx(allowMultilineJsxText bool) ast.Kind`.
    pub fn scan_jsx_token_ex(&mut self, allow_multiline_jsx_text: bool) -> Kind {
        self.state.full_start_pos = self.state.pos;
        self.state.token_start = self.state.pos;
        let ch = self.char_();
        match u8::try_from(ch) {
            Err(_) => {
                self.state.token = Kind::EndOfFile;
            }
            Ok(b'<') => {
                if self.char_at(1) == b'/' as i32 {
                    self.state.pos += 2;
                    self.state.token = Kind::LessThanSlashToken;
                } else {
                    self.state.pos += 1;
                    self.state.token = Kind::LessThanToken;
                }
            }
            Ok(b'{') => {
                self.state.pos += 1;
                self.state.token = Kind::OpenBraceToken;
            }
            Ok(_) => {
                // First non-whitespace character on this line.
                let mut first_non_whitespace: isize = 0;
                // These initial values are special because the first line is:
                // firstNonWhitespace = 0 to indicate that we want leading whitespace
                loop {
                    let (ch, size) = self.char_and_size();
                    if size == 0 || ch == b'{' as i32 {
                        break;
                    }
                    if ch == b'<' as i32 {
                        if is_conflict_marker_trivia(self.text, self.state.pos) {
                            let new_pos = self.scan_conflict_marker_at(self.state.pos);
                            self.state.pos = new_pos;
                            self.state.token = Kind::ConflictMarkerTrivia;
                            return self.state.token;
                        }
                        break;
                    }
                    if ch == b'>' as i32 {
                        self.error_at(&UNEXPECTED_TOKEN_DID_YOU_MEAN_OR_GT, self.state.pos, 1, &[]);
                    } else if ch == b'}' as i32 {
                        self.error_at(&UNEXPECTED_TOKEN_DID_YOU_MEAN_OR_RBRACE, self.state.pos, 1, &[]);
                    }
                    // FirstNonWhitespace is 0, then we only see whitespaces so far. If we see a linebreak, we want to ignore that whitespaces.
                    // i.e (- : whitespace)
                    //      <div>----
                    //      </div> becomes <div></div>
                    //
                    //      <div>----</div> becomes <div>----</div>
                    if stringutil::is_line_break(as_char(ch)) && first_non_whitespace == 0 {
                        first_non_whitespace = -1;
                    } else if !allow_multiline_jsx_text
                        && stringutil::is_line_break(as_char(ch))
                        && first_non_whitespace > 0
                    {
                        // Stop JsxText on each line during formatting. This allows the formatter to
                        // indent each line correctly.
                        break;
                    } else if !stringutil::is_white_space_like(as_char(ch)) {
                        first_non_whitespace = self.state.pos as isize;
                    }
                    self.state.pos += size;
                }
                let value = &self.bytes()[self.state.full_start_pos..self.state.pos];
                self.state.token_value = Cow::Borrowed(value);
                self.state.token = Kind::JsxText;
                if first_non_whitespace == -1 {
                    self.state.token = Kind::JsxTextAllWhiteSpaces;
                }
            }
        }
        self.state.token
    }

    /// Scans a JSX identifier; these differ from normal identifiers in that they
    /// allow dashes.
    ///
    /// Go: `func (s *Scanner) ScanJsxIdentifier() ast.Kind`.
    pub fn scan_jsx_identifier(&mut self) -> Kind {
        if token_is_identifier_or_keyword(self.state.token) {
            // An identifier or keyword has already been parsed - check for a `-` or a single instance of `:` and then append it and
            // everything after it to the token
            // Do note that this means that this means that `scanJsxIdentifier` effectively _mutates_ the visible token without advancing to a new token
            // Any caller should be expecting this behavior and should only read the pos or token value after calling it.
            // Here scanIdentifierParts is reused to ensure Unicode escapes are handled.
            let parts = self.scan_identifier_parts(IdentifierVariant::Jsx);
            self.append_token_value(parts);
            self.state.token = self.token_value_kind();
        }
        self.state.token
    }

    /// Go: `func (s *Scanner) ScanJsxAttributeValue() ast.Kind`.
    pub fn scan_jsx_attribute_value(&mut self) -> Kind {
        self.state.full_start_pos = self.state.pos;
        // Skip whitespace between '=' and the value so tokenStart lands on the
        // opening quote, not on trivia.
        loop {
            let (ch, size) = self.char_and_size();
            if !(size > 0 && stringutil::is_white_space_like(as_char(ch))) {
                break;
            }
            self.state.pos += size;
        }
        self.state.token_start = self.state.pos;
        match u8::try_from(self.char_()) {
            Ok(b'"') | Ok(b'\'') => {
                let value = self.scan_string(true /*jsx_attribute_string*/);
                self.state.token_value = value;
                self.state.token = Kind::StringLiteral;
                self.state.token
            }
            _ => {
                // If this scans anything other than `{`, it's a parse error.
                self.scan()
            }
        }
    }

    /// Go: `func (s *Scanner) ReScanJsxAttributeValue() ast.Kind`.
    pub fn re_scan_jsx_attribute_value(&mut self) -> Kind {
        self.state.pos = self.state.full_start_pos;
        self.state.token_start = self.state.full_start_pos;
        self.scan_jsx_attribute_value()
    }

    // ────────────────────────────────────────────────────────────────────────
    // JSDoc scanning
    // ────────────────────────────────────────────────────────────────────────

    /// In addition to the usual JSDoc ast.Kinds, can also return
    /// Kind::JSDocCommentTextToken.
    ///
    /// Go: `func (s *Scanner) ScanJSDocCommentTextToken(inBackticks bool) ast.Kind`.
    pub fn scan_jsdoc_comment_text_token(&mut self, in_backticks: bool) -> Kind {
        self.state.full_start_pos = self.state.pos;
        self.state.token_flags = TokenFlags::NONE;
        if self.state.pos >= self.text.len() {
            self.state.token = Kind::EndOfFile;
            return self.state.token;
        }
        self.state.token_start = self.state.pos;
        let bytes = self.bytes();
        loop {
            let (ch, size) = self.char_and_size();
            if !(self.state.pos < self.text.len()
                && !stringutil::is_line_break(as_char(ch))
                && ch != b'`' as i32)
            {
                break;
            }
            if !in_backticks {
                if ch == b'{' as i32 {
                    break;
                } else if ch == b'@' as i32 {
                    // @ doesn't start a new tag inside ``, and elsewhere, only after whitespace and before identifier
                    let (previous, _) = decode_last_rune_in_string(&bytes[..self.state.pos]);
                    if stringutil::is_white_space_single_line(as_char(previous)) {
                        let (next, _) =
                            crate::go_shims::decode_rune_in_string(&bytes[self.state.pos + size..]);
                        if is_identifier_start(next) {
                            break;
                        }
                    }
                }
            }
            self.state.pos += size;
        }
        if self.state.pos == self.state.token_start {
            return self.scan_jsdoc_token();
        }
        let value = &self.bytes()[self.state.token_start..self.state.pos];
        self.state.token_value = Cow::Borrowed(value);
        self.state.token = Kind::JSDocCommentTextToken;
        self.state.token
    }

    /// Peek at the character at the current scanner position (expected to be
    /// right after '@') and return true if a JSDoc tag can follow. Identifier
    /// starts indicate a tag name. Whitespace, newlines, and EOF are also
    /// accepted to support incomplete tags for code completion.
    ///
    /// Go: `func (s *Scanner) CanFollowJSDocAt() bool`.
    pub fn can_follow_jsdoc_at(&self) -> bool {
        if self.state.pos >= self.text.len() {
            return true;
        }
        let (ch, _) = crate::go_shims::decode_rune_in_string(&self.bytes()[self.state.pos..]);
        is_identifier_start(ch)
            || stringutil::is_white_space_single_line(as_char(ch))
            || stringutil::is_line_break(as_char(ch))
    }

    /// Go: `func (s *Scanner) ScanJSDocToken() ast.Kind`.
    pub fn scan_jsdoc_token(&mut self) -> Kind {
        self.state.full_start_pos = self.state.pos;
        self.state.token_flags = TokenFlags::NONE;
        if self.state.pos >= self.text.len() {
            self.state.token = Kind::EndOfFile;
            return self.state.token;
        }

        self.state.token_start = self.state.pos;
        let (ch, size) = self.char_and_size();
        self.state.pos += size;
        let newline_token = |s: &mut Scanner| {
            s.state.token_flags |= TokenFlags::PRECEDING_LINE_BREAK;
            s.state.token = Kind::NewLineTrivia;
            s.state.token
        };
        match u8::try_from(ch) {
            Ok(b'\t' | 0x0B | 0x0C | b' ') => {
                loop {
                    let (ch, size) = self.char_and_size();
                    if !(size > 0 && stringutil::is_white_space_single_line(as_char(ch))) {
                        break;
                    }
                    self.state.pos += size;
                }
                self.state.token = Kind::WhitespaceTrivia;
                self.state.token
            }
            Ok(b'@') => {
                self.state.token = Kind::AtToken;
                self.state.token
            }
            Ok(b'\r') => {
                if self.char_() == b'\n' as i32 {
                    self.state.pos += 1;
                }
                newline_token(self)
            }
            Ok(b'\n') => newline_token(self),
            Ok(b'*') => {
                self.state.token = Kind::AsteriskToken;
                self.state.token
            }
            Ok(b'{') => {
                self.state.token = Kind::OpenBraceToken;
                self.state.token
            }
            Ok(b'}') => {
                self.state.token = Kind::CloseBraceToken;
                self.state.token
            }
            Ok(b'[') => {
                self.state.token = Kind::OpenBracketToken;
                self.state.token
            }
            Ok(b']') => {
                self.state.token = Kind::CloseBracketToken;
                self.state.token
            }
            Ok(b'(') => {
                self.state.token = Kind::OpenParenToken;
                self.state.token
            }
            Ok(b')') => {
                self.state.token = Kind::CloseParenToken;
                self.state.token
            }
            Ok(b'<') => {
                self.state.token = Kind::LessThanToken;
                self.state.token
            }
            Ok(b'>') => {
                self.state.token = Kind::GreaterThanToken;
                self.state.token
            }
            Ok(b'=') => {
                self.state.token = Kind::EqualsToken;
                self.state.token
            }
            Ok(b',') => {
                self.state.token = Kind::CommaToken;
                self.state.token
            }
            Ok(b'.') => {
                self.state.token = Kind::DotToken;
                self.state.token
            }
            Ok(b'`') => {
                self.state.token = Kind::BacktickToken;
                self.state.token
            }
            Ok(b'#') => {
                self.state.token = Kind::HashToken;
                self.state.token
            }
            Ok(_) | Err(_) => {
                self.state.pos = self.state.token_start;
                if self.scan_identifier(0, IdentifierVariant::Jsx) {
                    self.state.token = self.token_value_kind();
                    return self.state.token;
                }
                self.state.pos = self.state.token_start + size;
                self.state.token = Kind::Unknown;
                self.state.token
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Identifier scanning
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (s *Scanner) scanIdentifier(prefixLength int, variant identifierVariant) bool`.
    pub(crate) fn scan_identifier(&mut self, prefix_length: usize, variant: IdentifierVariant) -> bool {
        let start = self.state.pos;
        self.state.pos += prefix_length;
        let identifier_start = self.state.pos;
        let mut ch = self.char_();
        // Fast path for simple ASCII identifiers
        if variant != IdentifierVariant::Jsx
            && (stringutil::is_ascii_letter(as_char(ch)) || ch == b'_' as i32 || ch == b'$' as i32)
        {
            self.state.pos += 1;
            self.scan_ascii_while(|b| {
                b.is_ascii_lowercase() || b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'$'
            });
            ch = self.char_();
            if ch < 0x80 && ch != b'\\' as i32 {
                self.set_token_value_slice(start);
                return true;
            }
            self.state.pos = identifier_start;
        }
        let (ch, size) = self.char_and_size();
        if is_identifier_start(ch) {
            let language_variant = match variant {
                IdentifierVariant::Jsx => LanguageVariant::JSX,
                _ => LanguageVariant::Standard,
            };
            let mut size = size;
            let mut ch = ch;
            loop {
                self.state.pos += size;
                let (next_ch, next_size) = self.char_and_size();
                ch = next_ch;
                size = next_size;
                if !is_identifier_part_ex(ch, language_variant) {
                    break;
                }
            }
            self.set_token_value_slice(start);
            if ch == b'\\' as i32 {
                let parts = self.scan_identifier_parts(variant);
                self.append_token_value(parts);
            }
            return true;
        }
        if ch == b'\\' as i32 {
            let allow_surrogate_pair_escape = variant == IdentifierVariant::RegExpGroupName;
            if let Some(escaped) = self.scan_identifier_escape(is_identifier_start, allow_surrogate_pair_escape) {
                let mut v = Vec::new();
                v.extend_from_slice(&self.bytes()[start..identifier_start]);
                v.extend_from_slice(&rune_bytes(escaped));
                let parts = self.scan_identifier_parts(variant);
                v.extend_from_slice(&parts);
                self.state.token_value = Cow::Owned(v);
                return true;
            }
        }
        false
    }

    /// Go: `func (s *Scanner) scanIdentifierParts(variant identifierVariant) string`.
    pub(crate) fn scan_identifier_parts(&mut self, variant: IdentifierVariant) -> Cow<'a, [u8]> {
        let bytes = self.bytes();
        let mut builder = ValueBuilder::new(bytes);
        let mut start = self.state.pos;
        let language_variant = match variant {
            IdentifierVariant::Jsx => LanguageVariant::JSX,
            _ => LanguageVariant::Standard,
        };
        let allow_surrogate_pair_escape = variant == IdentifierVariant::RegExpGroupName;
        loop {
            let (ch, size) = self.char_and_size();
            if is_identifier_part_ex(ch, language_variant) {
                self.state.pos += size;
                continue;
            }
            if ch == b'\\' as i32 {
                let escape_start = self.state.pos;
                if let Some(escaped) =
                    self.scan_identifier_escape(|c| is_identifier_part_ex(c, language_variant), allow_surrogate_pair_escape)
                {
                    builder.append_range(start, escape_start);
                    builder.append_bytes(&rune_bytes(escaped));
                    start = self.state.pos;
                    continue;
                }
            }
            break;
        }
        builder.finish(start, self.state.pos)
    }

    /// Go: `func (s *Scanner) scanIdentifierEscape(isValid func(rune) bool, allowSurrogatePairEscape bool) (rune, bool)`.
    fn scan_identifier_escape(
        &mut self,
        is_valid: impl Fn(i32) -> bool,
        allow_surrogate_pair_escape: bool,
    ) -> Option<i32> {
        let escaped = self.peek_unicode_escape();
        if escaped >= 0 && is_valid(escaped) {
            return Some(self.scan_unicode_escape(true));
        }
        if allow_surrogate_pair_escape
            && self.char_at(2) != b'{' as i32
            && stringutil::is_high_surrogate(escaped)
        {
            // Unlike normal identifiers, group names in regular expressions, whether in Unicode mode or not,
            // accept \u HexLeadSurrogate \u HexTrailSurrogate as part of RegExpIdentifierName.
            // See https://github.com/tc39/ecma262/pull/1869 for the change.
            let saved_pos = self.state.pos;
            let saved_token_flags = self.state.token_flags;
            self.scan_unicode_escape(false);
            // scanLowSurrogateEscape also accepts the braced form used in string literals,
            // but RegExpIdentifierName does not allow it.
            if self.char_at(2) != b'{' as i32 {
                if let Some(code_point) = self.scan_low_surrogate_escape(escaped) {
                    if is_valid(code_point) {
                        return Some(code_point);
                    }
                }
            }
            self.state.pos = saved_pos;
            self.state.token_flags = saved_token_flags;
        }
        None
    }

    // ────────────────────────────────────────────────────────────────────────
    // String / template scanning
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (s *Scanner) scanString(jsxAttributeString bool) string`.
    pub(crate) fn scan_string(&mut self, jsx_attribute_string: bool) -> Cow<'a, [u8]> {
        let quote = self.char_();
        if quote == b'\'' as i32 {
            self.state.token_flags |= TokenFlags::SINGLE_QUOTE;
        }
        self.state.pos += 1;
        let bytes = self.bytes();
        // Fast path for simple strings without escape sequences.
        let str_len = memchr::memchr(quote as u8, &bytes[self.state.pos..self.end]);
        if let Some(str_len) = str_len {
            let str_start = self.state.pos;
            let slice = &bytes[str_start..str_start + str_len];
            if jsx_attribute_string
                || (memchr::memchr(b'\\', slice).is_none()
                    && memchr::memchr(b'\r', slice).is_none()
                    && memchr::memchr(b'\n', slice).is_none())
            {
                self.state.pos += str_len + 1;
                return Cow::Borrowed(slice);
            }
        }
        let mut builder = ValueBuilder::new(bytes);
        let mut start = self.state.pos;
        let final_seg;
        loop {
            let ch = self.char_();
            if ch < 0 {
                final_seg = (start, self.state.pos);
                self.state.token_flags |= TokenFlags::UNTERMINATED;
                self.error(&UNTERMINATED_STRING_LITERAL);
                break;
            }
            if ch == quote {
                final_seg = (start, self.state.pos);
                self.state.pos += 1;
                break;
            }
            if ch == b'\\' as i32 && !jsx_attribute_string {
                builder.append_range(start, self.state.pos);
                let escape = self.scan_escape_sequence(
                    EscapeSequenceScanningFlags::STRING
                        | EscapeSequenceScanningFlags::REPORT_ERRORS,
                );
                builder.append_bytes(&escape);
                start = self.state.pos;
                continue;
            }
            if (ch == b'\n' as i32 || ch == b'\r' as i32) && !jsx_attribute_string {
                final_seg = (start, self.state.pos);
                self.state.token_flags |= TokenFlags::UNTERMINATED;
                self.error(&UNTERMINATED_STRING_LITERAL);
                break;
            }
            self.state.pos += 1;
        }
        builder.finish(final_seg.0, final_seg.1)
    }

    /// Go: `func (s *Scanner) scanTemplateAndSetTokenValue(shouldEmitInvalidEscapeError bool) ast.Kind`.
    pub(crate) fn scan_template_and_set_token_value(
        &mut self,
        should_emit_invalid_escape_error: bool,
    ) -> Kind {
        let started_with_backtick = self.char_() == b'`' as i32;
        self.state.pos += 1;
        let mut start = self.state.pos;
        let bytes = self.bytes();
        let mut builder = ValueBuilder::new(bytes);
        let token;
        let final_seg;
        loop {
            self.scan_ascii_while(|b| b != b'`' && b != b'$' && b != b'\\' && b != b'\r');
            let ch = self.char_();
            if ch < 0 || ch == b'`' as i32 {
                final_seg = (start, self.state.pos);
                if ch == b'`' as i32 {
                    self.state.pos += 1;
                } else {
                    self.state.token_flags |= TokenFlags::UNTERMINATED;
                    self.error(&UNTERMINATED_TEMPLATE_LITERAL);
                }
                token = if started_with_backtick {
                    Kind::NoSubstitutionTemplateLiteral
                } else {
                    Kind::TemplateTail
                };
                break;
            }
            if ch == b'$' as i32 && self.char_at(1) == b'{' as i32 {
                final_seg = (start, self.state.pos);
                self.state.pos += 2;
                token = if started_with_backtick {
                    Kind::TemplateHead
                } else {
                    Kind::TemplateMiddle
                };
                break;
            }
            if ch == b'\\' as i32 {
                builder.append_range(start, self.state.pos);
                let mut flags = EscapeSequenceScanningFlags::STRING;
                if should_emit_invalid_escape_error {
                    flags |= EscapeSequenceScanningFlags::REPORT_ERRORS;
                }
                let escape = self.scan_escape_sequence(flags);
                builder.append_bytes(&escape);
                start = self.state.pos;
                continue;
            }
            // Speculated ECMAScript 6 Spec 11.8.6.1:
            // <CR><LF> and <CR> LineTerminatorSequences are normalized to <LF> for Template Values
            if ch == b'\r' as i32 {
                builder.append_range(start, self.state.pos);
                self.state.pos += 1;
                if self.char_() == b'\n' as i32 {
                    self.state.pos += 1;
                }
                builder.append_bytes(b"\n");
                start = self.state.pos;
                continue;
            }
            self.state.pos += 1;
        }
        self.state.token_value = builder.finish(final_seg.0, final_seg.1);
        token
    }

    /// Go: `func (s *Scanner) scanEscapeSequence(flags EscapeSequenceScanningFlags) string`.
    pub(crate) fn scan_escape_sequence(
        &mut self,
        flags: EscapeSequenceScanningFlags,
    ) -> Cow<'a, [u8]> {
        let start = self.state.pos;
        self.state.pos += 1;
        let mut ch = self.char_();
        if ch < 0 {
            self.error(&UNEXPECTED_END_OF_TEXT);
            return Cow::Borrowed(&[]);
        }
        self.state.pos += 1;
        match u8::try_from(ch) {
            Ok(b'0') => {
                // Although '0' preceding any digit is treated as LegacyOctalEscapeSequence,
                // '\08' should separately be interpreted as '\0' + '8'.
                if !stringutil::is_digit(as_char(self.char_())) {
                    return Cow::Borrowed(b"\x00");
                }
                // '\01', '\011'
                // Go: fallthrough to the octal cases.
                return self.scan_octal_escape_sequence_tail(flags, start, ch);
            }
            Ok(b'1'..=b'3') => {
                // '\1', '\17', '\177'
                return self.scan_octal_escape_sequence_tail(flags, start, ch);
            }
            Ok(b'4'..=b'7') => {
                // '\4', '\47' but not '\477'
                return self.scan_octal_escape_sequence_tail(flags, start, ch);
            }
            Ok(b'8' | b'9') => {
                // the invalid '\8' and '\9'
                self.state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                if flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS) {
                    if flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                        && !flags.intersects(EscapeSequenceScanningFlags::ATOM_ESCAPE)
                    {
                        self.error_at(
                            &DECIMAL_ESCAPE_SEQUENCES_AND_BACKREFERENCES_ARE_NOT_ALLOWED_IN_A_CHARACTER_CLASS,
                            start,
                            self.state.pos - start,
                            &[],
                        );
                    } else {
                        self.error_at(
                            &ESCAPE_SEQUENCE_0_IS_NOT_ALLOWED,
                            start,
                            self.state.pos - start,
                            &[escape_text(self, start)],
                        );
                    }
                    return Cow::Owned(vec![ch as u8]);
                }
                let value = &self.bytes()[start..self.state.pos];
                return Cow::Borrowed(value);
            }
            Ok(b'b') => return Cow::Borrowed(b"\b"),
            Ok(b't') => return Cow::Borrowed(b"\t"),
            Ok(b'n') => return Cow::Borrowed(b"\n"),
            Ok(b'v') => return Cow::Borrowed(b"\v"),
            Ok(b'f') => return Cow::Borrowed(b"\f"),
            Ok(b'r') => return Cow::Borrowed(b"\r"),
            Ok(b'\'') => return Cow::Borrowed(b"'"),
            Ok(b'"') => return Cow::Borrowed(b"\""),
            Ok(b'u') => {
                // '\uDDDD' and '\u{DDDDDD}'
                let extended = self.char_() == b'{' as i32;
                self.state.pos -= 2;
                let code_point =
                    self.scan_unicode_escape(flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS));
                if extended {
                    if !flags.intersects(EscapeSequenceScanningFlags::ALLOW_EXTENDED_UNICODE_ESCAPE) {
                        self.state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                        if flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS) {
                            self.error_at(
                                &UNICODE_ESCAPE_SEQUENCES_ARE_ONLY_AVAILABLE_WHEN_THE_UNICODE_U_FLAG_OR_THE_UNICODE_SETS_V_FLAG_IS_SET,
                                start,
                                self.state.pos - start,
                                &[],
                            );
                        }
                    }
                    if code_point < 0 {
                        let value = &self.bytes()[start..self.state.pos];
                        return Cow::Borrowed(value);
                    }
                    // In string literals, a high surrogate \u{...} followed by a low
                    // surrogate escape forms a single code point, exactly as adjacent
                    // UTF-16 code units would in a JavaScript string.
                    if !flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                        && stringutil::is_high_surrogate(code_point)
                    {
                        if let Some(combined) = self.scan_low_surrogate_escape(code_point) {
                            return Cow::Owned(rune_bytes(combined));
                        }
                    }
                    return Cow::Owned(stringutil::encode_js_string_rune(code_point));
                }
                if code_point < 0 {
                    let value = &self.bytes()[start..self.state.pos];
                    return Cow::Borrowed(value);
                } else if stringutil::is_high_surrogate(code_point) {
                    if !flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION) {
                        // Combine \uHigh followed by any low surrogate escape (\uLow or
                        // \u{Low}) into a single code point in string literals, matching
                        // how adjacent UTF-16 code units pair in a JavaScript string.
                        if let Some(combined) = self.scan_low_surrogate_escape(code_point) {
                            return Cow::Owned(rune_bytes(combined));
                        }
                    } else if flags.intersects(EscapeSequenceScanningFlags::ANY_UNICODE_MODE)
                        && self.char_() == b'\\' as i32
                        && self.char_at(1) == b'u' as i32
                        && self.char_at(2) != b'{' as i32
                    {
                        // In regex AnyUnicodeMode, combine \uHigh\uLow so scanClassRanges
                        // can compare the pair numerically. In non-unicode regex mode they
                        // are separate atoms, and extended \u{...} escapes never combine.
                        let saved_pos = self.state.pos;
                        let next_code_point = self.scan_unicode_escape(
                            flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS),
                        );
                        if stringutil::is_low_surrogate(next_code_point) {
                            return Cow::Owned(rune_bytes(
                                stringutil::surrogate_pair_to_code_point(code_point, next_code_point),
                            ));
                        }
                        self.state.pos = saved_pos;
                    }
                }
                // Lone surrogate: encode as CESU-8 so it survives losslessly. In a
                // non-unicode regex this also lets scanClassRanges compare it numerically.
                return Cow::Owned(stringutil::encode_js_string_rune(code_point));
            }
            Ok(b'x') => {
                // '\xDD'
                while self.state.pos < start + 4 {
                    if !stringutil::is_hex_digit(as_char(self.char_())) {
                        self.state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                        if flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS) {
                            self.error(&HEXADECIMAL_DIGIT_EXPECTED);
                        }
                        let value = &self.bytes()[start..self.state.pos];
                        return Cow::Borrowed(value);
                    }
                    self.state.pos += 1;
                }
                self.state.token_flags |= TokenFlags::HEX_ESCAPE;
                let hex_text = &self.text[start + 2..self.state.pos];
                let escaped_value = i32::from_str_radix(hex_text, 16).unwrap_or(0);
                return Cow::Owned(rune_bytes(escaped_value));
            }
            Ok(b'\r') => {
                // when encountering a LineContinuation (i.e. a backslash and a line terminator sequence),
                // the line terminator is interpreted to be "the empty code unit sequence".
                if self.char_() == b'\n' as i32 {
                    self.state.pos += 1;
                }
                return Cow::Borrowed(b"");
            }
            Ok(b'\n') => {
                return Cow::Borrowed(b"");
            }
            Ok(_) | Err(_) => {
                // ch was read as a single byte; for multi-byte UTF-8 characters,
                // we need to decode the full rune and advance past all its bytes.
                if ch >= 0x80 {
                    self.state.pos -= 1; // back up past the single-byte advance
                    let (decoded, size) = self.char_and_size();
                    ch = decoded;
                    self.state.pos += size;
                }
                // LineContinuation: a backslash followed by a line terminator is "the empty code unit sequence".
                if ch == 0x2028 || ch == 0x2029 {
                    return Cow::Borrowed(b"");
                }
                if flags.intersects(EscapeSequenceScanningFlags::ANY_UNICODE_MODE)
                    || (flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                        && !flags.intersects(EscapeSequenceScanningFlags::ANNEX_B)
                        && is_identifier_part(as_char(ch) as i32))
                {
                    self.error_at(
                        &tsc_diagnostics::THIS_CHARACTER_CANNOT_BE_ESCAPED_IN_A_REGULAR_EXPRESSION,
                        start,
                        self.state.pos - start,
                        &[],
                    );
                }
                Cow::Owned(rune_bytes(ch))
            }
        }
    }

    /// The shared tail of Go's '0'–'7' escape cases (fallthrough chain):
    /// up to two further octal digits, invalid-escape flags, and either the
    /// diagnostic + decoded char (reporting) or the raw text slice.
    fn scan_octal_escape_sequence_tail(
        &mut self,
        flags: EscapeSequenceScanningFlags,
        start: usize,
        ch: i32,
    ) -> Cow<'a, [u8]> {
        if stringutil::is_octal_digit(as_char(self.char_())) {
            self.state.pos += 1;
        }
        // '\17', '\177'
        if stringutil::is_octal_digit(as_char(self.char_())) {
            self.state.pos += 1;
        }
        // '\47'
        self.state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
        if flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS) {
            let octal_text = &self.text[start + 1..self.state.pos];
            let code = i32::from_str_radix(octal_text, 8).unwrap_or(0);
            if flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                && !flags.intersects(EscapeSequenceScanningFlags::ATOM_ESCAPE)
                && ch != b'0' as i32
            {
                self.error_at(
                    &OCTAL_ESCAPE_SEQUENCES_AND_BACKREFERENCES_ARE_NOT_ALLOWED_IN_A_CHARACTER_CLASS_IF_THIS_WAS_INTENDED_AS_AN_ESCAPE_SEQUENCE_USE_THE_SYNTAX_0_INSTEAD,
                    start,
                    self.state.pos - start,
                    &[format!("\\x{code:02x}")],
                );
            } else {
                self.error_at(
                    &OCTAL_ESCAPE_SEQUENCES_ARE_NOT_ALLOWED_USE_THE_SYNTAX_0,
                    start,
                    self.state.pos - start,
                    &[format!("\\x{code:02x}")],
                );
            }
            return Cow::Owned(rune_bytes(code));
        }
        let value = &self.bytes()[start..self.state.pos];
        Cow::Borrowed(value)
    }

    /// Go: `func (s *Scanner) scanUnicodeEscape(shouldEmitInvalidEscapeError bool) rune` —
    /// known to be at `\u`.
    pub(crate) fn scan_unicode_escape(&mut self, should_emit_invalid_escape_error: bool) -> i32 {
        self.state.pos += 2;
        let start = self.state.pos;
        let extended = self.char_() == b'{' as i32;
        let hex_digits;
        if extended {
            self.state.pos += 1;
            hex_digits = self.scan_hex_digits(1, true, false);
        } else {
            self.state.token_flags |= TokenFlags::UNICODE_ESCAPE;
            hex_digits = self.scan_hex_digits(4, false, false);
        }
        if hex_digits.is_empty() {
            self.state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
            if should_emit_invalid_escape_error {
                self.error(&HEXADECIMAL_DIGIT_EXPECTED);
            }
            return -1;
        }
        let hex_text = std::str::from_utf8(&hex_digits).unwrap_or("");
        let hex_value = i32::from_str_radix(hex_text, 16).unwrap_or(i32::MAX);
        if extended {
            let mut is_invalid_extended_escape = false;
            if hex_value > 0x10FFFF {
                if should_emit_invalid_escape_error {
                    self.error_at(
                        &AN_EXTENDED_UNICODE_ESCAPE_VALUE_MUST_BE_BETWEEN_0X0_AND_0X10FFFF_INCLUSIVE,
                        start + 1,
                        self.state.pos - start - 1,
                        &[],
                    );
                }
                is_invalid_extended_escape = true;
            }
            if self.state.pos >= self.end {
                if should_emit_invalid_escape_error {
                    self.error(&UNEXPECTED_END_OF_TEXT);
                }
                is_invalid_extended_escape = true;
            } else if self.char_() == b'}' as i32 {
                self.state.pos += 1;
            } else {
                if should_emit_invalid_escape_error {
                    self.error(&UNTERMINATED_UNICODE_ESCAPE_SEQUENCE);
                }
                is_invalid_extended_escape = true;
            }
            if is_invalid_extended_escape {
                self.state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                return -1;
            }
            self.state.token_flags |= TokenFlags::EXTENDED_UNICODE_ESCAPE;
        }
        hex_value
    }

    /// scanLowSurrogateEscape attempts to consume a low-surrogate Unicode
    /// escape (either '\uLow' or '\u{Low}') immediately following an
    /// already-scanned high surrogate and combine them into a single
    /// supplementary code point. This mirrors how adjacent UTF-16 code units
    /// form a surrogate pair in a JavaScript string, regardless of which
    /// escape syntax produced each half. On success it returns the combined
    /// code point; otherwise it restores the scanner position and returns None.
    ///
    /// Go: `func (s *Scanner) scanLowSurrogateEscape(high rune) (rune, bool)`.
    fn scan_low_surrogate_escape(&mut self, high: i32) -> Option<i32> {
        if self.char_() != b'\\' as i32 || self.char_at(1) != b'u' as i32 {
            return None;
        }
        let saved_pos = self.state.pos;
        let saved_token_flags = self.state.token_flags;
        // Speculatively scan the escape with diagnostics suppressed: if it isn't a
        // low surrogate we rewind below, and the caller re-scans the same escape and
        // reports any error then, so reporting here would duplicate diagnostics.
        let low = self.scan_unicode_escape(false);
        if stringutil::is_low_surrogate(low) {
            return Some(stringutil::surrogate_pair_to_code_point(high, low));
        }
        self.state.pos = saved_pos;
        self.state.token_flags = saved_token_flags;
        None
    }

    /// Current character is known to be a backslash. Check for Unicode escape
    /// of the form '\uXXXX' or '\u{XXXXXX}' and return code point value if
    /// valid Unicode escape is found. Otherwise return -1.
    ///
    /// Go: `func (s *Scanner) peekUnicodeEscape() rune`.
    fn peek_unicode_escape(&mut self) -> i32 {
        if self.char_at(1) == b'u' as i32 {
            let save_pos = self.state.pos;
            let save_token_flags = self.state.token_flags;
            let code_point = self.scan_unicode_escape(false);
            self.state.pos = save_pos;
            self.state.token_flags = save_token_flags;
            return code_point;
        }
        -1
    }

    // ────────────────────────────────────────────────────────────────────────
    // Number scanning
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (s *Scanner) scanNumber() ast.Kind`.
    pub(crate) fn scan_number(&mut self) -> Kind {
        let mut start = self.state.pos;
        let fixed_part: Cow<'a, [u8]>;
        if self.char_() == b'0' as i32 {
            self.state.pos += 1;
            if self.char_() == b'_' as i32 {
                self.state.token_flags |=
                    TokenFlags::CONTAINS_SEPARATOR | TokenFlags::CONTAINS_INVALID_SEPARATOR;
                self.error_at(&NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, self.state.pos, 1, &[]);
                self.state.pos = start;
                fixed_part = self.scan_number_fragment();
            } else {
                let (digits, is_octal) = self.scan_digits();
                if digits.is_empty() {
                    fixed_part = Cow::Borrowed(b"0");
                } else if !is_octal {
                    self.state.token_flags |= TokenFlags::CONTAINS_LEADING_ZERO;
                    fixed_part = Cow::Borrowed(digits);
                } else {
                    // PORT: Go's `strconv.ParseInt(digits, 8, 64)` returns the
                    // clamped maximum on overflow (error ignored); unwrap_or
                    // mirrors the clamped value.
                    let val = i64::from_str_radix(digits, 8).unwrap_or(i64::MAX);
                    self.state.token_value = Cow::Owned(val.to_string().into_bytes());
                    self.state.token_flags |= TokenFlags::OCTAL;
                    let with_minus = self.state.token == Kind::MinusToken;
                    let literal = format!(
                        "{}0o{}",
                        if with_minus { "-" } else { "" },
                        format!("{val:o}")
                    );
                    if with_minus {
                        start -= 1;
                    }
                    self.error_at(
                        &OCTAL_LITERALS_ARE_NOT_ALLOWED_USE_THE_SYNTAX_0,
                        start,
                        self.state.pos - start,
                        &[literal],
                    );
                    return Kind::NumericLiteral;
                }
            }
        } else {
            fixed_part = self.scan_number_fragment();
        }
        let fixed_part_end = self.state.pos;
        let mut fractional_part: Cow<'a, [u8]> = Cow::Borrowed(&[]);
        let mut exponent_preamble: Cow<'a, [u8]> = Cow::Borrowed(&[]);
        let mut exponent_part: Cow<'a, [u8]> = Cow::Borrowed(&[]);
        if self.char_() == b'.' as i32 {
            self.state.pos += 1;
            fractional_part = self.scan_number_fragment();
        }
        let mut end = self.state.pos;
        if self.char_() == b'E' as i32 || self.char_() == b'e' as i32 {
            self.state.pos += 1;
            self.state.token_flags |= TokenFlags::SCIENTIFIC;
            if self.char_() == b'+' as i32 || self.char_() == b'-' as i32 {
                self.state.pos += 1;
            }
            let start_numeric_part = self.state.pos;
            exponent_part = self.scan_number_fragment();
            if exponent_part.is_empty() {
                self.error(&DIGIT_EXPECTED);
            } else {
                exponent_preamble = Cow::Borrowed(&self.bytes()[end..start_numeric_part]);
                end = self.state.pos;
            }
        }
        if self.state.token_flags.intersects(TokenFlags::CONTAINS_SEPARATOR) {
            let mut v = Vec::new();
            v.extend_from_slice(&fixed_part);
            if !fractional_part.is_empty() {
                v.push(b'.');
                v.extend_from_slice(&fractional_part);
            }
            if !exponent_part.is_empty() {
                v.extend_from_slice(&exponent_preamble);
                v.extend_from_slice(&exponent_part);
            }
            self.state.token_value = Cow::Owned(v);
        } else {
            let value = &self.bytes()[start..end];
            self.state.token_value = Cow::Borrowed(value);
        }
        if self.state.token_flags.intersects(TokenFlags::CONTAINS_LEADING_ZERO) {
            self.error_at(&DECIMALS_WITH_LEADING_ZEROS_ARE_NOT_ALLOWED, start, self.state.pos - start, &[]);
            self.canonicalize_number_value();
            return Kind::NumericLiteral;
        }
        let result;
        if fixed_part_end == self.state.pos {
            result = self.scan_big_int_suffix();
        } else {
            self.canonicalize_number_value();
            result = Kind::NumericLiteral;
        }
        let (ch, _) = self.char_and_size();
        if is_identifier_start(ch) {
            let id_start = self.state.pos;
            let id = self.scan_identifier_parts(IdentifierVariant::Standard);
            if result != Kind::BigIntLiteral
                && id.len() == 1
                && self.bytes()[id_start] == b'n'
            {
                if self.state.token_flags.intersects(TokenFlags::SCIENTIFIC) {
                    self.error_at(&A_BIGINT_LITERAL_CANNOT_USE_EXPONENTIAL_NOTATION, start, self.state.pos - start, &[]);
                    return result;
                }
                if fixed_part_end < id_start {
                    self.error_at(&A_BIGINT_LITERAL_MUST_BE_AN_INTEGER, start, self.state.pos - start, &[]);
                    return result;
                }
            }
            self.error_at(
                &AN_IDENTIFIER_OR_KEYWORD_CANNOT_IMMEDIATELY_FOLLOW_A_NUMERIC_LITERAL,
                id_start,
                self.state.pos - id_start,
                &[],
            );
            self.state.pos = id_start;
        }
        result
    }

    /// Go: `func (s *Scanner) scanNumberFragment() string`.
    pub(crate) fn scan_number_fragment(&mut self) -> Cow<'a, [u8]> {
        let bytes = self.bytes();
        let mut builder = ValueBuilder::new(bytes);
        let mut start = self.state.pos;
        let mut allow_separator = false;
        let mut is_previous_token_separator = false;
        loop {
            let before = self.state.pos;
            self.scan_ascii_while(|b| b.is_ascii_digit());
            if self.state.pos > before {
                allow_separator = true;
                is_previous_token_separator = false;
            }
            let ch = self.char_();
            if ch == b'_' as i32 {
                self.state.token_flags |= TokenFlags::CONTAINS_SEPARATOR;
                if allow_separator {
                    allow_separator = false;
                    is_previous_token_separator = true;
                    builder.append_range(start, self.state.pos);
                } else {
                    self.state.token_flags |= TokenFlags::CONTAINS_INVALID_SEPARATOR;
                    if is_previous_token_separator {
                        self.error_at(
                            &MULTIPLE_CONSECUTIVE_NUMERIC_SEPARATORS_ARE_NOT_PERMITTED,
                            self.state.pos,
                            1,
                            &[],
                        );
                    } else {
                        self.error_at(&NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, self.state.pos, 1, &[]);
                    }
                }
                self.state.pos += 1;
                start = self.state.pos;
                continue;
            }
            break;
        }
        if is_previous_token_separator {
            self.state.token_flags |= TokenFlags::CONTAINS_INVALID_SEPARATOR;
            self.error_at(&NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, self.state.pos - 1, 1, &[]);
        }
        builder.finish(start, self.state.pos)
    }

    /// Go: `func (s *Scanner) scanDigits() (string, bool)`.
    pub(crate) fn scan_digits(&mut self) -> (&'a [u8], bool) {
        let start = self.state.pos;
        let mut is_octal = true;
        while stringutil::is_digit(as_char(self.char_())) {
            if !stringutil::is_octal_digit(as_char(self.char_())) {
                is_octal = false;
            }
            self.state.pos += 1;
        }
        (&self.bytes()[start..self.state.pos], is_octal)
    }

    /// Go: `func (s *Scanner) scanHexDigits(minCount int, scanAsManyAsPossible bool, canHaveSeparators bool) string`
    /// (hexDigitCache memo dropped — see header).
    pub(crate) fn scan_hex_digits(
        &mut self,
        min_count: usize,
        scan_as_many_as_possible: bool,
        can_have_separators: bool,
    ) -> Cow<'a, [u8]> {
        let start = self.state.pos;
        let mut digit_count = 0usize;
        let mut allow_separator = false;
        let mut is_previous_token_separator = false;
        let mut saw_separator = false;
        while digit_count < min_count || scan_as_many_as_possible {
            let ch = self.char_();
            if stringutil::is_hex_digit(as_char(ch)) {
                allow_separator = can_have_separators;
                is_previous_token_separator = false;
                digit_count += 1;
            } else if can_have_separators && ch == b'_' as i32 {
                self.state.token_flags |= TokenFlags::CONTAINS_SEPARATOR;
                saw_separator = true;
                if allow_separator {
                    allow_separator = false;
                    is_previous_token_separator = true;
                } else if is_previous_token_separator {
                    self.error_at(
                        &MULTIPLE_CONSECUTIVE_NUMERIC_SEPARATORS_ARE_NOT_PERMITTED,
                        self.state.pos,
                        1,
                        &[],
                    );
                } else {
                    self.error_at(&NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, self.state.pos, 1, &[]);
                }
            } else {
                break;
            }
            self.state.pos += 1;
        }
        if is_previous_token_separator {
            self.error_at(&NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, self.state.pos - 1, 1, &[]);
        }
        if digit_count < min_count {
            return Cow::Borrowed(&[]);
        }
        let digits = &self.bytes()[start..self.state.pos];
        // Go: strings.ReplaceAll(digits, "_", "") + strings.ToLower(digits) —
        // hex digits are ASCII, so ASCII lowercasing is exact.
        if !saw_separator && !digits.iter().any(u8::is_ascii_uppercase) {
            return Cow::Borrowed(digits);
        }
        let mut v: Vec<u8> = digits
            .iter()
            .filter(|&&b| b != b'_')
            .copied()
            .collect();
        v.make_ascii_lowercase();
        Cow::Owned(v)
    }

    /// Go: `func (s *Scanner) scanBinaryOrOctalDigits(base int32) string` —
    /// Go's strings.Builder is replaced by the borrowed fast path (no
    /// separator seen → the source slice), identical output.
    pub(crate) fn scan_binary_or_octal_digits(&mut self, base: u8) -> Cow<'a, [u8]> {
        let start = self.state.pos;
        let mut allow_separator = false;
        let mut is_previous_token_separator = false;
        let mut saw_separator = false;
        loop {
            let ch = self.char_();
            if stringutil::is_digit(as_char(ch)) && (ch - b'0' as i32) < base as i32 {
                allow_separator = true;
                is_previous_token_separator = false;
            } else if ch == b'_' as i32 {
                self.state.token_flags |= TokenFlags::CONTAINS_SEPARATOR;
                saw_separator = true;
                if allow_separator {
                    allow_separator = false;
                    is_previous_token_separator = true;
                } else if is_previous_token_separator {
                    self.error_at(
                        &MULTIPLE_CONSECUTIVE_NUMERIC_SEPARATORS_ARE_NOT_PERMITTED,
                        self.state.pos,
                        1,
                        &[],
                    );
                } else {
                    self.error_at(&NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, self.state.pos, 1, &[]);
                }
            } else {
                break;
            }
            self.state.pos += 1;
        }
        if is_previous_token_separator {
            self.error_at(&NUMERIC_SEPARATORS_ARE_NOT_ALLOWED_HERE, self.state.pos - 1, 1, &[]);
        }
        let digits = &self.bytes()[start..self.state.pos];
        if saw_separator {
            Cow::Owned(
                digits
                    .iter()
                    .filter(|&&b| b != b'_')
                    .copied()
                    .collect(),
            )
        } else {
            Cow::Borrowed(digits)
        }
    }

    /// Go (inlined at the '0b'/'0o' Scan arms): `s.tokenValue = "0b"/"0o" + digits`.
    fn set_base_prefixed_number_value(&mut self, prefix: &[u8], digits: Cow<'a, [u8]>) {
        // The fast path borrows the source text when the literal already reads
        // exactly prefix+digits (lowercase prefix, no separators).
        let start = self.state.token_start;
        if !self.state.token_flags.intersects(TokenFlags::CONTAINS_SEPARATOR)
            && self.bytes()[start..start + 2] == *prefix
        {
            let value = &self.bytes()[start..self.state.pos];
            self.state.token_value = Cow::Borrowed(value);
        } else {
            let mut v = Vec::with_capacity(2 + digits.len());
            v.extend_from_slice(prefix);
            v.extend_from_slice(&digits);
            self.state.token_value = Cow::Owned(v);
        }
    }

    /// Go: `func (s *Scanner) scanBigIntSuffix() ast.Kind`.
    pub(crate) fn scan_big_int_suffix(&mut self) -> Kind {
        if self.char_() == b'n' as i32 {
            let token_value = std::mem::replace(&mut self.state.token_value, Cow::Borrowed(&[]));
            let mut token_value = match token_value {
                // The borrowed token value always ends at the current position
                // (all number paths slice text[...:pos]), so the 'n' can be
                // borrowed too.
                Cow::Borrowed(b) => {
                    let start = self.state.pos - b.len();
                    Cow::Borrowed(&self.bytes()[start..self.state.pos + 1])
                }
                Cow::Owned(mut v) => {
                    v.push(b'n');
                    Cow::Owned(v)
                }
            };
            if self
                .state
                .token_flags
                .intersects(TokenFlags::BINARY_OR_OCTAL_SPECIFIER)
            {
                let text = String::from_utf8_lossy(&token_value).into_owned();
                token_value =
                    Cow::Owned(format!("{}n", jsnum::parse_pseudo_big_int(&text)).into_bytes());
            }
            self.state.token_value = token_value;
            self.state.pos += 1;
            return Kind::BigIntLiteral;
        }
        // Go: numberCache memo.
        let token_value = std::mem::replace(&mut self.state.token_value, Cow::Borrowed(&[]));
        if let Some(cached) = self
            .number_cache
            .as_ref()
            .and_then(|cache| cache.get(token_value.as_ref()))
        {
            self.state.token_value = cached.clone();
        } else {
            let canonical: Vec<u8> = match std::str::from_utf8(&token_value) {
                // Unreachable in practice: numeric literal values are ASCII.
                Err(_) => token_value.to_vec(),
                Ok(text) => jsnum::from_string(text).string().into_bytes(),
            };
            let value = if canonical == *token_value {
                token_value
            } else {
                Cow::Owned(canonical)
            };
            self.number_cache
                .get_or_insert_with(Default::default)
                .insert(value.as_ref().to_vec(), value.clone());
            self.state.token_value = value;
        }
        Kind::NumericLiteral
    }

    /// Go: `s.tokenValue = jsnum.FromString(s.tokenValue).String()` — the
    /// canonicalization shared by the leading-zero and fractional paths. When
    /// the canonical form is byte-identical, the borrowed source slice is kept
    /// (Go assigns an equal string; observable value is the same).
    fn canonicalize_number_value(&mut self) {
        let token_value = std::mem::replace(&mut self.state.token_value, Cow::Borrowed(&[]));
        let canonical: Vec<u8> = match std::str::from_utf8(&token_value) {
            Err(_) => token_value.to_vec(),
            Ok(text) => jsnum::from_string(text).string().into_bytes(),
        };
        self.state.token_value = if canonical == *token_value {
            token_value
        } else {
            Cow::Owned(canonical)
        };
    }

    /// Go: `func (s *Scanner) scanInvalidCharacter()`.
    pub(crate) fn scan_invalid_character(&mut self) {
        let (_, size) = self.char_and_size();
        self.error_at(&INVALID_CHARACTER, self.state.pos, size, &[]);
        self.state.pos += size;
        self.state.token = Kind::Unknown;
    }

    /// `GetIdentifierToken(s.tokenValue)` — identifier token values are
    /// always valid UTF-8 (surrogate escapes combine before reaching here).
    fn token_value_kind(&self) -> Kind {
        let text = std::str::from_utf8(&self.state.token_value).unwrap_or("");
        get_identifier_token(text)
    }

    /// scanConflictMarkerTrivia via the scanner's error reporter.
    fn scan_conflict_marker_at(&mut self, pos: usize) -> usize {
        let text = self.text; // copy the &'a str out to release the borrow
        let mut report = |msg: &'static Message, p: usize, length: usize, args: &[String]| {
            self.error_at(msg, p, length, args);
        };
        scan_conflict_marker_trivia(text.as_bytes(), pos, Some(&mut report))
    }
}

impl Default for Scanner<'_> {
    fn default() -> Self {
        Scanner::new()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ValueBuilder — Go's strings.Builder sites (see header). Before the first
// append the value is a contiguous slice of the source text, so the fast path
// borrows instead of building. `finish` receives the final segment exactly
// where Go performs its last WriteString/return-slice.
// ────────────────────────────────────────────────────────────────────────────

struct ValueBuilder<'a> {
    bytes: &'a [u8],
    buf: Option<Vec<u8>>,
}

impl<'a> ValueBuilder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        ValueBuilder { bytes, buf: None }
    }

    fn append_range(&mut self, start: usize, end: usize) {
        self.buf
            .get_or_insert_with(Vec::new)
            .extend_from_slice(&self.bytes[start..end]);
    }

    fn append_bytes(&mut self, value: &[u8]) {
        self.buf
            .get_or_insert_with(Vec::new)
            .extend_from_slice(value);
    }

    fn finish(mut self, final_start: usize, final_end: usize) -> Cow<'a, [u8]> {
        match self.buf.take() {
            None => Cow::Borrowed(&self.bytes[final_start..final_end]),
            Some(mut v) => {
                v.extend_from_slice(&self.bytes[final_start..final_end]);
                Cow::Owned(v)
            }
        }
    }
}

/// Go: `string(rune)` for the runes the escape scanner produces — UTF-8
/// encoding of a valid code point; invalid values encode U+FFFD exactly as
/// Go's string(rune) does.
fn rune_bytes(r: i32) -> Vec<u8> {
    match char::from_u32(r as u32) {
        Some(c) => c.encode_utf8(&mut [0u8; 4]).as_bytes().to_vec(),
        None => "\u{FFFD}".as_bytes().to_vec(),
    }
}

/// The `s.text[start:s.pos]` argument of `Escape_sequence_0_is_not_allowed`.
fn escape_text<'s>(s: &'s Scanner<'_>, start: usize) -> String {
    String::from_utf8_lossy(&s.bytes()[start..s.state.pos]).into_owned()
}

// ────────────────────────────────────────────────────────────────────────────
// hasJSDocTag
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func hasJSDocTag(text string, tags ...string) bool` — reports whether
/// text starts with one of the given tag names followed by a valid JSDoc tag
/// terminator (whitespace, '}', '*', or end-of-string).
fn has_jsdoc_tag(text: &[u8], tags: &[&str]) -> bool {
    for tag in tags {
        let tag = tag.as_bytes();
        if !text.starts_with(tag) {
            continue;
        }
        if text.len() == tag.len() {
            return true;
        }
        let ch = text[tag.len()];
        if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'}' || ch == b'*' {
            return true;
        }
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// Token/keyword helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func tokenIsIdentifierOrKeyword(token ast.Kind) bool` (utilities.go).
pub fn token_is_identifier_or_keyword(token: Kind) -> bool {
    token >= Kind::Identifier
}

/// Go: `func GetIdentifierToken(str string) ast.Kind`.
pub fn get_identifier_token(s: &str) -> Kind {
    if s.len() >= 2 && s.len() <= 12 {
        let b = s.as_bytes()[0];
        if b.is_ascii_lowercase() {
            if let Some(keyword) = text_to_keyword(s) {
                return keyword;
            }
        }
    }
    Kind::Identifier
}

/// Go: `func IsValidIdentifier(s string) bool`.
pub fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    for (i, ch) in s.chars().enumerate() {
        if i == 0 && !is_identifier_start(ch as i32) || i != 0 && !is_identifier_part(ch as i32) {
            return false;
        }
    }
    true
}

/// Go: `func isWordCharacter(ch rune) bool` — Section 6.1.4.
fn is_word_character(ch: i32) -> bool {
    stringutil::is_ascii_letter(as_char(ch))
        || stringutil::is_digit(as_char(ch))
        || ch == b'_' as i32
}

/// Go: `func IsIdentifierStart(ch rune) bool`.
pub fn is_identifier_start(ch: i32) -> bool {
    stringutil::is_ascii_letter(as_char(ch))
        || ch == b'_' as i32
        || ch == b'$' as i32
        || (ch >= 0x80 && stringutil::is_unicode_identifier_start(as_char(ch)))
}

/// Go: `func IsIdentifierPart(ch rune) bool`.
pub fn is_identifier_part(ch: i32) -> bool {
    is_identifier_part_ex(ch, LanguageVariant::Standard)
}

/// Go: `func IsIdentifierPartEx(ch rune, languageVariant core.LanguageVariant) bool`.
pub fn is_identifier_part_ex(ch: i32, language_variant: LanguageVariant) -> bool {
    is_word_character(ch)
        || ch == b'$' as i32
        || (ch >= 0x80 && stringutil::is_unicode_identifier_part(as_char(ch)))
        // ":" is part of JSXNamespacedName, but not JSXIdentifier.
        || (language_variant == LanguageVariant::JSX && ch == b'-' as i32)
}

/// Go: `func TokenToString(token ast.Kind) string` (tokenToText table).
pub fn token_to_text(token: Kind) -> &'static str {
    match token {
        Kind::AbstractKeyword => "abstract",
        Kind::AccessorKeyword => "accessor",
        Kind::AnyKeyword => "any",
        Kind::AsKeyword => "as",
        Kind::AssertsKeyword => "asserts",
        Kind::AssertKeyword => "assert",
        Kind::BigIntKeyword => "bigint",
        Kind::BooleanKeyword => "boolean",
        Kind::BreakKeyword => "break",
        Kind::CaseKeyword => "case",
        Kind::CatchKeyword => "catch",
        Kind::ClassKeyword => "class",
        Kind::ContinueKeyword => "continue",
        Kind::ConstKeyword => "const",
        Kind::ConstructorKeyword => "constructor",
        Kind::DebuggerKeyword => "debugger",
        Kind::DeclareKeyword => "declare",
        Kind::DefaultKeyword => "default",
        Kind::DeferKeyword => "defer",
        Kind::DeleteKeyword => "delete",
        Kind::DoKeyword => "do",
        Kind::ElseKeyword => "else",
        Kind::EnumKeyword => "enum",
        Kind::ExportKeyword => "export",
        Kind::ExtendsKeyword => "extends",
        Kind::FalseKeyword => "false",
        Kind::FinallyKeyword => "finally",
        Kind::ForKeyword => "for",
        Kind::FromKeyword => "from",
        Kind::FunctionKeyword => "function",
        Kind::GetKeyword => "get",
        Kind::IfKeyword => "if",
        Kind::ImmediateKeyword => "immediate",
        Kind::ImplementsKeyword => "implements",
        Kind::ImportKeyword => "import",
        Kind::InKeyword => "in",
        Kind::InferKeyword => "infer",
        Kind::InstanceOfKeyword => "instanceof",
        Kind::InterfaceKeyword => "interface",
        Kind::IntrinsicKeyword => "intrinsic",
        Kind::IsKeyword => "is",
        Kind::KeyOfKeyword => "keyof",
        Kind::LetKeyword => "let",
        Kind::ModuleKeyword => "module",
        Kind::NamespaceKeyword => "namespace",
        Kind::NeverKeyword => "never",
        Kind::NewKeyword => "new",
        Kind::NullKeyword => "null",
        Kind::NumberKeyword => "number",
        Kind::ObjectKeyword => "object",
        Kind::PackageKeyword => "package",
        Kind::PrivateKeyword => "private",
        Kind::ProtectedKeyword => "protected",
        Kind::PublicKeyword => "public",
        Kind::OverrideKeyword => "override",
        Kind::OutKeyword => "out",
        Kind::ReadonlyKeyword => "readonly",
        Kind::RequireKeyword => "require",
        Kind::GlobalKeyword => "global",
        Kind::ReturnKeyword => "return",
        Kind::SatisfiesKeyword => "satisfies",
        Kind::SetKeyword => "set",
        Kind::SourceKeyword => "source",
        Kind::StaticKeyword => "static",
        Kind::StringKeyword => "string",
        Kind::SuperKeyword => "super",
        Kind::SwitchKeyword => "switch",
        Kind::SymbolKeyword => "symbol",
        Kind::ThisKeyword => "this",
        Kind::ThrowKeyword => "throw",
        Kind::TrueKeyword => "true",
        Kind::TryKeyword => "try",
        Kind::TypeKeyword => "type",
        Kind::TypeOfKeyword => "typeof",
        Kind::UndefinedKeyword => "undefined",
        Kind::UniqueKeyword => "unique",
        Kind::UnknownKeyword => "unknown",
        Kind::UsingKeyword => "using",
        Kind::VarKeyword => "var",
        Kind::VoidKeyword => "void",
        Kind::WhileKeyword => "while",
        Kind::WithKeyword => "with",
        Kind::YieldKeyword => "yield",
        Kind::AsyncKeyword => "async",
        Kind::AwaitKeyword => "await",
        Kind::OfKeyword => "of",
        Kind::OpenBraceToken => "{",
        Kind::CloseBraceToken => "}",
        Kind::OpenParenToken => "(",
        Kind::CloseParenToken => ")",
        Kind::OpenBracketToken => "[",
        Kind::CloseBracketToken => "]",
        Kind::DotToken => ".",
        Kind::DotDotDotToken => "...",
        Kind::SemicolonToken => ";",
        Kind::CommaToken => ",",
        Kind::LessThanToken => "<",
        Kind::GreaterThanToken => ">",
        Kind::LessThanEqualsToken => "<=",
        Kind::GreaterThanEqualsToken => ">=",
        Kind::EqualsEqualsToken => "==",
        Kind::ExclamationEqualsToken => "!=",
        Kind::EqualsEqualsEqualsToken => "===",
        Kind::ExclamationEqualsEqualsToken => "!==",
        Kind::EqualsGreaterThanToken => "=>",
        Kind::PlusToken => "+",
        Kind::MinusToken => "-",
        Kind::AsteriskAsteriskToken => "**",
        Kind::AsteriskToken => "*",
        Kind::SlashToken => "/",
        Kind::PercentToken => "%",
        Kind::PlusPlusToken => "++",
        Kind::MinusMinusToken => "--",
        Kind::LessThanLessThanToken => "<<",
        Kind::LessThanSlashToken => "</",
        Kind::GreaterThanGreaterThanToken => ">>",
        Kind::GreaterThanGreaterThanGreaterThanToken => ">>>",
        Kind::AmpersandToken => "&",
        Kind::BarToken => "|",
        Kind::CaretToken => "^",
        Kind::ExclamationToken => "!",
        Kind::TildeToken => "~",
        Kind::AmpersandAmpersandToken => "&&",
        Kind::BarBarToken => "||",
        Kind::QuestionToken => "?",
        Kind::QuestionQuestionToken => "??",
        Kind::QuestionDotToken => "?.",
        Kind::ColonToken => ":",
        Kind::EqualsToken => "=",
        Kind::PlusEqualsToken => "+=",
        Kind::MinusEqualsToken => "-=",
        Kind::AsteriskEqualsToken => "*=",
        Kind::AsteriskAsteriskEqualsToken => "**=",
        Kind::SlashEqualsToken => "/=",
        Kind::PercentEqualsToken => "%=",
        Kind::LessThanLessThanEqualsToken => "<<=",
        Kind::GreaterThanGreaterThanEqualsToken => ">>=",
        Kind::GreaterThanGreaterThanGreaterThanEqualsToken => ">>>=",
        Kind::AmpersandEqualsToken => "&=",
        Kind::BarEqualsToken => "|=",
        Kind::CaretEqualsToken => "^=",
        Kind::BarBarEqualsToken => "||=",
        Kind::AmpersandAmpersandEqualsToken => "&&=",
        Kind::QuestionQuestionEqualsToken => "??=",
        Kind::AtToken => "@",
        Kind::HashToken => "#",
        Kind::BacktickToken => "`",
        _ => "",
    }
}

/// Go: `func StringToToken(s string) ast.Kind`.
pub fn string_to_token(s: &str) -> Kind {
    text_to_token(s).unwrap_or(Kind::Unknown)
}

/// Go: `func GetViableKeywordSuggestions() []string` — Go iterates a map
/// (randomized order); the declaration order here is a valid realization.
pub fn get_viable_keyword_suggestions() -> Vec<&'static str> {
    KEYWORDS_WITH_LONG_NAMES.to_vec()
}

static KEYWORDS_WITH_LONG_NAMES: &[&str] = &[
    "abstract", "accessor", "asserts", "bigint", "boolean", "constructor", "continue", "declare",
    "default", "delete", "extends", "finally", "function", "implement", "implements", "instanceof",
    "interface", "intrinsic", "module", "namespace", "number", "package", "protected", "readonly",
    "satisfies", "static", "string", "symbol", "typeof", "undefined", "unknown",
];

// (The Go textToKeyword map keys with len > 2, in declaration order.)
