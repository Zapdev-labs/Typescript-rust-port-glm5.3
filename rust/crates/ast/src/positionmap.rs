// Ported from tsc/internal/ast/positionmap.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PositionMap provides bidirectional mapping between UTF-8 byte offsets
// (used by the Go port and this Rust port) and UTF-16 code unit offsets
// (used by JavaScript/TypeScript).
//
// For ASCII-only text, the two are identical. For text containing non-ASCII
// characters, the offsets diverge because multi-byte UTF-8 sequences map to
// different numbers of UTF-16 code units:
//   - U+0000..U+007F:   1 byte  in UTF-8, 1 code unit  in UTF-16
//   - U+0080..U+07FF:   2 bytes in UTF-8, 1 code unit  in UTF-16
//   - U+0800..U+FFFF:   3 bytes in UTF-8, 1 code unit  in UTF-16
//   - U+10000..U+10FFFF: 4 bytes in UTF-8, 2 code units in UTF-16 (surrogate pair)

/// Go: `type PositionMap struct` — the entries are private there; the port
/// keeps them private too (`ascii_only`, `entries`).
#[derive(Debug, Default)]
pub struct PositionMap {
    /// True if the text contains only ASCII characters, meaning UTF-8 byte
    /// offsets and UTF-16 code unit offsets are identical.
    ascii_only: bool,
    /// For each multi-byte character: the UTF-8 byte offset AFTER the
    /// character and the cumulative delta (utf8 - utf16) at that character,
    /// enabling O(log n) conversion in either direction.
    entries: Vec<PositionMapEntry>,
}

#[derive(Clone, Copy, Debug)]
struct PositionMapEntry {
    /// UTF-8 byte offset AFTER this multi-byte character.
    utf8_pos: i32,
    /// Cumulative (utf8 - utf16) offset difference after this character.
    delta: i32,
}

/// Go: `func ComputePositionMap(text string) *PositionMap`.
///
/// PORT: takes `&[u8]` rather than `&str` — Go strings are byte-indexed and
/// the scanner's lone-surrogate sentinel encoding (stringutil.EncodeJSStringRune,
/// used by TestPositionMapLoneSurrogateSentinel) is invalid UTF-8 that a Rust
/// `&str` cannot hold. Callers pass `text.as_bytes()`.
pub fn compute_position_map(text: &[u8]) -> PositionMap {
    let mut pm = PositionMap {
        ascii_only: true,
        entries: Vec::new(),
    };
    let mut delta: i32 = 0;
    let mut i = 0usize;
    while i < text.len() {
        let b = text[i];
        if b < 0x80 {
            i += 1;
            continue;
        }
        let (r, size) = tsc_stringutil::decode_js_string_rune(&text[i..]);
        let utf16_size: i32 = if r >= 0x10000 { 2 } else { 1 };
        delta += size as i32 - utf16_size;
        pm.entries.push(PositionMapEntry {
            utf8_pos: (i + size) as i32,
            delta,
        });
        i += size;
    }
    pm.ascii_only = pm.entries.is_empty();
    pm
}

impl PositionMap {
    /// Go: `func (pm *PositionMap) IsAsciiOnly() bool`.
    pub fn is_ascii_only(&self) -> bool {
        self.ascii_only
    }

