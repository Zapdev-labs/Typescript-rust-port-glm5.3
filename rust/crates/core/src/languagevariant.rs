// Ported from tsc/internal/core/languagevariant.go @ ec47d33c23e464a17cdf2475632cba629bee8763

// go:generate npx hereby generate:languagevariant

/// LanguageVariant mirrors Go's `type LanguageVariant int32`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum LanguageVariant {
    #[default]
    Standard = 0,
    JSX = 1,
}

impl LanguageVariant {
    pub fn from_i32(value: i32) -> Option<LanguageVariant> {
        match value {
            0 => Some(LanguageVariant::Standard),
            1 => Some(LanguageVariant::JSX),
            _ => None,
        }
    }
}
