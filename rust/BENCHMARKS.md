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

**2026-10-10 17:42 — OFFICIAL QUIET-RIG RUN (the syzkaller campaign ended;
load avg 1.57 on 16 cores). `bench-parse.sh --samples 10`, both drivers
release-built from the same tree (rust 89308a375b), medians of 10:**

| date | git (rust) | threads | wall median (rust) | wall median (go) | rust/go | RSS rust/go | MB/s rust | shape_hash |
|---|---|---|---|---|---|---|---|---|
| 2026-10-10 | 89308a375b | 1 | 174.3 | 139.9 | **1.246** | 20.1/40.3 MB | 45.5 | 487377d6e977e9ef ✓ MATCH |
| 2026-10-10 | 89308a375b | 16 | 28.6 | 33.4 | **0.855** | 43.8/79.7 MB | 277.5 | 487377d6e977e9ef ✓ MATCH |

The official verdict **supersedes the loaded-run numbers**: contention had
flattered the ratio — quiet Go is much faster single-threaded (139.9 ms vs
313.3 loaded / 254.5 the day-one lock; the box's day-one state likely also
carried background load). The honest state: **FIRST GREEN (1.246× ≤ 1.30×)
at t=1; Rust 1.17× FASTER at t=16 (0.855×); peak RSS HALF of Go's at both
thread counts (20 vs 40 MB, 44 vs 80 MB)**. The hardening target ≤1.00× at
t=1 has a real ~25% single-thread gap on this JSX-heavy corpus (≈198 vs
159 ns/node) — the multi-corpus numbers below (type-fest 0.743×, vite
0.835×, zod 1.003×) were also loaded-rig provisionals and need quiet re-
measurement before being quoted as final. Raw outputs: rig
`~/bench-results/phaseA/`.

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

**2026-10-10 18:10 — QUIET-RIG RE-MEASURE (the provisional caveat above
cleared).** Same drivers as the official Phase A run (89308a375b, load
avg 0.7-1.6), parity re-verified (shape hashes identical on both sides),
medians of 5:

| corpus | rust/go t=1 | rust/go t=16 |
|---|---|---|
| type-fest | **1.000** (31.46 vs 31.47 ms) | **0.483** (5.83 vs 12.08 ms, Rust 2.07× faster) |
| zod | 1.180 (104.3 vs 88.3 ms) | **0.765** (18.2 vs 23.8 ms) |
| vite | 1.117 (70.7 vs 63.3 ms) | **0.593** (10.6 vs 17.9 ms) |

The loaded-run ratios were contention-flattered. The honest quiet picture
on real-world code: single-threaded Rust is dead-even on type-fest and
11-18% behind on the zod/vite mixes; multi-threaded Rust is 1.3-2.1×
FASTER across the board. The single-thread hardening gap (~12-25%) is
concentrated in the d.ts-heavy/JSX-heavy mixes — same profile as the race
corpus (1.246× t=1).

tsc/testdata (torture corpus, 12,957 files, 2,584,705 nodes, 7,218 intentional
errors) — **PARITY GREEN** (2026-10-08 05:10, shape `b2b6a3ebc8e90d02` on both
sides) after the two parser fixes (26d303a562): `!`-definite-assignment on a
parameter with a generic type annotation (6 diagnostics lost to a
`mem::take` in `finish_source_file` — `reparseTopLevelAwait`'s replay found an
empty sink) and multi-case JSX mismatched-close recovery (predicates compared
the `JsxClosingElement` wrapper instead of `ClosingElement.TagName()`). The
corpus previously diverged 6 nodes / 4 errors. One known residual:
`compiler/octalLiteralAndEscapeSequence.ts` escape-slicing diagnostic
(`\x27` vs `\x13f`, range 420-423 vs 420-424) — scanner-crate, invisible to
the aggregate gate; fixed (Go's octal-escape ladder gives first digits 4-7
only one further digit — `\47`, never `\477` — since 0o4xx ≥ 256).
On 2 of 3 new real-world corpora the Rust parser is faster than Go even at
threads=1 — the 1.051× single-thread gap on the race corpus is JSX-heavy
file mix, not a general parser deficit.

