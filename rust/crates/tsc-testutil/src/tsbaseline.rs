// Port of tsc/internal/testutil/tsbaseline @
// ec47d33c23e464a17cdf2475632cba629bee8763 — the baseline emitters, at the
// PARSE+BIND depth the M6 conformance runner drives.
//
// File map (Go → Rust):
//   tsbaseline/util.go             → the helpers below (path-prefix
//                                     stripping, lib/built/tsconfig file
//                                     predicates, path sanitization)
//   tsbaseline/error_baseline.go   → the `.errors.txt` emitter
//                                     (do_error_baseline / get_error_baseline
//                                     / iterate_error_baseline)
//
// PORT notes:
//   - The `.types` / `.symbols` emitters (type_symbol_baseline.go) and the
//     content-mapper / JS-emit / sourcemap / module-resolution emitters are
//     CHECKER- or EMIT-scope and are not ported; only their baseline-name
//     mapping (`.tsx?$` → `.types` / `.symbols`) is kept so the climb
//     tracker can count the reference trees. They land with the checker
//     wave (rust/crates/checker) — the conformance runner's verdicts are
//     `.errors.txt`-only until then (see rust/CONFORMANCE.md).
//   - The emitter renders with the Go `diagnosticwriter` recipe
//     (WriteFormatDiagnostic + the squiggle/section layout), but related
//     information drops the `file(line,col)` location: the Rust
//     `ast::Diagnostic` carries related files as unresolvable node ids
//     (PORTING-NOTES' file-id gap), so both conformance drivers render
//     `!!! related TSxxxx: <message>` without the location prefix.
//     Parse-level diagnostics do not carry related information in
//     practice, so reference parity is unaffected on the wired depth.
//   - The pretty-printing arm (`pretty: true`, FormatDiagnosticsWithColorAndContext
//     + WriteErrorSummaryText) is not ported: the harness always baselines
//     with `pretty=false` for compiler/conformance suites
//     (`c.result.Options.Pretty` is unset in every ported test case).

use tsc_ast::Diagnostic;

/// Go: `const harnessNewLine = "\r\n"`.
pub const HARNESS_NEWLINE: &str = "\r\n";

// ──────────────────────────────────────────────────────────────────────
// util.go
// ──────────────────────────────────────────────────────────────────────

/// Go: `lineDelimiter = regexp.MustCompile("\r?\n")` — splits on `\n`,
/// dropping an optional preceding `\r`.
pub fn split_line_delimited(text: &str) -> Vec<&str> {
    text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect()
}

