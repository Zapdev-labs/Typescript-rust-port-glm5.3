# Conformance Tracker

The M6 conformance runner (SPEC §8): a port of tsc's `testrunner` +
`testutil` baseline machinery driving the Rust compiler at the wired
depth (**parse + bind**), with a byte-identical Go twin proving the
runner-structure parity, and this file tracking the climb — the match
fraction of the Rust pipeline's rendered `.errors.txt` against
`tsc/testdata/baselines/reference/`.

## Method

- **Crates**: `crates/tsc-testutil` (baseline golden-file machinery,
  the `.errors.txt` emitter, config expansion, fixtures) and
  `crates/tsc-testrunner` (the case parser, the `-test.run`-style filter,
  the generated 88-entry option table, the per-case pipeline).
- **Depth control** — `Stage::{Parse,Bind,Check}` in
  `tsc-testrunner/src/compiler_runner.rs`. `Parse` builds the program,
  `Bind` (default) additionally calls `bind_source_files`, and `Check`
  is the explicit unwired boundary: asking for it is an error, no fake
  output. The plug point is the Go program layer's `GetSemanticDiagnostics`
  (`tsc/internal/compiler/program.go`), landing with the checker wave
  (`crates/checker`).
- **Twin discipline**: both drivers run the *same ported pipeline* —
  `tsc-bench conformance` (Rust) and `go-bench conformance`
  (`rust/bench/go-driver/conformance.go`, mirroring the port, not the
  original harness) — over the same corpus with the same flag grammar
  (`--corpus --baselines --suite --limit --filter --stage --local-root`)
  and the same NDJSON contract per case:
  `{"case","baseline","verdict","errors_count","shape_hash"}` (field
  order fixed; `shape_hash` is FNV-1a 64 over the case, baseline, error
  count, and one fold line per diagnostic in the render sort order;
  `errors_count: -1` with an empty hash is the panic sentinel — the
  panic message goes to STDERR only).
- **Determinism by construction**: BTreeMap/sorted iteration everywhere
  (Go's map order is random — the port's rule), sorted case enumeration,
  sorted configuration description order per the cartesian worker, and
  vfstest `from_map` (which sorts internally) in both drivers.
- **Climb metric**: `match` = the rendered `.errors.txt` content equals
  the reference (or both sides have no content). `mismatch` = reference
  exists and differs; `missing-baseline` = the pipeline produces content
  but no reference exists (the real harness ran the checker — see the
  gate table below).

## Commands

```sh
# fixture matrix (both drivers must emit byte-identical NDJSON)
M=rust/bench/gates/conformance-matrix
cargo run -p bench-harness -- conformance --corpus $M/cases --baselines $M/baselines --suite conformance
(cd rust/bench/go-driver && go build -o $BIN/go-bench .) && $BIN/go-bench conformance \
    --corpus $M/cases --baselines $M/baselines --suite conformance   # identical flags

# real corpus climb (first 300 case files after skip/filter)
cargo run -p bench-harness -- conformance --corpus tsc/testdata/tests/cases/conformance \
    --baselines tsc/testdata/baselines/reference --suite conformance --limit 300
$BIN/go-bench conformance ...   # identical flags; diff the two NDJSON streams

# unit tests + lints
cargo test -p tsc-testutil -p tsc-testrunner
cargo clippy -p tsc-testutil -p tsc-testrunner -p bench-harness --all-targets
```

Build discipline for the runs above: `CARGO_TARGET_DIR` outside the repo,
scratch and binaries under `$HOME` (see `rust/BENCHMARKS.md` for the
twin-run conventions).

## Measured numbers (2026-10-08, first green slice)

**Fixture matrix** (9 cases: clean parse, multifile include, parse
error, expected parse error, in-test tsconfig, option variant, reference
roots, @currentDirectory):

- Rust: 9/9 `match` (100.0%), twin NDJSON byte-identical, double-run
  deterministic.
- Reference baselines committed at
  `rust/bench/gates/conformance-matrix/baselines/conformance/*.errors.txt`.

**First 300 case files** of `tsc/testdata/tests/cases/conformance`
(384 records — option-variant cases expand to one record per
configuration):

| driver | records | match | mismatch | missing-baseline | panic |
|--------|---------|-------|----------|------------------|-------|
| Rust   | 384 | 213 (**55.5%**) | 163 | 8 | 0 |
| Go twin| 384 | 143 (37.2%)     | 163 | 78 | 0 |

