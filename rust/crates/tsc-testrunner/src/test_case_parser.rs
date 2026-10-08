// Port of tsc/internal/testrunner/test_case_parser.go @
// ec47d33c23e464a17cdf2475632cba629bee8763 — the @filename / @option
// directive machinery.
//
// PORT notes:
//   - Go's `ParseTestFilesAndSymlinks[T]` generic exists for the fourslash
//     harness (its `parseFile` builds fourslash state). The compiler runner
//     only ever instantiates it with `*testUnit`, so the port is the
//     concrete `Vec<TestUnit>` form (the generic re-opens with fourslash).
//   - Go's `makeUnitsFromTest` also parses an in-test tsconfig.json unit
//     through `tsoptions.ParseJsonSourceFileConfigFileContent` (a parsed
//     `*tsoptions.ParsedCommandLine`); the port keeps the tsconfig unit in
//     [`TestCaseContent`] and lets the runner's written-tsconfig route do
//     the parsing (the M5 program-twin construction — compiler_runner.rs),
//     since the Rust tsoptions' AST-config surface is PORT-noted unwired.
//   - The regexes (`optionRegex`, `linkRegex`) are hand-rolled line
//     matchers: Go's `(?m)^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)` and the @link
//     form, over already-split lines (the port splits on `\r?\n` first,
//     exactly the `lineDelimiter` split Go applies).

use std::collections::BTreeMap;

use tsc_scanner::skip_trivia;
use tsc_tspath::get_base_file_name;

/// Go: `type rawCompilerSettings map[string]string` — lowercase directive
/// name → value as written (the port's BTreeMap pins iteration order).
pub type RawCompilerSettings = BTreeMap<String, String>;

/// Go: `type testUnit struct { content, name string }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestUnit {
    pub content: String,
    pub name: String,
}

/// Go: `type testCaseContent struct` — minus the parsed `tsConfig`
/// (PORT-noted above: the tsconfig unit travels as the unit itself).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestCaseContent {
    pub test_unit_data: Vec<TestUnit>,
    /// The in-test tsconfig.json/jsconfig.json unit, if any (Go: `tsConfig`
    /// parsed + `tsConfigFileUnitData`).
    pub ts_config_file_unit_data: Option<TestUnit>,
    /// Go: `symlinks` — symlink path → target path.
    pub symlinks: BTreeMap<String, String>,
}

/// Go: `type ParseTestFilesOptions` — the implicit-first-file gate
/// (fourslash-only; the compiler runner passes the zero value).
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseTestFilesOptions {
    pub allow_implicit_first_file: bool,
}

/// Go: `fourslashDirectives` — file-specific directives recorded per-unit
/// (consumed by the fourslash harness; kept for parity).
const FOURSLASH_DIRECTIVES: [&str; 2] = ["emitthisfile", "noopen"];

/// Go: `makeUnitsFromTest(code, fileName)` — split a test file into named
/// units (the tsconfig-parsing arm is PORT-noted in the module header).
pub fn make_units_from_test(code: &str, file_name: &str) -> TestCaseContent {
    let (units, symlinks, raw_current_directory, _global_options) =
        parse_test_files_and_symlinks_with_options(code, file_name, ParseTestFilesOptions::default());

    // Go normalizes the current directory through ToRootedDirectoryPath
    // when set; the runner does that when it consumes the units, so the
    // raw value travels on (the caller owns rooted-path resolution).
    let _ = raw_current_directory;

    TestCaseContent {
        test_unit_data: units,
        ts_config_file_unit_data: None,
        symlinks,
    }
}

/// Go: `ParseTestFilesAndSymlinks` (the default-options form).
pub fn parse_test_files_and_symlinks(
    code: &str,
    file_name: &str,
) -> (Vec<TestUnit>, BTreeMap<String, String>, String, RawCompilerSettings) {
    parse_test_files_and_symlinks_with_options(code, file_name, ParseTestFilesOptions::default())
}

