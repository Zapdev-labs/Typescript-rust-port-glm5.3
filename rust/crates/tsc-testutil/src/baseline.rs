// Port of tsc/internal/testutil/baseline/baseline.go @
// ec47d33c23e464a17cdf2475632cba629bee8763.
//
// The golden-file accept/compare machinery. Go's `Run` is `testing.T`-coupled
// (it t.Error()s on divergence and records paths for `baseline.Track()`); the
// port returns the comparison outcome and lets the caller decide — the
// conformance runner (tsc-testrunner) maps it onto the twin's
// match/mismatch/missing-baseline verdicts.
//
// PORT notes:
//   - `DiffText` (the patience unified diff) is NOT ported: Go pulls
//     github.com/peter-evans/patience, which is not in the workspace's
//     approved dependency set (SPEC §5.11), and its only consumers in the
//     ported surface (module-resolution / content-mapper baselines) are
//     checker-scope and not ported. The twin discipline's verdicts use
//     plain equality (`diff(1)` compares the drivers' NDJSON).
//   - baseline/testmain.go (`Track`, `TSGO_BASELINE_TRACKING_DIR`) is the
//     CI's herecfg baseline-tracking plumbing; it has no port (the runner
//     is driver-invoked, not test-main-invoked).
//   - The Go `localRoot`/`referenceRoot` are repo-derived package vars; the
//     port takes them as an explicit [`BaselinePaths`] so the fixture
//     matrix can carry its own committed reference tree.

use std::path::{Path, PathBuf};

/// Go: `baseline.Options` — only `Subfolder` survives; `DiffFixupOld` /
/// `DiffFixupNew` / `SkipDiffWithOld` feed the (unported) patience-diff
/// display path and the module-resolution baseline, both PORT-noted above.
#[derive(Clone, Debug, Default)]
pub struct BaselineOptions {
    /// The suite subfolder under the baseline roots (e.g. "conformance").
    pub subfolder: String,
}

/// Go: `baseline.NoContent` — the sentinel for "no baseline required"
/// (no diagnostics, so no `.errors.txt` at all).
pub const NO_CONTENT: &str = "<no content>";

/// The two baseline roots (Go: the package vars derived from
/// `repo.TestDataPath()`: `testdata/baselines/local` and
/// `testdata/baselines/reference`).
#[derive(Clone, Debug)]
pub struct BaselinePaths {
    /// `baselines/local` — where new/changed baselines get written.
    pub local_root: PathBuf,
    /// `baselines/reference` — the committed golden tree.
    pub reference_root: PathBuf,
}

impl BaselinePaths {
    /// Go: `localPath = filepath.Join(localRoot, subfolder, fileName)`.
    pub fn local_path(&self, opts: &BaselineOptions, file_name: &str) -> PathBuf {
        self.local_root.join(&opts.subfolder).join(file_name)
    }

    /// Go: `referencePath = filepath.Join(referenceRoot, subfolder, fileName)`.
    pub fn reference_path(&self, opts: &BaselineOptions, file_name: &str) -> PathBuf {
        self.reference_root.join(&opts.subfolder).join(file_name)
    }
}

/// The outcome of one golden-file comparison (Go: the silent pass plus the
/// `t.Errorf` branches of `writeComparison`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComparisonOutcome {
    /// Actual equals the reference (or both sides expect no content).
    Match,
    /// No reference exists and actual is real content — a new baseline was
    /// written to the local root (Go: "new baseline created at %s").
    NewBaseline { local: PathBuf },
    /// The reference exists but actual differs — the local copy was written
    /// (Go: "the baseline file %s has changed").
    Changed { local: PathBuf, reference: PathBuf },
    /// Actual is `NO_CONTENT` but a reference exists — a `.delete` marker
    /// was written (Go's `local+".delete"` branch).
    Deleted { local: PathBuf, reference: PathBuf },
}

/// Go: `func baseline.Run(t, fileName, actual, opts)` minus the tracking and
/// the `testing.T` coupling.
pub fn run_baseline(paths: &BaselinePaths, file_name: &str, actual: &str, opts: &BaselineOptions) -> ComparisonOutcome {
    let local = paths.local_path(opts, file_name);
    let reference = paths.reference_path(opts, file_name);
    write_comparison(actual, &local, &reference)
}

