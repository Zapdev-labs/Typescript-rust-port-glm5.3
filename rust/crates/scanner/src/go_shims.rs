// Local ports of Go standard-library / core-package pieces that the scanner
// package depends on but that have no Rust counterpart yet @
// ec47d33c23e464a17cdf2475632cba629bee8763.
//
// These are PORT: TODO(porting) dedup candidates:
//   - `get_spelling_suggestion_for_strings` / `levenshtein_with_max` mirror
//     tsc/internal/core/core.go (`GetSpellingSuggestionForStrings`). core.go
//     is not yet ported as tsc-core; when it lands, delete this copy.
//   - `utf16_len` + `UTF16Offset` mirror core.go `UTF16Len`/`UTF16Offset`.
//   - `script_target_string` mirrors core/scripttarget_stringer_generated.go
//     (`ScriptTarget.String()`).
//   - `decode_last_rune_in_string` mirrors Go `utf8.DecodeLastRuneInString`
//     over raw bytes (stringutil's utf8 module is pub(crate); the scanner
//     needs byte-level access for positions that may not be char-aligned,
//     e.g. `isConflictMarkerTrivia`'s `text[:pos-2]` probe).
//   - `is_type_node_kind` mirrors ast/utilities.go `IsTypeNodeKind`
//     (ast/utilities.go wholesale is a separate M-task).

use tsc_ast::Kind;
use tsc_core::options_generated::ScriptTarget;

const RUNE_ERROR: i32 = 0xFFFD;

// ────────────────────────────────────────────────────────────────────────────
// utf8.DecodeRuneInString / utf8.DecodeLastRuneInString (Go algorithm over raw
// bytes; never panics on truncated/misaligned input, including (RuneError, 1))
// ────────────────────────────────────────────────────────────────────────────

// Lead byte → expected sequence size (Go's utf8 lookup); 0 for invalid leads.
#[inline]
fn rune_start_size(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0, // 0x80..=0xBF (continuation), 0xC0/0xC1 (overlong), 0xF5..=0xFF
    }
}

#[inline]
fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// Go: `func utf8.DecodeRuneInString(s string) (rune, int)` over raw bytes.
/// The scanner's `text` is always valid UTF-8, but positions derived from
/// truncated slices (e.g. `text[:pos-2]`) may not be, so this mirrors Go's
/// acceptance rules exactly (overlong / surrogate / out-of-range → RuneError).
pub fn decode_rune_in_string(s: &[u8]) -> (i32, usize) {
    if s.is_empty() {
        return (RUNE_ERROR, 0);
    }
    let b0 = s[0];
    let size = rune_start_size(b0);
    if size == 0 {
        return (RUNE_ERROR, 1);
    }
    if s.len() < size {
        return (RUNE_ERROR, 1);
    }
    for i in 1..size {
        if !is_continuation(s[i]) {
            return (RUNE_ERROR, 1);
        }
    }
    let r: i32 = match size {
        2 => ((b0 & 0x1F) as i32) << 6 | (s[1] & 0x3F) as i32,
        3 => ((b0 & 0x0F) as i32) << 12 | ((s[1] & 0x3F) as i32) << 6 | (s[2] & 0x3F) as i32,
        _ => {
            ((b0 & 0x07) as i32) << 18
                | ((s[1] & 0x3F) as i32) << 12
                | ((s[2] & 0x3F) as i32) << 6
                | (s[3] & 0x3F) as i32
        }
    };
    let too_short = match size {
        2 => r < 0x80,    // overlong
        3 => r < 0x800 || (0xD800..=0xDFFF).contains(&r),
        _ => r < 0x10000 || r > 0x10FFFF,
    };
    if too_short {
        return (RUNE_ERROR, 1);
    }
    (r, size)
}

