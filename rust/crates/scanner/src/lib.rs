// Ported from tsc/internal/scanner @ ec47d33c23e464a17cdf2475632cba629bee8763
//
//! Package scanner port: the TypeScript lexer.
//!
//! File mapping (Go file → Rust module):
//!   - `scanner/scanner.go`      → [`scanner`] (Scanner, Scan, ReScan* family,
//!     keyword maps, line/position math, SkipTrivia, conflict markers)
//!   - `scanner/regexp.go`       → `regexp` (RegExp literal re-scan validation;
//!     private — Go exports nothing from that file)
//!   - `scanner/unicodeproperties.go` → `unicodeproperties` (ECMA-262 tables
//!     66/67; private — Go exports nothing from that file)
//!   - `scanner/utilities.go`    → [`utilities`]
//!   - `scanner/scanner_test.go` → `scanner_test` (cfg(test))
//!
//! Cross-cutting port decisions (details at each site):
//!   - Source text is a borrowed `&str` (positions are UTF-8 byte offsets,
//!     identical to the Go port). All Go `s.text[a:b]` slices operate on
//!     `text.as_bytes()` so token values can carry the lone-surrogate CESU-8
//!     sentinel bytes produced by `stringutil::encode_js_string_rune`
//!     (invalid UTF-8 — see PORTING-NOTES, ComputePositionMap).
//!   - `token_value` is a `Cow<'a, [u8]>`: the fast path is a borrowed slice
//!     of the source text (zero allocation); materialization happens only
//!     where Go's `strings.Builder` sites allocate (escape sequences,
//!     separators, lone surrogates).
//!   - Go `int` positions become `usize` inside the scanner; public helpers
//!     that accept possibly-synthesized positions (`skip_trivia*`) take
//!     `i32` (`core::TextPos`) to preserve Go's negative-position handling.
//!   - Go `rune` becomes `i32` (EOF sentinel `-1`), mirroring stringutil's
//!     rune-typed surrogate helpers. `char`-typed stringutil predicates take
//!     the rune via a loss-free `i32`→`char` conversion (`-1` and other
//!     non-code-point values map to U+FFFD, which is inert in every
//!     predicate the scanner consults — same result as Go's comparisons).

pub mod go_shims;
pub mod scanner;
mod regexp;
mod unicodeproperties;
pub mod utilities;

pub use scanner::*;

#[cfg(test)]
mod scanner_test;

/// Go `rune` → `char` conversion for the stringutil predicates (which are
/// `char`-typed in the Rust port). `-1` (the scanner's EOF sentinel) and
/// other non-code-point values map to U+FFFD, which is inert in every
/// predicate the scanner consults (whitespace/line-break/identifier tables
/// never classify U+FFFD), so the comparison result is unchanged.
#[inline]
pub(crate) fn as_char(ch: i32) -> char {
    if !(0..=0x10FFFF).contains(&ch) {
        return '\u{FFFD}';
    }
    char::from_u32(ch as u32).unwrap_or('\u{FFFD}')
}