/// Go: `nonWhitespace = regexp.MustCompile("\\S")` — every non-`\s` byte
/// becomes a space. RE2's `\s` is exactly `[\t\n\f\r ]`.
fn replace_non_whitespace(line: &str) -> String {
    line.chars()
        .map(|c| if matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ') { c } else { ' ' })
        .collect()
}

/// Go: `tsExtension = regexp.MustCompile("\\.tsx?$")` — the baseline-name
/// mapping. `extension` is the replacement (".errors.txt" / ".types" /
/// ".symbols").
pub fn replace_ts_extension(name: &str, extension: &str) -> String {
    let stripped = name
        .strip_suffix(".tsx")
        .or_else(|| name.strip_suffix(".ts"))
        .unwrap_or(name);
    format!("{stripped}{extension}")
}

/// Go: `testPathCharacters = regexp.MustCompile("[\\^<>:\"|?*%]")`.
fn replace_test_path_characters(name: &str) -> String {
    name.chars()
        .map(|c| if matches!(c, '^' | '<' | '>' | ':' | '"' | '|' | '?' | '*' | '%') { '_' } else { c })
        .collect()
}

/// Go: `testPathDotDot = regexp.MustCompile("\\.\\./")`.
fn replace_test_path_dot_dot(name: &str) -> String {
    name.replace("../", "__dotdot/")
}

/// Go: `testPathPrefixReplacer` — simultaneous left-to-right replacement,
/// patterns tried in argument order at each position (strings.Replacer
/// semantics).
fn replace_simultaneous(text: &str, pairs: &[(&str, &str)]) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    'outer: while i < bytes.len() {
        for (from, to) in pairs {
            if text[i..].starts_with(from) {
                out.push_str(to);
                i += from.len();
                continue 'outer;
            }
        }
        // advance one full UTF-8 character (Go advances one rune here in
        // practice via the replacer's rune decoding)
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_char_len(first_byte: u8) -> usize {
    if first_byte < 0x80 {
        1
    } else if first_byte >> 5 == 0b110 {
        2
    } else if first_byte >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Go: `removeTestPathPrefixes(text, retainTrailingDirectorySeparator)`.
pub fn remove_test_path_prefixes(text: &str, retain_trailing_directory_separator: bool) -> String {
    const PAIRS: &[(&str, &str)] = &[
        ("/.ts/", ""),
        ("/.lib/", ""),
        ("/.src/", ""),
        ("bundled:///libs/", ""),
        ("file:///./ts/", "file:///"),
        ("file:///./lib/", "file:///"),
        ("file:///./src/", "file:///"),
    ];
    const PAIRS_TRAILING: &[(&str, &str)] = &[
        ("/.ts/", "/"),
        ("/.lib/", "/"),
        ("/.src/", "/"),
        ("bundled:///libs/", "/"),
        ("file:///./ts/", "file:///"),
        ("file:///./lib/", "file:///"),
        ("file:///./src/", "file:///"),
    ];
    if retain_trailing_directory_separator {
        replace_simultaneous(text, PAIRS_TRAILING)
    } else {
        replace_simultaneous(text, PAIRS)
    }
}

/// Go: `isDefaultLibraryFile` — basename starts "lib." and ends ".d.ts".
pub fn is_default_library_file(file_path: &str) -> bool {
    let file_name = tsc_tspath::get_base_file_name(file_path);
    file_name.starts_with("lib.") && file_name.ends_with(".d.ts")
}

const LIB_FOLDER: &str = "built/local/";
const BUILT_FOLDER: &str = "/.ts";

/// Go: `isBuiltFile`.
pub fn is_built_file(file_path: &str) -> bool {
    file_path.starts_with(LIB_FOLDER) || file_path.starts_with(&format!("{BUILT_FOLDER}/"))
}

/// Go: `isTsConfigFile` (the !!! comment is Go's own).
pub fn is_ts_config_file(path: &str) -> bool {
    // !!! fix to check for just prefixes/suffixes
    path.contains("tsconfig") && path.contains("json")
}

/// Go: `sanitizeTestFilePath` — used as the dupe-case map key.
pub fn sanitize_test_file_path(name: &str) -> String {
    let path = replace_test_path_characters(name);
    let path = path.replace('\\', "/");
    let path = replace_test_path_dot_dot(&path);
    // Go: CaseInsensitive.Canonicalize(NormalizePath(path)) then TrimPrefix "/"
    let path = tsc_tspath::CaseSensitivity::CaseInsensitive
        .canonicalize(path.as_str())
        .to_string();
    path.strip_prefix('/').unwrap_or(&path).to_string()
}

// ──────────────────────────────────────────────────────────────────────
// error_baseline.go — the .errors.txt emitter
// ──────────────────────────────────────────────────────────────────────

/// One diagnostic paired with the file it was reported against (Go:
/// `*ast.Diagnostic` + `.File()` through the pointer graph; the Rust ast
/// crate's node-id gap makes the pairing explicit — same shape the compiler
/// crate's `FileDiagnostic` established).
pub struct ErrorBaselineDiagnostic<'a> {
    /// The source file's rooted path ("" = a global diagnostic).
    pub file_name: &'a str,
    /// The source file's text ("" when global) — line starts are computed
    /// from it (Go: the source file's ECMALineMap).
    pub file_text: &'a str,
    /// The diagnostic itself.
    pub diagnostic: &'a Diagnostic,
}

impl ErrorBaselineDiagnostic<'_> {
    /// Go: `diagnosticwriter.FlattenDiagnosticMessage` (the non-pretty arm):
    /// `Diagnostic.Localize` + the chain walk. The chain indent is two
    /// spaces per level, joined by the harness newline.
    fn localize(&self) -> String {
        let d = self.diagnostic;
        let mut out = localize_diagnostic(d);
        for chain in d.message_chain() {
            flatten_chain(&mut out, chain, 1);
        }
        out
    }
}