    /// Go: `func (pm *PositionMap) UTF8ToUTF16(utf8Offset int) int`.
    pub fn utf8_to_utf16(&self, utf8_offset: i32) -> i32 {
        if self.ascii_only {
            return utf8_offset;
        }
        // Binary search: find the last entry where utf8Pos <= utf8Offset.
        let entries = &self.entries;
        let (mut lo, mut hi) = (0usize, entries.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if entries[mid].utf8_pos <= utf8_offset {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            // Before any multi-byte character
            return utf8_offset;
        }
        utf8_offset - entries[lo - 1].delta
    }

    /// Go: `func (pm *PositionMap) UTF16ToUTF8(utf16Offset int) int`.
    pub fn utf16_to_utf8(&self, utf16_offset: i32) -> i32 {
        if self.ascii_only {
            return utf16_offset;
        }
        // We need the last entry where (utf8Pos - delta) <= utf16Offset.
        // (utf8Pos - delta) is the UTF-16 offset of that entry's character.
        let entries = &self.entries;
        let (mut lo, mut hi) = (0usize, entries.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let utf16_pos = entries[mid].utf8_pos - entries[mid].delta;
            if utf16_pos <= utf16_offset {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return utf16_offset;
        }
        utf16_offset + entries[lo - 1].delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ported from positionmap_test.go TestPositionMapASCII.
    #[test]
    fn position_map_ascii() {
        let text = "const x = 1;";
        let pm = compute_position_map(text.as_bytes());
        assert!(pm.is_ascii_only());
        for i in 0..=text.len() {
            assert_eq!(pm.utf8_to_utf16(i as i32), i as i32);
            assert_eq!(pm.utf16_to_utf8(i as i32), i as i32);
        }
    }

    /// Ported from positionmap_test.go TestPositionMapTwoByte.
    #[test]
    fn position_map_two_byte() {
        // "café" — é (U+00E9) is 2 bytes UTF-8, 1 code unit UTF-16
        let text = "const café = 1;\nconst x = 2;";
        let pm = compute_position_map(text.as_bytes());
        assert!(!pm.is_ascii_only());

        // Everything before é (byte offset 9) should be identity
        for i in 0..10 {
            assert_eq!(pm.utf8_to_utf16(i), i);
        }
        // é starts at UTF-8 byte 9, UTF-16 offset 9: same
        assert_eq!(pm.utf8_to_utf16(9), 9);
        // After é (byte 11 in UTF-8 = code unit 10 in UTF-16), delta is 1
        assert_eq!(pm.utf8_to_utf16(11), 10);

        // 'x' on second line: UTF-8 byte 23, UTF-16 offset 22
        let x_utf8 = text.rfind('x').expect("x") as i32;
        assert_eq!(pm.utf8_to_utf16(x_utf8), x_utf8 - 1);
        // Reverse: UTF-16 offset 22 should map to UTF-8 byte 23
        let x_utf16 = x_utf8 - 1;
        assert_eq!(pm.utf16_to_utf8(x_utf16), x_utf8);
    }

    /// Ported from positionmap_test.go TestPositionMapFourByte.
    #[test]
    fn position_map_four_byte() {
        // 🎉 (U+1F389) is 4 bytes UTF-8, 2 code units UTF-16
        let text = "const a = \"🎉\";\nconst b = 2;";
        let pm = compute_position_map(text.as_bytes());
        assert!(!pm.is_ascii_only());
        let b_utf8 = text.rfind('b').expect("b") as i32;
        let b_utf16 = b_utf8 - 2; // delta of 2 from emoji
        assert_eq!(pm.utf8_to_utf16(b_utf8), b_utf16);
        assert_eq!(pm.utf16_to_utf8(b_utf16), b_utf8);
    }

    /// Ported from positionmap_test.go TestPositionMapMultipleNonASCII.
    #[test]
    fn position_map_multiple_non_ascii() {
        // "à" (U+00E0) = 2 bytes UTF-8, 1 code unit UTF-16 (delta +1)
        // "🎉" (U+1F389) = 4 bytes UTF-8, 2 code units UTF-16 (delta +2)
        let text = "à🎉x";
        let pm = compute_position_map(text.as_bytes());
        let tests: [(i32, i32); 4] = [(0, 0), (2, 1), (6, 3), (7, 4)];
        for (utf8, utf16) in tests {
            assert_eq!(pm.utf8_to_utf16(utf8), utf16, "utf8 {utf8}");
            assert_eq!(pm.utf16_to_utf8(utf16), utf8, "utf16 {utf16}");
        }
    }

    /// Ported from positionmap_test.go TestPositionMapLoneSurrogateSentinel —
    /// the scanner's sentinel encoding for a lone surrogate (invalid UTF-8, so
    /// the text is assembled as raw bytes).
    #[test]
    fn position_map_lone_surrogate_sentinel() {
        let mut text: Vec<u8> = b"a".to_vec();
        text.extend(tsc_stringutil::encode_js_string_rune(0xD800));
        text.push(b'b');
        let pm = compute_position_map(&text);
        assert!(!pm.is_ascii_only());
        assert_eq!(pm.utf8_to_utf16(text.len() as i32), 3);
        assert_eq!(pm.utf16_to_utf8(2), text.len() as i32 - 1);
    }

    /// Ported from positionmap_test.go TestPositionMapRoundtrip.
    #[test]
    fn position_map_roundtrip() {
        let text = "let café = \"🎉\"; // naïve";
        let pm = compute_position_map(text.as_bytes());
        let utf16_len = pm.utf8_to_utf16(text.len() as i32);
        for i in 0..=utf16_len {
            let utf8_pos = pm.utf16_to_utf8(i);
            let back = pm.utf8_to_utf16(utf8_pos);
            assert_eq!(back, i, "roundtrip utf16->utf8->utf16 at {i}");
        }
    }
}
