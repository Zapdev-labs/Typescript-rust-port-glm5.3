// Ported from tsc/internal/diagnostics/diagnostics.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Ported from tsc/internal/diagnostics/doc.go:
//
//! Package diagnostics contains generated localizable diagnostic messages.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

mod loc_generated;
mod messages_generated;

pub use messages_generated::*;

pub type Key = &'static str;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Warning,
    Error,
    Suggestion,
    Message,
}

impl Category {
    /// Go: `func (category Category) Name() string` (the trailing
    /// `panic("Unhandled diagnostic category")` is unreachable against this
    /// exhaustive enum).
    pub fn name(self) -> &'static str {
        match self {
            Category::Warning => "warning",
            Category::Error => "error",
            Category::Suggestion => "suggestion",
            Category::Message => "message",
        }
    }
}

#[derive(Debug)]
pub struct Message {
    code: i32,
    category: Category,
    key: Key,
    text: &'static str,
    reports_unnecessary: bool,
    elided_in_compatibility_pyramid: bool,
    reports_deprecated: bool,
}

impl Message {
    pub fn code(&self) -> i32 {
        self.code
    }
    pub fn category(&self) -> Category {
        self.category
    }
    pub fn key(&self) -> Key {
        self.key
    }
    pub fn reports_unnecessary(&self) -> bool {
        self.reports_unnecessary
    }
    pub fn elided_in_compatibility_pyramid(&self) -> bool {
        self.elided_in_compatibility_pyramid
    }
    pub fn reports_deprecated(&self) -> bool {
        self.reports_deprecated
    }

    /// For debugging only.
    ///
    /// Go: `func (m *Message) String() string` — `text` (kept under the
    /// stub's `text` accessor name, which dependent crates already use).
    pub fn text(&self) -> &'static str {
        self.text
    }

    /// Go: `func (m *Message) Localize(locale locale.Locale, args ...any) string`.
    pub fn localize(&self, locale: &tsc_locale::Locale, args: &[&dyn fmt::Display]) -> String {
        localize(locale, Some(self), "", &stringify_args(args))
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

// Most diagnostics carry a message pointer, so only build the lookup when a key is used.
//
// Go: `var messagesByKey = sync.OnceValue(func() map[Key]*Message {...})`.
fn messages_by_key() -> &'static HashMap<Key, &'static Message> {
    static MESSAGES_BY_KEY: OnceLock<HashMap<Key, &'static Message>> = OnceLock::new();
    MESSAGES_BY_KEY.get_or_init(|| {
        let mut messages = HashMap::with_capacity(ALL_MESSAGES.len());
        for message in ALL_MESSAGES {
            messages.insert(message.key(), message);
        }
        messages
    })
}

/// Go: `func keyToMessage(key Key) *Message` — `None` where Go returns nil.
pub fn key_to_message(key: Key) -> Option<&'static Message> {
    messages_by_key().get(key).copied()
}

/// Go: `func Localize(locale locale.Locale, message *Message, key Key, args ...string) string`.
pub fn localize(
    locale: &tsc_locale::Locale,
    message: Option<&Message>,
    key: Key,
    args: &[String],
) -> String {
    let message = match message {
        Some(message) => message,
        None => key_to_message(key).unwrap_or_else(|| panic!("Unknown diagnostic message: {key}")),
    };

    // Go: `if localized, ok := getLocalizedMessages(...)[message.key]; ok { text = localized }`.
    let localized_messages = loc_generated::get_localized_messages(locale);
    if let Some(localized) = localized_messages
        .as_ref()
        .and_then(|messages| messages.get(message.key()))
    {
        return format(localized, args);
    }
    format(message.text(), args)
}

/// Go: `func Format(text string, args []string) string` — replaces `{N}`
/// placeholders (the `placeholderRegexp` `{(\d+)}`) with the Nth argument,
/// panicking with "Invalid formatting placeholder" on an out-of-range index.
///
/// PORT: Go first runs `strings.ToValidUTF8(arg, "\uFFFD")` over the args;
/// Rust `String`s are always valid UTF-8, so that pass is a no-op here.
pub fn format(text: &str, args: &[String]) -> String {
    if args.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len() + args.len() * 4);
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        // Go's regex only matches `{` + ASCII digits + `}`; anything else
        // (a non-digit, an empty pair, a missing `}`) passes through with
        // the `{` intact.
        if let Some(close) = after.find('}') {
            let inner = &after[..close];
            if !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()) {
                let index: usize = inner.parse().unwrap_or(usize::MAX);
                if index >= args.len() {
                    panic!("Invalid formatting placeholder");
                }
                out.push_str(&args[index]);
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Go: `func StringifyArgs(args []any) []string` — string args pass through,
/// everything else is formatted with `%v` (Rust `Display` is the `%v`
/// equivalent). Returns `Vec::new()` where Go returns nil.
pub fn stringify_args(args: &[&dyn fmt::Display]) -> Vec<String> {
    if args.is_empty() {
        return Vec::new();
    }
    args.iter().map(|arg| arg.to_string()).collect()
}

/// Go: `func NewAdHocMessage(message string) *Message` — an error-category
/// message carrying arbitrary runtime text.
///
/// PORT: Go returns a heap-allocated `*Message` that escapes; the Rust
/// `Message` is a static-table type (`text: &'static str`), so the text (and
/// the message) are leaked to give the result the same `'static` lifetime as
/// table entries. Call sites are rare, so the leak is bounded.
pub fn new_adhoc_message(message: String) -> &'static Message {
    Box::leak(Box::new(Message {
        code: -1,
        category: Category::Error,
        key: "-1",
        text: Box::leak(message.into_boxed_str()),
        reports_unnecessary: false,
        elided_in_compatibility_pyramid: false,
        reports_deprecated: false,
    }))
}

#[cfg(test)]
mod diagnostics_test;
