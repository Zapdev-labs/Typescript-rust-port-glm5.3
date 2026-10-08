// Port of tsc/internal/testrunner/compiler_test_filter.go @
// ec47d33c23e464a17cdf2475632cba629bee8763 — the `-test.run` pattern filter
// the compiler/conformance suites apply to case basenames.
//
// PORT notes:
//   - Go compiles the component with `regexp.Compile` and consumes only
//     `LiteralPrefix` (never the matcher); the port embeds a structural
//     RE2-subset validator + literal-prefix decoder (no regex crate is in
//     the workspace's approved dependency set). The port mirrors Go's
//     observed LiteralPrefix behavior — including the whole-expression
//     capture-group transparency (`(Example/Other)` → "Example/Other",
//     complete) and the top-level-alternation/character-class/flag-group
//     zero-prefix cases (verified against Go regexp at port time).
//   - `strconv.IsPrint` is approximated by `!is_control && (ASCII space ||
//     !is_whitespace)` — Go's IsPrint is the Unicode graphic set plus ASCII
//     space; the approximation agrees on every character class the filter
//     discriminates (space, NBSP, NUL, tabs, control bytes).

/// Go: `func compilerTestFileFilter(pattern string) func(string) bool` —
/// build the predicate over case basenames. Patterns that would need a
/// real matcher (or a rewrite) accept everything.
pub fn compiler_test_file_filter(pattern: &str) -> Box<dyn Fn(&str) -> bool + '_> {
    let accept_all: Box<dyn Fn(&str) -> bool> = Box::new(|_| true);
    if compiler_test_name_needs_rewrite(pattern) {
        return accept_all;
    }
    let Some(parts) = split_compiler_test_filter(pattern) else {
        return accept_all;
    };
    if parts.len() < 2 {
        return accept_all;
    }
    let component = &parts[1];
    let anchored = component.starts_with('^');
    let expression = component.strip_prefix('^').unwrap_or(component);
    let Some(prefix) = literal_prefix(expression) else {
        // Go: regexp.Compile error → accept all.
        return accept_all;
    };
    if prefix.is_empty()
        || compiler_test_name_needs_rewrite(&prefix)
        || (!anchored && !prefix.chars().any(char::is_uppercase))
    {
        return accept_all;
    }
    Box::new(move |filename: &str| {
        if compiler_test_name_needs_rewrite(filename) {
            return true;
        }
        if anchored {
            return filename.starts_with(prefix.as_str()) || prefix.starts_with(&format!("{filename}_"));
        }
        if filename.contains(prefix.as_str()) {
            return true;
        }
        for (index, _) in filename.char_indices() {
            if prefix.starts_with(&format!("{}_", &filename[index..])) {
                return true;
            }
        }
        false
    })
}

/// Go: `func compilerTestNameNeedsRewrite(name) bool` — whitespace or
/// non-printable characters (the test runner rewrites such names).
pub fn compiler_test_name_needs_rewrite(name: &str) -> bool {
    name.chars().any(|c| c.is_whitespace() || !is_print(c))
}

/// Go: `strconv.IsPrint` approximation (see the module header).
fn is_print(c: char) -> bool {
    !c.is_control() && (c == ' ' || !c.is_whitespace())
}

/// Go: `func splitCompilerTestFilter(pattern string) []string` — split on
/// '/' at group depth 0 outside character classes; a top-level '|'
/// nulls the whole split. Returns `None` for Go's nil.
fn split_compiler_test_filter(pattern: &str) -> Option<Vec<String>> {
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut in_class = false;
    let mut escaped = false;
    for (index, character) in pattern.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if in_class {
            if character == ']' {
                in_class = false;
            }
            continue;
        }
        match character {
            '[' => in_class = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' => {
                if depth == 0 {
                    return None;
                }
            }
            '/' => {
                if depth == 0 {
                    parts.push(pattern[start..index].to_string());
                    start = index + 1;
                }
            }
            _ => {}
        }
    }
    parts.push(pattern[start..].to_string());
    Some(parts)
}

