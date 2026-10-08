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

## 2026-10-07 crates/spanmap + crates/vfs — internal/spanmap + internal/vfs port
- `internal/spanmap` -> `tsc-spanmap` (one file pair, lib + tests): Go's
  exported-but-out-of-range `Kind`/`Feature` values must survive round trips
  for `Validate`, so both are i32 newtypes over the Go consts (not enums);
  only `Fidelity` is a closed enum. Nil `*SpanMap` receivers (identity
  mapping) are `impl SpanMapRef for Option<&SpanMap>`; `sync.Once` ->
  `OnceLock`. Marshal/unmarshal go through the `tsc-json` facade.
- `internal/vfs` -> `tsc-vfs` as one crate with modules mirroring the Go
  package tree (`sysfs`, `vfs`, `walkdir`, `internal` (pub(crate)),
  `iovfs`, `osvfs`, `vfstest`, `wrapvfs`); subpackages `cachedvfs`,
  `trackingvfs`, `vfsmatch`, `vfsmock` are not ported (no ported package
  imports them).
- Trait design: this Go revision's `FS` is flat (no RealFS/SortedReadDirFS
  composition here — that text in the task brief corresponds to a different
  upstream revision), ported as one object-safe `Fs` trait (`Send + Sync`,
  `Arc<dyn Fs>` values). The Go io/fs substrate becomes `sysfs::SubFs`
  (`stat`/`read_file`/`read_dir`, "." is the sub-root); the iovfs
  capability interfaces (`RealpathFS`/`WritableFS`, detected by Go type
  assertion) become default methods on `SubFs` — defaults reproduce what
  `iovfs.From` does when the wrapped fs.FS lacks the interface (identity
  realpath, failing writes), so trait-object dynamic dispatch replaces the
  type assertion. `fs.Sub` -> `sysfs::PrefixSubFs`.
- Go stdlib doubles: `fstest.MapFS` semantics are folded into
  `vfstest::MapFS` (the Go package embeds it anyway); `fstest.TestFS`
  consistency checks and the Go test `assert`/`vfsmock` infra are dropped
  with PORT comments at each test site. `fs.SkipDir`/`fs.SkipAll` ->
  `WalkDirControl` enum; the three-channel Go walkFunc error flow ->
  `WalkAbort` enum.
- Divergences: `internal.read_file` returns `None` (not a lossy string) for
  non-UTF-8 bytes after UTF-8 BOM handling; Go `ReadFile` would produce an
  invalid string. Error sentinels are `io::Error` constructors with the
  exact Go message strings. `File::set_times` (futimens) replaces Go
  `os.Chtimes`; `MapFS` mod times are `SystemTime` (zero -> `UNIX_EPOCH`).
- Validation: `cargo test -p tsc-spanmap -p tsc-vfs` 67/67 green
  (32 spanmap + 35 vfs), clippy `--all-targets` clean for both crates
  (pre-existing tsc-tspath warnings untouched), `cargo check --workspace
  --exclude tsc-ast` green.

## 2026-10-07 crates/ast + tools/gen-ast — M2 hand-written AST surface
- NodeData boxing decision (SPEC §5.1/§12): EVERY `NodeData` variant is
  boxed (`NodeData::X(Box<X>)`), giving a fixed 48-byte `Node` slot
  (kind/flags/loc/id/parent/data). Go compares one interface word + one
  pointer of indirection, so one box is Go parity; the spec's 64–96B window
  is met without per-variant enum-size tuning. Locked by the
  `node_size_is_48_bytes` test.
- Go `Type any` members (SyntheticExpression.Type, SyntheticReferenceExpression
  .Type): `Arc<dyn Any + Send + Sync>` storage, so all 191 node structs
  derive `Clone` uniformly (Go copies the interface value; the Arc shares
  the payload). An earlier `Box<dyn Any>` + per-struct manual Clone (fresh
  `()` payload) was replaced — the manual impl reset the payload on every
  clone, which diverged from Go.
- `NodeStore` seam (visitor.rs): resolution + allocation over the
  per-`SourceFile` arena (`node`/`node_mut`/`file`/`alloc`, plus
  `clone_node`/`new_block`/`new_modifier_list` covering the Go factory
  surface the visitor needs). Go's `*Node` becomes `NodeId` (packed
  `file_id:20 | local_index:44`, `NONE = u64::MAX`). SourceFile implements
  it; the M4 program-level file table implements the cross-file case.