/// Go: `func utf8.DecodeLastRuneInString(s string) (rune, int)`.
pub fn decode_last_rune_in_string(s: &[u8]) -> (i32, usize) {
    let end = s.len();
    if end == 0 {
        return (RUNE_ERROR, 0);
    }
    let r0 = s[end - 1];
    if r0 < 0x80 {
        return (r0 as i32, 1);
    }
    if end == 1 {
        // Go's loop runs from end-2 down; with nothing before the non-ASCII
        // last byte there is no valid sequence to find.
        return (RUNE_ERROR, 1);
    }
    // guard against O(n^2) behavior
    let lim = end.saturating_sub(4);
    let mut start = end - 2;
    loop {
        if !is_continuation(s[start]) {
            // s[start] is the lead byte of the candidate sequence.
            let (r, size) = decode_rune_in_string(&s[start..end]);
            if start + size != end {
                return (RUNE_ERROR, 1);
            }
            return (r, size);
        }
        if start == lim {
            break;
        }
        start -= 1;
    }
    (RUNE_ERROR, 1)
}

// ────────────────────────────────────────────────────────────────────────────
// core.go: UTF16Offset + UTF16Len
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type UTF16Offset int` — a character offset measured in UTF-16 code units.
pub type UTF16Offset = i32;

/// Go: `func UTF16Len(s string) UTF16Offset` — the number of UTF-16 code units
/// needed to represent the given UTF-8 encoded string.
pub fn utf16_len(s: &str) -> UTF16Offset {
    // Fast path: scan for non-ASCII bytes. For ASCII-only strings, each byte
    // is one UTF-16 code unit, so we can return len(s) directly.
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b >= 0x80 {
            // Found non-ASCII; count the ASCII prefix, then decode the rest.
            let mut n = i as UTF16Offset;
            for r in s[i..].chars() {
                n += utf16_rune_len(r);
            }
            return n;
        }
    }
    s.len() as UTF16Offset
}

/// Go: `func utf16.RuneLen(r rune) int` for runes that can appear in a valid
/// `&str` (never a surrogate / out of range, where Go returns -1 —
/// unreachable with the scanner's UTF-8 `text`).
#[inline]
fn utf16_rune_len(r: char) -> UTF16Offset {
    if (r as u32) > 0xFFFF { 2 } else { 1 }
}

// ────────────────────────────────────────────────────────────────────────────
// core/scripttarget_stringer_generated.go: ScriptTarget.String()
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (i ScriptTarget) String() string` (stringer output, e.g. "ES2022").
pub fn script_target_string(t: ScriptTarget) -> String {
    match t {
        ScriptTarget::None => "None",
        ScriptTarget::ES5 => "ES5",
        ScriptTarget::ES2015 => "ES2015",
        ScriptTarget::ES2016 => "ES2016",
        ScriptTarget::ES2017 => "ES2017",
        ScriptTarget::ES2018 => "ES2018",
        ScriptTarget::ES2019 => "ES2019",
        ScriptTarget::ES2020 => "ES2020",
        ScriptTarget::ES2021 => "ES2021",
        ScriptTarget::ES2022 => "ES2022",
        ScriptTarget::ES2023 => "ES2023",
        ScriptTarget::ES2024 => "ES2024",
        ScriptTarget::ES2025 => "ES2025",
        ScriptTarget::ES2026 => "ES2026",
        ScriptTarget::ESNext => "ESNext",
        ScriptTarget::JSON => "JSON",
    }
    .to_string()
}

