# TypeScript Compiler — Rust Port

A Rust port of the TypeScript compiler's Go port
(`tsc/internal/checker` et al.), built to beat the Go baseline on
speed and memory while matching its diagnostics byte-for-byte.

**Current standing** (2026-10-20, quiet rig): parse race t1 **0.977x**
and t16 **0.766x** (Rust faster at both thread counts), peak RSS
~49% of Go, Phase 0 micro geomean **0.101x**; check-stage twin
divergence 210/7,567 records against the Go driver. Tranche history
and methodology: [`rust/SPEC.md`](rust/SPEC.md),
[`rust/PORTING-NOTES.md`](rust/PORTING-NOTES.md),
[`rust/CONFORMANCE.md`](rust/CONFORMANCE.md),
[`rust/BENCHMARKS.md`](rust/BENCHMARKS.md).

This README covers **installing and running** the Rust side.

## Requirements

- **Rust** stable toolchain ≥ 1.85 (the workspace pins
  `rust/rust-toolchain.toml`; `rustup` picks it up automatically).
  Install via [rustup](https://rustup.rs):
  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **A checkout of this repository** (the Rust workspace lives in
  `rust/` but reads the TypeScript sources and test data from the
  surrounding tree: `tsc/`, notably `tsc/testdata/` for the
  conformance corpus and baselines).
- Optional, for the Go-vs-Rust benchmarks: **Go** (any recent
  toolchain) to build the Go driver in `rust/bench/go-driver`, and
  `cc` for `rust/bench/rsswrap.c` (peak-RSS measurement).

No system packages beyond a C toolchain for the benchmark harness;
the crates themselves are pure Rust with a small approved dependency
set (see `rust/Cargo.toml`).

## Build

From the repository root (or `rust/` — same workspace):

```sh
cd rust
cargo build --release -p bench-harness
```

This produces the driver binary:

```
rust/target/release/tsc-bench
```

`tsc-bench` is the all-in-one frontend used for conformance runs and
benchmarks. Subcommands used day-to-day:

```sh
# Conformance sweep (check stage) against the TypeScript baselines:
rust/target/release/tsc-bench conformance \
  --corpus  tsc/testdata/tests/cases/conformance \
  --baselines tsc/testdata/baselines/reference \
  --suite conformance --stage check

# Parse/bind stages, thread-count scaling, and a tiny gated corpus:
rust/target/release/tsc-bench conformance ... --stage parse   # or --stage bind
rust/target/release/tsc-bench program --corpus rust/bench/gates/program-matrix
```

Each run prints one JSON record per case (`verdict`, `errors_count`,
`shape_hash`) — the twin-test contract against the Go driver.

## Test

```sh
cd rust
cargo test -p tsc-checker -p tsc-testrunner -p tsc-binder \
           -p tsc-tsoptions -p tsc-scanner -p tsc-parser
cargo clippy -p tsc-checker   # warnings are kept at zero
```

The `fcs*_top_cluster_pins.rs` / `fcs*_top_cluster_records.rs` suites
pin converged conformance records so regressions fail loudly.

## Benchmarks (optional)

Both scripts run from the repository root and compare the Rust
binary against the Go driver on identical work (see
`rust/bench/DRIVER-CONTRACT.md`):

```sh
bash rust/scripts/bench-micro.sh   # Phase 0 micro benches
bash rust/scripts/bench-parse.sh    # Phase A parse race (threads 1/16)
```

## Remote-rig workflow (maintainers)

Heavy builds and full-corpus sweeps are meant to run on the build rig,
not the local machine. The helper scripts mirror the tree and run
everything remotely:

```sh
# sync local edits to the rig (in-place, nice'd):
TS_RSYNC_INPLACE=1 nice -n 19 bash rust/scripts/remote-sync.sh

# build / test remotely:
ssh <rig> 'bash -lc "cd ~/TypeScript/rust && \
  cargo build --release --bin tsc-bench"'
```

See `rust/scripts/rig-setup.sh` for first-time rig provisioning and
`rust/scripts/remote-run.sh` / `remote-test.sh` for the remaining
helpers.

## Repository layout

| Path | Purpose |
|---|---|
| `rust/crates/` | The port itself — `parser`, `binder`, `checker` (the big one), `tsc-testrunner`, plus support crates (`tsc-core`, `tsc-diagnostics`, `tsc-tspath`, ...). |
| `rust/bench/` | Go driver, gated corpora, `rsswrap.c`, and the driver contract. |
| `rust/scripts/` | Benchmarks and rig sync/run/test helpers. |
| `rust/SPEC.md` | Porting spec: milestones, crate map, gates. |
| `rust/PORTING-NOTES.md` | Tranche-by-tranche landing log. |
| `tsc/`, `packages/`, `tools/` | The upstream TypeScript tree (source material; only `rust/**` carries the port's commits). |

---

The upstream Microsoft TypeScript README is available at
https://github.com/microsoft/TypeScript#readme.
