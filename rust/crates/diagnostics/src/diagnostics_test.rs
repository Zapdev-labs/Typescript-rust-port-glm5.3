// Ported from tsc/internal/diagnostics/diagnostics_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT(English-only): the non-English `TestLocalize` cases (de-DE, fr-FR,
// es-ES, ja-JP, zh-CN, ko-KR, ru-RU — both plain and with-args) and the
// loc-file machinery (`TestLocaleFiles`, `validateLocaleFile`,
// `TestLocaleFilesIgnoreStaleDiagnostics`, `TestGenerateLocalizations`,
// `validateLocalizedMessages`, `placeholderSet`) are not ported: this port
// ships no locale packs yet (see loc_generated.rs), so `localize` falls back
// to the English text for every locale. The English-path cases are ported
// verbatim below.

use std::fmt;

use super::*;

fn locale(tag: &str) -> tsc_locale::Locale {
    let (locale, ok) = tsc_locale::parse(tag);
    assert!(ok, "failed to parse locale {tag:?}");
    locale
}

/// Go passes `args []any`; the ported API takes `&[&dyn Display]`.
fn args<'a>(values: &'a [&'a str]) -> Vec<&'a dyn fmt::Display> {
    values
        .iter()
        .map(|value| value as &dyn fmt::Display)
        .collect()
}

/// Go: `TestLocalize` — English-path cases.
#[test]
fn test_localize() {
    struct Case {
        name: &'static str,
        message: &'static Message,
        locale: tsc_locale::Locale,
        args: Vec<&'static str>,
        expected: &'static str,
    }

    let tests = [
        Case {
            name: "english default",
            message: &IDENTIFIER_EXPECTED,
            locale: locale("en"),
            args: vec![],
            expected: "Identifier expected.",
        },
        Case {
            name: "undefined locale uses english",
            message: &IDENTIFIER_EXPECTED,
            locale: tsc_locale::DEFAULT,
            args: vec![],
            expected: "Identifier expected.",
        },
        Case {
            name: "with single argument",
            message: &X_0_EXPECTED,
            locale: locale("en"),
            args: vec![")"],
            expected: "')' expected.",
        },
        Case {
            name: "with multiple arguments",
            message: &THE_PARSER_EXPECTED_TO_FIND_A_1_TO_MATCH_THE_0_TOKEN_HERE,
            locale: locale("en"),
            args: vec!["{", "}"],
            expected: "The parser expected to find a '}' to match the '{' token here.",
        },
        Case {
            name: "fallback to english for unknown locale",
            message: &IDENTIFIER_EXPECTED,
            locale: locale("af-ZA"),
            args: vec![],
            expected: "Identifier expected.",
        },
    ];

    for tt in &tests {
        let result = tt.message.localize(&tt.locale, &args(&tt.args));
        assert_eq!(result, tt.expected, "case {:?}", tt.name);
    }
}

/// Go: `TestLocalize_ByKey`.
#[test]
fn test_localize_by_key() {
    struct Case {
        name: &'static str,
        key: Key,
        locale: tsc_locale::Locale,
        args: Vec<&'static str>,
        expected: &'static str,
    }

    let tests = [
        Case {
            name: "by key without args",
            key: "Identifier_expected_1003",
            locale: locale("en"),
            args: vec![],
            expected: "Identifier expected.",
        },
        Case {
            name: "by key with args",
            key: "_0_expected_1005",
            locale: locale("en"),
            args: vec![")"],
            expected: "')' expected.",
        },
    ];

    for tt in &tests {
        let result = localize(
            &tt.locale,
            None,
            tt.key,
            &tt.args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        assert_eq!(result, tt.expected, "case {:?}", tt.name);
    }
}

/// Generated-table parity: `ALL_MESSAGES` mirrors Go's `allMessages` —
/// every entry is present in the key lookup, codes are unique and in
/// ascending order, and the count matches diagnosticMessages.json.
#[test]
fn test_all_messages_table() {
    assert_eq!(ALL_MESSAGES.len(), 2222);
    let mut previous_code = 0;
    for message in ALL_MESSAGES {
        assert!(message.code() > previous_code, "codes must be ascending");
        previous_code = message.code();
        // Pointer identity, matching Go's `keyToMessage(key) == p` over the
        // static table.
        let found = key_to_message(message.key()).expect("key must resolve");
        assert!(std::ptr::eq(found, message));
    }
    // The hand-ported stub messages now come from the generated table.
    assert_eq!(CANNOT_FIND_NAME_0.key(), "Cannot_find_name_0_2304");
    assert_eq!(CANNOT_FIND_NAME_0.code(), 2304);
    assert_eq!(X_0_IS_DECLARED_HERE.key(), "_0_is_declared_here_2728");
    assert_eq!(X_0_IS_DECLARED_HERE.code(), 2728);
    assert_eq!(
        X_RESOLUTION_MODE_SHOULD_BE_EITHER_REQUIRE_OR_IMPORT.key(),
        "resolution_mode_should_be_either_require_or_import_1453"
    );
    assert_eq!(
        X_RESOLUTION_MODE_SHOULD_BE_EITHER_REQUIRE_OR_IMPORT.code(),
        1453
    );
}

/// Go: `Format`'s "Invalid formatting placeholder" panic path (exercised by
/// the placeholder machinery in Go's locale-file tests; ported here against
/// the English text).
#[test]
#[should_panic(expected = "Invalid formatting placeholder")]
fn test_format_out_of_range_placeholder_panics() {
    format("'{1}' expected.", &["only".to_string()]);
}

#[test]
#[should_panic(expected = "Invalid formatting placeholder")]
fn test_format_huge_index_panics() {
    // Go: strconv.ParseInt fails on overflow → panic.
    format("'{9999999999999}' expected.", &["x".to_string()]);
}

#[test]
fn test_format_non_placeholder_braces_pass_through() {
    assert_eq!(format("a { b", &["x".to_string()]), "a { b");
    assert_eq!(format("{}", &["x".to_string()]), "{}");
    assert_eq!(format("{ 0}", &["x".to_string()]), "{ 0}");
    assert_eq!(format("{a}", &["x".to_string()]), "{a}");
    assert_eq!(format("{{0}}", &["x".to_string()]), "{x}");
}

#[test]
fn test_format_without_args_returns_text() {
    assert_eq!(format("'{0}' expected.", &[]), "'{0}' expected.");
}

#[test]
fn test_new_adhoc_message() {
    let message = new_adhoc_message("custom ad-hoc failure".to_string());
    assert_eq!(message.code(), -1);
    assert_eq!(message.category(), Category::Error);
    assert_eq!(message.key(), "-1");
    assert_eq!(message.text(), "custom ad-hoc failure");
    assert!(!message.reports_unnecessary());
    assert!(!message.elided_in_compatibility_pyramid());
    assert!(!message.reports_deprecated());
}

#[test]
#[should_panic(expected = "Unknown diagnostic message: no_such_key_99999")]
fn test_localize_unknown_key_panics() {
    localize(&tsc_locale::DEFAULT, None, "no_such_key_99999", &[]);
}

#[test]
fn test_stringify_args() {
    assert!(stringify_args(&[]).is_empty());
    let value = 42;
    let stringified = stringify_args(&[&"str", &value, &true]);
    assert_eq!(
        stringified,
        ["str".to_string(), "42".to_string(), "true".to_string()]
    );
}

#[test]
fn test_category_name() {
    assert_eq!(Category::Warning.name(), "warning");
    assert_eq!(Category::Error.name(), "error");
    assert_eq!(Category::Suggestion.name(), "suggestion");
    assert_eq!(Category::Message.name(), "message");
}