/// Go: `ParseTestFilesAndSymlinksWithOptions` — the `parseFile` closure is
/// the concrete `*testUnit` constructor (PORT-noted in the module header).
#[allow(clippy::type_complexity)]
pub fn parse_test_files_and_symlinks_with_options(
    code: &str,
    file_name: &str,
    options: ParseTestFilesOptions,
) -> (Vec<TestUnit>, BTreeMap<String, String>, String, RawCompilerSettings) {
    let mut test_units: Vec<TestUnit> = Vec::new();

    let lines = split_line_delimited(code);

    let mut current_file_content = String::new();
    let mut current_file_name = String::new();
    let mut seen_content_line = false;
    let mut has_seen_file = false;
    if options.allow_implicit_first_file {
        current_file_name = file_name.to_string();
    }
    let mut current_directory = String::new();
    let mut current_file_options: BTreeMap<String, String> = BTreeMap::new();
    let mut symlinks: BTreeMap<String, String> = BTreeMap::new();
    let mut global_options: RawCompilerSettings = BTreeMap::new();

    for line in lines {
        if parse_symlink_from_test(line, &mut symlinks) {
            continue;
        }
        if let Some((meta_data_name, meta_data_value)) = find_option_directive(line) {
            // Comment line, check for global/file @options and record them.
            let meta_data_name = meta_data_name.to_lowercase();
            let meta_data_value = meta_data_value.trim().to_string();
            if meta_data_name == "currentdirectory" {
                current_directory = meta_data_value.clone();
            }
            if meta_data_name != "filename" {
                if meta_data_name == "symlink" && !current_file_name.is_empty() {
                    for link in meta_data_value.split(',') {
                        let link = link.trim();
                        if !link.is_empty() {
                            symlinks.insert(link.to_string(), current_file_name.clone());
                        }
                    }
                } else if FOURSLASH_DIRECTIVES.contains(&meta_data_name.as_str()) {
                    // File-specific option
                    current_file_options.insert(meta_data_name, meta_data_value);
                } else {
                    // Global option (Go's duplicate-different-value panic
                    // is commented out there for baseline compatibility —
                    // the port keeps the silent overwrite).
                    global_options.insert(meta_data_name, meta_data_value);
                }
                continue;
            }

            // New @filename statement after having collected some code for
            // the previous metadata.
            if !current_file_name.is_empty() {
                // Always save for regular tests, but skip the empty
                // implicit first file for fourslash.
                let should_save_file =
                    !options.allow_implicit_first_file || !current_file_content.is_empty() || has_seen_file;
                if should_save_file {
                    has_seen_file = true;
                    test_units.push(TestUnit {
                        content: current_file_content.clone(),
                        name: current_file_name.clone(),
                    });
                }
                // Reset local data
                current_file_content.clear();
                seen_content_line = false;
                current_file_name = meta_data_value;
                current_file_options.clear();
            } else {
                // First metadata marker in the file.
                let has_content_before_first_filename = !current_file_content.is_empty()
                    && skip_trivia(&current_file_content, 0) as usize != current_file_content.len();
                if has_content_before_first_filename && !options.allow_implicit_first_file {
                    panic!("Non-comment test content appears before the first '// @Filename' directive");
                }
                // Save the implicit first file (fourslash).
                if has_content_before_first_filename
                    && options.allow_implicit_first_file
                    && !current_file_name.is_empty()
                {
                    has_seen_file = true;
                    test_units.push(TestUnit {
                        content: current_file_content.clone(),
                        name: current_file_name.clone(),
                    });
                }
                // Reset for the new file
                current_file_content.clear();
                seen_content_line = false;
                current_file_name = meta_data_value.trim().to_string();
                current_file_options.clear();
            }
        } else {
            // Subfile content line: append to the current subfile content,
            // inserting a newline if needed. Fourslash tests preserve
            // leading blank lines via seenContentLine; compiler tests drop
            // them (the Len() != 0 form — matching TS's harness behavior).
            if options.allow_implicit_first_file {
                if seen_content_line {
                    current_file_content.push('\n');
                }
                seen_content_line = true;
            } else if !current_file_content.is_empty() {
                current_file_content.push('\n');
            }
            current_file_content.push_str(line);
        }
    }

    // normalize the fileName for the single file case
    if test_units.is_empty() && current_file_name.is_empty() {
        current_file_name = get_base_file_name(file_name).into_owned();
    }

    // EOF, push whatever remains (Go's parseFile never errors for testUnit).
    test_units.push(TestUnit {
        content: current_file_content,
        name: current_file_name,
    });

    (test_units, symlinks, current_directory, global_options)
}