- `NodeVisitor` is a trait, not a struct-of-closures: Go's
  `getDeepCloneVisitor` builds a closure that captures the visitor itself
  (illegal in Rust struct-literal form). `Visit` + `store()` are required;
  the `NodeVisitorHooks` function fields become overridable `hook_*`
  default methods that route to the `*_core` methods (Go's exported
  VisitNode/VisitNodes/... algorithms). `VisitEachChild` snapshots the node
  out of the arena before the walk (a growing `Vec` would invalidate
  borrows; Go's GC tolerates the aliasing) and returns `None` for
  "unchanged" (Go returns the same `*Node`).
- SourceFile split (source_file.rs): Go's single `*SourceFile` struct
  becomes `SourceFileNodeData` (the `NodeData` payload — everything reachable
  via `node.AsSourceFile()`: statements, parser/binder fields, diagnostics,
  indicator `Cell`s) + `SourceFile` (the arena owner: `Vec<Node>` with the
  file node at slot 0, text, parse options, the `OnceLock` caches, and the
  cross-package `GetOrComputeData` cell map). The payload cannot contain
  the arena that stores the payload.
- `SourceFileNodeData::visit_each_child` mirrors Go's
  `UpdateSourceFile`+`copyFrom` exactly: only the copyFrom-copied fields
  carry over; binder results, counts, and diagnostics reset to NewSourceFile
  zero values (Go deliberately skips them).
- ComputeECMALineStarts is local to source_file.rs (core.go is not yet
  ported as a crate); dedup into the core.go port when it lands.
  `ComputePositionMap` takes `&[u8]`, not `&str`: the scanner's
  lone-surrogate sentinel encoding (stringutil.EncodeJSStringRune, covered
  by the ported TestPositionMapLoneSurrogateSentinel) is invalid UTF-8.
- for_each_child audit (task item 5): the only ast.json member-order
  divergence from Go's generated visitors is JSDocParameterOrPropertyTag —
  Go's hand-written `visitEachChild_JSDocParameterOrPropertyTag` orders
  name/typeExpression by `IsNameFirst` and visitEachChild puts name before
  comment; the generator now emits both literally (conditional
  `is_name_first` order in `for_each_child`; Go's fixed order in
  `visit_each_child`). A second divergence found during the audit:
  `FunctionLikeWithBodyBase.Body` was skipped by the generator (shadowed by
  the kind-param `Body` field), so `Body` never routed through
  `visit_function_body` — fixed (the skip remains for struct fields only).
  Spot-checked ~20 other kinds against Go: member order matches.
- Deferrals (each with a TODO marker at the site): `GetExternalModuleIndicator
  Options`/`isFileForcedToBeModuleByFormat` (needs core.CompilerOptions +
  ModuleDetectionKind), content-mapper surface (ContentMapperSourceFileInfo/
  SpanMap/OriginalText/MappedDiagnosticDirective — needs tsc-spanmap wiring),
  `GetOrCreateToken`/`createToken` + tokenCache (needs NodeFactory),
  `GetNameTable`/`GetDeclarationMap` (deep utilities.go chains),
  `resolveJSDoc` (parser-owned `parseJSDocForNode` registration hook),
  `ReparsedClones`/`Hash`/language-service caches, NodeFactory itself,
  utilities.go wholesale (utilities.rs carries the minimal subset the M2
  surface needs: SetParentInChildren, findChildNode, IsImportMeta,
  TryGetAmbientModuleNameFromSymbolName, IsOptionalChain, IsJsxOpening
  LikeElement, GetSourceFileOfNode), subtreefacts.go (the JSX-tag walk in
  parseoptions.rs therefore walks the whole tree — identical results, no
  SubtreeFacts early-out), and flow.go (FlowNodeId is a placeholder handle).
- Diagnostic (diagnostic.rs) is the minimal struct port (accessors, setter
  chain methods) with `file: Option<NodeId>` handles; the full
  diagnostic.go surface (Localize/displayMessageArgs/message chains) is a
  separate task. `Symbol` (symbol.rs) keeps Go's field set with `SymbolId`
  back-references; `SymbolTable = OrderedMap<String, SymbolId>`
  (insertion-ordered per SPEC §5.2; keys stay `String` until the Atom
  interner decision). `GetSourceFileOfSymbol` takes the program's symbol
  slice to walk `symbol.Parent` (Go dereferences `*Symbol`).
- Flags files carry the exact Go bit values (spot-check tests) and are
  textually-scoped `define_flags!` newtypes (no bitflags crate). Composite
  consts reference siblings via `Self::`. `GetFunctionFlags` matches Go's
  fallthrough semantics per kind.
- Validation: `CARGO_TARGET_DIR=/tmp/tsrs-ast2 cargo test -p tsc-ast` —
  48/48 green (kind table, flags bit values, NodeId packing, PositionMap
  suite incl. the lone-surrogate sentinel, ECMALineStarts, SourceFile
  arena/lazy caches/bind-once, visitor core (SyntaxList lifting, prefix
  rebuild, drop/replace), deep-clone contracts, precedence tables);
  `cargo check --workspace` green; clippy `-p tsc-ast --all-targets` and
  `-p gen-ast` clean (pre-existing tsc-tspath warnings untouched);
  `gen-ast --check` confirms the committed generated files are current
  (kind_generated.rs 1081 lines, ast_generated.rs 12417 lines).

## 2026-10-07 crates/packagejson — internal/packagejson port (full)
- Go struct embedding (Fields ⊃ Header/Path/DependencyFields, PackageJson ⊃
  Fields, ExportsOrImports ⊃ JSONValue) is nested sub-structs
  (`fields.header_fields`, ...); promoted methods
  (HasDependency/RangeDependencies/GetRuntimeDependencyNames) are delegating
  impls at each embedding level. ExportsOrImports flattens the JSONValue embed
  into the same (type_, value) shape with Go's exact per-method panic texts.
- `Expected.UnmarshalJSON` receives raw field bytes; the Rust Deserialize
  captures the same bytes via `Box<RawValue>` (serde "raw_value") and mirrors
  the null-check + first-byte classification + retry-decode. Reflection-backed
  `ExpectedJSONType` became the `ExpectedJsonType` trait (float kinds →
  "unknown" — Go's switch omits the float kinds). `Expected` PartialEq ignores
  `actual_json_type` (the Go tests compare with IgnoreUnexported).
- `JSONValue.Value any` is a generic payload enum `JSONValuePayload<T>` so
  Go's generic `unmarshalJSONValueFrom[T]` stays one function (elements are
  the concrete type: JSONValue or ExportsOrImports).
- Duplicate member names: Go decodes with `AllowDuplicateNames(true)`
  (last wins); serde's derive *rejects* duplicate fields, so `Fields`,
  `TypeScriptFields`, and `ContentMapperFields` use hand-rolled
  duplicate-tolerant map visitors (OrderedMap/JSONValue paths were already
  tolerant). `tsc_json::allow_duplicate_names` stays a documented no-op.
- InfoCache stores `Arc<InfoCacheEntry>` (Rust SyncMap hands out owned
  clones, so Go pointer identity maps to Arc identity; WithPackageDirectory
  takes `self: Arc<Self>` to return the same entry). Go's nil-receiver
  `Exists`/`GetContents` are `Option<&Self>` associated fns (collections
  convention). `PackageJson`'s versionPaths/versionTraces/once collapse into
  one `OnceLock<VersionPathsState>`; `VersionPaths.paths` (Go caches per
  value-copy) is a shared `Arc<OnceLock<_>>`. `VersionPaths::exists` keeps
  the Version/pathsJSON checks; the `v != nil` guard is caller-side
  `Option::is_some_and`.
- Tests: all four Go test files ported (TestJSONValue, TestExpected,
  TestExports, TestParse 3/3 cases, TestPackageDirectory, cache-identity,
  ForEachAncestor). BenchmarkPackageJSON/ParseJSONText — PORT: deferred —
  requires tsc-parser (M3 wave); the UnmarshalJSON halves' code path runs on
  the same fixtures in `parse_file_fixtures` (no Rust bench harness yet;
  repo package.json fixture is absent → skipped like Go's SkipIfNotExist,
  date-fns.json parses). cache.go has no in-package Go tests; 5 focused
  GetVersionPaths trace/state tests added, marked PORT: no Go counterpart.
- Validation: CARGO_TARGET_DIR=/tmp/tsrs-pkg cargo test -p tsc-packagejson
  16/16 green; clippy --all-targets clean for the crate (pre-existing
  tsc-tspath warnings untouched); cargo check green.
