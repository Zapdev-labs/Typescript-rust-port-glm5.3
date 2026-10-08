// Ported from tsc/internal/core/pattern.go @ ec47d33c23e464a17cdf2475632cba629bee8763

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pattern {
    pub text: String,
    pub star_index: i32, // -1 for exact match
}

pub fn try_parse_pattern(pattern: &str) -> Pattern {
    let star_index = pattern.find('*').map(|i| i as i32).unwrap_or(-1);
    if star_index == -1 || !pattern[star_index as usize + 1..].contains('*') {
        return Pattern {
            text: pattern.to_owned(),
            star_index,
        };
    }
    Pattern::default()
}

impl Pattern {
    pub fn is_valid(&self) -> bool {
        self.star_index == -1 || self.star_index < self.text.len() as i32
    }

    pub fn matches(&self, candidate: &str) -> bool {
        if self.star_index == -1 {
            return self.text == candidate;
        }
        candidate.len() as i64 >= self.text.len() as i64 - 1
            && candidate.starts_with(&self.text[..self.star_index as usize])
            && candidate.ends_with(&self.text[self.star_index as usize + 1..])
    }

    pub fn matched_text<'a>(&self, candidate: &'a str) -> &'a str {
        if !self.matches(candidate) {
            panic!("candidate does not match pattern");
        }
        if self.star_index == -1 {
            return "";
        }
        // PORT: Go's int arithmetic cannot underflow; keep the same
        // evaluation order in i64 before slicing (matches() has already
        // guaranteed the range is valid).
        let end = candidate.len() as i64 - self.text.len() as i64
            + self.star_index as i64
            + 1;
        &candidate[self.star_index as usize..end as usize]
    }
}

// PORT: Go's FindBestPatternMatch returns the zero value of T when no value
// matches; the port returns Option<&T> instead (nil → None).
pub fn find_best_pattern_match<'a, T>(
    values: &'a [T],
    get_pattern: impl Fn(&T) -> Pattern,
    candidate: &str,
) -> Option<&'a T> {
    let mut best_pattern: Option<&'a T> = None;
    let mut longest_match_prefix_length: i32 = -1;
    for value in values {
        let pattern = get_pattern(value);
        if (pattern.star_index == -1 || pattern.star_index > longest_match_prefix_length)
            && pattern.matches(candidate)
        {
            best_pattern = Some(value);
            longest_match_prefix_length = pattern.star_index;
        }
    }
    best_pattern
}
