// Ported from tsc/internal/diagnostics/loc_generated.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT(English-only): Go embeds a gzipped message pack per non-English locale
// (`loc/*.json.gz` + `loadLocaleData`) and matches locales with
// `golang.org/x/text/language.Matcher`. This port ships no locale packs yet,
// so every pack slot is `None` and `get_localized_messages` always returns
// `None` — callers fall back to the English text baked into
// `messages_generated.rs`, exactly as Go does for `language.Und` and for
// English itself (Go's `localeFuncs[0]` is nil). The indirection below keeps
// the matcher/pack shape so a later wave can hang real packs off
// `locale_messages` without touching diagnostics.go code.

use std::collections::HashMap;

use crate::Key;
use tsc_locale::{DEFAULT, Locale};

/// Go: `type map[Key]string` — the localized-text table for one pack.
pub(crate) type LocalizedMessages = HashMap<Key, String>;

/// Go: `var matcher = language.NewMatcher([]language.Tag{...})` — the
/// supported locale tags, in matcher order (English first, as the default).
pub(crate) const SUPPORTED_LOCALES: &[&str] = &[
    "en", "zh-CN", "zh-TW", "cs-CZ", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "ko-KR", "pl-PL",
    "pt-BR", "ru-RU", "tr-TR",
];

/// Go: `var localeFuncs = []func() map[Key]string{ nil /* English */, zhCN, ... }`
/// — index into `SUPPORTED_LOCALES`; `None` is Go's nil entry (no pack
/// loaded). English is `nil` because its text lives in the generated
/// message table itself.
pub(crate) fn locale_messages(index: usize) -> Option<LocalizedMessages> {
    // PORT(English-only): no packs shipped yet. When one lands, unmarshal
    // the pack data here (Go: `loadLocaleData` over the embedded gzip).
    let _ = index;
    None
}

/// Go: `func getLocalizedMessages(loc language.Tag) map[Key]string` — nil
/// for `language.Und`, English, and any locale with no matching pack.
pub(crate) fn get_localized_messages(loc: &Locale) -> Option<LocalizedMessages> {
    if *loc == DEFAULT {
        return None;
    }

    // Go: `_, index, confidence := matcher.Match(loc)` with a
    // `confidence >= language.Low` guard. The matcher prefers an exact tag,
    // then a same-language fallback, then the default (English, whose pack
    // is nil). Prefix matching on the tag covers the supported set.
    let tag = loc.to_string();
    let index = SUPPORTED_LOCALES
        .iter()
        .position(|candidate| tag_matches(candidate, &tag))?;
    locale_messages(index)
}

fn tag_matches(candidate: &str, tag: &str) -> bool {
    let candidate = candidate.to_ascii_lowercase();
    let tag = tag.to_ascii_lowercase();
    tag == candidate
        || tag.starts_with(&format!("{candidate}-"))
        || candidate.starts_with(&format!("{tag}-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsc_locale::parse;

    #[test]
    fn test_get_localized_messages() {
        // `language.Und` → nil.
        assert!(get_localized_messages(&DEFAULT).is_none());
        // English → nil (text lives in the generated table).
        let (en, ok) = parse("en");
        assert!(ok);
        assert!(get_localized_messages(&en).is_none());
        // Known non-English tags resolve to a pack slot; every slot is
        // `None` in the English-only port.
        let (de, ok) = parse("de-DE");
        assert!(ok);
        assert!(get_localized_messages(&de).is_none());
        let (de_variant, ok) = parse("de-AT");
        assert!(ok);
        assert!(get_localized_messages(&de_variant).is_none());
        // Unknown locales fall back to the default (nil) pack.
        let (af, ok) = parse("af-ZA");
        assert!(ok);
        assert!(get_localized_messages(&af).is_none());
    }
}