// ────────────────────────────────────────────────────────────────────────────
// core.go: GetSpellingSuggestionForStrings + levenshteinWithMax
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetSpellingSuggestionForStrings(name string, candidates iter.Seq[string]) string`
/// — the closest candidate by (case-weighted) Levenshtein distance, or ""
/// when nothing is close enough. Go map iteration order is randomized, so
/// any candidate order is a valid realization.
pub fn get_spelling_suggestion_for_strings<'i>(
    name: &str,
    candidates: impl Iterator<Item = &'i str>,
) -> String {
    let rune_name: Vec<char> = name.chars().collect();
    let maximum_length_difference = std::cmp::max(2, (rune_name.len() as f64 * 0.34) as usize);
    // If the best result is worse than this, don't bother.
    let mut best_distance = (rune_name.len() as f64 * 0.4).floor() + 0.9;
    let mut best_candidate = String::new();
    let mut has_best = false;
    for candidate_name in candidates {
        let max_len = std::cmp::max(candidate_name.len(), rune_name.len());
        let min_len = std::cmp::min(candidate_name.len(), rune_name.len());
        if candidate_name.is_empty() || max_len - min_len > maximum_length_difference {
            continue;
        }
        if candidate_name == name {
            continue;
        }
        // Only consider candidates less than 3 characters long when they
        // differ by case. Otherwise, don't bother, since a user would
        // usually notice differences of a 2-character name.
        if candidate_name.len() < 3 && !candidate_name.eq_ignore_ascii_case(name) {
            continue;
        }
        let distance = levenshtein_with_max(
            &rune_name,
            &candidate_name.chars().collect::<Vec<_>>(),
            best_distance,
        );
        if distance < 0.0 {
            continue;
        }
        tsc_debug::assert_!(distance <= best_distance); // Else levenshteinWithMax should return undefined
        if distance < best_distance {
            best_distance = distance;
            best_candidate = candidate_name.to_string();
            has_best = true;
        } else if !has_best || candidate_name < best_candidate.as_str() {
            best_candidate = candidate_name.to_string();
            has_best = true;
        }
    }
    best_candidate
}

// PORT: Go compares case with `unicode.ToLower` (simple 1:1 mapping) and
// case-insensitive equality with `strings.EqualFold`. Every candidate the
// scanner feeds in is an ECMA-262 property name/value (ASCII), where ASCII
// folding is identical. `char::to_lowercase` (full mapping) would differ
// only on multi-char lower expansions, which those tables never contain.
/// Go: `func levenshteinWithMax(buffers *levenshteinBuffers, s1 []rune, s2 []rune, maxValue float64) float64`
/// — returns -1 when the distance exceeds maxValue. Go pools the two buffers;
/// Rust allocates locally (cold error-suggestion path).
fn levenshtein_with_max(s1: &[char], s2: &[char], max_value: f64) -> f64 {
    let buffer_size = s2.len() + 1;
    let mut previous = vec![0.0f64; buffer_size];
    let mut current = vec![0.0f64; buffer_size];

    let big = max_value + 0.01;
    for (i, slot) in previous.iter_mut().enumerate() {
        *slot = i as f64;
    }
    for i in 1..=s1.len() {
        let c1 = s1[i - 1];
        let min_j = std::cmp::max(((i as f64) - max_value).ceil() as usize, 1).min(buffer_size);
        let max_j = std::cmp::min((max_value + i as f64).floor() as usize, s2.len());
        let mut col_min = i as f64;
        current[0] = col_min;
        for slot in current.iter_mut().take(min_j).skip(1) {
            *slot = big;
        }
        for j in min_j..=max_j {
            let substitution_distance =
                if s1[i - 1].to_lowercase().eq(s2[j - 1].to_lowercase()) {
                    previous[j - 1] + 0.1
                } else {
                    previous[j - 1] + 2.0
                };
            let dist = if c1 == s2[j - 1] {
                previous[j - 1]
            } else {
                (previous[j] + 1.0).min((current[j - 1] + 1.0).min(substitution_distance))
            };
            current[j] = dist;
            col_min = col_min.min(dist);
        }
        for slot in current.iter_mut().take(buffer_size).skip(max_j + 1) {
            *slot = big;
        }
        if col_min > max_value {
            // Give up — everything in this column is > max and it can't get
            // better in future columns.
            return -1.0;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let res = previous[s2.len()];
    if res > max_value {
        return -1.0;
    }
    res
}

// ────────────────────────────────────────────────────────────────────────────
// ast/utilities.go: IsTypeNodeKind
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsTypeNodeKind(kind Kind) bool` (ast/utilities.go).
pub fn is_type_node_kind(kind: Kind) -> bool {
    match kind {
        Kind::AnyKeyword
        | Kind::UnknownKeyword
        | Kind::NumberKeyword
        | Kind::BigIntKeyword
        | Kind::ObjectKeyword
        | Kind::BooleanKeyword
        | Kind::StringKeyword
        | Kind::SymbolKeyword
        | Kind::VoidKeyword
        | Kind::UndefinedKeyword
        | Kind::NeverKeyword
        | Kind::IntrinsicKeyword
        | Kind::ExpressionWithTypeArguments
        | Kind::JSDocAllType
        | Kind::JSDocNullableType
        | Kind::JSDocNonNullableType
        | Kind::JSDocOptionalType
        | Kind::JSDocVariadicType => return true,
        _ => {}
    }
    Kind::FirstTypeNode <= kind && kind <= Kind::LastTypeNode
}