## Phase B — program race (M4/M5 gates)

Corpus bind bench (mirrors `BenchmarkBind`), program build (mirrors
`BenchmarkNewProgram`), `--showConfig` byte-parity vs `tsgo-go`.

### M4 parity gates — showconfig + resolvereact twins (2026-10-08 06:30)

Both drivers grew two twin subcommands (`rust/bench/gates/` fixture
matrices, commit 10254560d3): `showconfig` (tsconfig → ParseConfigFileTextToJson
→ ParseJsonConfigFileContent with extends → ConvertToTSConfig →
MarshalIndentWrite, one NDJSON line per config) and `resolvereact`
(project dirs × fixed 21-specifier list → NewResolver + ResolveModuleName
ESM, per-record lines + FNV-1a shape-hash summary). Verdict = plain diff.

| run | verdict |
|---|---|
| showconfig × tsconfig-matrix (18 cases: extends 2/3-deep + cycle + array, configDir templates, JSONC, unknown options) | IDENTICAL (local + rig) |
| showconfig × resolver-matrix | IDENTICAL (local + rig) |
| resolvereact × resolver-matrix (exports/imports self-reference, scoped, index, node16) | IDENTICAL, 43 lines (local + rig) |
| resolvereact × tsconfig-matrix | IDENTICAL, 379 lines (local + rig) |
| showconfig × packages/typescript (real-world extends+nodenext) | IDENTICAL |
| showconfig × excalidraw tsconfig (real-world) | IDENTICAL (rig) |
| determinism double-runs | byte-identical |

One real divergence was found by the twins and fixed the same day (`bdd088c5db`):
`tsc/testdata/fixtures/compiler/tsconfig.json`'s `"types": []` — Go emits the
explicitly-empty list; the port dropped it (core's CompilerOptions collapsed
Go's nil-vs-empty slices into `Vec`). Fixed with an `Option<Vec<_>>` shape
regenerated through gen-options for all 9 list fields; the whole-repo
showconfig sweep (28 configs) now has ZERO divergences, and every twin run
is byte-identical on the rig with the fix.

Rig environment note: the rig's btrfs is fully allocated (metadata chunks
exhaust under the concurrent syzkaller campaign — rsync renames and cargo
target-dir creates fail ENOSPC while data writes fit). Workarounds in use:
`TS_RSYNC_INPLACE=1` (rename-free sync) and a tmpfs `CARGO_TARGET_DIR`
for rig builds. A root `btrfs balance` would fix it properly.

### M4-final + M5 parity gates — symbols and program twins (2026-10-08 10:20)

The `symbols` twin (M4 wave E, `ef81ae3778`): canonical pre-order AST walk
(Locals → node.Symbol → LocalSymbol → children, Members then Exports,
GlobalExports; keys sorted by `EscapeInternalSymbolName`) — one NDJSON line
per file + FNV-1a shape hash. Found two real binder bugs before going green
(`declareSymbolEx` nil-Type panic path, 40 files; `jsx_namespaced_name_text`
missing arm, 3 files) and closed the M4 gate: conformance sweep
`tsc/testdata/tests/cases/conformance` (5,935 files / 19MB) IDENTICAL
local + rig.

The `program` twin (M5 gate, `d0572578c7`): one-file programs per corpus
root over the bundled `lib.d.ts` VFS — per-root
`config_errors/loaded/parsed/bound/symbols/duplicates/parse_diag/
program_errors/unresolved` + FNV-1a shape hash (field order = the contract;
fold recipe documented + FNV-vectored on both drivers). Found two real
compiler-crate divergences before going green: `get_unresolved_imports` was
guarded by a never-set flag (Go never sets `usesUriStyleNodeCoreModules`, so
the sweep was dead — the missing-import fixture caught it) and
`get_program_diagnostics` folded the include processor's full collection
where Go folds `GetGlobalDiagnostics()` only.

