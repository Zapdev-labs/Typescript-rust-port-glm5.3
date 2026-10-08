// gen-diagnostics: diagnostic message table generator for tsc-diagnostics.
// Reads tsc/internal/diagnostics/diagnosticMessages.json (the single source
// of truth — SPEC §5.7/§5.8/§6; do not fork the JSON) and emits
// rust/crates/diagnostics/src/messages_generated.rs, mirroring Go's
// tsc/internal/diagnostics/diagnostics_generated.go.
//
// Deterministic and idempotent: the output is a pure function of the JSON
// input (messages are emitted in ascending code order, matching Go's
// `slices.SortFunc` over unique codes), so re-running with an unchanged input
// produces byte-identical output.
//
// Var naming follows Go's `convertPropertyName` (generate.go) exactly, with
// the Rust-side transform being a single `.to_uppercase()` — the pattern
// established by the hand-ported stub messages in
// crates/diagnostics/src/lib.rs (e.g. Go `Cannot_find_name_0` → Rust
// `CANNOT_FIND_NAME_0`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Go: `diagnosticMessage` in generate.go. Field names match the JSON; note
/// `elidedInCompatabilityPyramid` — the spelling error is [sic] in Strada.
#[derive(serde::Deserialize)]
struct RawMessage {
    category: String,
    code: i32,
    #[serde(default, rename = "reportsUnnecessary")]
    reports_unnecessary: bool,
    #[serde(default, rename = "reportsDeprecated")]
    reports_deprecated: bool,
    #[serde(default, rename = "elidedInCompatabilityPyramid")]
    elided_in_compatibility_pyramid: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("gen-diagnostics: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let input = manifest_dir
        .join("../../..")
        .join("tsc/internal/diagnostics/diagnosticMessages.json");
    let output = manifest_dir
        .join("../..")
        .join("crates/diagnostics/src/messages_generated.rs");

    let messages = read_raw_messages(&input)?;
    let output_text = generate(&messages)?;

    // Go: `format.Source` over the generated buffer — run rustfmt over the
    // output so the emitted file is `cargo fmt`-stable (and stays
    // byte-identical on regeneration). The temp file lives next to the
    // target so rustfmt resolves the same config `cargo fmt` would (from
    // the target path's ancestors).
    let temp = output.with_extension("rs.tmp");
    std::fs::write(&temp, &output_text)
        .map_err(|e| format!("failed to write {}: {e}", temp.display()))?;
    let formatted = match std::process::Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .arg(&temp)
        .output()
    {
        Ok(result) if result.status.success() => {
            let formatted = std::fs::read_to_string(&temp).map_err(|e| {
                format!("failed to read {}: {e}", temp.display())
            })?;
            let _ = std::fs::remove_file(&temp);
            formatted
        }
        Ok(result) => {
            let _ = std::fs::remove_file(&temp);
            eprintln!(
                "gen-diagnostics: rustfmt failed, keeping unformatted output:\n{}",
                String::from_utf8_lossy(&result.stderr)
            );
            output_text
        }
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            eprintln!("gen-diagnostics: rustfmt unavailable ({err}), keeping unformatted output");
            output_text
        }
    };

    // Idempotent write: only touch the file when the content changed.
    let existing = std::fs::read_to_string(&output).ok();
    let changed = existing.as_deref() != Some(formatted.as_str());
    if changed {
        std::fs::write(&output, &formatted)
            .map_err(|e| format!("failed to write {}: {e}", output.display()))?;
    }

    println!(
        "gen-diagnostics: {} messages -> {} ({} bytes{})",
        messages.len(),
        output.display(),
        formatted.len(),
        if changed { "" } else { ", unchanged" }
    );
    Ok(())
}

