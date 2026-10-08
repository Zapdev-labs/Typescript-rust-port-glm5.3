// Port of tsc/internal/testutil/fixtures/benchfixtures.go @
// ec47d33c23e464a17cdf2475632cba629bee8763.
//
// Go's fixtures pull the filefixture package (FromString/FromFile over
// repo.TestDataPath()); the port inlines the two-field fixture shape and
// resolves the file-backed entries lazily against
// `tsc_repo::test_data_path()` — the fixture list is only a data table for
// bench drivers, so the filefixture machinery (watch semantics) is not
// ported.

use std::path::PathBuf;

/// Go: `filefixture.Fixture` (the FromString/FromFile constructors collapse
/// into the resolved table below).
#[derive(Clone, Debug)]
pub struct Fixture {
    /// The fixture's logical name.
    pub name: String,
    /// The fixture text ("string" fixtures), or `None` for file-backed
    /// fixtures (resolve through [`Fixture::file_path`]).
    pub content: Option<String>,
    /// The on-disk source for file-backed fixtures (absolute path).
    pub file_path: Option<PathBuf>,
}

impl Fixture {
    /// FromString shape: inline content.
    fn from_string(name: &str, content: &str) -> Fixture {
        Fixture {
            name: name.to_string(),
            content: Some(content.to_string()),
            file_path: None,
        }
    }

    /// FromFile shape: resolved against the test-data root.
    fn from_file(name: &str, relative: &str) -> Fixture {
        Fixture {
            name: name.to_string(),
            content: None,
            file_path: Some(PathBuf::from(tsc_repo::test_data_path()).join(relative)),
        }
    }

    /// The fixture's text (file-backed fixtures are read on demand).
    pub fn text(&self) -> Option<String> {
        if let Some(content) = &self.content {
            return Some(content.clone());
        }
        self.file_path.as_ref().and_then(|path| std::fs::read_to_string(path).ok())
    }
}

/// Go: `var BenchFixtures`.
pub fn bench_fixtures() -> Vec<Fixture> {
    vec![
        Fixture::from_string("empty.ts", ""),
        Fixture::from_file("checker.ts", "fixtures/compiler/checker.ts"),
        Fixture::from_file("dom.generated.d.ts", "fixtures/lib/dom.generated.d.ts"),
        Fixture::from_file("Herebyfile.mjs", "fixtures/typescript/Herebyfile.mjs"),
        Fixture::from_file(
            "jsxComplexSignatureHasApplicabilityError.tsx",
            "fixtures/testcases/compiler/jsxComplexSignatureHasApplicabilityError.tsx",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_fixtures_table_matches_go() {
        let fixtures = bench_fixtures();
        assert_eq!(fixtures.len(), 5);
        assert_eq!(fixtures[0].name, "empty.ts");
        assert_eq!(fixtures[0].text().as_deref(), Some(""));
        // the file-backed fixtures resolve under the test-data root
        assert!(fixtures[1].file_path.as_ref().is_some_and(|path| path.ends_with("fixtures/compiler/checker.ts")));
    }
}