| run | verdict |
|---|---|
| program × program-matrix (15 roots: parse errors, in-matrix import, `/// <reference lib>` pull, missing bare import, UMD global, JSON root, JSDoc typedefs, JSX, `.d.ts`-only, jsconfig companion) | IDENTICAL (local + rig) |
| program determinism double-runs | byte-identical (local + rig) |
| program × packages/typescript/lib (5 roots: .js/.cjs/.d.ts/.d.cts) | IDENTICAL |
| program × packages/typescript/scripts (2 real .ts roots, one with an in-dir import) | IDENTICAL (local + rig) |
| symbols × fixtures + fuzz seeds | IDENTICAL (local + rig) |
| symbols × conformance corpus (5,935 files) | IDENTICAL (local + rig) |
| dump/showconfig/resolvereact regressions | IDENTICAL |

M5 waves landed and rig-validated before the gate: compiler + bundled +
outputpaths (`c692d74345`, incl. the fully ported `test_program` asserting
exact program order vs the 96-entry esnext lib list) and the checker spike
(`fc21c66df9`, SPEC §5.4/§5.5 strategies proven). Full Phase B bind/program
TIMING stays queued behind the rig's syzkaller campaign (load ~33 on
16 cores); the program twin is the parity rail M6's checker-level fields
extend as waves land.

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

## Phase B — program race (first timing entries; 2026-10-09, workstation)

Environment NOTE: these are workstation measurements (16-core, load avg ~2),
not the quiet-rig protocol — the rig entries remain the official record;
these land the first Phase B numbers + the two perf/correctness findings
that came out of the race.

**Corpus: `packages/typescript/src` (125 roots, 2.2 MB, one program per
root, target esnext — 96-entry default lib set parsed per program).**

| date | git (rust) | phase | rust | go | rust/go |
|---|---|---|---|---|---|
| 2026-10-09 | (pre) sequential binds | program+bind | 7.47 s | 3.43 s | 2.18× |
| 2026-10-09 | parallel binds + collectFiles fix | program+bind | 6.50 s | 3.44 s | 1.89× |

**2026-10-10 18:13 — RIG-OFFICIAL RUN (quiet rig, same drivers as the
official Phase A run, 89308a375b; medians of 5, `program --corpus
packages/typescript/src`):**

| date | git (rust) | phase | rust | go | rust/go | peak RSS rust/go |
|---|---|---|---|---|---|---|
| 2026-10-10 | 89308a375b | program+bind | 5.562 s | 3.325 s | **1.673×** | 357 / 125 MB (**2.85×**) |

The program twin is byte-identical on the rig (TWIN-IDENTICAL, cmp of the
per-root NDJSON). The rig number (1.673×) is better than the workstation's
1.89× — the same parallel-binds + collectFiles tree. **RSS finding: the
program-build phase retains 2.85× Go's memory (357 vs 125 MB peak)** — the
opposite of the parse phase (Rust at HALF of Go there), pointing at the
program/bind arenas (per-program node/string storage retained across the
125 roots; Go frees per program via GC). An RSS-hardening lever for the
next pass: release per-root arenas before the next root (Rust holds all
125 programs' retained structures; Go's GC reclaims between roots).

Phase split (env `TSC_BENCH_PROFILE=1`, both drivers instrumented
identically, medians of repeated runs): seed 52 ms / config 6 ms /
program 3121 ms / bind 1900 ms / record 20 ms (rust) vs seed 112 ms /
config 7 ms / program 2202 ms / bind 1134 ms / record ~0 (go). Rust's map
FS seeding is FASTER than Go's (Go sorts FromMap keys; 0.53×).

