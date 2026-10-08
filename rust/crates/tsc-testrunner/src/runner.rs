// Port of tsc/internal/testrunner/runner.go @
// ec47d33c23e464a17cdf2475632cba629bee8763.
//
// Go's `Runner` is `testing.T`-coupled (`RunTests(t)` registers subtests);
// the port splits the two halves the Go interface fuses: enumeration (the
// driver walks cases in its own deterministic order) and execution (the
// per-case pipeline in compiler_runner.rs). `run_tests` keeps Go's shape —
// run every runner's enumeration — for the runner-crate tests.

/// Go: `type Runner interface { EnumerateTestFiles() []string; RunTests(t) }`
/// — the enumeration half (`RunTests`' per-case work is the runner's own
/// `run_case` pipeline; Go fans it through `t.Run` subtests the port
/// replaces with the driver's line-per-case contract).
pub trait Runner {
    /// Go: `EnumerateTestFiles`.
    fn enumerate_test_files(&mut self) -> Vec<String>;
}

/// Go: `func runTests(t, runners)` — every runner, in order.
pub fn run_tests(runners: &mut [&mut dyn Runner]) -> Vec<String> {
    let mut files = Vec::new();
    for runner in runners.iter_mut() {
        files.extend(runner.enumerate_test_files());
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRunner(&'static [&'static str]);

    impl Runner for FakeRunner {
        fn enumerate_test_files(&mut self) -> Vec<String> {
            self.0.iter().map(|file| file.to_string()).collect()
        }
    }

    #[test]
    fn run_tests_concatenates_every_runners_enumeration() {
        let mut first = FakeRunner(&["a.ts", "b.ts"]);
        let mut second = FakeRunner(&["c.ts"]);
        let files = run_tests(&mut [&mut first as &mut dyn Runner, &mut second]);
        assert_eq!(files, vec!["a.ts".to_string(), "b.ts".to_string(), "c.ts".to_string()]);
    }
}
