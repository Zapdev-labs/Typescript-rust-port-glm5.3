// Port of tsc/internal/testutil @ ec47d33c23e464a17cdf2475632cba629bee8763
// (the subset the M6 conformance runner drives at PARSE+BIND depth).
//
// File map (Go → Rust):
//   testutil/testutil.go          → lib.rs (this file; only the test-main
//                                    helpers that apply outside `go test` —
//                                    see the PORT notes below)
//   testutil/baseline/baseline.go → baseline.rs (the golden-file
//                                    write/compare machinery)
//   testutil/baseline/testmain.go → (not ported — PORT note in baseline.rs)
//   testutil/tsbaseline/*.go       → tsbaseline.rs (util.go + the
//                                    .errors.txt emitter machinery; the
//                                    .types/.symbols emitters are checker-
//                                    scope and stay structural-only — PORT
//                                    notes inside)
//   testutil/harnessutil/*.go     → harness.rs (the subset compiler_runner
//                                    needs at this depth: test files,
//                                    file-based test configurations,
//                                    enumeration; CompileFiles is superseded
//                                    by the runner's written-tsconfig
//                                    program construction — see
//                                    tsc-testrunner)
//   testutil/fixtures/*.go        → fixtures.rs (the bench fixture list)
//
// PORT notes (testutil.go):
//   - `AssertPanics`/`RecoverAndFail` are `go test` recover plumbing; the
//     Rust runner catches panics with `catch_unwind` at its own boundary
//     (see tsc-testrunner), so they have no port here.
//   - `testProgramIsSingleThreaded` gates Go's checker pool fan-out (race
//     detector + env); the Rust program layer is single-threaded at this
//     depth, so the gate is inert.
//   - `CheckDataOnly` is a Go-reflection audit of shared host data; the
//     Rust port's hosts are plain `Arc<dyn _>` with no reflective audit
//     surface (PORTING-NOTES).
//   - Skipped subpackages (checker/emit/LSP-scope, or unused at this
//     depth): autoimporttestutil, contentmappertest, emittestutil,
//     filefixture (the runner inlines its fixture list — fixtures.rs),
//     fsbaselineutil (the `--transpile` local-baseline watcher),
//     jstest, lsptestutil, parsetestutil (fourslash), projecttestutil,
//     race, stringtestutil.

pub mod baseline;
pub mod fixtures;
pub mod harness;
pub mod tsbaseline;