/// The structural RE2-subset literal prefix: `Some(prefix)` for a valid
/// pattern (the longest literal every match must begin with), `None` when
/// the pattern fails to compile.
///
/// Mirrors the Go `regexp` observations from port time:
///   - whole-expression capture groups are transparent:
///     `(Example/Other)` → "Example/Other" (complete)
///   - flag groups (`(?i)...`) fold case → no literal prefix
///   - top-level alternation → no literal prefix
///   - any other metacharacter ends the prefix (incomplete)
fn literal_prefix(expression: &str) -> Option<String> {
    // A single whole-expression capture group wraps the rest — recurse into
    // it transparently (Go: OpCapture of an OpLiteral; verified at port
    // time: `(Example/Other)` → "Example/Other").
    if expression.starts_with('(') && !expression[1..].starts_with('?') {
        let Some(close) = matching_group_close(expression) else {
            return None; // unbalanced group — invalid
        };
        if close == expression.len() - 1 {
            return literal_prefix(&expression[1..close]);
        }
        // the group is part of a larger expression — Go's prefix stops
        // there; the main scan below breaks on the '(' immediately.
    }
    let mut prefix = String::new();
    let chars: Vec<(usize, char)> = expression.char_indices().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let (_, c) = chars[i];
        match c {
            '\\' => {
                // escape: decode the literal
                let Some((_, next)) = chars.get(i + 1) else {
                    return None; // trailing backslash — invalid
                };
                match next {
                    'x' => {
                        // \xHH (or \x{...}) hex escape
                        let after = &expression[chars[i + 1].0 + 1..];
                        let (decoded, width) = decode_hex_escape(after)?;
                        prefix.push(decoded);
                        i += 2 + width;
                    }
                    't' => {
                        prefix.push('\t');
                        i += 2;
                    }
                    'n' => {
                        prefix.push('\n');
                        i += 2;
                    }
                    'r' => {
                        prefix.push('\r');
                        i += 2;
                    }
                    'f' => {
                        prefix.push('\u{c}');
                        i += 2;
                    }
                    'v' => {
                        prefix.push('\u{b}');
                        i += 2;
                    }
                    'a' => {
                        prefix.push('\u{7}');
                        i += 2;
                    }
                    c if !c.is_alphanumeric() => {
                        // punctuation escape: the character itself
                        prefix.push(*c);
                        i += 2;
                    }
                    // character classes / anchors / backrefs (\d, \b, \1):
                    // not a literal — the prefix ends here
                    _ => break,
                }
            }
            // top-level alternation: no common literal prefix
            '|' => return Some(String::new()),
            // metacharacters: the prefix ends before them
            '(' | ')' | '[' | ']' | '{' | '}' | '?' | '*' | '+' => break,
            '^' | '$' | '.' => break,
            c => {
                prefix.push(c);
                i += 1;
            }
        }
    }
    // Structural validity: balanced parens/classes (approximation of RE2's
    // full validation — see the module header PORT note).
    if !structurally_valid(expression) {
        return None;
    }
    Some(prefix)
}

