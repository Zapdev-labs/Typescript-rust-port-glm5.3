# Porting Notes

Non-obvious deviations from the Go port forced by Rust semantics. Each entry:
file(s), the Go pattern, the Rust substitution, and why it is behavior-preserving.

Format: `## <date> <rust file> — <Go source>`

## 2026-10-07 rust/Cargo.toml — dependency additions beyond SPEC §5.11
- `serde`/`serde_json`: sanctioned by §5.11 for tsconfig/api; also used as the
  substrate of `tsc-json` (Go's `json` package is a thin facade over
  `encoding/json/v2`, ~100 LOC — not a JSONC parser. The spec's "~800 LOC
  JSONC-ish parser" description is stale; tsconfig quirks live in tsoptions).
- `num-bigint`: Go uses `math/big` in `jsnum` (PseudoBigInt + huge-int parse).
  Rust std has no bigint; num-bigint has matching truncated div/rem semantics.

## 2026-10-07 local `cargo check`/`test` by porting agents — SPEC §9.2
SPEC §9.2 says no builds/tests run on the workstation because the machine has
no toolchain "by design". A toolchain (cargo 1.97.1) is in fact installed and
IDE-resident. Agents use local `cargo test`/`clippy` with isolated
`CARGO_TARGET_DIR=/tmp/tsrs-check-*` purely as an iteration aid. The
authoritative gate remains `remote-test.sh` on dih@192.168.1.15; nothing
local-only counts toward a milestone gate.

## 2026-10-07 crates/collections — OrderedMap JSON marshalling deferred
Go `OrderedMap.MarshalJSONTo`/`UnmarshalJSONFrom` (json.MarshalerTo impls)
deferred behind `// TODO(port)` until `tsc-json` exists; serde
Serialize/Deserialize impls provided in the interim.

## 2026-10-07 SPEC corrections discovered while porting (Go is truth)
- `json/` is a ~100-LOC facade over encoding/json/v2, not a JSONC parser.
- `contentmapper` imports ast/parser/ipc/jsonrpc — cannot be a Layer-1 port;
  deferred until those crates exist (post-M3).
- `symlinks` imports ast/module — deferred likewise.
- `sourcemap` imports scanner — deferred to M3+ (spec said "emit side first";
  the emit path still pulls scanner types).
- `vfs` imports `osutil` — ported as `tsc-osutil` in M1 despite being listed
  in Layer 6.


## 2026-10-07 rust/Cargo.toml — rustc-hash pin "3" → "2"
The workspace pinned `rustc-hash = "3"` but no 3.x is resolvable from
crates.io (the 3.0.x line was yanked upstream; index max is 2.1.3). Pinned
to "2" so tsc-collections can build.

## 2026-10-07 crates/collections — M1 port shape
- `SyncMap`/`SyncSet` over `RwLock<FxHashMap>`: `load`/`load_or_store`/`range`/
  `to_map`/`keys`/`clone` return owned clones (K/V: Clone) since references
  can't outlive the lock. `range` iterates a cloned snapshot — sync.Map::Range
  does the same via its readOnly snapshot — so callbacks may safely call back
  into the map. Iteration order unspecified, same as Go.
- `CopyOnWriteMap`/`CopyOnWriteSet`: `Rc<FxHashMap>` + `Rc::make_mut` replaces
  Go's `owned` flag + maps.Clone; `enter_scope` returns `impl FnOnce(&mut Self)`
  (call `restore(&mut c)` — Rust closures can't capture `&mut self`).
- Nil-able `*Set`/`*OrderedMap` receiver+arg pairs are `Option<&T>` associated
  fns: `Set::{equals,is_subset_of,intersects,unioned_with}`,
  `OrderedMap::equal_func`, `diff_ordered_maps{,_func}`.
- `MultiMap` is `IndexMap`-backed: Go's map iteration is randomized, so
  first-seen key order is a deterministic superset of the Go contract.
- `Set`/`MultiMap` keep their public `m` field (Go `M`); `Set.keys()` returns
  `&FxHashSet<T>` mirroring Go returning the map itself.

## 2026-10-07 crates/{tspath,jsnum} — two fn visibility relaxations for bench-harness
Go benchmarks `hasRelativePathSegment` (tspath) and `Number.toInt32` (jsnum)
in-package (both unexported). The Rust bench driver (`tsc-bench`, Phase 0
micro-race, SPEC.md §15.1) lives in a separate crate, so both fns are `pub`
instead of `pub(crate)`. No behavior change; recorded per §4.4.

## 2026-10-07 bench-harness crate + rust/bench/go-driver — new (SPEC §15)
`tsc-bench` (rust/crates/bench-harness) is a NEW crate with no Go counterpart:
it exists to race ported Rust code against the Go originals using identical
inputs. The Go driver (`rust/bench/go-driver`) is a new Go module added to the
root `go.work` (fork-level config, one line); it imports `tsc/internal/...`
but never modifies them.

## 2026-10-07 tspath — `as_path()` clone is a Go-string-semantics divergence (measured)
Go strings are (ptr, len) headers: `RootedFilePath.AsPath()` copies 16 bytes,
zero heap traffic. The Rust port backs these types with `String`, so the
borrowed `as_path(&self) -> RootedPath` clones the bytes. Owned conversions
(`From<RootedFilePath> for RootedPath`) already move and are zero-copy.
Phase 0 measured the cost: `RootedFilePathToPathKey/AlreadyRooted` runs at
0.83x vs Go (the worst tspath ratio; siblings are 0.52-0.64x) because the
per-call `as_path()` alloc dominates the tiny op.
NOT fixed now: a `repr(transparent)` pointer transmute would make the borrow
conversion zero-cost but introduces unsafe into a green M1 crate mid-flight.
Revisit if Phase B program-level profiling shows `path_key` on a hot path —
then prefer restructuring the bench/real callers to use owned conversions or
a view type over unsafe punning. Raw outputs: ~/bench-results/phase0 (rig).

## 2026-10-07 tools/gen-options + crates/core — options_generated.go/compileroptions.go port
- `gen-options` mirrors what `generate-options.ts` emits into
  `core/options_generated.go` from a hand-mirrored data file
  (`tools/gen-options/data/options-model.json`, subset of
  `tools/scripts/tsc/options.ts`; provenance + re-verify steps in
  `data/README.md`). Go output is never an input (SPEC §6); it is the parity
  baseline for the generated tests.
- Go slice nil-vs-empty collapses: option list fields are plain `Vec`s, so
  `Equals`'s `(a == nil) != (b == nil) || !slices.Equal(a, b)` reduces to
  `a != b`, and `GetEffectiveTypeRoots` treats an explicitly empty
  `typeRoots` as unset. `*int` -> `Option<i64>` (Go int is 64-bit here);
  `Paths` keeps nil-ness exactly via `Option<OrderedMap>`.
- `ScriptTargetLatest`/`LatestStandard` (same-discriminant Go value aliases)
  become associated consts (`ScriptTarget::Latest`), `#[allow]`ing
  `non_upper_case_globals` for Go-parity names; likewise the free
  `ResolutionMode*` consts in compileroptions.rs.
- Go's panicking `String()` stringers (ModuleResolutionKind, JsxEmit) become
  `Display` impls that panic on the zero value identically; `from_i32`
  replaces Go's untyped-int round-trip.
- `ModuleKindToModuleResolutionKind` map -> `Option`-returning fn (Go map
  lookup on a missing key yields the zero value, which no caller relies on).
- `EmptyCompilerOptions` (shared zero pointer) -> `empty_compiler_options()`
  returning a fresh default (not `Sync`, no static).
- `noCopy` embed and json tags dropped (no Rust counterpart).
- Derived `PartialOrd`/`Ord` on the option enums equal the numeric order
  (generator-validated: member values monotonic in declaration order), so
  Go's int32 range checks port directly to variant comparisons.