/// Go: `func writeComparison(t, actualContent, local, reference)`.
pub fn write_comparison(actual_content: &str, local: &Path, reference: &Path) -> ComparisonOutcome {
    if actual_content.is_empty() {
        // Go panics: the caller must return baseline.NoContent instead.
        panic!("the generated content was \"\". Return 'baseline.NoContent' if no baselining is required.");
    }
    if let Some(parent) = local.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            panic!("failed to create directories for the local baseline file {}", local.display());
        }
    }
    // Go removes a stale local file first (an empty MkdirAll-only tree is
    // not a valid baseline); the port matches.
    if local.exists() && std::fs::remove_file(local).is_err() {
        panic!("failed to remove the local baseline file {}", local.display());
    }

    // Go reads the reference raw (any bytes become the string); the port
    // lossy-converts so non-UTF-8 references still compare, like Go's.
    let reference_bytes = std::fs::read(reference);
    let found_expected = reference_bytes.is_ok();
    let expected: std::borrow::Cow<'_, str> = match &reference_bytes {
        Ok(bytes) => String::from_utf8_lossy(bytes),
        Err(_) => std::borrow::Cow::Borrowed(NO_CONTENT),
    };
    if expected == actual_content && !(actual_content == NO_CONTENT && found_expected) {
        return ComparisonOutcome::Match;
    }
    if actual_content == NO_CONTENT {
        // Go appends ".delete" to the full local path; build it textually
        // (`with_extension` would only swap a fixed extension).
        let delete_path = PathBuf::from(format!("{}.delete", local.display()));
        if std::fs::write(&delete_path, b"").is_err() {
            panic!("failed to write the local baseline file {}", delete_path.display());
        }
        return ComparisonOutcome::Deleted {
            local: delete_path,
            reference: reference.to_path_buf(),
        };
    }
    if std::fs::write(local, actual_content).is_err() {
        panic!("failed to write the local baseline file {}", local.display());
    }
    if !found_expected {
        return ComparisonOutcome::NewBaseline { local: local.to_path_buf() };
    }
    ComparisonOutcome::Changed {
        local: local.to_path_buf(),
        reference: reference.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch dir for baseline writes (never /tmp — the local tmpfs is
    /// quota-limited): cargo's per-crate CARGO_TARGET_TMPDIR when driven by
    /// cargo, else $HOME/.confrunner-scratch.
    fn scratch(tag: &str) -> PathBuf {
        let base = std::env::var_os("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".confrunner-scratch")))
            .expect("no scratch dir available");
        let dir = base.join("baseline-test").join(tag);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    fn paths(tag: &str) -> BaselinePaths {
        let dir = scratch(tag);
        BaselinePaths {
            local_root: dir.join("local"),
            reference_root: dir.join("reference"),
        }
    }

    #[test]
    fn match_when_reference_content_is_identical() {
        let paths = paths("match");
        std::fs::create_dir_all(paths.reference_root.join("suite")).expect("mkdir");
        std::fs::write(paths.reference_root.join("suite/a.errors.txt"), "gold").expect("write");
        let outcome = run_baseline(&paths, "a.errors.txt", "gold", &BaselineOptions { subfolder: "suite".into() });
        assert_eq!(outcome, ComparisonOutcome::Match);
    }

    #[test]
    fn new_baseline_when_reference_missing_and_actual_has_content() {
        let paths = paths("new");
        let outcome = run_baseline(&paths, "a.errors.txt", "actual", &BaselineOptions { subfolder: "suite".into() });
        assert_eq!(
            outcome,
            ComparisonOutcome::NewBaseline { local: paths.local_root.join("suite").join("a.errors.txt") }
        );
        // the local copy was written
        assert_eq!(std::fs::read_to_string(paths.local_root.join("suite/a.errors.txt")).unwrap(), "actual");
    }

    #[test]
    fn no_content_matches_only_a_missing_reference() {
        let paths = paths("nocontent");
        // missing reference: NoContent == NoContent (expected default) — match
        let outcome = run_baseline(&paths, "a.errors.txt", NO_CONTENT, &BaselineOptions { subfolder: "suite".into() });
        assert_eq!(outcome, ComparisonOutcome::Match);
        // present reference: .delete marker
        std::fs::create_dir_all(paths.reference_root.join("suite")).expect("mkdir");
        std::fs::write(paths.reference_root.join("suite/a.errors.txt"), "stale gold").expect("write");
        let outcome = run_baseline(&paths, "a.errors.txt", NO_CONTENT, &BaselineOptions { subfolder: "suite".into() });
        assert!(matches!(outcome, ComparisonOutcome::Deleted { .. }));
        assert!(paths.local_root.join("suite/a.errors.txt.delete").exists());
    }

    #[test]
    fn changed_when_reference_differs() {
        let paths = paths("changed");
        std::fs::create_dir_all(paths.reference_root.join("suite")).expect("mkdir");
        std::fs::write(paths.reference_root.join("suite/a.errors.txt"), "old gold").expect("write");
        let outcome = run_baseline(&paths, "a.errors.txt", "new actual", &BaselineOptions { subfolder: "suite".into() });
        assert!(matches!(outcome, ComparisonOutcome::Changed { .. }));
        assert_eq!(std::fs::read_to_string(paths.local_root.join("suite/a.errors.txt")).unwrap(), "new actual");
    }

    #[test]
    #[should_panic(expected = "the generated content was \"\"")]
    fn empty_actual_panics_like_go() {
        let paths = paths("empty");
        let _ = run_baseline(&paths, "a.errors.txt", "", &BaselineOptions { subfolder: "suite".into() });
    }
}
