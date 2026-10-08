// Ported from tsc/internal/core/version.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::sync::OnceLock;

// This is a var so it can be overridden by ldflags.
// PORT: Go overrides `version` via `-ldflags`; the Rust port pins it as a
// static. The CLI crate can substitute its own banner text at the edge.
pub static VERSION: &str = "7.1.0-dev";

pub fn version() -> &'static str {
    VERSION
}

// PORT: Go computes `versionMajorMinor` once at package init via a func
// literal (panicking on invalid input before main); Rust computes it lazily
// with OnceLock, panicking on first call for an invalid version string.
pub fn version_major_minor() -> &'static str {
    static VERSION_MAJOR_MINOR: OnceLock<String> = OnceLock::new();
    VERSION_MAJOR_MINOR.get_or_init(|| {
        let mut seen_major = false;
        let i = VERSION
            .char_indices()
            .find(|&(_, r)| {
                if r == '.' {
                    if seen_major {
                        return true;
                    }
                    seen_major = true;
                }
                false
            })
            .map(|(i, _)| i);
        if i.is_none() {
            panic!("invalid version string: {VERSION}");
        }
        VERSION[..i.unwrap()].to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_major_minor_finds_second_dot() {
        assert_eq!(version(), "7.1.0-dev");
        assert_eq!(version_major_minor(), "7.1");
    }
}