Twin parity on the slice: **304/384 records byte-identical** (79.2%).
All 80 divergent records trace to two program-layer gaps (below), none
to the runner structure. Runtime: ~69 s Rust (dev profile) vs ~10 s Go.

**M5 program gate regression**: `tsc-bench program` vs `go-bench
program` over `rust/bench/gates/program-matrix` — byte-identical
(15 records), no regression from the M6 work.

## Gate table

| gate | target | measured | state |
|------|--------|----------|-------|
| first green slice | fixture matrix twin identical + climb ≥ 50% | matrix identical; 55.5% | **PASS** |
| M6 done | climb ≥ 90% on the 300-case slice | 55.5% | blocked (below) |

A test is counted "pass" in the legacy table only when its full baseline
set is byte-identical to the reference tree; the climb % above is the
single-`.errors.txt` slice at parse+bind depth.

## Known gaps (all program-layer, surfaced by the twin)

1. **Removed-option diagnostics (TS5102/TS5108)** — Go's
   `verifyCompilerOptions` (`tsc/internal/compiler/program.go`)
   reports removed compiler options and enum values
   (`target=ES5`, `downlevelIteration`, …) as program diagnostics
   anchored at the config source file. ~78 of the 80 divergent
   records. Porting needs the AST-config-node diagnostic machinery
   (`CreateDiagnosticForNodeInSourceFile`) — the same surface the
   include-reason `GetMatchedFileSpec` gap needs (below).
2. **JS parse-diagnostic scope (TS8009)** — the Rust parser reports
   `'async' modifier can only be used in TypeScript files` for `.js`
   units at parse depth where Go emits it from the checker
   (`asyncArrowFunction_allowJs`, 1 record, 4 diagnostics).
3. **Include-reason matched-spec arms** — the RootFile reason always
   renders "Root file specified for compilation"; Go's
   `GetMatchedFileSpec`/`GetMatchedIncludeSpec` ("Part of 'files' list
   in tsconfig.json", "Matched by include pattern …") and their
   related-info arms are unported (the fixture matrix avoids the arm;
   it fires on tsconfig `files` entries that fail to load).
4. **Related-info location** — the Rust `ast::Diagnostic` carries
   related files as node ids (the PORTING-NOTES file-id gap), so both
   conformance drivers render `!!! related TSxxxx: <message>` without
   the `file(line,col)` prefix. Parse-level diagnostics do not carry
   related information in practice.

## Subtleties (PORT notes kept in the crates)

- Baseline names: `<base>(<config desc>).errors.txt` — flat under the
  suite folder (`baselines/reference/conformance/` is flat); the config
  description is the sorted `key=value` join of the varying options.
- The synthetic per-case tsconfig is `__conformance__.tsconfig.json`
  (options + the root `files` list, minus `.json`/`.tsbuildinfo`
  units); an in-test `tsconfig.json` unit takes the verbatim route.
- The in-test tsconfig unit seeds the same map-FS path twice (unit +
  config entry) — the Go twin's map assignment dedupes silently; the
  Rust side folds the entry list through a BTreeMap first
  (last write wins, mirroring Go).
- `@link`/`@symlink` entries are seeded as **file copies** of their
  target unit (the depth-controlled simplification; a resolved
  target's bytes are identical — a real `vfs.Symlink` lands with the
  emit wave). Targets outside the unit set are dropped.
- `split_option_values` keeps the `*` literal (no enum-map expansion —
  PORT note in `tsc-testutil/src/harness.rs`); exclusions `-x`/`!x` and
  first-occurrence dedupe are Go's.
- `iterate_error_baseline` compares prefix-stripped (relative) strings
  through `ComparePaths` directly — no rooting (Go compares relatives
  too).
- `newLine` state is shared across sections (global errors, per-file
  headers, squiggles, `!!!` lines) — the first `!!!` line after a
  squiggle gets the separator; each section's builder is reset but the
  `firstLine` flag is not re-initialized.
- `parseTestFilesAndSymlinks` runs with the implicit-first-file gate
  OFF (the compiler runner's options; the fourslash harness's form is
  kept as a const for parity) and panics on non-comment content before
  the first `@filename` — Go's exact behavior.
- `@filename` fallback: a single-file case's unit name is the case's
  basename; its content keeps the trailing newline (the final empty
  split line writes the separator — Go's line join).
- The `.delete` marker semantics: `NoContent` + an existing reference
  is a `mismatch` at this depth (the real harness accepts the delete).
