// Ported from tsc/internal/core/textchange.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use crate::text::TextRange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub text_range: TextRange,
    pub new_text: String,
}

impl TextChange {
    // PORT: positions are UTF-8 byte offsets (§5.6); slicing panics on a
    // non-boundary exactly like Go's string slicing would on invalid bytes.
    pub fn apply_to(&self, text: &str) -> String {
        format!(
            "{}{}{}",
            &text[..self.text_range.pos() as usize],
            self.new_text,
            &text[self.text_range.end() as usize..]
        )
    }
}

pub fn apply_bulk_edits(text: &str, edits: &[TextChange]) -> String {
    let mut b = String::new();
    b.reserve(text.len());
    let mut last_end: i32 = 0;
    for e in edits {
        let start = e.text_range.pos();
        if start != last_end {
            b.push_str(&text[last_end as usize..start as usize]);
        }
        b.push_str(&e.new_text);

        last_end = e.text_range.end();
    }
    b.push_str(&text[last_end as usize..]);

    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_to_and_bulk_edits() {
        let tc = TextChange {
            text_range: TextRange::new(1, 3),
            new_text: "XY".to_owned(),
        };
        assert_eq!(tc.apply_to("abcdef"), "aXYdef");

        let edits = vec![
            TextChange {
                text_range: TextRange::new(0, 2),
                new_text: "A".to_owned(),
            },
            TextChange {
                text_range: TextRange::new(2, 5),
                new_text: "BC".to_owned(),
            },
        ];
        // Edit 2 starts where edit 1 ends: no gap text is copied.
        assert_eq!(apply_bulk_edits("0123456", &edits), "ABC56");
    }
}