/// The byte index of the `)` matching the `(` at byte 0, or None when
/// unbalanced.
fn matching_group_close(expression: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_class = false;
    let mut escaped = false;
    for (index, c) in expression.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if in_class {
            if c == ']' {
                in_class = false;
            }
            continue;
        }
        match c {
            '[' => in_class = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Decodes `\xHH` / `\x{HHHH}` at the start of `after`; returns the char and
/// the number of chars consumed after the `x`.
fn decode_hex_escape(after: &str) -> Option<(char, usize)> {
    if after.starts_with('{') {
        let end = after.find('}')?;
        let value = u32::from_str_radix(&after[1..end], 16).ok()?;
        return Some((char::from_u32(value)?, end + 2));
    }
    let hex = &after[..2.min(after.len())];
    if hex.len() < 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some((char::from_u32(value)?, 2))
}

/// The RE2-subset validity approximation: balanced groups and classes, no
/// trailing escape.
fn structurally_valid(expression: &str) -> bool {
    let mut depth = 0i32;
    let mut in_class = false;
    let mut escaped = false;
    for c in expression.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if in_class {
            if c == ']' {
                in_class = false;
            }
            continue;
        }
        match c {
            '[' => in_class = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0 && !in_class && !escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go: `TestCompilerTestFileFilter` (the table, verbatim).
    #[test]
    fn test_compiler_test_file_filter() {
        let tests: &[(&str, &str, &str, bool)] = &[
            ("unfiltered", "", "other.ts", true),
            ("parentOnly", "TestLocal", "other.ts", true),
            ("filenamePrefix", "TestLocal/flakyDiagnostic1038", "flakyDiagnostic1038.ts", true),
            ("differentFilename", "TestLocal/flakyDiagnostic1038", "other.ts", false),
            ("substring", "TestLocal/Diagnostic1038", "flakyDiagnostic1038.ts", true),
            ("anchoredPrefix", "TestLocal/^lowercase", "lowercase.ts", true),
            ("anchoredMismatch", "TestLocal/^lowercase", "other.ts", false),
            ("unanchoredLowercaseFallback", "TestLocal/target", "other.ts", true),
            ("configuration", "TestLocal/^Example\\.ts_target=es2020$", "Example.ts", true),
            ("configurationSubstring", "TestLocal/Example\\.ts_target=es2020$", "someExample.ts", true),
            ("differentConfigurationFile", "TestLocal/^Example\\.ts_target=es2020$", "other.ts", false),
            ("spaceConfigurationFallback", "TestLocal/Example\\.ts target=es2020/error", "Example.ts", true),
            ("anchoredSpaceConfigurationFallback", "TestLocal/^Example\\.ts target=es2020$", "Example.ts", true),
            ("tabConfigurationFallback", "TestLocal/Example\\.ts\ttarget=es2020/error", "Example.ts", true),
            ("unicodeSpaceFallback", "TestLocal/^Example\u{a0}Name", "Example_Name.ts", true),
            ("nonPrintablePatternFallback", "TestLocal/^Example\u{0}Name", "other.ts", true),
            ("escapedWhitespacePrefixFallback", "TestLocal/^Example\\x20Name", "other.ts", true),
            ("whitespaceFilenameFallback", "TestLocal/^Example_Name", "Example Name.ts", true),
            ("nonPrintableFilenameFallback", "TestLocal/^Example\\x00Name", "Example\u{0}Name.ts", true),
            ("nestedSubtest", "TestLocal/flakyDiagnostic1038/error", "flakyDiagnostic1038.ts", true),
            ("caseInsensitiveFallback", "TestLocal/(?i)Example", "other.ts", true),
            ("alternationFallback", "TestLocal/Example|TestLocal/Other", "Other.ts", true),
            ("groupFallback", "TestLocal/(Example|Other)", "Other.ts", true),
            ("groupedSlash", "TestLocal/(Example/Other)", "other.ts", false),
            ("classSlashFallback", "TestLocal/[A/B]", "other.ts", true),
            ("escapedSlash", "TestLocal/Example\\/Other", "other.ts", false),
            ("invalidExpressionFallback", "TestLocal/(", "other.ts", true),
            ("anchoredParent", "^TestLocal$/Example", "Example.ts", true),
        ];
        for (name, pattern, filename, included) in tests {
            let filter = compiler_test_file_filter(pattern);
            assert_eq!(filter(filename), *included, "{name}: pattern {pattern:?} on {filename:?}");
        }
    }

    #[test]
    fn literal_prefix_mirrors_the_go_observations() {
        assert_eq!(literal_prefix("flakyDiagnostic1038").as_deref(), Some("flakyDiagnostic1038"));
        assert_eq!(literal_prefix("lowercase").as_deref(), Some("lowercase"));
        assert_eq!(literal_prefix("Example\\.ts_target=es2020").as_deref(), Some("Example.ts_target=es2020"));
        // whole-expression capture group is transparent
        assert_eq!(literal_prefix("(Example/Other)").as_deref(), Some("Example/Other"));
        // classes, alternations, flag groups: no prefix
        assert_eq!(literal_prefix("[A/B]").as_deref(), Some(""));
        assert_eq!(literal_prefix("(?i)Example").as_deref(), Some(""));
        assert_eq!(literal_prefix("Example|Other").as_deref(), Some(""));
        // escaped '/' is the literal character
        assert_eq!(literal_prefix("Example\\/Other").as_deref(), Some("Example/Other"));
        // \x20 decodes to a space; \x00 to NUL
        assert_eq!(literal_prefix("Example\\x20Name").as_deref(), Some("Example Name"));
        assert_eq!(literal_prefix("Example\\x00Name").as_deref(), Some("Example\u{0}Name"));
        // metacharacters end the prefix
        assert_eq!(literal_prefix("Example.ts_target=es2020$").as_deref(), Some("Example.ts_target=es2020"));
        // unbalanced / trailing escapes are compile errors
        assert_eq!(literal_prefix("("), None);
        assert_eq!(literal_prefix("Example\\"), None);
    }
}