/// Go: `lineDelimiter.Split` (`\r?\n`).
fn split_line_delimited(text: &str) -> Vec<&str> {
    text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect()
}

/// Go: `optionRegex = ^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)` on one line: `//`,
/// then optional whitespace, `@`, a word, optional whitespace, `:`,
/// optional whitespace, then the rest of the line. Returns the name and
/// the raw value (not yet trimmed).
fn find_option_directive(line: &str) -> Option<(&str, &str)> {
    let after_slashes = line.strip_prefix("//")?;
    // \s* within the line (no newline can appear here)
    let after_ws = skip_inline_whitespace(after_slashes);
    let after_at = after_ws.strip_prefix('@')?;
    // \w+ = [0-9A-Za-z_]+
    let name_len = after_at
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .count();
    if name_len == 0 {
        return None;
    }
    let (name, rest) = after_at.split_at(name_len);
    let rest = skip_inline_whitespace(rest);
    let rest = rest.strip_prefix(':')?;
    let rest = skip_inline_whitespace(rest);
    // ([^\r\n]*) — the rest of the line
    Some((name, rest))
}

/// `\s*` confined to one line (Go's `\s` over a single-line slice; the
/// regex's `\s` class is `[\t\n\f\r ]`, and no newline can be present).
fn skip_inline_whitespace(line: &str) -> &str {
    line.trim_start_matches(['\t', '\u{b}', '\u{c}', '\r', ' '])
}

/// Go: `extractCompilerSettings(content)` — every option directive in the
/// file, value trimmed and `;`-suffixed stripped (used for the
/// configuration expansion; global options are also collected by the
/// parser walk above).
pub fn extract_compiler_settings(content: &str) -> RawCompilerSettings {
    let mut opts = RawCompilerSettings::new();
    for line in split_line_delimited(content) {
        if let Some((name, value)) = find_option_directive(line) {
            let value = value.trim().strip_suffix(';').unwrap_or(value.trim());
            opts.insert(name.to_lowercase(), value.to_string());
        }
    }
    opts
}

/// Go: `parseSymlinkFromTest` — `// @link: <original> -> <symlink>`
/// records symlinks[symlink] = original.
fn parse_symlink_from_test(line: &str, symlinks: &mut BTreeMap<String, String>) -> bool {
    let Some(matched) = find_link_directive(line) else {
        return false;
    };
    symlinks.insert(matched.1.trim().to_string(), matched.0.trim().to_string());
    true
}

