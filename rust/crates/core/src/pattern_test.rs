// Ported from tsc/internal/core/pattern_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go's TestPatternOverlappingMatch is t.Errorf-assertion based; the port
// keeps the same assertions one-to-one.

use crate::pattern::try_parse_pattern;

#[test]
fn test_pattern_overlapping_match() {
    let p = try_parse_pattern("ab*ab");
    assert!(
        !p.matches("ab"),
        "expected 'ab' not to match 'ab*ab'"
    );
    assert!(
        p.matches("abXab"),
        "expected 'abXab' to match 'ab*ab'"
    );
    assert_eq!(p.matched_text("abXab"), "X");
    assert!(
        p.matches("abab"),
        "expected 'abab' to match 'ab*ab'"
    );
    assert_eq!(p.matched_text("abab"), "");
}
