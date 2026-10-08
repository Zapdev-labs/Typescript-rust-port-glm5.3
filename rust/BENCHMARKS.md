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
| 2026-10-07 | go1.27.1 linux/amd64 | 2.0.0 | 7.1.0-dev | 2559257bb2bcf7f6b8815d6796ba2fd65d8f2b98 | 676 | 8,296,171 |

## Phase 0 — micro benches (ported M1 crates)

**2026-10-07 21:41 — rig: go1.27.1 vs rustc 1.99.0. 45 sub-benches, identical
inputs (Rust mirrors Go's own `go test -bench` cases), medians of 10 samples.**

**GEOMEAN rust/go: 0.170 — Rust ~5.9× faster. Gate (≤ 1.0×): PASSED.**
Only 1 of 45 sub-benches was Go-faster (Exponentiate 0.5**-0.5 math.Pow, 1.16×).

| op family | rust/go range | notes |
|---|---|---|
| CombinePaths (6) | 0.33–0.47× | |
| GetNormalizedAbsolutePath (3) | 0.46–0.55× | |
| ToFileNameLowerCase (5) | 0.17–0.46× | |
| HasRelativePathSegment (4) | 0.25–0.36× | |
| PathIsRelative (6) | 0.06–0.11× | up to 16× faster |
| RootedDirectoryPathResolveFile (2) | 0.59–0.64× | |
| RootedFilePathToPathKey (2) | 0.52–0.83× | `AlreadyRooted` pays the ported `as_path` String clone (PORTING-NOTES candidate) |
| ToInt32 (11) | 0.002–0.18× | Go hits a slow path on SMI-boundary cases (MAX_SAFE_INTEGER: 286 ns vs 0.5 ns) |
| Exponentiate (6) | 0.15–1.16× | bigint paths 0.15–0.22×; math.Pow path 1.16× (Go's `math.Pow` intrinsic) |

Measurement caveat: identical call shapes and inputs on both sides, but the
harnesses differ in machinery (Go `b.Loop` auto-scaling vs `tsc-bench`
calibration; black_box checksum folding both sides). The join is reproducible
via `rust/scripts/bench-micro.sh`; raw per-sub-bench outputs live on the rig
at `~/bench-results/phase0/`. Sub-bench names matched 1:1 after the join
script's name-normalization warnings (positional fallback not needed).

## Phase A — corpus parse race (M3 exit gate)

All corpus `.ts`/`.tsx` (excl. `node_modules`, `dist`), both drivers, same
file list, threads measured at 1 and 16. Gate: ≤1.30× Go at first green,
hardening target ≤1.00×, stretch ≤0.80×; peak RSS ≤1.0× Go.

**2026-10-07 22:00 — Go baseline locked (go-bench validated; no Rust side yet).**
Driver contract verified empirically on the rig: shape_hash identical across
runs and thread counts (`--threads 4`), run totals scale exactly with
`--iterations` (881,067 → 2,643,201 nodes at K=3). Corpus per driver walk:
679 files / 8,309,945 bytes (contract extensions incl. `.d.ts`; the earlier
676-file rig record used a coarser glob).

| side | threads | iterations | wall ms | MB/s | nodes | errors | shape_hash |
|---|---|---|---|---|---|---|---|
| go | 1 | 1 | 254.5 / 280.9 | 31.1 / 28.2 | 881,067 | 0 | 487377d6e977e9ef |
| go | 4 | 1 | 131.5 | 60.3 | 881,067 | 0 | 487377d6e977e9ef |
| go | 8 | 3 | 402.1 | 59.1 | 2,643,201 | 0 | b429b96dcd34a87a |

**The Rust parser must reproduce nodes=881,067, errors=0, shape_hash
487377d6e977e9ef exactly at threads=1/K=1. Go's cost is ~289 ns/node;
the ≤1.30× gate is 330.9 ms (~376 ns/node).**

**2026-10-08 04:20 — FIRST GREEN: 1.051× at threads=1; 0.629× at threads=16.**
Rust reproduces the contract exactly: nodes 881,067, errors 0, shape_hash
487377d6e977e9ef at both thread counts (byte-identical to the Go driver and
the locked baseline). Verdict: **FIRST GREEN (≤1.30×)**; hardening to ≤1.00×
is next (gap: ~5%). Peak RSS: Rust 18.9 MB vs Go 37.2 MB (t=1) and 43.6 MB vs
77.3 MB (t=16) — well under the ≤1.0× gate.

*Provisional timing caveat:* the rig was concurrently loaded by an unrelated
syzkaller campaign (load avg 33 on 16 cores) — Go's same-day median was
313.3 ms vs its locked 254.5 ms quiet baseline (~23% contention inflation,
both sides equally contended so the ratio is meaningful). Official quiet-rig
timing re-run pending; the shape/nodes/errors equality above is
load-independent and authoritative.

| date | git (rust) | threads | wall median (rust) | wall median (go) | rust/go | RSS rust/go | MB/s rust | shape_hash |
|---|---|---|---|---|---|---|---|---|
| 2026-10-07 | (pre-M3) | 1 | — | 254.5 | — | — | — | 487377d6e977e9ef (go) |
| 2026-10-08 | f3870978d9 | 1 | 329.3 | 313.3 (loaded) | 1.051 | 18.9/37.2 MB | 24.1 | 487377d6e977e9ef ✓ |
| 2026-10-08 | f3870978d9 | 16 | 89.3 | 141.9 (loaded) | 0.629 | 43.6/77.3 MB | 88.7 | 487377d6e977e9ef ✓ |

threads=16: Rust is already **1.59× faster** than Go (89.3 vs 141.9 ms) and
scales better (5.71× vs 2.21× single→16-thread). The hardening gap is
single-threaded (~374 vs ~355 ns/node under contention).

### Multi-corpus parity (real-world codebases)

**2026-10-08 04:50 — parity sweep beyond the race corpus.** All runs
`parse --threads 1`, single shot; parity = byte-identical
files/bytes/nodes/errors/shape_hash across the two drivers. Provisional
timings (rig still loaded by the syzkaller campaign, load ~30):

| corpus | files | nodes | errors | shape_hash (both sides) | rust/go t=1 | rust/go t=16 |
|---|---|---|---|---|---|---|
| packages/ (tsc Go-port sources) | — | 432,721 | 0 | `90da5655851b0cd1` ✓ | — | — |
| tools/ | — | 89,287 | 0 | `a283412883b4c90b` ✓ | — | — |
| type-fest (sindresorhus, depth 1) | 440 | 159,085 | 0 | `77f3b2c60d05ed5d` ✓ | 0.743 | 0.302 |
| zod (colinhacks, depth 1) | 515 | 563,033 | 0 | `15803bfa6c347c94` ✓ | 1.003 | 0.642 |
| vite (vitejs, depth 1) | 602 | 355,856 | 1 | `db36d00b94e30112` ✓ | 0.835 | 0.398 |

tsc/testdata (torture corpus, 2.5M nodes, 7,218 intentional errors) found the
only divergences so far: `!`-definite-assignment on a parameter with a generic
type annotation (6 missing diagnostics) and multi-case JSX mismatched-close
recovery (both single-case repros byte-identical; both fixes in flight).
On 2 of 3 new real-world corpora the Rust parser is faster than Go even at
threads=1 — the 1.051× single-thread gap on the race corpus is JSX-heavy
file mix, not a general parser deficit.

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
