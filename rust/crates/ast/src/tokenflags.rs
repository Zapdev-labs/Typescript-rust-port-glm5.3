// Ported from tsc/internal/ast/tokenflags.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Exact bit values from Go. PORT: Go declares `type TokenFlags int32` (signed,
// unlike the other flag sets); the port keeps i32 so the bit patterns match.

define_flags!(TokenFlags, i32);

impl TokenFlags {
    pub const PRECEDING_LINE_BREAK: TokenFlags = TokenFlags(1 << 0);
    pub const PRECEDING_JSDOC_COMMENT: TokenFlags = TokenFlags(1 << 1);
    pub const UNTERMINATED: TokenFlags = TokenFlags(1 << 2);
    /// e.g. `\u{10ffff}`
    pub const EXTENDED_UNICODE_ESCAPE: TokenFlags = TokenFlags(1 << 3);
    /// e.g. `10e2`
    pub const SCIENTIFIC: TokenFlags = TokenFlags(1 << 4);
    /// e.g. `0777`
    pub const OCTAL: TokenFlags = TokenFlags(1 << 5);
    /// e.g. `0x00000000`
    pub const HEX_SPECIFIER: TokenFlags = TokenFlags(1 << 6);
    /// e.g. `0b0110010000000000`
    pub const BINARY_SPECIFIER: TokenFlags = TokenFlags(1 << 7);
    /// e.g. `0o777`
    pub const OCTAL_SPECIFIER: TokenFlags = TokenFlags(1 << 8);
    /// e.g. `0b1100_0101`
    pub const CONTAINS_SEPARATOR: TokenFlags = TokenFlags(1 << 9);
    /// e.g. `\u00a0`
    pub const UNICODE_ESCAPE: TokenFlags = TokenFlags(1 << 10);
    /// e.g. `\uhello`
    pub const CONTAINS_INVALID_ESCAPE: TokenFlags = TokenFlags(1 << 11);
    /// e.g. `\xa0`
    pub const HEX_ESCAPE: TokenFlags = TokenFlags(1 << 12);
    /// e.g. `0888`
    pub const CONTAINS_LEADING_ZERO: TokenFlags = TokenFlags(1 << 13);
    /// e.g. `0_1`
    pub const CONTAINS_INVALID_SEPARATOR: TokenFlags = TokenFlags(1 << 14);
    pub const PRECEDING_JSDOC_LEADING_ASTERISKS: TokenFlags = TokenFlags(1 << 15);
    /// e.g. `'abc'`
    pub const SINGLE_QUOTE: TokenFlags = TokenFlags(1 << 16);
    /// Preceding JSDoc comment contains @deprecated
    pub const PRECEDING_JSDOC_WITH_DEPRECATED: TokenFlags = TokenFlags(1 << 17);
    /// Preceding JSDoc comment contains @see or @link
    pub const PRECEDING_JSDOC_WITH_SEE_OR_LINK: TokenFlags = TokenFlags(1 << 18);
    pub const BINARY_OR_OCTAL_SPECIFIER: TokenFlags =
        TokenFlags(Self::BINARY_SPECIFIER.0 | Self::OCTAL_SPECIFIER.0);
    pub const WITH_SPECIFIER: TokenFlags = TokenFlags(Self::HEX_SPECIFIER.0 | Self::BINARY_OR_OCTAL_SPECIFIER.0);
    pub const STRING_LITERAL_FLAGS: TokenFlags = TokenFlags(
        Self::UNTERMINATED.0
            | Self::HEX_ESCAPE.0
            | Self::UNICODE_ESCAPE.0
            | Self::EXTENDED_UNICODE_ESCAPE.0
            | Self::CONTAINS_INVALID_ESCAPE.0
            | Self::SINGLE_QUOTE.0,
    );
    pub const NUMERIC_LITERAL_FLAGS: TokenFlags = TokenFlags(
        Self::SCIENTIFIC.0
            | Self::OCTAL.0
            | Self::CONTAINS_LEADING_ZERO.0
            | Self::WITH_SPECIFIER.0
            | Self::CONTAINS_SEPARATOR.0
            | Self::CONTAINS_INVALID_SEPARATOR.0,
    );
    pub const TEMPLATE_LITERAL_LIKE_FLAGS: TokenFlags = TokenFlags(
        Self::UNTERMINATED.0
            | Self::HEX_ESCAPE.0
            | Self::UNICODE_ESCAPE.0
            | Self::EXTENDED_UNICODE_ESCAPE.0
            | Self::CONTAINS_INVALID_ESCAPE.0,
    );
    pub const REGULAR_EXPRESSION_LITERAL_FLAGS: TokenFlags = Self::UNTERMINATED;
    pub const IS_INVALID: TokenFlags = TokenFlags(
        Self::OCTAL.0
            | Self::CONTAINS_LEADING_ZERO.0
            | Self::CONTAINS_INVALID_SEPARATOR.0
            | Self::CONTAINS_INVALID_ESCAPE.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_flags_bit_values_mirror_go() {
        assert_eq!(TokenFlags::PRECEDING_LINE_BREAK, TokenFlags(1 << 0));
        assert_eq!(TokenFlags::UNTERMINATED, TokenFlags(1 << 2));
        assert_eq!(TokenFlags::UNICODE_ESCAPE, TokenFlags(1 << 10));
        assert_eq!(TokenFlags::CONTAINS_INVALID_ESCAPE, TokenFlags(1 << 11));
        assert_eq!(TokenFlags::SINGLE_QUOTE, TokenFlags(1 << 16));
        assert_eq!(TokenFlags::PRECEDING_JSDOC_WITH_DEPRECATED, TokenFlags(1 << 17));
        assert_eq!(TokenFlags::PRECEDING_JSDOC_WITH_SEE_OR_LINK, TokenFlags(1 << 18));
    }

    #[test]
    fn token_flag_composites_mirror_go() {
        assert_eq!(
            TokenFlags::WITH_SPECIFIER,
            TokenFlags::HEX_SPECIFIER | TokenFlags::BINARY_SPECIFIER | TokenFlags::OCTAL_SPECIFIER
        );
        assert!(TokenFlags::STRING_LITERAL_FLAGS.intersects(TokenFlags::SINGLE_QUOTE));
        assert!(!TokenFlags::STRING_LITERAL_FLAGS.intersects(TokenFlags::HEX_SPECIFIER));
        assert_eq!(TokenFlags::REGULAR_EXPRESSION_LITERAL_FLAGS, TokenFlags::UNTERMINATED);
        assert!(TokenFlags::IS_INVALID.intersects(TokenFlags::OCTAL));
        assert!(TokenFlags::IS_INVALID.intersects(TokenFlags::CONTAINS_INVALID_ESCAPE));
        assert!(!TokenFlags::IS_INVALID.intersects(TokenFlags::SCIENTIFIC));
    }
}