**Finding 1 — the program twin was NONDETERMINISTIC (a pre-existing
correctness bug, fixed): `collectFiles` did not follow Go's
`if task.loadedTask != nil { task = task.loadedTask }` reassignment, so a
file reachable from multiple import sites claimed at whichever parent's
task object won the PARALLEL LOAD RACE — the program order (and the
twin's `program_files` fold) varied run to run AND diverged from Go on
this corpus (5 runs, 5 outputs; the 125-record twin never matched). With
the follow (filesparser.rs), three consecutive runs are byte-identical to
the Go driver. The 300-slice/program-matrix/conformance-matrix/symbols
twins were insensitive (verified all green after the fix; the symbols
twin — 5,935 conformance files — additionally proves the PARALLEL BINDS
are output-identical).**

**Finding 2 — parallel `BindSourceFiles`** (Go's `core.NewWorkGroup`):
scoped threads over text-weighted contiguous chunks (equal-count chunks
straggle on the big lib `.d.ts` files). Bind: 3108 ms (serial) → 1900 ms
(parallel). The speedup plateaus ~1.6× on 16 cores (4→8→16 CPUs nearly
identical — the bind is allocation/pointer-chasing heavy, not lock-
serialized; LD_PRELOAD tcmalloc improves BOTH phases ~15-20% (program
3445→2762 ms, bind 1870→1687 ms) — the allocator is a real lever for the
next hardening pass (Go ships a tcmalloc-class allocator; the Rust
binary links plain glibc malloc). Remaining program-phase gap (1.42×) is
parse-dominated (the parse race on this corpus: 1.18× t=1).

Next perf leads: (1) the allocator (mimalloc as the binary's
GlobalAlloc — a bench-harness-only dependency, the port crates stay
allocator-agnostic), (2) `as_path()` String clones on the path-key hot
paths (M1's 0.83× RootedFilePathToPathKey note), (3) the text
double-copy (vfs read_file's owned String + the parser's stored copy —
Go shares immutable strings).

**2026-10-10 18:19 — ALLOCATOR EXPERIMENT (perf lead 1, landed as an
optional bench-harness feature).** `--features mimalloc` swaps the bench
binary's global allocator (mimalloc 0.1, `#[global_allocator]` gated in
main.rs; the port crates stay allocator-agnostic; default builds use the
system allocator untouched). Output is byte-identical to the default
build (cmp of the per-root NDJSON, local + rig).

| variant | rig wall median | rig peak RSS | vs go (3.325 s / 125 MB) |
|---|---|---|---|
| default (glibc) | 5.562 s | 357 MB | 1.673× / 2.85× |
| mimalloc | **4.696 s** | 299 MB | **1.412×** / 2.39× |

Medians of 5, quiet rig (load ~1.1), same tree. Wall 0.844× of the
default build (−15.6%); the workstation A/B was even stronger (7.00 →
5.50 s, 0.786×, RSS 647 → 264 MB). The parse phase is allocator-neutral
(micro-runs are a wash; mimalloc reserves arenas so tiny-run RSS triples
— irrelevant at scale). Decision pending: whether official Phase B runs
enable the feature (Go ships a tcmalloc-class allocator, so an allocator
choice on the Rust side is legitimate engineering, but it must be
declared in the record either way). The remaining ~1.41× is
parse-dominated per the phase split (program 3.1 vs 2.2 s, bind 1.9 vs
1.1 s).

**2026-10-10 18:30 — PERF LEAD 2 (as_path/path-key clones): landed, NO
measurable win — lead exhausted.** Removed the String clones from the
CaseSensitivity hot wrappers (compare_file_paths cloned both args,
contains_file_path cloned the child; path_key round-tripped through an
owned RootedPath) and added path_key_str; seven program/module call
sites swapped. Output byte-identical, tspath+compiler tests green — but
the program race is unchanged (local A/B: 7.24 vs 7.00 s median, within
noise). The M1 0.83× RootedFilePathToPathKey note does not reproduce at
the program-build scale: these paths are NOT the hot allocation site.
The remaining single-thread gap lives in the parse/scan phase itself
(and the Phase B split: program 3.1 vs 2.2 s, bind 1.9 vs 1.1 s).
