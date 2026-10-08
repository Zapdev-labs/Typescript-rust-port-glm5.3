# Benchmark Results — tsrs (Rust port) vs tsgo (Go port)

Append-only log of the Rust port racing the Go port on the test rig.
Protocol: `rust/SPEC.md` §15. Every timed run records date, toolchain
versions, corpus SHA, both-side git SHAs, median wall time
(`hyperfine --warmup 2 --runs 10`) and peak RSS (`rsswrap`).
A >10% median regression vs the previous entry in the same phase blocks that
milestone's green status until explained or fixed.

## Rig

- Host: `dih@192.168.1.15` — AMD Ryzen 7 PRO 6850H (16 cores, SMT), 23 GiB
  RAM, Omarchy (Arch) x86_64
- Rust: rustc 1.99.0 · Go: (recorded by `rig-setup.sh`) ·
  hyperfine: (recorded by `rig-setup.sh`)
- Baseline binary: `~/bench-bin/tsgo-go` — built from `~/TypeScript/tsc` at the
  SHA in `rust/GO-BASELINE.txt`
- Rust drivers: `rust/crates/bench-harness` (`tsc-bench`) ·
  Go drivers: `rust/bench/go-driver` (`go-bench`) — identical CLI + JSON output
  incl. `shape_hash` (node count + kind histogram + error count), which must
  match across languages for a timed run to count.
- Corpus: `~/bench-corpus/excalidraw` — Excalidraw (excalidraw.com),
  SHA recorded below; files/bytes recorded by `rig-setup.sh`.

### Rig setup record

| date | go | hyperfine | tsgo version | corpus SHA | ts files | ts bytes |
|---|---|---|---|---|---|---|
| — | — | — | — | — | — | — |

## Phase 0 — micro benches (ported M1 crates)

Mirrors of Go's own `go test -bench` cases (`tsc/internal/tspath/path_test.go`,
`typed_paths_test.go`, `jsnum/jsnum_test.go`). Go run via
`go test -bench '^X$' -benchtime=Nx -count=10`; Rust via
`tsc-bench micro --op <name> --samples 10`. Compare medians.

| date | op | rust/go median | verdict | note |
|---|---|---|---|---|
| — | — | — | — | (first entries land today) |

## Phase A — corpus parse race (M3 exit gate)

All corpus `.ts`/`.tsx` (excl. `node_modules`, `dist`), both drivers, same
file list, threads measured at 1 and 16. Gate: ≤1.30× Go at first green,
hardening target ≤1.00×, stretch ≤0.80×; peak RSS ≤1.0× Go.

| date | git (rust) | threads | wall median (rust) | wall median (go) | rust/go | RSS rust/go | MB/s rust | shape_hash |
|---|---|---|---|---|---|---|---|---|
| — | — | — | — | — | — | — | — | — |

## Phase B — program race (M4/M5 gates)

Corpus bind bench (mirrors `BenchmarkBind`), program build (mirrors
`BenchmarkNewProgram`), `--showConfig` byte-parity vs `tsgo-go`.

| date | bench | rust median | go median | rust/go | parity |
|---|---|---|---|---|---|
| — | — | — | — | — | — |

## Phase C — end-to-end typecheck (M6+)

Full `tsc --noEmit -p ~/bench-corpus/excalidraw` vs `tsgo-go` (after `npm ci`
on the rig). Diagnostics must be byte-identical (modulo
`rust/ACCEPTED-DELTAS.txt`) *before* timing counts.

| date | tool | wall median | peak RSS | diag parity | rust/go |
|---|---|---|---|---|---|
| — | — | — | — | — | — |