/// Go: `readRawMessages` — decode the JSON map (text → message), reject
/// duplicate codes, and return the messages sorted by ascending code.
/// (Go sorts with an unstable sort, but codes are unique, so the order is
/// total.)
fn read_raw_messages(path: &Path) -> Result<Vec<(String, RawMessage)>, String> {
    let file =
        std::fs::read(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let raw: HashMap<String, RawMessage> = serde_json::from_slice(&file)
        .map_err(|e| format!("failed to decode {}: {e}", path.display()))?;

    let mut messages: Vec<(String, RawMessage)> = raw.into_iter().collect();
    let mut seen_codes: HashMap<i32, &str> = HashMap::with_capacity(messages.len());
    for (text, message) in &messages {
        if let Some(existing) = seen_codes.get(&message.code) {
            return Err(format!(
                "diagnostics {existing:?} and {text:?} both use code {}",
                message.code
            ));
        }
        seen_codes.insert(message.code, text.as_str());
    }
    messages.sort_by_key(|(_, message)| message.code);
    Ok(messages)
}

/// Go: `generateDiagnostics` — emit one `Message` static per diagnostic and
/// the `ALL_MESSAGES` table, mirroring `diagnostics_generated.go`.
fn generate(messages: &[(String, RawMessage)]) -> Result<String, String> {
    let mut out = String::with_capacity(messages.len() * 220);
    out.push_str("// @generated by rust/tools/gen-diagnostics from tsc/internal/diagnostics/diagnosticMessages.json — DO NOT EDIT.\n");
    out.push_str("//\n");
    out.push_str("// Mirrors tsc/internal/diagnostics/diagnostics_generated.go @ ec47d33c23e464a17cdf2475632cba629bee8763.\n");
    out.push_str("// Static names are the uppercased Go identifiers (convertPropertyName in\n");
    out.push_str(
        "// tsc/internal/diagnostics/generate.go); codes, keys, and texts are verbatim.\n\n",
    );
    out.push_str("use crate::{Category, Message};\n\n");

    let mut used_names: HashMap<String, ()> = HashMap::with_capacity(messages.len());
    let mut used_keys: HashMap<String, ()> = HashMap::with_capacity(messages.len());

    for (text, message) in messages {
        let (go_var_name, key) = convert_property_name(text, message.code);
        let name = go_var_name.to_uppercase();
        if used_names.insert(name.clone(), ()).is_some() {
            return Err(format!(
                "diagnostic static name collision after uppercasing: {name:?} (text {text:?})"
            ));
        }
        if used_keys.insert(key.clone(), ()).is_some() {
            return Err(format!("diagnostic key collision: {key:?} (text {text:?})"));
        }

        let category = match message.category.as_str() {
            "Warning" => "Category::Warning",
            "Error" => "Category::Error",
            "Suggestion" => "Category::Suggestion",
            "Message" => "Category::Message",
            other => {
                return Err(format!(
                    "unknown category {other:?} for code {}",
                    message.code
                ));
            }
        };

        // Emitted in one canonical flat layout; the rustfmt pass at the end
        // produces the final, `cargo fmt`-stable formatting.
        writeln!(out, "pub static {name}: Message = Message {{").unwrap();
        writeln!(
            out,
            "    code: {},\n    category: {category},\n    key: \"{}\",\n    text: \"{}\",\n    reports_unnecessary: {},\n    elided_in_compatibility_pyramid: {},\n    reports_deprecated: {},\n}};\n",
            message.code,
            escape_rust_string(&key),
            escape_rust_string(text),
            message.reports_unnecessary,
            message.elided_in_compatibility_pyramid,
            message.reports_deprecated,
        )
        .unwrap();
    }

    // Go: `var allMessages = [...]**Message{...}` — a static table of
    // pointers to the statics above.
    writeln!(
        out,
        "pub static ALL_MESSAGES: [&Message; {}] = [",
        messages.len()
    )
    .unwrap();
    for (text, message) in messages {
        let (go_var_name, _) = convert_property_name(text, message.code);
        writeln!(out, "    &{},", go_var_name.to_uppercase()).unwrap();
    }
    out.push_str("];\n");
    Ok(out)
}

/// Go: `convertPropertyName(origName string, code int) (varName, key string)`
/// in tsc/internal/diagnostics/generate.go, ported branch-for-branch. The
/// returned var name is the *Go* identifier (the caller uppercases it for
/// the Rust static).
fn convert_property_name(orig_name: &str, code: i32) -> (String, String) {
    let mut var_name = String::with_capacity(orig_name.len());
    for r in orig_name.chars() {
        match r {
            '*' => var_name.push_str("_Asterisk"),
            '/' => var_name.push_str("_Slash"),
            ':' => var_name.push_str("_Colon"),
            // PORT: Go uses `unicode.IsLetter`/`unicode.IsDigit`; Rust's
            // `is_alphabetic`/`is_numeric` differ only on non-ASCII edge
            // categories (Nl, Other_Alphabetic, No). diagnosticMessages.json
            // texts are entirely ASCII, so the two agree on this input.
            r if r.is_alphabetic() || r.is_numeric() => var_name.push(r),
            _ => var_name.push('_'),
        }
    }

    // Go: `multipleUnderscoreRegexp = _+` — collapse runs of underscores.
    var_name = collapse_underscores(&var_name);

    // Go: `leadingUnderscoreUnlessDigitRegexp = ^_+(\D)` → "$1".
    var_name = strip_leading_underscores(&var_name);

    // Go: `trailingUnderscoreRegexp = _$` — remove a trailing underscore.
    if var_name.ends_with('_') {
        var_name.pop();
    }

    // Go: key = varName truncated to 100 bytes + "_" + code.
    let mut key = var_name.clone();
    if key.len() > 100 {
        // Byte slicing, same as Go (`key[:100]`).
        key.truncate(100);
    }
    key.push('_');
    write!(key, "{code}").unwrap();

    // Go: `if !token.IsExported(varName)` — prefix so the identifier is
    // exported (uppercase initial): "X" when it starts with "_", else "X_".
    let is_exported = var_name.chars().next().is_some_and(|r| r.is_uppercase());
    if !is_exported {
        if var_name.starts_with('_') {
            var_name.insert(0, 'X');
        } else {
            var_name.insert_str(0, "X_");
        }
    }

    if var_name.is_empty() {
        // Go: log.Fatalf("failed to convert property name to exported identifier")
        panic!("failed to convert property name to exported identifier: {orig_name:?}");
    }

    (var_name, key)
}

fn collapse_underscores(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_underscore = false;
    for c in s.chars() {
        if c == '_' {
            if !prev_underscore {
                out.push(c);
            }
            prev_underscore = true;
        } else {
            out.push(c);
            prev_underscore = false;
        }
    }
    out
}

/// Port of `^_+(\D)` → "$1" (with the regex engine's backtracking made
/// explicit): for a string of `k` leading underscores followed by `rest`:
/// - `rest` starts with a non-digit → all `k` underscores are removed
///   (`_+` matches the full run, `(\D)` captures `rest`'s first char);
/// - `rest` starts with a digit and `k >= 2` → the first `k-1` underscores
///   match `_+` and the last underscore is the `(\D)` capture, so the run
///   collapses to a single underscore;
/// - `rest` starts with a digit and `k <= 1`, or `rest` is empty → no match.
fn strip_leading_underscores(s: &str) -> String {
    let leading = s.chars().take_while(|&c| c == '_').count();
    if leading == 0 {
        return s.to_string();
    }
    let rest = &s[leading..];
    match rest.chars().next() {
        Some(c) if c.is_numeric() => {
            if leading >= 2 {
                format!("_{rest}")
            } else {
                s.to_string()
            }
        }
        Some(_) => rest.to_string(),
        None => s.to_string(),
    }
}

/// Escape `s` for a Rust string literal.
fn escape_rust_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                write!(out, "\\u{{{:x}}}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_property_name() {
        // Plain text: identifier is the text with non-alphanumerics as
        // underscores, plus the code in the key.
        assert_eq!(
            convert_property_name("Unterminated string literal.", 1002),
            (
                "Unterminated_string_literal".to_string(),
                "Unterminated_string_literal_1002".to_string()
            )
        );
        // Leading underscore kept when followed by a digit, X-prefixed to
        // make it exported.
        assert_eq!(
            convert_property_name("'{0}' expected.", 1005),
            ("X_0_expected".to_string(), "_0_expected_1005".to_string())
        );
        // Leading underscore before a non-digit is stripped; lowercase
        // start still needs the X_ prefix.
        assert_eq!(
            convert_property_name(
                "'super' must be followed by an argument list or member access.",
                1034
            ),
            (
                "X_super_must_be_followed_by_an_argument_list_or_member_access".to_string(),
                "super_must_be_followed_by_an_argument_list_or_member_access_1034".to_string()
            )
        );
        // Uppercase start is already exported — no prefix.
        assert_eq!(
            convert_property_name("Cannot find name '{0}'.", 2304),
            (
                "Cannot_find_name_0".to_string(),
                "Cannot_find_name_0_2304".to_string()
            )
        );
        // Special characters get named escapes; surrounding punctuation
        // collapses into the underscores around them and the trailing
        // underscore is stripped.
        assert_eq!(
            convert_property_name("'{*}'.", 1),
            ("Asterisk".to_string(), "Asterisk_1".to_string())
        );
    }

    #[test]
    fn test_strip_leading_underscores() {
        assert_eq!(strip_leading_underscores("__foo"), "foo");
        assert_eq!(strip_leading_underscores("_0foo"), "_0foo");
        assert_eq!(strip_leading_underscores("__0foo"), "_0foo");
        assert_eq!(strip_leading_underscores("foo"), "foo");
        assert_eq!(strip_leading_underscores("___"), "___");
    }

    #[test]
    fn test_key_truncates_at_100_bytes() {
        let long = "A".repeat(120);
        let (var_name, key) = convert_property_name(&long, 42);
        assert_eq!(var_name, long);
        let code = 42;
        assert_eq!(key, format!("{}_{code}", "A".repeat(100)));
    }
}