/// Go: `linkRegex = ^\/{2}\s*@link\s*:\s*([^\r\n]*)\s*->\s*([^\r\n]*)` —
/// returns (original, symlink), untrimmed.
fn find_link_directive(line: &str) -> Option<(&str, &str)> {
    let after_slashes = line.strip_prefix("//")?;
    let after_ws = skip_inline_whitespace(after_slashes);
    let after_at = after_ws.strip_prefix('@')?;
    let rest = after_at.strip_prefix("link")?;
    // \s* : \s*
    let rest = skip_inline_whitespace(rest);
    let rest = rest.strip_prefix(':')?;
    let rest = skip_inline_whitespace(rest);
    // ([^\r\n]*) \s* -> \s* ([^\r\n]*)
    // The first capture is greedy up to the last "->" separator on the line.
    let sep = rest.rfind("->")?;
    let (original, after_sep) = (rest[..sep].trim_end(), rest[sep + 2..].trim_start());
    Some((original, after_sep))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go: `TestMakeUnitsFromTest`.
    #[test]
    fn test_make_units_from_test() {
        let code = "// @strict: true\n\
                    // @noEmit: true\n\
                    // @filename: firstFile.ts\n\
                    function foo() { return \"a\"; }\n\
                    // normal comment\n\
                    // @filename: secondFile.ts\n\
                    // some other comment\n\
                    function bar() { return \"b\"; }";
        let expected = TestCaseContent {
            test_unit_data: vec![
                TestUnit {
                    content: "function foo() { return \"a\"; }\n// normal comment".to_string(),
                    name: "firstFile.ts".to_string(),
                },
                TestUnit {
                    content: "// some other comment\nfunction bar() { return \"b\"; }".to_string(),
                    name: "secondFile.ts".to_string(),
                },
            ],
            ts_config_file_unit_data: None,
            symlinks: BTreeMap::new(),
        };
        assert_eq!(make_units_from_test(code, "simpleTest.ts"), expected);
    }

    #[test]
    fn single_file_case_normalizes_to_the_basename() {
        let (units, symlinks, current_dir, global) =
            parse_test_files_and_symlinks("let x = 1;\n", "/corpus/simpleTest.ts");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].name, "simpleTest.ts");
        // Go: the trailing newline yields a final empty line, and the join
        // rule writes the separator (content keeps the trailing "\n")
        assert_eq!(units[0].content, "let x = 1;\n");
        assert!(symlinks.is_empty());
        assert_eq!(current_dir, "");
        assert!(global.is_empty());
    }

    #[test]
    fn global_options_and_current_directory_are_recorded() {
        let code = "// @currentDirectory: /base\n// @target: esnext\n// @filename: a.ts\nlet a;";
        let (units, _symlinks, current_dir, global) =
            parse_test_files_and_symlinks(code, "/corpus/test.ts");
        assert_eq!(units.len(), 1);
        assert_eq!(current_dir, "/base");
        assert_eq!(global.get("target").map(String::as_str), Some("esnext"));
    }

    #[test]
    fn extract_compiler_settings_trims_and_strips_semicolons() {
        let settings = extract_compiler_settings("// @target: esnext;\n// @strict: TRUE\n");
        assert_eq!(settings.get("target").map(String::as_str), Some("esnext"));
        assert_eq!(settings.get("strict").map(String::as_str), Some("TRUE"));
    }

    #[test]
    fn link_directives_record_symlinks() {
        let code = "// @filename: original.ts\nlet a;\n// @link: original.ts -> linked.ts";
        let (units, symlinks, _, _) = parse_test_files_and_symlinks(code, "/corpus/test.ts");
        assert_eq!(units.len(), 1);
        assert_eq!(symlinks.get("linked.ts").map(String::as_str), Some("original.ts"));
    }

    #[test]
    fn symlink_directive_points_at_the_current_file() {
        let code = "// @filename: original.ts\nlet a;\n// @symlink: linked.ts\n// @filename: other.ts\nlet b;";
        let (units, symlinks, _, _) = parse_test_files_and_symlinks(code, "/corpus/test.ts");
        assert_eq!(units.len(), 2);
        assert_eq!(symlinks.get("linked.ts").map(String::as_str), Some("original.ts"));
    }

    #[test]
    fn non_comment_content_before_first_filename_panics_like_go() {
        let result = std::panic::catch_unwind(|| {
            let _ = parse_test_files_and_symlinks("let x = 1;\n// @filename: a.ts\nlet a;", "/corpus/t.ts");
        });
        assert!(result.is_err());
    }

    #[test]
    fn implicit_first_file_gate_keeps_leading_content() {
        let code = "leading content\n// @filename: a.ts\nlet a;";
        let (units, _, _, _) = parse_test_files_and_symlinks_with_options(
            code,
            "/corpus/implicit.ts",
            ParseTestFilesOptions { allow_implicit_first_file: true },
        );
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].name, "/corpus/implicit.ts");
        assert_eq!(units[0].content, "leading content");
        assert_eq!(units[1].name, "a.ts");
    }
}
