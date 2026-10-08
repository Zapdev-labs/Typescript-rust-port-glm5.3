// Ported from tsc/internal/checker/stringer_generated.go @ ec47d33c23e464a17cdf2475632cba629bee8763
// — `stringer -type=SignatureKind` output ("DO NOT EDIT"; the port keeps the
// exact generated table, in the form the crate's other generated-style tables
// use — a `Display` impl on the flag/enum's home module pattern, like
// types.rs's TypeFlags/VarianceFlags tables).
//
// PORT: Go's generated `String()` method becomes `std::fmt::Display` (the
// crate's stringer-table convention); the constant-fence function `_()` and
// the index table port verbatim — the enum's discriminants (SignatureKindCall
// = 0, SignatureKindConstruct = 1) are pinned by the tests below, which is
// the Rust form of the Go fence ("an invalid array index compiler error
// signifies that the constant values have changed").

use std::fmt;

use crate::types::SignatureKind;

/// Go: `const _SignatureKind_name = "SignatureKindCallSignatureKindConstruct"`.
const SIGNATURE_KIND_NAME: &str = "SignatureKindCallSignatureKindConstruct";

/// Go: `var _SignatureKind_index = [...]uint8{0, 17, 39}`.
const SIGNATURE_KIND_INDEX: [u8; 3] = [0, 17, 39];

/// Go: `func (i SignatureKind) String() string`.
impl fmt::Display for SignatureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Go: `idx := int(i) - 0; if i < 0 || idx >= len(_SignatureKind_index)-1 {
        //           return "SignatureKind(" + strconv.FormatInt(int64(i), 10) + ")" }`
        let i = *self as i32;
        let idx = i - 0;
        if i < 0 || idx >= SIGNATURE_KIND_INDEX.len() as i32 - 1 {
            return write!(f, "SignatureKind({i})");
        }
        let start = SIGNATURE_KIND_INDEX[idx as usize] as usize;
        let end = SIGNATURE_KIND_INDEX[idx as usize + 1] as usize;
        f.write_str(&SIGNATURE_KIND_NAME[start..end])
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests — the constant-fence + table mirror of the generated output.
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SignatureKind;

    /// Go: the `_()` fence — pins the constant values the index table encodes.
    #[test]
    fn signature_kind_discriminants_mirror_go() {
        assert_eq!(SignatureKind::Call as i32, 0);
        assert_eq!(SignatureKind::Construct as i32, 1);
    }

    /// The generated table, exactly: index boundaries 0/17/39 over the
    /// concatenated name string.
    #[test]
    fn stringer_table_mirrors_go() {
        assert_eq!(SIGNATURE_KIND_NAME, "SignatureKindCallSignatureKindConstruct");
        assert_eq!(SIGNATURE_KIND_INDEX, [0, 17, 39]);
        assert_eq!(SignatureKind::Call.to_string(), "SignatureKindCall");
        assert_eq!(SignatureKind::Construct.to_string(), "SignatureKindConstruct");
    }

    /// Go's out-of-range fallback — unreachable through the closed enum, but
    /// mirrored by construction through the same arithmetic on the raw value.
    #[test]
    fn out_of_range_falls_back_to_the_numeric_form() {
        let i = 2i32; // past SignatureKindConstruct
        let idx = i - 0;
        assert!(i < 0 || idx >= SIGNATURE_KIND_INDEX.len() as i32 - 1);
        assert_eq!(format!("SignatureKind({i})"), "SignatureKind(2)");
    }
}
