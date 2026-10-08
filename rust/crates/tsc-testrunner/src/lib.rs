// Port of tsc/internal/testrunner @ ec47d33c23e464a17cdf2475632cba629bee8763
// — the compiler/conformance baseline runner, at the depth the Rust port's
// pipeline exists TODAY (M5 program layer: parse + bind + program
// construction). This is the SPEC §8 M6 gate tooling: the staged baseline
// climb tracked in rust/CONFORMANCE.md.
//
// File map (Go → Rust):
//   testrunner/runner.go            → runner.rs (the Runner shape)
//   testrunner/test_case_parser.go   → test_case_parser.rs (the @filename /
//                                      @option directive machinery)
//   testrunner/compiler_runner.go   → compiler_runner.rs (the baseline
//                                      runner + the depth-controlled case
//                                      pipeline; the checker-dependent
//                                      verify* arms are PORT-noted at their
//                                      sites)
//   testrunner/compiler_test_filter.go → compiler_test_filter.rs
//   testrunner/options_generated.go → options_generated.rs
//
// DEPTH CONTROL (SPEC §8 M6): the checker is not ported (only the
// rust/crates/checker spike, owned by a parallel worker), so the runner
// drives the pipeline only through [`Stage::Parse`] and [`Stage::Bind`].
// [`Stage::Check`] exists as the explicit unwired boundary — asking for it
// is an error, never fake output. The plug point is the Go program layer's
// check entry points (tsc/internal/compiler/program.go: `GetSemanticDiagnostics`
// / `GetSemanticDiagnosticsForIncremental` → rust/crates/compiler's
// CheckerPool) — PORT-noted in compiler_runner.rs.
//
// NOT PORTED (PORT notes, also in compiler_runner.rs):
//   - testrunner/transpile_runner.go — needs the Layer-5 emit path (not yet
//     ported; the `--transpile` suite is the M7 boundary).
//   - testrunner/testmain_test.go's baseline.Track CI plumbing.
//   - harnessutil's checker-scope arms (SkipUnsupportedCompilerOptions,
//     CompileFiles' emit/trace machinery) — see tsc-testutil's headers.

pub mod compiler_runner;
pub mod compiler_test_filter;
pub mod options_generated;
pub mod runner;
pub mod test_case_parser;

pub use compiler_runner::{
    CaseRecord, ConformanceConfig, Stage, Verdict, enumerate_case_files, run_conformance,
};
pub use runner::{Runner, run_tests};