/// Go: `Diagnostic.Localize(locale.Default)` — `messageText` when the
/// message is adhoc, else the pattern formatted with the args.
fn localize_diagnostic(d: &Diagnostic) -> String {
    if d.message().is_none() && !d.message_text().is_empty() {
        return d.message_text().to_string();
    }
    match d.message() {
        Some(message) => tsc_diagnostics::format(message.text(), d.message_args()),
        None => String::new(),
    }
}

/// Go: `flattenDiagnosticMessageChain`.
fn flatten_chain(out: &mut String, chain: &Diagnostic, level: usize) {
    out.push_str(HARNESS_NEWLINE);
    for _ in 0..level {
        out.push_str("  ");
    }
    out.push_str(&localize_diagnostic(chain));
    for child in chain.message_chain() {
        flatten_chain(out, child, level + 1);
    }
}

/// Go: `minimalDiagnosticsToString` with `pretty=false` —
/// `WriteFormatDiagnostics`: per diagnostic
/// `file(line+1,char+1): category TScode: message\r\n` (no file prefix for
/// globals).
fn minimal_diagnostics_to_string(diagnostics: &[&ErrorBaselineDiagnostic<'_>]) -> String {
    let mut out = String::new();
    for d in diagnostics {
        let diag = d.diagnostic;
        if !d.file_name.is_empty() {
            let line_starts = tsc_ast::compute_ecma_line_starts(d.file_text);
            let (line, character) = line_and_utf16_character(&line_starts, d.file_text, diag.pos());
            out.push_str(&remove_test_path_prefixes(d.file_name, false));
            out.push_str(&format!("({},{})", line + 1, character.0 + 1));
            out.push_str(": ");
        }
        out.push_str(&format!("{} TS{}: ", diag.category().name(), diag.code()));
        out.push_str(&d.localize());
        out.push_str(HARNESS_NEWLINE);
    }
    out
}

/// Go: `scanner.GetECMALineAndUTF16CharacterOfPosition` over a text +
/// precomputed line starts (the scanner crate's SourceFileLike shape needs
/// an owned source file; the emitter only ever has text here).
fn line_and_utf16_character(line_starts: &[i32], text: &str, pos: i32) -> (usize, tsc_scanner::UTF16Offset) {
    let line = tsc_scanner::compute_line_of_position(line_starts, pos);
    let character = tsc_scanner::utf16_len(&text[line_starts[line] as usize..pos as usize]);
    (line, character)
}

/// Go: `GetErrorBaseline` (non-pretty): the whole `.errors.txt` body — the
/// minimal-diagnostics header block, the global-error block, and one
/// squiggle section per input file.
///
/// `input_files` is Go's `[]*harnessutil.TestFile` (unit name + content);
/// the diagnostics are the compilation's at the wired depth. Empty
/// diagnostics → the caller emits [`crate::baseline::NO_CONTENT`] (Go:
/// `DoErrorBaseline`).
pub fn get_error_baseline(
    input_files: &[(&str, &str)],
    diagnostics: &[&ErrorBaselineDiagnostic<'_>],
) -> String {
    iterate_error_baseline(input_files, diagnostics).join("")
}

/// Go: `DoErrorBaseline`'s content selection: no diagnostics → NoContent.
pub fn error_baseline_content(
    input_files: &[(&str, &str)],
    diagnostics: &[&ErrorBaselineDiagnostic<'_>],
) -> String {
    if diagnostics.is_empty() {
        crate::baseline::NO_CONTENT.to_string()
    } else {
        get_error_baseline(input_files, diagnostics)
    }
}

/// The diagnostics-location header fixup: Go's
/// `diagnosticsLocationPrefix = regexp.MustCompile("(?im)^(lib.*\\.d\\.ts)\\(\\d+,\\d+\\)")`
/// → `$1(--,--)` on the minimal-diagnostics block. Hand-rolled (lines are
/// already split; `(?i)` on the leading `lib`).
fn replace_diagnostics_location_prefix(line: &str) -> String {
    let lower = line.to_lowercase();
    if !lower.starts_with("lib") {
        return line.to_string();
    }
    // find ".d.ts(" after the lib prefix, then digits "," digits ")"
    let Some(open) = lower.find(".d.ts(") else {
        return line.to_string();
    };
    let rest = &line[open + ".d.ts(".len()..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b',' {
        return line.to_string();
    }
    let after_comma = &rest[i + 1..];
    let bytes = after_comma.as_bytes();
    let mut j = 0;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    if j == 0 || j >= bytes.len() || bytes[j] != b')' {
        return line.to_string();
    }
    format!("{}(--,--)", &line[..open + ".d.ts(".len()])
}

/// Go: `iterateErrorBaseline` (pretty=false).
fn iterate_error_baseline(input_files: &[(&str, &str)], diagnostics: &[&ErrorBaselineDiagnostic<'_>]) -> Vec<String> {
    // Go sorts a clone via CompareASTDiagnostics (path, pos, end, code,
    // category, source, message identity, args). The emitter's own
    // diagnostics arrive unsorted.
    let mut sorted: Vec<&ErrorBaselineDiagnostic<'_>> = diagnostics.iter().copied().collect();
    sorted.sort_by(|a, b| compare_error_baseline_diagnostics(a, b));

    let mut result: Vec<String> = Vec::new();

    // ── The minimal-diagnostics block ──
    let top = minimal_diagnostics_to_string(&sorted);
    let top = remove_test_path_prefixes(&top, false);
    let top: Vec<&str> = split_line_delimited(&top);
    let top: Vec<String> = top.iter().map(|line| replace_diagnostics_location_prefix(line)).collect();
    let top = top.join(HARNESS_NEWLINE);
    result.push(format!("{top}{HARNESS_NEWLINE}{HARNESS_NEWLINE}"));

    // ── Global errors (no file) ──
    let mut output_lines = String::new();
    let mut first_line = true;
    let mut new_line = |first: &mut bool, out: &mut String| {
        if *first {
            *first = false;
            return;
        }
        out.push_str(HARNESS_NEWLINE);
    };
    for d in &sorted {
        if d.file_name.is_empty() {
            output_error_text(&mut new_line, &mut output_lines, d);
        }
    }
    result.push(output_lines.clone());
    output_lines.clear();

    // ── One section per input file ──
    for (unit_name, unit_content) in input_files {
        // Go: filter by ComparePaths(removePrefixes(errFile), removePrefixes(unit), CaseInsensitive) == 0
        let file_errors: Vec<&&ErrorBaselineDiagnostic<'_>> = sorted
            .iter()
            .filter(|d| {
                !d.file_name.is_empty()
                    && tsc_tspath::CaseSensitivity::CaseInsensitive.compare_paths(
                        &tsc_tspath::to_rooted_path(&remove_test_path_prefixes(d.file_name, false), &Default::default()),
                        &tsc_tspath::to_rooted_path(&remove_test_path_prefixes(unit_name, false), &Default::default()),
                    ) == 0
            })
            .collect();

        // Header
        new_line(&mut first_line, &mut output_lines);
        output_lines.push_str(&format!(
            "==== {} ({} errors) ====",
            remove_test_path_prefixes(unit_name, false),
            file_errors.len()
        ));

        let line_starts = tsc_ast::compute_ecma_line_starts(unit_content);
        let lines = split_line_delimited(unit_content);

        for (line_index, line) in lines.iter().enumerate() {
            let this_line_start = line_starts[line_index];
            // Go fakes the next line start on the last line so errors on the
            // file's last character squiggle correctly.
            let next_line_start = if line_index == lines.len() - 1 {
                unit_content.len() as i32
            } else {
                line_starts[line_index + 1]
            };
            new_line(&mut first_line, &mut output_lines);
            output_lines.push_str("    ");
            output_lines.push_str(line);
            for err_diagnostic in &file_errors {
                let d = err_diagnostic.diagnostic;
                let err_start = d.pos();
                let end = err_start + d.len();
                if end >= this_line_start && (err_start < next_line_start || line_index == lines.len() - 1) {
                    // squiggle the span on this line (byte offsets; the
                    // squiggle length counts runes — Go utf8.RuneCountInString)
                    let relative_offset = err_start - this_line_start;
                    let length = (end - err_start) - 0.max(this_line_start - err_start);
                    let squiggle_start = 0.max(relative_offset) as usize;
                    output_lines.push_str(HARNESS_NEWLINE);
                    output_lines.push_str("    ");
                    let line_bytes = line.as_bytes();
                    let squiggle_start = squiggle_start.min(line_bytes.len());
                    output_lines.push_str(&replace_non_whitespace(&line[..squiggle_start]));
                    let squiggle_end = squiggle_start.max((squiggle_start + length as usize).min(line_bytes.len()));
                    let tilde_count = line[squiggle_start..squiggle_end].chars().count();
                    for _ in 0..tilde_count {
                        output_lines.push('~');
                    }
                    if line_index == lines.len() - 1 || next_line_start > end {
                        output_error_text(&mut new_line, &mut output_lines, err_diagnostic);
                    }
                }
            }
        }
        result.push(output_lines.clone());
        output_lines.clear();
    }

    result
}

/// Go: `outputErrorText` — the `!!! category TScode: message` lines (and
/// `!!! related TScode...: message` for related information; the location
/// prefix is PORT-noted away in the module header).
fn output_error_text(
    new_line: &mut impl FnMut(&mut bool, &mut String),
    output_lines: &mut String,
    d: &ErrorBaselineDiagnostic<'_>,
) {
    let message = d.localize();
    let message = remove_test_path_prefixes(&message, false);
    let diag = d.diagnostic;
    let mut err_lines: Vec<String> = Vec::new();
    for line in split_line_delimited(&message) {
        if line.is_empty() {
            continue;
        }
        err_lines.push(format!("!!! {} TS{}: {}", diag.category().name(), diag.code(), line));
    }
    for info in diag.related_information() {
        // PORT: no location prefix (see the module header).
        err_lines.push(format!("!!! related TS{}: {}", info.code(), localize_diagnostic(info)));
    }
    let mut first = true;
    for e in &err_lines {
        new_line(&mut first, output_lines);
        output_lines.push_str(e);
    }
}

/// Go: `CompareASTDiagnostics` → `ast.CompareDiagnostics` (path via the
/// file pairing, then loc, code, category, source, message identity, args).
fn compare_error_baseline_diagnostics(a: &ErrorBaselineDiagnostic<'_>, b: &ErrorBaselineDiagnostic<'_>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let cmp = a.file_name.cmp(b.file_name);
    if cmp != Ordering::Equal {
        return cmp;
    }
    let (da, db) = (a.diagnostic, b.diagnostic);
    let cmp = da.pos().cmp(&db.pos());
    if cmp != Ordering::Equal {
        return cmp;
    }
    let cmp = da.end().cmp(&db.end());
    if cmp != Ordering::Equal {
        return cmp;
    }
    let cmp = da.code().cmp(&db.code());
    if cmp != Ordering::Equal {
        return cmp;
    }
    let cmp = da.category().name().cmp(db.category().name());
    if cmp != Ordering::Equal {
        return cmp;
    }
    let cmp = da.source().cmp(db.source());
    if cmp != Ordering::Equal {
        return cmp;
    }
    let cmp = message_identity(da).cmp(&message_identity(db));
    if cmp != Ordering::Equal {
        return cmp;
    }
    da.message_args().cmp(db.message_args())
}

/// Go: `getDiagnosticMessageIdentity` (the version the compiler crate
/// ports crate-locally — repeated here so the emitter does not depend on
/// checker-scope packages).
fn message_identity(d: &Diagnostic) -> String {
    if !d.message_text().is_empty() {
        return d.message_text().to_string();
    }
    if let Some(message) = d.message() {
        if d.code() == -1 {
            return message.text().to_string();
        }
    }
    d.message_key().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_ts_extension_like_the_regex() {
        assert_eq!(replace_ts_extension("foo.ts", ".errors.txt"), "foo.errors.txt");
        assert_eq!(replace_ts_extension("foo.tsx", ".types"), "foo.types");
        assert_eq!(replace_ts_extension("foo.d.ts", ".symbols"), "foo.d.symbols");
        assert_eq!(replace_ts_extension("foo.js", ".errors.txt"), "foo.js");
    }

    #[test]
    fn removes_test_path_prefixes() {
        assert_eq!(remove_test_path_prefixes("/.src/foo.ts", false), "foo.ts");
        assert_eq!(remove_test_path_prefixes("/.lib/lib.es5.d.ts", false), "lib.es5.d.ts");
        assert_eq!(remove_test_path_prefixes("/.src/", false), "");
        // retainTrailingDirectorySeparator keeps the separator
        assert_eq!(remove_test_path_prefixes("/.src/foo", true), "/foo");
    }

    #[test]
    fn file_predicates() {
        assert!(is_default_library_file("/.src/lib.es5.d.ts"));
        assert!(!is_default_library_file("/.src/lib.es5.js"));
        assert!(is_ts_config_file("/.src/tsconfig.json"));
        assert!(!is_ts_config_file("/.src/foo.ts"));
        assert!(is_built_file("/.ts/foo.ts"));
        assert!(!is_built_file("/.src/foo.ts"));
    }

    #[test]
    fn sanitizes_test_paths() {
        assert_eq!(sanitize_test_file_path("/.src/A|B.ts"), "a_b.ts");
        assert_eq!(sanitize_test_file_path("/.src/../up.ts"), "__dotdot/up.ts");
    }

    #[test]
    fn location_prefix_fixup_on_lib_lines() {
        assert_eq!(
            replace_diagnostics_location_prefix("lib.es5.d.ts(12,34): error TS1: x"),
            "lib.es5.d.ts(--,--): error TS1: x"
        );
        // non-lib lines and non-matching shapes pass through
        assert_eq!(replace_diagnostics_location_prefix("foo.ts(1,2): error"), "foo.ts(1,2): error");
        assert_eq!(replace_diagnostics_location_prefix("lib.es5.d.ts(a,b)"), "lib.es5.d.ts(a,b)");
    }

    #[test]
    fn renders_a_parse_error_section_like_the_reference_shape() {
        // The reference shape (testdata/baselines/reference/conformance/ArrowFunction1.errors.txt):
        //   header block, blank, "==== name (N errors) ====", the source
        //   line, squiggle line, "!!! error TSxxxx: message".
        let sf = tsc_parser::parse_source_file(
            tsc_ast::SourceFileParseOptions::default(),
            "var v = (a: ) => {",
            tsc_core::scriptkind::ScriptKind::TS,
        );
        let d = &sf.diagnostics()[0];
        let view = ErrorBaselineDiagnostic {
            file_name: "/.src/ArrowFunction1.ts",
            file_text: "var v = (a: ) => {",
            diagnostic: d,
        };
        let content = error_baseline_content(&[("/.src/ArrowFunction1.ts", "var v = (a: ) => {")], &[&view]);
        assert_ne!(content, crate::baseline::NO_CONTENT);
        let normalized = content.replace("\r\n", "\n");
        assert!(normalized.starts_with("ArrowFunction1.ts(1,13): error TS"), "{normalized}");
        assert!(normalized.contains("==== ArrowFunction1.ts (1 errors) ===="), "{normalized}");
        assert!(normalized.contains("    var v = (a: ) => {"), "{normalized}");
        assert!(normalized.contains("                ~"), "{normalized}");
        assert!(normalized.contains("!!! error TS"), "{normalized}");
    }

    #[test]
    fn empty_diagnostics_yield_no_content() {
        assert_eq!(error_baseline_content(&[("/.src/a.ts", "let a;")], &[]), crate::baseline::NO_CONTENT);
    }
}
