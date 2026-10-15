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

## 2026-10-07 gen-ast + crates/ast — NodeFactory (M3 wave 2)
- gen-ast now emits a third output, `crates/ast/src/factory_generated.rs`
  (7339 lines): the per-kind `New*`/`Update*` constructors, generated from the
  same ast.json with generate-go-ast.ts's exact rules — members filtered by
  the *resolved* noFactory (schema.ts folds goOnly in: CaseOrDefaultClause's
  FallthroughFlowNode is not a New* param), `isKindParam` members become the
  `kind: Kind` argument (Token/BindingPattern/ForInOrOfStatement/...),
  `isNodeFlagsMember` members set Node.Flags (bitmask-masked where ast.json
  says: PropertyAccessExpression & NodeFlagsOptionalChain), other `bitmask`
  members mask into the data field (StringLiteral & TokenFlagsStringLiteralFlags),
  `hasTextContent` bumps text_count, and kind-alias constructors
  (NewJSTypeAliasDeclaration, NewJSImportDeclaration) plus Go's per-node
  Update switch are mirrored. Uncovered struct fields (flow/symbol links,
  facts, RawText on NoSubstitutionTemplateLiteral) take Go's fresh-struct zero
  values. 193 New* + 166 Update* generated — 1:1 with Go ast_generated.go
  (verified: full inventory order-parity test + a 359/359 positional
  signature cross-check).
- Mapping: Go `*Node` children → NodeId (mandatory slots use `NodeId::NONE`
  for Go nil), `*NodeList`/`*ModifierList` → Option<NodeList>/Option<ModifierList>,
  string params → `&str` (payload owns the copy — Go shares source slices),
  `[]string` raw → Vec<Box<str>>, `any` → Arc<dyn Any>. Param names are the
  snake_cased member names (so `Type` is `type_`, matching the struct field,
  where Go says `typeNode`); argument ORDER is Go's exactly.
- NodeFactory core is hand-written in factory.rs: it BORROWS the NodeStore
  arena (`&mut dyn NodeStore`, SPEC §5.1) and returns NodeId handles; the
  parser flow is SourceFile::new (file node pre-allocated at arena slot 0,
  since SourceFile::payload assumes nodes[0]) → NodeFactory::new(&mut file) →
  build → factory.new_source_file(statements, eof) (Go's
  f.NewSourceFile(opts, text, ...) — opts/text live on the arena owner here;
  it stamps node_count/text_count into the payload where Go's parser assigns
  `result.NodeCount = p.factory.NodeCount()`). Go's NodeFactoryHooks
  (OnCreate/OnUpdate/OnClone) are deferred (transformer-era; parser passes
  none). UpdateSourceFile is an in-place payload write (PORT: the arena
  holds exactly one SourceFile node, so the Go fresh-node + copyFrom +
  updateNode reduces to in-place; identity never changes).
- NodeStore gained `file_id()` (required method; SourceFile + the visitor
  TestStore implement it) — the factory's NewSourceFile needs it to address
  slot 0; Go has no counterpart (pointer graph).
- Token cache ported: TokenCacheKey{parent: NodeId, loc: TextRange} +
  SourceFile::get_or_create_token + create_token (Go ast.go, verbatim kind
  dispatch; token text is copied from the source slice, PORT). The cache is a
  SourceFile field as in Go. The scoped factory inside create_token is
  discarded, so cached tokens never hit parse counters (Go's separate
  tokenFactory).
- Tests: 57/57 green (48 prior + 9 new: inventory parity against the
  hand-extracted Go list, constructor/flags/for_each_child order, Update
  identity + flags/loc copy, new_source_file counters, token cache incl. the
  kind-mismatch panic, modifier lists; plus the generated compile-time
  addressability probe for all 359 methods). node_size_is_48_bytes untouched
  and green. `gen-ast --check` green/idempotent for all three outputs;
  clippy -p tsc-ast -p gen-ast --all-targets clean (tsc-tspath warnings are
  another worker's in-flight crate); bench-harness check green; tsc-scanner's
  60 errors are all inside its own in-flight files (none reference crates/ast).

## M3 Wave 3 — parser (parser/jsdoc/references/utilities; reparser deferred)

- `parse_source_file(opts, source_text, script_kind) -> SourceFile` by value at
  the crate root — the bench seam's pinned signature (Go can return nil only
  through dead-in-practice paths; PORT-noted in the seam module).
- Pool: Go's `parserPool sync.Pool` → plain per-call `Parser` construct
  (PORT; revisit in the hardening pass — the pool exists to amortize the
  state struct, which Rust moves for free).
- `references.go`: `file`-derived reads (`IsDeclarationFile`,
  `IsExternalModule`) are loop-invariant → hoisted into the entry snapshot;
  mutable file state batched through a `ReferenceCollector` and written back
  once at exit (Go interleaves appends with the walk). Go's `file` param
  became dead after the hoist and was dropped.
- `GetLeading/TrailingCommentRanges` live in the parser crate (Go keeps them
  in the scanner package, but they need `NodeFactory`, which the scanner
  worker excluded — consumer hosts them, PORT-bannered).
- Two race-blocker bugs found by the first corpus run + the testdata gate:
  (1) generated `for_each_child` emitted mandatory `NodeId` slots without
  Go's `visit()` nil filter, so `NodeId::NONE` (the nil mapping for
  JSDoc-only slots like `PropertyAssignment.Type`) leaked to walkers — fixed
  in gen-ast (179 guarded emissions) + regenerated;
  (2) `get_external_module_name`'s `ExportDeclaration` arm downcast to
  `ImportDeclaration` data — panicked on every `export … from` (4/176
  testdata files); now dispatches per-kind like Go's `ModuleSpecifier()`,
  `export { a };` correctly yields none.
- Phase A first green: 1.051× at threads=1 / 0.629× at threads=16 on the
  Excalidraw corpus (nodes/errors/shape byte-identical to the Go driver);
  see rust/BENCHMARKS.md for the provisional-load caveat.

## M3 Wave 4 — parser-parity fixes (two dump divergences vs the Go driver)

- **finishSourceFile must not drain the diagnostics sink** (parser.rs):
  Go's `finishSourceFile` passes `p.diagnostics` to
  `attachFileToDiagnostics` WITHOUT consuming it (it only stamps `.File`
  on the shared entries), so the slice stays live for
  `reparseTopLevelAwait` — which snapshots it into `savedParseDiagnostics`
  AFTER the first `finishSourceFile` call and replays the entries that
  fall in non-await statement ranges while re-generating the rest. The
  port had `std::mem::take` there, so by reparse time the sink was empty,
  the replay copied nothing, and every diagnostic raised before the
  await-span statement vanished. Repro:
  `tsc/testdata/tests/cases/compiler/reachabilityChecksNoCrash1.ts`
  (`export async function … (p!: AsyncIterable<T>): Promise<T[]> { … await … }`):
  Go 19 diags vs Rust 13, tree byte-identical — Rust dropped exactly the
  6 parameter-list-recovery diagnostics (positions 90–111) that live
  inside the replayed prefix. Fix: clone (not take) `diagnostics` and
  `jsdoc_diagnostics` in `finish_source_file`, leaving the sink alive
  like Go. (`reparse_top_level_await`'s own `mem::take` into
  `saved_parse_diagnostics` is the faithful mirror of Go's
  `savedParseDiagnostics := p.diagnostics; p.diagnostics = nil`.)
- **TagNamesAreEquivalent takes TagName(), not the JsxClosingElement**
  (jsx.rs): Go's mismatched-close recovery calls
  `TagNamesAreEquivalent(opening.TagName(), child.ClosingElement.TagName())`
  — the closing element's TAG NAME node. The port passed the
  `JsxClosingElement` node itself at the two recovery predicates (the
  "stop after parsing a mismatched child … reattach the `</div>` higher"
  break in `parse_jsx_children`, and the lastChild-restructure check in
  `parse_jsx_element_or_self_closing_element_or_fragment`).
  `tag_names_are_equivalent` first compares node kinds, and
  `JsxClosingElement != Identifier`, so both predicates were constantly
  false: the recovery never fired, `<div><span></div>` kept the outer
  `<div>` open to EOF, swallowing every following statement as JSX
  text/children (Rust: 72 nodes/13 errors vs Go: 78/11 on
  `tsc/testdata/tests/cases/conformance/jsx/jsxParsingError2.tsx`,
  minimal repro `let x1 = <div><span></div>;` — Go 1 error, Rust 3).
  Fix: extract `.tag_name` from the `JsxClosingElement` data at both
  predicates. (The other three call sites were already correct.)
- Validation: both repros + all 194 conformance/jsx files dump-identical;
  compiler/ sweep has a single remaining divergence,
  `octalLiteralAndEscapeSequence.ts` — PRE-EXISTING at HEAD and identical
  before/after these fixes (Go reports the `\x27` escape @420-423, Rust
  `\x13f` @420-424) — a scanner-crate escape-sequence slicing issue, out
  of parser scope (reported, not touched). tsc-parser 10/10 tests, gate
  GATE: panics=0, clippy clean in parser files.

### M3 wave-3 follow-up — scanner octal-escape digit ladder (orchestrator)

- **Octal-escape first-digit cap** (scanner.rs): Go's
  `scanEscapeSequence` is a fallthrough ladder — `case '0'` (after its
  IsDigit guard) and `case '1','2','3'` reach BOTH digit consumptions
  (up to 3 digits total, `\011`/`\177`/`\377`), while `case '4'–'7'`
  enters at the LAST consumption only: at most one further digit
  (`\47`, never `\477`, since 0o4xx ≥ 256 exceeds the \377 cap).
  The port's shared `scan_octal_escape_sequence_tail` consumed two
  further digits for every first digit, so `\477` decoded 0o477=319
  (`\x13f`, range 420-424) where Go decodes `\47`=39 (`\x27`,
  420-423) and leaves the third `7` as string text.
  Found by the full-corpus dump diff
  (`tsc/testdata/tests/cases/compiler/octalLiteralAndEscapeSequence.ts`);
  fix: gate the first consumption on `low_first_digit` (true for
  first digits '0'–'3', false for '4'–'7'), mirroring the ladder.
- Validation: the octal torture file byte-identical (109 errors,
  dump diff IDENTICAL); 14-escape edge matrix (strings + templates +
  regex, `\4`/`\47`/`\477`/`\0`/`\08`/`\011`/`\177`/`\377`/`\400`/
  `\077`/`\77`/`\310`/`\7`) IDENTICAL; tsc-scanner 6/6, tsc-parser
  10/10, testdata gate 176/176 panics=0.

### M3 follow-up — corpus walk global sort is a raw-path string sort (bench-harness)

- **`sort.Strings` vs `PathBuf` Ord** (parse.rs `index_corpus`): both
  drivers sort the walked file list globally, but Go sorts the raw path
  STRINGS while the port sorted `Vec<PathBuf>` — PathBuf's `Ord` is
  component-wise, so on the one file/dir prefix collision in the corpus
  (`declarations.ts` vs `declarations/`) the orders differ: Go's string
  sort puts the FILE first ('.' 0x2E < '/' 0x2F); component-wise puts the
  directory first. All parse-mode aggregates are commutative sums, so
  shape parity held regardless — the dump's per-file slot order exposed
  it (found by the full-corpus dump diff). Fix: sort by the encoded path
  bytes (`as_os_str().cmp`), mirroring `sort.Strings`; regression test
  `corpus_walk_sorts_by_raw_path_bytes_not_components`.
- Validation: full tsc/testdata corpus (12,957 files) dump diff
  IDENTICAL — every file, node, diagnostic, position, arg, histogram
  byte-identical between go-bench and tsc-bench.

### M4 wave A — the binder (rust/crates/binder)

Ported tsc/internal/binder @ ec47d33c: binder.go (2,801 lines → binder.rs,
4,200) + nameresolver.go (506 → nameresolver.rs, 910); referenceresolver.go
is deferred to the emit wave per the M4 milestone note in SPEC.md. Shims
(dedup candidates, folded back when the owning wave lands):
ast_shims.rs (1,908 — utilities.go subset + Go's lazily-assigned
GetNodeId/GetSymbolId link-store ids), node_data.rs (667 — the Go
nodeData interface dispatchers over the frozen generated payloads),
scanner_shims.rs (178 — GetErrorRangeForNode plus re-exports of the
scanner package's ported helpers).

- **Symbol/flow arenas** (the crate-wide PORT contract, SPEC §5.10): Go's
  GC `*Symbol`/`*FlowNode`/`*FlowList` become SymbolId/FlowNodeId/
  FlowListId indexes into Binder-owned Vecs, handed back as `BindResult`
  (the node payloads keep the handles; ids are per-file and the M5
  program merge re-bases them). `bind_source_file` returns
  `Option<BindResult>` — None via the bind_once latch Go enforces on
  SourceFile.
- **TableLoc enum** replaces Go's `*ast.SymbolTable` map-reference
  parameter (declareSymbol/declareModuleAndExportName each pass exactly
  one of Locals(NodeId)/Exports(SymbolId)/Members(SymbolId)/GlobalExports).
- **LabelSlot** replaces Go's method-value `getTarget(activeLabel)` param
  (Break/Continue); ActiveLabel is a Box chain with break_target/
  continue_target slots.
- **bind_each_child** collects children into a Vec first: Node is borrowed
  out of the store the Binder mutates mid-walk (Go iterated the live
  struct).
- **NameResolver hooks** are `Option<Box<dyn FnMut...>>` fields (Go's
  checker-supplied closures with defaults); `Resolve` is a faithful port of
  the scope-climbing loop including the Decorator/ExportSpecifier location
  reassignment, which the loop tail consumes (lastLocation = location, NOT
  the pre-switch node — a subtle Go behavior the port initially got wrong
  and the unused-assignment warnings exposed).
- Two Go-faithfulness notes: getStrictModeBlockScopeFunctionDeclarationMessage
  is dead code in the Go tree too (kept verbatim under #[allow(dead_code)]);
  ReturnStatement gets NO flow node in Go (bindReturnStatement only
  rewires currentFlow) — the smoke test asserts identifiers and the
  container end/return flow nodes instead.
- Validation: smoke 13/13; testdata bind gate (parser gate's walk rules +
  fuzz seeds, parse+bind under catch_unwind) 176/176 panics=0 with
  127,783 symbols / 200,815 flow nodes / 6 bind diagnostics;
  clippy zero warnings on tsc-binder; tsc-ast / tsc-parser / bench-harness
  unaffected. Gate command: `CARGO_TARGET_DIR=/tmp/tsrs-binder cargo test
  -p tsc-binder --test testdata_gate -- --ignored --nocapture`.

### M5 module resolution (rust/crates/module)

Ported tsc/internal/module @ ec47d33c: resolver.go (2,648 → resolver.rs,
4,750), types.go (types.rs), util.go (util.rs), cache.go (cache.rs),
staticresolver.go (staticresolver.rs); plus a deliberately in-crate port of
tsc/internal/vfs/vfsmatch (vfsmatch.rs — the vfs crate skipped that
subpackage; hoist into tsc-vfs when a second consumer appears).

- **Resolver trait** (Go `Resolver` interface): `Arc<dyn Resolver>` mirrors
  Go interface values (vfs crate precedent). `ResolveModuleName`'s vestigial
  error channel (always nil at this revision) is dropped — the result tuple
  is `(Option<Arc<ResolvedModule>>, Vec<DiagAndArgs>)` where the Option is
  always Some (Go returns a non-nil `*ResolvedModule` even for unresolved
  modules; `ResolvedModule::is_resolved` mirrors Go's nil-tolerant
  `IsResolved()` as an associated fn over `Option<&Self>`). Cache-hit
  results hand back the cached `Arc` directly (no clone).
- **Tracer ownership**: Go's `resolutionState` shares a `*tracer` with its
  parent state (loadModuleFromTargetExportOrImport swaps name/
  containingDirectory and restores them). The port makes the tracer an
  owned `Option<Tracer>` slot that is `take()`n into nested states and
  restored on return; `trace!` expands to `write_trace(&mut tracer, ...)`.
- **LoaderKind enum**: Go's loadModuleFromNearestNodeModulesDirectory
  closures become a Copy enum (NodeRelative / SpecificNodeModulesDirectory /
  DirectoryWorker) dispatched by run_loader; closures can't capture `&mut`
  state twice.
- **ParsedPatternsCache** keys on the `paths` mapping CONTENT (Vec snapshot)
  instead of Go's `*OrderedMap` pointer — CompilerOptions owns the map by
  value, so there is no stable pointer; cached patterns are a pure function
  of content (behavioral superset, identical results).
- **packagejson: added `PackageJson::new(fields, parseable)`** — the crate's
  private `OnceLock` typesVersions state made cross-crate struct-update
  construction illegal (`..Default::default()` from outside the crate).
  The constructor is the port of Go's `PackageJson{Fields: ..., Parseable: ...}`
  literal (lazy state starts unset, like Go's zero sync.Once).
- **Race tests** (Go channels + goroutines): `Gate` (Mutex<bool> + Condvar;
  `close` = channel close, `wait_closed` = `t.Context().Done()` timeout
  receive) and `Arrivals` (counting Condvar) replace `chan struct{}`;
  `blockingFS`/`flipFileExistsFS`/`caseInsensitiveViewFS` are hand-written
  `Fs` delegation wrappers (Go embeds vfs.FS); panics are caught with
  `catch_unwind(AssertUnwindSafe(..))` + `resume_unwind` (Go `recover()`).
- **Tests ported**: 32 of 33 Go tests (37 Rust tests incl. table-test
  sub-cases): resolver_test.rs (28 incl. the two 3526/1290/4478 race
  regressions), resolver_internal_test.rs (4), staticresolver_test.rs (1).
  `TestResolverSharedData` is SKIPPED: it reflects over the Go type graph
  via `testutil.CheckDataOnly`; Rust has no runtime reflection and
  ResolutionData's Arc-only shape is enforced by its type definition.
- **Two port bugs the tests caught**: (1) `parse_node_module_package_root`
  used the unchecked `RootedDirectoryPath::from` where Go uses
  `RootedDirectoryPathFromAbsolute` (normalizes), yielding `/a/node_modules/`
  with a trailing separator; (2) `new_static_resolutions` iterated the
  (empty) map instead of the entries Vec after a capacity-fix refactor,
  dropping every static resolution.
- Validation: `cargo test -p tsc-module` 37/37 (incl. all three
  deterministic race tests); `tsc-packagejson` 16/16 after the
  `PackageJson::new` addition; `cargo clippy -p tsc-module --all-targets`
  zero warnings; `cargo check -p bench-harness` green. Gate: `CARGO_TARGET_DIR=/tmp/tsrs-module cargo test -p tsc-module`.


## M4 Wave C — tsoptions + contentmapper (internal/tsoptions, internal/contentmapper)

- **Crates**: `tsc-tsoptions` (rust/crates/tsoptions), `tsc-contentmapper`
  (rust/crates/contentmapper). Per-file port table:
  commandlineoption.go→commandlineoption.rs, namemap.go→namemap.rs,
  enummaps.go→enummaps.rs, errors.go→errors.rs, diagnostics.go→diagnostics.rs,
  parsinghelpers.go→parsinghelpers.rs, tsconfigparsing.go→tsconfigparsing.rs
  (M4 subset), parsedcommandline.go→parsedcommandline.rs (watch machinery
  deferred), commandlineparser.go→commandlineparser.rs,
  parsedbuildcommandline.go→parsedbuildcommandline.rs, parsedoptions.go→
  parsedoptions.rs, rawcompileroptions.go→rawcompileroptions.rs,
  showconfig.go→showconfig.rs, wildcarddirectories.go→wildcarddirectories.rs,
  options_generated.go/declarations_generated.go→options_generated.rs/
  declarations_generated.rs (gen-options output),
  contentmapper.go→contentmapper/src/lib.rs+field_json.rs,
  compiler_option_fields (Go reflection walk)→compiler_option_fields_generated.rs.
  **Not ported**: contentmapper host.go/hostimpl.go (JSON-RPC process host —
  needs ipc/process spawning; M6 territory), `ParseResult` transform path,
  parsedcommandline.go's `ParsedCommandLineWatcher`/changed-files machinery.
- **`any` → `CompilerOptionsValue`**: Go's `any` option values become a
  closed enum (Null/Bool/Int/Float/Str/StrArray/Array/Object/PathsMap/
  PluginImports/Tristate + the 6 core option enums). List results (`[]any`
  of validated strings) surface as `StrArray`. `PartialEq` is structural
  (Go `==` over `any`); `Int`==`Float` cross-compares as f64 like Go's
  float64 JSON numbers. Go's `floatOrInt32ToFlag` panics on a string —
  the port's `as_<T>_flag()` panics identically (raw JSON strings must be
  converted through the enum maps first, exactly as in Go).
- **Two map shapes collapse into `NameMap`**: Go's `*NameMap`
  (ordered, short-name aware) and `CommandLineOptionNameMap`
  (map[string]*CommandLineOption) are one type; keys are lowercase names.
  Go's exact-then-lowercase `Get` reads the same because declaration names
  are checked case-sensitively at the use sites (`opt.Name != key`
  case-mismatch detection, RawCompilerOptions::finalize's skip rule).
- **Tables are `static`s**: `CommandLineOption` is const-constructible and
  the generated declaration literals are fully explicit (a const literal
  cannot omit fields the way Go's composite literals can). Go's
  package-init maps become `OnceLock` accessors. Go's `ElementOptions`
  map field becomes `ElementOptionsKind` + lazy `element_options(kind)`.
- **${configDir} semantics**: `startsWithConfigDirTemplate` is
  case-insensitive but `getSubstitutedPathWithConfigDirTemplate` replaces
  the exact `${configDir}` spelling with `./` (Go's own TODO asymmetry);
  `parsedCompilerOptions.ParseOption` captures template-bearing values in
  `unresolved_paths` and the generated
  `handle_option_config_dir_template_substitution` resolves them against
  the config dir — exact port. `parse_option` skips Null values (Go's
  exported `ParseCompilerOptions` guard).
- **Enum values convert before parsing**: `convertOptionsFromJson` is
  ported exactly (unknown-key → unknown-option error via the spelling
  suggestion; case-mismatch → did-you-mean; `convertJsonOption` runs the
  type check (`isCompilerOptionsValue`), enum mapping, locale validation,
  list-element mapping + falsy filter, and `normalizeNonListOptionValue`
  path rooting BEFORE `parse_option`). Without it, raw JSON strings
  ("target": "es2020") would panic in `floatOrInt32ToFlag` — in Go too.
- **tsconfigparsing M4 substitutions** (PORT notes at file top):
  (1) Go parses tsconfig as a JSON SourceFile for node-positioned
  diagnostics; the port parses JSON text directly into
  `CompilerOptionsValue` (order-preserving recursive-descent) and
  diagnostics degrade to compiler diagnostics (zero spans) until the
  AST-config path lands. (2) `extends` module-specifier resolution is a
  seam: `ResolveConfigHost::resolve` (wire to tsc-module on the M6
  integration; unresolvable = Go's cannot-read diagnostic). (3)
  `getFileNamesFromConfigSpecs` (glob file enumeration) deferred — the
  parsed result carries validated specs and empty `file_names`.
  (4) `ProjectReference` lives in tsoptions, not core (M6 core gap).
  (5) `parse_json_config_file_content` seeds the resolution stack with the
  config file's own path when the caller passes an empty stack (Go's
  callers seed it), so extends cycles are detected self-contained.
  (6) The extended-config cache seam stores the raw map (Go caches the
  parsed command line) and rebuilds the parsed form on hit.
- **wildcarddirectories**: `vfsmatch.IsImplicitGlob` is a local PORT
  (belongs to the tsc-vfs wave); the exclude arm takes an `is_excluded`
  predicate instead of running the vfsmatch SpecMatcher (724 Go LOC, also
  future tsc-vfs work). The directory-derivation logic is the full port.
- **contentmapper `TransformIdentity`**: xxh3 is not in the approved dep
  set (no network), so the port hashes the identical byte layout with
  128-bit FNV-1a — deterministic in-process, but NOT the same fingerprint
  as Go. Callers must not compare fingerprints across implementations.
  `MarshalDeclaredOptions` walks the 132 json-tag fields via the generated
  match instead of reflection (same per-field marshal, zero values
  skipped, manifest order preserved).
- **parsedoptions `Equals`**: core's CompilerOptions/TypeAcquisition have
  no Rust equality yet (Go has generated `Equals`); the port compares
  compiler options through their serialized (deterministic) option maps —
  same values in, same equality out. Swap to a generated `PartialEq` when
  core grows one.
- **Tests ported** (41 green): commandlineparser_test.rs (10 — parse
  scenarios, removed watch options, response files incl. nested + quoting,
  typeRoots rooting, null override, build flag validation, resolved
  project paths), showconfig_test.rs (6 — the deterministic showconfig
  runner incl. extends merge, first-alias enum serialization, default
  include filtering, key-order marshal), decls_test.rs (2 — the Go
  path-kind table + group counts), parsinghelpers_test.rs (6 —
  TestMergeCompilerOptionsRawNulls, tri-state/string-array/project-reference
  parsers, configDir template trio), rawcompileroptions_test.rs (2 — Go's
  TestRawCompilerOptionsFinalizePaths verbatim + round trip),
  tsconfigparsing_test.rs (6 — spec validation errors, extends cycle,
  module-specifier seam, parseContentMapper, the config-file entry point,
  JSON key order), wildcarddirectories_test.rs (4 — dot-prefixed include,
  drive root, non-ASCII paths, isImplicitGlob), contentmapper
  marshal_test.rs (5 — virtual extensions, identity, equality,
  transform-identity determinism, declared-options marshal). Reflection-
  based Go tests (TestCompilerOptionsDeclaration, TestMergeCompilerOptions
  field walk, TestShowConfigSerialization's reflected serializer,
  TestParseCompilerOptionNoMissingFields) are covered instead by the
  generator's field-for-field Go-parity checker
  (tools/gen-options/data/parity-check-declarations.mjs — PARITY OK for
  all 4 declaration groups: 29 common / 97 compiler / 4 typeAcquisition /
  7 build).
- **Two port bugs the tests caught**: (1) `getSubstitutedPathWithConfigDir
  Template` strip-prefixed the template (leaving "/src" absolute) instead
  of Go's `strings.Replace(value, "${configDir}", "./", 1)` + normalize
  ("./src" → "/project/src"); (2) `convert_options_from_json_worker`
  passed raw JSON values straight into the generated parse, which panics
  on enum options ("target": "es2020") exactly like Go's
  `floatOrInt32ToFlag` — the full `convertOptionsFromJson` conversion loop
  (above) is required before parsing.
- Validation: `cargo test -p tsc-tsoptions -p tsc-contentmapper` 41/41;
  `cargo clippy -p tsc-tsoptions -p tsc-contentmapper --all-targets` zero
  warnings; `gen-options --check` green for all 4 outputs (core output
  byte-identical); Go-parity checker PARITY OK;
  `cargo check -p bench-harness` green. Gate:
  `CARGO_TARGET_DIR=/tmp/tsrs-tsoptions cargo test -p tsc-tsoptions -p tsc-contentmapper`.

## M4 Wave D — showconfig + resolvereact parity twins (bench-harness, gates, tsc-module, tsc-tsoptions)

Two new byte-identical twin harnesses over the `dump` twin's proven
pattern (same corpus walk, skip rules, minimal json_escape, exit codes):

- **`showconfig`**: corpus walk for `tsconfig.json`/`jsconfig.json`
  (sorted, `--tsconfig <path>` single-file mode) →
  `ParseConfigFileTextToJson` → `ParseJsonConfigFileContent` →
  `ConvertToTSConfig` → 4-space indent marshal. One NDJSON line per
  config: `{"file":<abs>,"ok":<bool>,"out":"<marshaled>","errors":<n>}`;
  unreadable configs emit the fixed marker `{"file":<abs>,"ok":false,
  "error":"cannot read file"}` (os vs io error strings differ across
  runtimes, so the marker is fixed). errors = parse text diagnostics +
  parsed errors.
- **`resolvereact`**: project-dir walk (any dir with package.json or
  tsconfig.json/jsconfig.json, node_modules skipped) → per dir, a fixed
  21-specifier canonical list (AST extraction would couple to the parser
  wave; the list is deterministic and covers bare/relative/absolute/
  dynamic/node_builtins specs, self-reference, scoped, index, subpath
  exports, extensions, imports field, missing) resolved against a
  synthetic `<dir>/__resolvereact__.ts` containing file, ESM mode,
  CompilerOptions from the dir's tsconfig or `{Bundler,ESNext,ESNext}`.
  Per record: `{"file":<containing>,"specifier":<s>,"kind":<moduleReso
  lution int>,"resolved":<path|null>,"failed":<bool>,"external":<bool>,
  "pkgId":"<id>"|null}` (pkgId only for non-zero PackageId), then one
  trailing summary `{"summary":true,"dirs":<n>,"specs":<n>,"resolved":
  <n>,"failed":<n>,"shape_hash":"<16-hex>"}` — the FNV-1a 64 fold over
  the exact per-record recipe `file=<f>\n specifier=<s>\n kind=<k>\n
  resolved=<r>\n failed=<0|1>\n external=<0|1>\n pkgId=<id>\n`
  (hand-verified vectors live in showconfig_seam.rs).

**Crate fixes the parity diff forced (all PORT-noted in source)**:

- **module/vfsmatch.rs**: ported Go vfsmatch.go:672-725 `SpecMatcher` +
  `newSpecMatcher` (match_string/match_file_name/match_index/
  match_file_name_index; `Usage`, `UNLIMITED_DEPTH`, `read_directory`
  re-exported from module/src/lib.rs). tsoptions now depends on
  tsc-module (Go: tsoptions imports tsc/internal/vfs).
- **tsoptions/tsconfigparsing.rs**:
  (1) JSONC parse in `parse_json_text` — comments, trailing commas, BOM
  strip (Go's tsconfig files are JSONC; the old parser was strict JSON).
  (2) Full `parseJsonConfigFileContent` worker: file enumeration via
  `get_file_names_from_config_specs` (literal/wildcard/wildcard-json
  maps, `get_supported_extensions`(+WithJson),
  `has_file_with_higher_priority_extension`,
  `remove_wildcard_files_with_lower_priority_extension`), the
  "No inputs were found in config file" diagnostic (with the
  StringifyJson args Go emits) and the empty-`files`-array diagnostic,
  implied exclude from outDir/declarationDir, default include seeding,
  `validateSpecs` + ${configDir} substitution. `ConfigFileSpecs` grows
  validated_include/exclude/files_specs + validated_file_names;
  `ParsedCommandLine` grows raw_*_spec_values/is_default_include_spec and
  a populated file_names.
  (3) `read_extended_config` returns (map, parsed, resolved_path) and
  `merge_extended_raw_into_config` rewrites extended include/exclude/
  files spec strings relative to the EXTENDING config's basePath
  (Go's ConvertToRelativePath + CombinePaths) at each extends level.
- **tsoptions/showconfig.rs**: `TSConfig::marshal` is 4-space indent
  (was 2), always emits `compilerOptions` (Go's json/v2 marshal emits the
  field unconditionally since it is non-omitempty) and renders empty
  arrays as `[]` (json/v2 renders empty non-nil slices; the Rust marshal
  previously emitted nothing).

**Known divergence surfaced and deliberately left (tsc-core scope)**:
the full-repo corpus run (28 configs) flags exactly one file,
`tsc/testdata/fixtures/compiler/tsconfig.json`, which sets `"types": []`
explicitly. Go emits `"types": []` (non-nil empty slice passes
`options.Types != nil` in serializeCompilerOptions); the Rust side drops
the key because core's `CompilerOptions` collapses Go's nil-vs-empty
slice distinction into `Vec` (documented core options_generated PORT
note) and the generated Rust serializer tests `!types.is_empty()`. A
faithful fix needs core list fields as `Option<Vec<_>>` + a gen-options
update — outside this wave's crate scope; fixture matrices stay free of
the pattern (matrix fixtures are byte-identical) and the twin correctly
reports the diff on any corpus that contains it.

**Fixtures committed** under rust/bench/gates/: `tsconfig-matrix/`
(18 cases: plain, 2/3-deep extends, extends cycle, ${configDir} templates,
relative/absolute-missing extends, jsconfig, empty compilerOptions, all
value shapes, unknown options, JSONC comments, files list, include/
exclude with invalid specs, extends array, references, extends-include
rewrite, empty files array) and `resolver-matrix/` (self-referencing
package with exports/imports, pkg with main/types, @scope/pkg with
exports+dist, idx index resolution, node16-proj with nodenext + a CJS
exports package under its node_modules — the fixture .gitignore negates
the root `node_modules/`+`dist` ignores so the tree actually commits).

- Validation: `cargo test -p bench-harness -p tsc-module -p tsc-tsoptions`
  all green (17/37+ /all); `cargo clippy` zero warnings on all three;
  `go vet ./rust/bench/go-driver` clean; every parity run byte-identical:
  showconfig over tsconfig-matrix (0 diff), resolver-matrix (0),
  --tsconfig single-file incl. unreadable marker (0), resolvereact over
  resolver-matrix (0) and tsconfig-matrix (0); real-world
  packages/typescript/tsconfig.json byte-identical (errors=0); the dump
  and parse twins re-verified byte-identical after the shared-code
  changes. Gate:
  `GOTOOLCHAIN=auto go run ./rust/bench/go-driver <cmd> --corpus <root>` vs
  `CARGO_TARGET_DIR=/tmp/tsrs-gates cargo run -p bench-harness -- <cmd> --corpus <root>`.

## M4 Wave D+1 — core nil-vs-empty slice fields (gen-options + core + tsoptions + contentmapper + module)

Closes the "Known divergence surfaced and deliberately left (tsc-core
scope)" above: the full-repo showconfig corpus flagged exactly one file,
`tsc/testdata/fixtures/compiler/tsconfig.json` (`"types": []` — Go emits
`"types": [],`; the port dropped the key). Root cause: Go's slice-typed
CompilerOptions fields distinguish nil (key absent) from non-nil empty
(the JSON decode of an explicit `[]`); the port collapsed both into a
plain `Vec` and its generated serializer tested `!is_empty()`.

**Shape chosen: `Option<Vec<_>>`** (None = Go nil, `Some(empty)` = Go
non-nil empty slice). It is the only faithful mapping because Go's
machinery tests the fields three ways — `ParseStringArray(value)` returns
nil for any non-array value (null/absent) and a non-nil slice for any
array (even empty); `mergeCompilerOptionFields`'s explicit-null branch
writes nil while its copy branch tests `!= nil`; and
`serializeCompilerOptions` emits iff `!= nil`. `Option` reproduces all
three exactly, and `a != b` on `Option<Vec<_>>` is literally Go's
`(a == nil) != (b == nil) || !slices.Equal(a, b)` in Equals.

**Field inventory (9 list fields, one generator `rust_type` change)**:
CompilerOptions `custom_conditions`, `lib`, `module_suffixes`, `plugins`,
`root_dirs`, `type_roots`, `types` + TypeAcquisition `include`/`exclude`
(paths was already `Option<OrderedMap>` = Go's pointer). Go slices:
`[]string`, `[]PluginImport`, `[]tspath.RootedDirectoryPath`.

**gen-options changes** (generator + regen, no hand-edits of generated
files; `cargo run -p gen-options -- --check` all four outputs current):
- main.rs: `rust_type` maps the three slice types to `Option<Vec<...>>`;
  PORT notes updated (header + Equals doc); the clone/equals parity test
  now pins nil-vs-empty distinct and Some(empty)==Some(empty).
- tsoptions.rs: `zero_expr` slices → `None` (Go's explicit-null writes
  nil); `not_zero` slices → `.is_some()` (Go `!= nil`); `value_variant`
  slices → `.clone().unwrap()` (guarded by is_some); `parser_assignment`
  routes `[]string` → `parse_string_array_option` / `[]string`+directory →
  `parse_directory_names_option` (see parsinghelpers below), lib/plugins
  arms wrapped in `Some`; `emit_show_config` List arms test `.is_some()`
  and emit `[]` for `Some(empty)` (Go: `if options.X != nil { Set }`);
  configDir `[]string` arm (currently dead) rewritten for Option.
- contentmapper emission: `string_array`/`plugin_imports`/
  `rooted_directory_array` calls pass `options.X.as_deref()`. PORT note
  fixed: Go's `reflect.Value.IsZero()` is false for a non-nil empty slice,
  so `Some(empty)` marshals `[]` — the old "skip empty" was a divergence.

**parsinghelpers.rs**: new `parse_string_array_option` /
`parse_directory_names_option` — Go `ParseStringArray` returns nil for
every non-`[]any` value (so null → None) and a non-nil slice for arrays;
`parseDirectoryNames` = `core.Map(ParseStringArray(...))` and `core.Map`
of nil is nil. The Vec-valued originals stay (paths-map values,
rawcompileroptions finalize, the unresolvedPaths block).

**contentmapper field_json.rs**: `string_array`/`plugin_imports`/
`rooted_directory_array` take `Option<&[T]>` (None skip, Some(empty) →
`[]`); `paths_map` drops its empty-map skip (Go IsZero tests the
*pointer* — a non-nil empty map marshals `{}`).

**Mechanical fallout (construction/read sites, all PORT-noted in
source)**: core/compileroptions.rs (`get_effective_type_roots` now
`.is_some()` = Go `TypeRoots != nil`; `uses_wildcard_types` over
`as_deref().is_some_and`; `get_effective_root_dirs` returns
`as_deref().unwrap_or(&[])` — Go returns the nil-able slice, callers only
iterate; tests Some-wrapped); module/resolver.rs (`resolve_from_type_root`
early-returns on None = Go `TypeRoots == nil`; `try_file` uses
`is_none_or(is_empty)` = Go `len(...) == 0`; conditions capacity
`map_or(0, Vec::len)` = Go `len(nil)`; `get_automatic_type_directive_names`
`clone().unwrap_or_default()` for the nil-able return +
`iter().flatten()`); module test literals `Some(vec![...])`;
tsoptions tests now assert Go's exact semantics (`--lib` empty →
`Some(Vec::new())`, `--customConditions null` → None — Go's
TestCustomConditionsNullOverride asserts nil).

- Validation: `cargo check --workspace` green;
  `cargo test -p tsc-core -p tsc-tsoptions -p tsc-contentmapper -p
  tsc-module -p bench-harness -p tsc-binder` all green (0 failed);
  clippy zero warnings on all touched crates; `cargo run -p gen-options
  -- --check` four outputs current; the dump twin re-verified
  byte-identical (parse is a benchmark, driver-header/timings only).
- Twin proof: known case `--tsconfig tsc/testdata/fixtures/compiler/
  tsconfig.json` now byte-identical, Go's `"types": [],` emitted;
  five twin parity runs byte-identical (showconfig tsconfig-matrix +
  resolver-matrix, resolvereact resolver-matrix + tsconfig-matrix,
  showconfig packages/typescript/tsconfig.json); whole-repo sweep
  `showconfig --corpus .` (28 configs) — ZERO divergences (was 1).

## M4 Wave E — the `.symbols` parity twin (bench-harness symbols_seam, go-driver, two binder fixes)

Closes the M4 gate's last open item (SPEC §8 M4: "`.symbols`-style output
for conformance corpus matches Go"): a new `symbols` subcommand on BOTH
drivers — `go-bench symbols --corpus <dir> [--fuzz <dir>]` /
`tsc-bench symbols --corpus <dir> [--fuzz <dir>]` — parse + bind
(`binder.BindSourceFile`, the entry the binder tests drive) + walk the
binder's symbol graph + dump per-file symbol tables as NDJSON. `diff` is
the verdict.

**Canonical walk (the ordering choice)**: Go's `SymbolTable` maps iterate
RANDOMLY and the port's `OrderedMap`s iterate in insertion order — neither
is a shared concept, so the canonical record order is a walk BOTH drivers
implement identically: AST pre-order (`ForEachChild` — proven identical by
the `dump` twin) visiting each node's Locals, then the node's Symbol, then
its LocalSymbol, then children; from each discovered symbol its Members
table, then its Exports table (canonical-key order); the file's
GlobalExports close the walk. This union is exactly Go binder.go's own
reachability contract ("not all symbols end up in tables" —
`bindAnonymousDeclaration`'s module symbol, type-literal symbols,
duplicate-identifier scratch symbols, the CJS `exports` property are
node-attached only). Canonical ids are encounter order; `parent` resolves
to the parent's canonical id.

**Canonical names/keys (the Go baseline-emitter ordering mirrored)**: Go's
`InternalSymbolNamePrefix` is the single byte 0xFE; the port models it as
U+00FE (UTF-8 C3 BE) — raw name BYTES differ between the ports, so the
twin canonicalizes every name/key through Go's own baseline
sanitization (`ast.EscapeInternalSymbolName`: leading internal prefix →
"__"; the same escape the `.symbols`/`.types` baselines rely on — the Go
port's baselines sanitize internal names precisely because \xFE bytes are
baseline-hostile) and sorts table keys by (canonical form, raw form).
Latent port hazard, PORT-noted in the twin: a user identifier literally
starting with "þ" would collide with the port's U+00FE prefix
representation (Go's 0xFE byte is invalid UTF-8 and cannot appear in a
scanned identifier); no corpus file contains "þ".

**Record format** (fields both sides emit identically; Go-only/transient
fields — CheckFlags, ExportSymbol, lazy symbol/node ids — dropped from
BOTH sides per the fallback rule):
`{"file":...,"ok":true,"alloc":<binder SymbolCount>,"symbols":<record
count>,"sym":[{"id":i,"name":"...","flags":"<%08x u32>","kind":<first
declaration kind ordinal, -1>,"parent":<canonical id>|null,
"declarations":["<kind>:<pos>",...],"exports":N,"members":M,
"valueDeclaration":"<kind>:<pos>"|null},...],"diag":[<bind diagnostics,
dump-line shape>],"shape_hash":"<FNV-1a 64 over the fold; vectors in
symbols_seam tests>"}`. Parse/bind panics emit `ok:false` lines instead of
aborting (the binder gate proves the corpus panic-free; any ok:false IS a
finding). Corpus walk = the binder gate's rule (every extension with a
known scriptKind binds — .js/.mjs/.json included — not just dump's bench
extensions), `--fuzz` indexes `go test fuzz v1` seeds like the gate's
sweep.

**Two real binder divergences found by the twin (fixed mirroring Go,
tests added to crates/binder/tests/smoke.rs)**:
- binder.rs `declare_symbol_ex` (Go binder.go declareSymbolEx's
  `ast.NodeIsMissing(node.Type())`): Go's `node.Type()` is nil for an
  ExportAssignment (`export default <expr>`), and `NodeIsMissing` treats
  nil as missing; the port's `node_data::type_of` returns `NodeId::NONE`
  for the payload's non-Option field and the old
  `.is_some_and(|t| node_is_missing(self.file.node(t)))` dereferenced the
  arena (panic: NodeId::NONE is foreign). Fixed to map nil/NONE/absent →
  missing. 40 conformance files (all export-default/export-equals
  conflicts) hit this.
- ast_shims.rs `jsx_namespaced_name_text` (Go ast.go `Node.Text()`'s
  KindJsxNamespacedName arm `namespace + ":" + name`): the ast crate's
  `node_text` does not carry the arm (panics "Unhandled case in
  Node.Text: KindJsxNamespacedName"), reached through
  `get_declaration_name`'s `IsPropertyNameLiteral || IsJsxNamespacedName`
  branch for JSX attributes (`a:b={1}`). Shimmed in the binder crate
  (ast crate out of wave scope); dedup candidate — fold into
  `node_text` when the utilities.go wave lands. 3 conformance .tsx files.

One further divergence surfaced on the fixtures corpus
(`typescript/Herebyfile.mjs`: Go +25 JSDoc-@typedef-derived symbols) that
is NOT binder-side: the parser's `reparseTags` (reparser.go) was an
in-flight no-op stub at HEAD — the JSDoc reparser port (parser crate,
separate wave) landed mid-task and closed it; the final in-tree run is
byte-identical including that file.

**Validation** (local, CARGO_TARGET_DIR=/tmp/tsrs-sym):
`tsc-bench symbols` vs `go-bench symbols` byte-identical (diff empty) on
the binder-gate corpus — `tsc/testdata/fixtures` +
`tsc/internal/parser/testdata/fuzz/FuzzParser` (176 files: 166 fixtures
all extensions + 10 fuzz seeds) — and the FULL conformance sweep
`tsc/testdata/tests/cases/conformance` (5,935 files; 5,723 .ts + 211 .tsx
+ 1 .js; ~19.4 MB output); determinism double-runs byte-identical on both
drivers/corpora; `cargo test -p bench-harness -p tsc-binder` green (24 +
15 + gate, incl. hand-verified FNV-1a vectors for the new fold);
`cargo clippy -p bench-harness -p tsc-binder --all-targets` zero warnings
in both crates; `go vet ./rust/bench/go-driver` clean; dump/showconfig/
resolvereact quick regression (fixtures matrix) still byte-identical.

## 2026-10-08 crates/parser/src/reparser.rs — parser/reparser.go (JSDoc reparse machinery)
The M3-inventory leftovers wave: the full function-by-function port of
`tsc/internal/parser/reparser.go` (752 LOC — the JSDoc tag REPARSE
machinery; the task description calling this "tree reuse for edits
(tryParse@reuse)" is stale) plus `ast.GetAssignmentDeclarationKind` +
`JSDeclarationKind` (utilities.go:1547) into `crates/ast/src/utilities.rs`.

- **`visitor.rs visit_node_core` NONE-sentinel pass-through (ast crate)**:
  Go's `visitNode(nil)` returns nil without calling Visit; the generated
  per-struct `visit_each_child` code hands the `NodeId::NONE` sentinel to
  `visit_node` for Go-nil fields (non-`Option` `NodeId` payload fields,
  e.g. `PropertySignatureDeclaration.type_`/`initializer`), so the
  sentinel is now treated as nil at the `visit_node_core` entry instead
  of being resolved through the arena (which panicked "NodeId::NONE is
  foreign to file"). Surfaced by `addDeepCloneReparse` of an @param
  `{{...}}` TypeLiteral whose property signatures carry NONE fields.
- **`getFunctionLikeHost` valueless `return`**: Go's
  `ReturnStatement.Expression()` is nil for `/**@type */ return` (fuzz
  seed 0ec0d5de7f0264d9); the port's host-expression accessor panicked.
  The ReturnStatement arm now reads the optional payload field directly
  (nil → no host, mirroring Go's nil Expression() chain).
- **File-list deviations (mechanical wiring, disclosed)**: beyond
  reparser.rs/reparser_test.rs/parser/src/lib.rs/ast/utilities.rs the
  wave required `parser/src/parser.rs` (Parser.reparsed_clones field,
  ParserState.reparsed_clones_len in mark/rewind, the reparseList→
  statements append in parse_source_file_worker, the sort +
  SourceFileNodeData snapshot in finish_source_file),
  `parser/src/jsdoc.rs` (removed the no-op reparse_tags stub),
  `ast/src/source_file.rs` (reparsed_clones field + accessors, not
  copied by visit_each_child like Go's copyFrom), and the
  `visitor.rs` sentinel fix above.
- **Deferred**: the `GetTokenPosOfNode` assertion of Go's
  TestJSDocTypeSourcePropagatesToConstructedReparse — scanner.go:2526
  surface not yet ported; the flags/text assertions covering the same
  reparsed node are kept (reparser_test.rs).
- **Note**: the binder's ast_shims.rs carries a faithful copy of the
  GetAssignmentDeclarationKind family (off-limits this wave); the
  ast/utilities.rs port is the canonical home — dedup candidate.

**Validation** (local, CARGO_TARGET_DIR=/tmp/tsrs-rep,
CARGO_INCREMENTAL=0 + `-C debuginfo=0` under /tmp quota pressure):
`cargo test -p tsc-parser -p tsc-ast` green (15 + 60, incl. the 5 new
reparser/assignment-kind tests); testdata gate `--ignored` GATE: files
swept=176 parsed_ok=176 **panics=0** (the valueless-return fuzz seed
regression above was caught by this gate); `cargo clippy -p tsc-parser
-p tsc-ast --all-targets` zero warnings; dump twin (go-bench/tsc-bench)
byte-identical on `reachabilityChecksNoCrash1.ts` (the required
cold-parse file), `tsc/testdata/fixtures` (82 files), and the full
`tsc/testdata/tests/cases/compiler` sweep (6,890 files) — the reparse
wiring does not disturb the cold-parse path.

## 2026-10-08 crates/checker — the M5 checker spike (types.go + utilities.go + checker.go slices)
The SPEC §8 M5 gate crate: `crates/checker` proves §5.4 (checker-eradication)
and §5.5 (store ownership) with the arena-backed `Type`/`TypeData` layer, the
full `checker/types.go` port, the full `checker/utilities.go` port, and the
checker.go slices that make `getTypeOfSymbol` infer the literal `1` from
`const x = 1` (and, per Go widening semantics, `number` from `let x = 1` —
see the gate-semantics note below).

- **§5.4 (`Type.checker` eradication), worked as designed**: `Type` carries
  `checker_shard: u16` (identity only, types.rs); every Go `t.checker` read
  becomes an explicit `&Checker`/`&mut Checker` parameter
  (`compare_types(c: &Checker, t1: TypeId, t2: TypeId)` is the worked
  example — panics on cross-shard compares exactly like Go's
  `t1.checker != t2.checker` guard); `*Type` becomes `TypeId` (the arena
  Vec slot-0 dummy makes id == index), and Go's pointer writes on types go
  through `Checker::type_mut`.
- **§5.5 (store ownership), worked as designed**: one program-level symbol
  arena behind the checker-local `Program` trait
  (`options/source_files/bind_source_files/node_store/symbol/symbol_mut/
  alloc_symbol`); `new_symbol` allocates from the same `SymbolId` space the
  binder used (no rebasing at merge — `checker_created_symbols_share_the_
  program_id_space` pins it), and the per-checker link tables stay keyed by
  `SymbolId` in the checker's link stores (links.rs: NodeLinkStore
  direct-in-page + SymbolArenaLinkStore `Option<u32>` pages into a Vec
  arena, Go's two laziness shapes with unit tests).
- **Gate-semantics divergence (SPEC wording vs Go)**: SPEC §8 says the gate
  is "`getTypeOfSymbol` on `let x = 1` infers `1`", but Go/TS semantics
  widen `let` to `number` and keep the fresh literal only for `const`
  (`getWidenedLiteralTypeForInitializer` → `getWidenedLiteralType`, and
  literal widening stops at the union constructor for booleans —
  `booleanType = getUnionType([false, true])`, an M6 banner in the spike).
  The demo test proves BOTH paths exactly per Go: `const x = 1` → fresh
  literal `1` (Number value 1.0, fresh/regular pair wired), `let y = 1` →
  the initializer still CHECKS as the fresh literal `1` (cached in
  TypeNodeLinks), and `getTypeOfSymbol` widens to the `number` intrinsic
  without asserting the SPEC's stale wording. Booleans stop at the M6
  union boundary (asserted via catch_unwind).
- **`number_key` canonicalization (judgment call)**: Go's literal-type
  interning keys `float64` map entries (IEEE 754 equality: -0.0 == 0.0,
  NaN != NaN); the port keys `FxHashMap<u64, TypeId>` by bits with -0.0
  canonicalized to 0.0 bits and NaN cached separately (mirrors Go's map
  behavior with exact dedupe parity, `-0.0/NaN intern tests`).
- **Default-options strictNullChecks (Go parity, verified against
  tristate.go)**: Go's `TSUnknown = iota` (0) is the zero value, so
  `&CompilerOptions{}` gives `GetStrictOptionValue(StrictNullChecks) ==
  strict != TSFalse == true` — `createWideningType` returns the SAME type
  under default options and a distinct `ContainsWideningType` type only
  with `Strict = TSFalse` (both pinned by tests; the naive
  "default = not strict" assumption is wrong for Go's zero values).
- **Bannered seams (M6)**: `getAliasedSymbol`, `getPackagesMap`,
  `CreateModuleNotFoundChain`/`CreateModeMismatchDetails`, boolean/union
  and anonymous-type constructors in `new_checker`, `getWidenedType`
  (getContextualType/union machinery), every non-literal
  `checkExpressionWorker` arm, `getTypeOfSymbol`'s non-variable arms,
  GetErrorRangeForNode (deep scanner machinery — the diagnostic range
  computation in `new_diagnostic_for_node`), and `printer.EscapeString`
  (inlined as `escape_string` in utilities.rs) — all `TODO(port M6)`
  panics at Go's line positions.
- **Files**: `src/types.rs` (complete types.go port, ~2560 lines incl.
  Go-parity unit tests), `src/utilities.rs` (complete utilities.go port,
  ~3400 lines; FeatureMapEntry table rebuilt via a `feature_entries(&[..])`
  data helper — identical table data), `src/checker.rs` (the Checker
  struct + Program trait + new_checker intrinsic-creation slice +
  getTypeOfSymbol chain + literal intern machinery + resolution-cycle
  detector + unit tests), `src/links.rs` (links.go), `src/ast_shims.rs`
  (Go ast.go helpers not yet exported by tsc-ast: the generated
  `Initializer()`/`Expression()` store-carrying dispatchers,
  QuestionToken/PostfixToken, outer-expression kinds, entity-name/text),
  `src/todo_shapes.rs` (M6/M7 placeholders: TypeMapperId, Tracer, FlowState,
  ...), `tests/spike.rs` (the gate demos + §5.4/§5.5 proofs).
- **Go-parity judgment calls inside utilities.rs**: `isReservedMemberName`/
  `isLateBoundName` use the crate's `\u{FE}`-code-point sentinel convention
  (tsc_ast::symbol) instead of Go's raw-0xFE byte checks (invalid UTF-8 in
  Rust strs); `getDeclarationNodeFlagsFromSymbol` reads uncached (Go's
  single-entry memo stays &mut-only in
  `get_combined_node_flags_cached`) so the utilities family can take
  `&Checker`; `getEnclosingContainer` inlines the parent walk (the
  container-flags table needs the `NodeId`, while `find_ancestor`'s
  visitor sees `&Node`); the `forEachYieldExpression` child walk is a
  hand-rolled payload enumeration shim (`collect_children_shim`, M7
  banner) pending the generated ForEachChild table.
- **Validation** (local, CARGO_TARGET_DIR=$HOME/.cargo-target/m5c,
  CARGO_INCREMENTAL=0 + `-C debuginfo=0`): `cargo test -p tsc-checker`
  green — 25 lib tests (types/links/checker/utilities Go-parity pins) +
  7 spike tests (the two literal gates, boolean union boundary, intrinsic
  seeding order 1..5 + widening pairs, §5.5 id-space proof, cycle-detector
  balance); `cargo clippy -p tsc-checker --all-targets` zero warnings;
  `cargo check --workspace --exclude tsc-compiler` green (tsc-compiler is
  being written concurrently by the sibling M5-compiler worker — its
  untracked src files were changing seconds-by-second during this wave);
  `cargo test -p tsc-ast -p tsc-binder` green (60 + 15, no regressions).

## 2026-10-08 rust/crates/{outputpaths,bundled,compiler} — M5 program layer (internal/outputpaths, internal/bundled, internal/compiler @ ec47d33c23e464a17cdf2475632cba629bee8763)
Three crates, one Go-package-per-crate: `tsc-outputpaths`,
`tsc-bundled`, `tsc-compiler` (all registered in the workspace; the
compiler crate depends on the other two plus outputpaths' host trait).

### tsc-outputpaths (outputpaths.go + commonsourcedirectory.go)
- **Files**: `src/outputpaths.rs` (OutputPathsHost trait — Go's interface
  made object-safe, `Arc<dyn Fs>` precedent), `src/commonsourcedirectory.rs`
  (`get_common_source_directory` with Go's lazy `files` thunk called eagerly
  at each use site + the nil-able `checkSourceFilesBelongToPath` as
  `Option<&mut dyn FnMut>`), `tests/outputpaths_test.rs` (5 tests: root-dir
  canonicalization, build-info name, source-in-new-dir). Go's
  `GetCommonSourceDirectory(options, files, ...)` callback shape keeps
  `#[allow(clippy::type_complexity)]` (inherent to Go's nil-able func value).

### tsc-bundled (bundled.go + embed + libs generated)
- **Files**: `src/lib.rs` + `src/embed.rs` (the always-embedded variant —
  Go's `noembed.go` build-tag counterpart is not ported; the port always
  embeds, `EMBEDDED` is a `true` const), `src/embed_generated.rs` +
  `src/libs_generated.rs` (generate.sh output — `include_str!`-driven where
  Go is `//go:embed`-driven; `sh generate.sh` reproducible), `tests/
  bundled_test.rs` (2 tests incl. the Go lib-list parity).
- `WrapFS` wraps unconditionally (the second Go sentence is dead without
  noembed).

### tsc-compiler (program.go, filesparser.go, fileloader.go, checkerpool.go, fileInclude.go, includeprocessor.go, processingDiagnostic.go, host.go, emitHost.go, projectreferencefilemapper.go, projectreferenceparser.go, projectreferencedtsfakinghost.go, emitter.go program-level bits)
- **File map** (one Go file = one Rust file, plus two crate-local helpers):
  lib.rs (pkg.go + the seam inventory), program.rs, files_parser.rs,
  file_loader.rs, checkerpool.rs, file_include.rs, include_processor.rs,
  processing_diagnostic.rs, host.rs, emit_host.rs,
  project_reference_file_mapper.rs, project_reference_parser.rs,
  project_reference_dts_faking_host.rs, emitter.rs (program-level bits:
  EmitOnly, SourceFileMayBeEmittedHost, sourceFileMayBeEmitted,
  getSourceFilesToEmit, isSourceFileNotJson), ast_shims.rs (crate-local —
  Go ast/utilities.go, ast/parseoptions.go, core/core.go, ast/ast.go helpers
  the loader needs; dedup candidates against tsc-ast), diagnostics_
  collection.rs (Go ast/diagnostic.go's DiagnosticsCollection; the struct
  itself already lives in tsc-ast).
- **Bannered seams (each PORT-noted at its site)**:
  - tracing not ported (no tracing crate): the loader drops the tracing
    hooks; CompilerHost keeps the Trace callback; ProgramHosts drops
    `Tracing`.
  - checker wave: `Checker = ()` alias; CheckerPool is a TODO-shaped trait
    (GetChecker without the release closure); the checkerPool struct
    (per-checker mutexes, NewChecker calls, fileAssociations) deferred with
    it. The pure FENNEL functions it is built from are fully ported
    (checkerpool.rs — the checkerpool_test.go surface).
  - M7 emitter boundary: emitHost.rs holds the program-level surface only
    (the printer/declarations halves are the M7 boundary); emitter.rs is the
    program-level bits only; Program.Emit is a TODO shape.
  - content-mapper: the host machinery (contentmapper.Project, transform
    pipeline) is not ported; content-mapper paths degrade to non-mapped
    loads exactly as Go's zero-config case; `ContentMapperExtensions`
    carried, unused (dead-code-allowed seam field).
  - tsoptions' AST-config surface (ConfigFile syntax walks,
    GetMatchedFileSpec, ParseInputOutputNames/SourceToProjectReference/
    OutputDtsToProjectReference) is not ported: config-positioned
    diagnostics degrade to compiler diagnostics (Go's nil-ConfigFile arm),
    and the mapper's source/output maps stay empty until that wave.
  - cachedvfs + symlinks not ported: the project-reference dts-faking host
    is the identity host; `resolveSymlink` degrades to the not-a-symlink
    caching (Store(path, nil)).
  - UpdateProgram/ReuseProgram not ported (owned `files` + content-mapper):
    the affected Go tests are skipped (see the test notes below).
  - verifyCompilerOptions/collectContentMapperOptionDiagnostics degrade to
    no-op seams (the AST-config wave owns them).
- **Judgment calls**:
  - **TaskArena**: Go's `*parseTask` graph (subTasks/redirectedParseTask/
    loadedTask pointers) becomes `TaskArena` (`Mutex<Vec<ParseTask>>`) +
    stable indices; every access happens under the path group's mutex,
    `with_task`/`with_task_read` check a task out and put it back inside the
    critical section (no unsafe aliasing — the workspace forbids unsafe
    code; Go's shared-pointer reads serialize identically through the
    checkout).
  - **ScopedWorkGroup** (files_parser.rs): Go's `w.wg.Queue(func(){...})`
    closures capture `&FilesParser`/`&FileLoader`/the group; `core.WorkFn`
    is `Box<dyn FnOnce() + Send>` whose trait-object default is `'static`,
    so the group is a crate-local `ScopedWorkGroup<'env>` with Go's
    queue/runAndWait surface running the closures on `std::thread::scope`
    (LIFO drain in the single-threaded variant, exactly like core's group;
    queue-during-run works; the group is shared as an `Arc` so the queued
    closures can clone it for the recursive `start` — Go's closures capture
    the group pointer the same way).
  - **pending sub-tasks deferred push**: Go pushes resolved imports while
    walking the file's module names; the port collects `pending_sub_tasks`
    and pushes after the walk (borrowck: the file is `&mut`-borrowed out of
    the task while `push_sub_task` needs the task) — same order.
  - **pointer-identity maps**: Go's `map[*FileIncludeReason]T` keys become
    `Arc::as_ptr as usize` keys (`reason_diagnostics`,
    `reason_to_reference_location`, `include_reason_to_related_info`).
  - **FileDiagnostic pairing**: Go's diagnostics are pointers into the
    owning source file; the port pairs each with its `file_name: Option<
    RootedFilePath>` (the resolver's diagnostics carry a NodeId file that
    cannot resolve to a path — parser file_id gap — so they're paired with
    the containing file's name at collection).
  - **ResolvedProjectReference newtype**: the orphan rule blocks
    implementing `module::ResolvedProjectReference` for tsoptions'
    ParsedCommandLine from this crate, so resolved configs travel as
    `Arc<ResolvedProjectReference>` (a newtype; the vfs-crate wrapper
    precedent). `Option<Arc<_>> → Option<Arc<dyn _>>` is not a coercion
    site, so `redirect_as_dyn` unsizes per-entry at a binding. The newtype
    carries a manual `Debug` (ParsedCommandLine derives none).
  - **PackageId map keys**: `module.PackageId` derives no `Hash` (the
    module crate is outside this wave's touch scope), so
    `package_id_to_source_file` is keyed by a composite string of every
    field (`package_id_key`) — same identity semantics as Go's map key.
  - **ModuleKind/ModuleResolutionKind are plain enums** in
    `tsc_core::options_generated` (compileroptions.rs re-exports only the
    `ResolutionMode` alias + consts); comparisons (`Node16 <= kind <=
    NodeNext`) rely on the derived Ord matching Go's numeric ordering.
  - **ModuleDetectionKind::None**: the Rust enum carries a `None` sentinel
    for the absent option; it maps to Go's zero value (Legacy behavior) in
    `get_external_module_indicator_options`.
  - **ScriptTarget Stringer**: Go's `GetEmitScriptTarget().String()` — the
    core crate has no ScriptTarget stringer module; the derived `Debug`
    spelling matches the Go stringer's names exactly
    (`_ScriptTarget_name_0/1`: "None", "ES5", "ES2015".."ESNext"), verified
    against scripttarget_stringer_generated.go.
  - **tsoptions surface deltas**: `get_supported_extensions`/
    `get_supported_extensions_with_json_if_resolve_json_module` +
    `ExtendedConfigCache` live in `tsc_tsoptions::tsconfigparsing` (not
    re-exported at the tsoptions crate root); the extension groups are
    `Vec<Vec<String>>` so `extension_is_one_of(&[&str])` call sites map
    each group to borrowed slices first.
  - **extractUnresolvedImportsFromSourceFile**: Go checks
    `resolution.Extension` (not the cache-key name) against the supported
    extensions — ported faithfully after a compile-pass misreading.
  - **new_compiler_host**: `contentMapperProject` dropped; nil trace →
    no-op; the nil extended-config cache is an always-miss closure in
    tests.
- **Tests** (`tests/checkerpool_test.rs` 7, `tests/program_test.rs` 4):
  - checkerpool: all pure-FENNEL tests ported verbatim
    (getCheckerAssociationBaseWeight/Policy/Order/Weights/Associations,
    shouldPrioritizeSourceFiles) +
    TestGetSourceFileForResolvedModuleUsesResolvedPath (Go's `Program`
    literal → `Program::default()` + pub `files`/`files_by_path`
    assignment; pointer-identity assert → `ptr::eq`).
  - program: **TestProgram** fully ported — the three ordering corpora
    (BasicFileOrdering/FileOrderingImports/FileOrderingCycles) assert the
    EXACT program order against the 96-entry esnext lib list + the load
    order, over `bundled::wrap_fs(vfstest::from_map(...))`; the port
    constructs the config by parsing a written tsconfig.json because the
    Rust tsoptions has no `NewParsedCommandLine(&opts, files, ...)` and
    `file_names` is not pub (`files` entries are included verbatim with no
    existence check, exactly as Go's literal list — same dataflow through
    the M4 config-parsing surface; target "esnext" replaces the raw
    `core.ScriptTargetESNext`).
    TestImportSourceProgram: the initial construction round ported
    (source-phase imports skip resolution; resolved-modules empty;
    unresolved imports empty); the UpdateProgram rounds + the
    collectPackageNames assertion are skipped with the not-ported seam.
    TestIncludeProcessorDiagnosticsWithMissingFileCasing ported (the
    casing diagnostic must not panic through GetProgramDiagnostics; a Rust
    test panic fails the test, which is Go's recover-based assertion).
    TestIncludeReasonDiagnosticsAreProgramLocal adapted: Go asserts
    *ast.Diagnostic pointer identity; the port's to_diagnostic returns
    values from a per-program cache, so the identity assertion becomes the
    cache-locality check (each program's cache holds exactly its own
    entries) + value-determinism by diagnostic signature
    (code/category/file name).
  - Skipped Go tests (PORT-noted at the file header): TestProgramSharedData
    (reflection — Rust's ownership rules encode the data-only discipline),
    TestProgramHostsAndFactories + TestClonedProgramProjectReferenceResolution
    (ReuseProgram/UpdateProgram + tracing + symlinks seams),
    BenchmarkNewProgram (the benchmark harness is the separate
    bench-harness crate, outside this wave's touch scope; the ported
    TestProgram covers the same constructions).
  - `str::trim_prefix` is unstable on this toolchain (nightly
    trim_prefix_suffix rework); the test uses `strip_prefix` + fallback.
- **Validation** (local, `CARGO_TARGET_DIR="$HOME/.cargo-target/m5a"`,
  `CARGO_INCREMENTAL=0 RUSTFLAGS="-C debuginfo=0"` — iteration aid per the
  §9.2 note above): `cargo test -p tsc-compiler -p tsc-outputpaths -p
  tsc-bundled` green (4+7 compiler + 5 outputpaths + 2 bundled);
  `cargo check --workspace` green; `cargo clippy --workspace --all-targets`
  zero warnings (includes a pre-existing type-complexity fix in
  outputpaths' callback parameter and an unused-import fix in the bundled
  test); `cargo test -p bench-harness -p tsc-binder` green (no
  regressions). The workspace enforces no-unsafe-code — the crate contains
  none (`with_task_read` was rewritten checkout/put-back instead of the
  unsafe alias Go's shared pointer admits).

## M5 gate — the `program` parity twin (bench-harness program_seam, go-driver, two compiler fixes)

Closes the M5 gate (SPEC §8 M5: "`tsc <file>` on a trivial program loads,
binds, parses `lib.d.ts`, and reports *parse-level* diagnostics
(checker not required)") as a byte-diffable harness — and becomes the
parity rail the M6 checker milestone rides: a new `program` subcommand on
BOTH drivers — `go-bench program --corpus <dir> | --file <path>` /
`tsc-bench program --corpus <dir> | --file <path>`. For every parseable
script file in the corpus (the `symbols` twin's walk rule — same skip
dirs, every file whose scriptKind is known, raw-path sort), each driver
builds a ONE-FILE `Program` exactly the way tsc/internal/compiler's
program_test.go drives it, binds it, and emits one NDJSON line per root;
`diff` is the verdict.

**Call shape (both sides)**: `vfstest.FromMap` seeded with the corpus
tree (all known-scriptKind files at their real absolute paths) + a
synthetic tsconfig `<corpus>/__program__.tsconfig.json` with text
`{"compilerOptions":{"target":"esnext"},"files":["<root>"]}` →
`bundled.WrapFS` (the lib.d.ts overlay) → `GetParsedCommandLineOfConfig-
File` over the same FS (PORT: Go's tests also construct configs via
`tsoptions.NewParsedCommandLine(&opts, files, ...)`; the Rust tsoptions
has no such constructor (`file_names` is not pub), so BOTH drivers take
the written-tsconfig route program_test.rs established — same dataflow
through the M4 config surface; the synthetic config mirrors Go's
`CompilerOptions{Target: ScriptTargetESNext}` corpus shape) →
`NewCompilerHost` over a **counting FS wrapper** (driver-local, identical
both sides, records every DISTINCT successfully-read path — the "loads
... parses lib.d.ts" half of the gate made countable) → `NewProgram` →
`BindSourceFiles`. Corpus mode seeds the whole walked tree so in-matrix
imports resolve inside the VFS; `--file` mode seeds only that file
(showconfig's dual-mode shape). Case sensitivity: CaseSensitive on BOTH
drivers (unlike Go's TestProgram corpus — machine-independent parity).
Exit codes agree: 0 ok, 1 unreadable root, 2 usage/unknown script kind.

**Record format** (one line per root; fields both sides emit
identically): `{"file":"...","ok":true,"config_errors":N,"loaded":N,
"parsed":N,"bound":N,"symbols":N,"duplicates":N,"parse_diag":[...],
"program_errors":M,"unresolved":K,"shape_hash":"<16 hex>"}` —
`config_errors` = `GetConfigFileParsingDiagnostics` length; `loaded` =
distinct host-FS reads (lib chain + program files + package.json
probes); `parsed` = `GetSourceFiles` (program order, also folded into
the hash); `bound` = files with `IsBound()` after `BindSourceFiles`;
`symbols` = the binder's per-file `SymbolCount` summed (Go's
`Program.SymbolCount()` adds the checker's count through
`ForEachCheckerParallel` — checker-scope, M6 — so BOTH sides sum the
binder count instead, PORT-noted); `duplicates` =
`DuplicateSourceFiles` length; `parse_diag` = ONLY parse-level
diagnostics per file (the SPEC gate) in the dump twin's record shape
plus a `file` key, in program file order then per-file
`Diagnostics()`→`JSDiagnostics()` order — i.e.
`GetSyntacticDiagnostics`' core concat; Go additionally appends
`getAdditionalJSSyntacticDiagnostics` for unchecked-JS files (deferred
in the port with the visitor/SubtreeFacts surface), so BOTH drivers
emit the raw parser+JSDoc set — no checker-level or JS-extras fields
exist in the record (the M6 boundary rule: agree by construction, note
the extension); `program_errors` = `GetProgramDiagnostics` (option
verification + include-processor GLOBAL diagnostics);
`unresolved` = `GetUnresolvedImports` (count in the line, sorted names
in the hash). shape_hash: FNV-1a 64 over the fold lines documented at
program_seam's `hash_lines` (`file=`/`config_errors=`/`loaded=` +
sorted `loadedfile=`/`parsed=` + program-order `programfile=`/
`bound=`/`symbols=`/`duplicates=`/`diag=<file>|<key>|<code>|<pos>|
<end>|<args joined \x01>`/`program_errors=`/`unresolved=` + sorted
`unresolvedname=`; vectors — including published FNV-1a "a"/"b"/
"foobar" and order-sensitivity checks — in the module's tests).
Panics (program build/bind) emit ok:false lines; any ok:false IS a
finding (the matrix is panic-free). Default options NOTE: with
`{"target":"esnext"}` and module unset, the parsed config's effective
moduleResolution is Bundler, so `resolveJsonModule` defaults TRUE —
`.json` roots are therefore VALID programs (99 esnext libs + root)
and relative `./data.json` imports resolve; both sides construct the
same config, so parity holds by construction (empirically confirmed).

**Two real compiler-crate divergences found by the twin (fixed
mirroring Go, program.rs):**
- `get_unresolved_imports` (Go `extractUnresolvedImports`): the port
  guarded the sweep with `!uses_uri_style_node_core_modules` AND
  `!skip_type_checking(file)` — NEITHER exists in Go, and Go never
  SETS `p.usesUriStyleNodeCoreModules` (it stays the zero Tristate
  forever), so the port's guard made `get_unresolved_imports` return
  `[]` unconditionally. The twin's missing-import fixture (bare
  `totally-missing-package`) surfaced it: Go K=1, port K=0. Fixed to
  Go's unfiltered every-file sweep (program.rs, PORT note at the site).
- `get_program_diagnostics` (Go `GetProgramDiagnostics`): the port
  folded the include processor's FULL collection
  (`get_diagnostics()` = global + per-file) where Go folds only
  `GetGlobalDiagnostics()`, and concatenated in a different order —
  file-scoped include diagnostics (referenced-location diagnostics
  attached through `processingDiagnostic.toDiagnostic`'s
  `diagnosticAt` path) would leak into the program set. Fixed to Go's
  exact shape: programDiagnostics, contentMapperDiagnostics, then the
  include processor's GLOBAL diagnostics, through
  `sort_and_deduplicate_diagnostics` (program.rs, PORT note at the
  site; `contentMapperOptionDiagnostics` is the not-ported
  content-mapper seam — empty, matching Go's nil-project case).

**Fixtures committed** under rust/bench/gates/program-matrix/ (15
roots): trivial `let x = 1;`; parse-error (2 diagnostics, args
exercised); in-matrix import (`./imported`); `/// <reference
lib="es2020" />` lib pull; bare missing-module import (resolution
failure — unresolved=1); UMD-global (`export as namespace` + global
augmentation); empty file; relative `./data.json` import + the
`.json` root itself; JSDoc typedefs/callback/malformed types (the
reparse path); JSX; a `.d.ts`-only root; and a jsconfig-companion
directory (jsconfig.json — a JSON root — beside a .ts file). No
checker-crate surface is touched anywhere.

**Validation** (local, `CARGO_TARGET_DIR="$HOME/.cargo-target/m5gate"`,
`CARGO_INCREMENTAL=0 RUSTFLAGS="-C debuginfo=0"`;
`GOTMPDIR/GOCACHE=$HOME/.m5gate-scratch` — /tmp is quota-limited on this
box): matrix runs byte-identical `go-bench program` vs `tsc-bench
program` (diff empty, 15/15 lines ok:true) and determinism double-runs
byte-identical on both drivers; `--file` mode byte-identical + exit
codes agree (0/1/2); real-world spot checks byte-identical over
`packages/typescript/lib` (5 roots: .js/.cjs/.d.ts/.d.cts) and
`packages/typescript/scripts` (2 real .ts roots, one pulling an
in-dir import); `cargo test -p bench-harness -p tsc-compiler -p
tsc-bundled -p tsc-outputpaths` green (29 + 7+4 + 2 + 5; the 5 new
program_seam tests include hand-verified FNV-1a vectors for the new
fold); `cargo clippy -p bench-harness -p tsc-compiler --all-targets`
zero warnings in both crates; `go vet` (GOTOOLCHAIN=auto) clean on the
go-driver; dump/showconfig/resolvereact/symbols quick regressions
(fixtures + tsconfig-matrix + resolver-matrix corpora) all still
byte-identical.

**M6 extension notes**: the twin deliberately emits NO checker-level
fields — `Program.SymbolCount()`'s checker half, semantic diagnostics,
`GetSemanticDiagnostics`/TS2307-style unresolved-import diagnostics,
and the unchecked-JS `getAdditionalJSSyntacticDiagnostics` extras are
checker-wave surfaces (the last also needs the visitor/SubtreeFacts
port). M6 should extend the record with: checker diagnostics per file
(keyed by file, dump shape), checker symbol/type counts, and — once
the AST-config wave lands — real-dir runs driven by a project's OWN
tsconfig (the twin currently synthesizes its config; a real tsconfig
needs `ConfigFile` syntax positioning for option diagnostics).

## M6 Wave 1 — checker periphery (mapper/jsdoc/exports/symboltracker/stringer)

Wave A of the M6 checker port (the wave/slice order is
`rust/M6-DISPATCH.md`; the slice survey lives there too). Files landed
in the 10:28/10:43 auto-checkpoints (0862775f25/7a55811d50) — worker did
no git; orchestrator verified on exactly that tree before recording.

- **mapper.rs (929 ln)** — the instantiation-mapper machinery:
  `TypeMapperKind` + `TypeMapperData` (Simple/Array/ArrayToSingle/
  Deferred/Function/Merged/Composite/Inference), the two-phase
  `plan()/MapAction` dispatch (Rust closures cannot re-borrow `&mut
  Checker` mid-map the way Go closures capture the mapper — the plan
  phase captures, the run phase re-borrows; PORT-noted at the trait),
  every Go factory + `combine/merge/prepend/append`, and
  push/popActiveMapper with **Go's slice-spare-capacity reuse** (a Vec
  with the same capacity trick, PORT-noted) and `clear_cached_inferences`
  landed early because the mapper caches need it.
- **jsdoc.rs (424 ln)** — `checkUnmatchedJSDocParameters` (the
  `arguments`-referencing arm dormant behind the S4
  `contains_arguments_reference` stub), tag walks, the jsdoc_cache seam.
- **exports.rs (893 ln)** — the ~70 exported checker wrappers: real
  bodies where the callee already exists, permissive stubs with
  `TODO(port M6 <wave>)` banners otherwise; Go's `Get*/get*` pairs
  collapsed with the pairing documented in the header.
- **symboltracker.rs (368 ln)** — `SymbolTrackerImpl` over
  `NodeBuilderContext` (todo_shapes gained the real shape + the
  `SymbolTracker` trait); `onDiagnosticReported` runs BEFORE the nil
  check, exactly as Go orders it.
- **stringer_generated.rs (82 ln)** — the generated `SignatureKind`
  stringer table, index array `[0,17,39]`, numeric fallback and all.
- **Cross-wave bug found by the wave's own tests** (fixed in
  `rust/crates/parser/src/reparser.rs`, outside W1 scope but
  Go-mandated): `find_matching_parameter` lacked Go's
  `parameterTag.Name().Kind == KindIdentifier` guard
  (tsc/internal/parser/reparser.go:639) — it panicked on `@param ns.b`.
  Guard added; tsc-parser 15 passed.

**State**: `checker.rs` gains `type_mappers: Vec<TypeMapperData>` +
`active_mappers` + `active_type_mappers_caches` + `inference_contexts`
and the W1 callee stubs (`instantiate_type`→identity [S7],
`get_type_from_type_node`→errorType [S5], `is_array_type`→false [S7],
`contains_arguments_reference`→false [S4], `get_inferred_type`→
errorType) — each banner names the wave that replaces it.
`todo_shapes.rs`: real `InferenceContext`/`InferenceContextId`,
`TrackedSymbolArgs` (SymbolId-based; RefCell invariance PORT),
`UnionReduction`. `utilities.rs`: the full `compare_type_mappers`
port (kind-wise Simple/Array/Merged payload compares — mapper IDENTITY
is `TypeMapperId` equality, mirroring Go's pointer identity).

**Validation**: tsc-checker 60 lib + 7 integration (35 new: 13 mapper,
5 jsdoc, 6 exports, 8 symboltracker, 3 stringer — Go had no tests for
these files; checker_test.go's 2 tests exercise S6 surface only);
tsc-parser 15 passed (reparser fix); clippy zero warnings
(tsc-checker + tsc-parser); program/symbols/dump twins byte-identical
post-change (the reparser fix warranted the dump re-run); bench-harness
builds (it does not depend on tsc-checker).

## 2026-10-08 crates/tsoptions + crates/compiler — M6 gap-1: the config-node
## diagnostic surface (removed-option diagnostics TS5102/TS5108 + the
## include-reason matched-spec arms)
Go `verifyCompilerOptions` (tsc/internal/compiler/program.go:909) positions
option diagnostics on the tsconfig AST (`CreateDiagnosticForNodeInSourceFile`),
and `fileInclude.go`'s RootFile reason arms report the as-written `files`/
`include` specs. The wave ports the minimum that surface needs:

- **crates/tsoptions/src/tsconfigparsing.rs (new ~230 ln section)** —
  `TsConfigSourceFile` + `new_tsconfig_source_file_from_file_path` (Go:
  `NewTsconfigSourceFileFromFilePath`; the config text parses as a JSON
  SourceFile through `tsc-parser`, ScriptKind::JSON) +
  `get_ts_config_object_literal_expression`/`for_each_property_assignment`/
  `for_each_ts_config_prop_array`/`get_ts_config_prop_array_element_value`/
  `get_options_syntax_by_array_element_value`/`get_callback_for_finding_property_assignment_by_value`
  (Go's config-syntax walks, exact "return callback result, even None"
  semantics). `ParsedCommandLine` gains Go's `ConfigFile` field
  (`config_file`), populated on the `get_parsed_command_line_of_config_file_path`
  route ONLY — the AST is the position surface; values/errors still come from
  the text parser (Go's `convertConfigFileToObject` AST→value conversion is
  the deferred full AST-config wave). Plus `config_name`/`get_matched_file_spec`/
  `get_matched_include_spec` (Go parsedcommandline.go) over the new
  `file_spec_by_path` + before-substitution spec lists.
- **crates/tsoptions/src/ast_shims.rs (new, 96 ln)** — `TryGetTextOfPropertyName`
  + the `Node.Text()`/`Expression()` arms the walks read (Go ast/utilities.go;
  ast crate doesn't port them yet — compiler/binder ast_shims precedent, dedup
  candidate).
- **crates/tsoptions/src/errors.rs** — `CreateDiagnosticForNodeInSourceFile`
  is the REAL positioned form now (`scanner::skip_trivia(text, node.Loc.Pos())`
  .. `node.End()`); the OrCompilerDiagnostic variant degrades exactly on Go's
  nil file/node inputs.
- **crates/compiler/src/program.rs** — `verifyCompilerOptions`' removed-option
  arm (the "Removed in TS7" block, Go order preserved: baseUrl/outFile/target
  ES5/module AMD|System|UMD/moduleResolution Classic|node10/alwaysStrict
  false/esModuleInterop false/allowSyntheticDefaultImports false/
  downlevelIteration) through the `OptionDiagnosticContext` closure-bundle
  mirror; baseUrl's `Use_0_instead` chain carries Go's
  `encoding/json`-escaped paths suggestion (hand-rolled `json_marshal_string`
  with HTML escaping ON — compiler crate has no serde_json). The REMAINDER of
  `verifyCompilerOptions` (strict/composite/paths/jsx/moduleKind arms,
  verifyProjectReferences, emit-path uniqueness) is PORT-noted at
  `verify_compiler_options` with its plug points (the context's
  `create_diagnostic_for_option` + Go's createDiagnosticForOptionName/
  createOptionValueDiagnostic fold).
- **crates/compiler/src/file_include.rs + include_processor.rs** — the RootFile
  `computeDiagnostic` arm (Part of 'files' list / Matched by include pattern /
  default-include / Root file) and `toRelatedInfo`'s files/include/types/lib/
  target arms through the config AST; `getCompilerOptionsObjectLiteralSyntax`
  memo restored (Go includeprocessor.go).

**PORT (Send/Sync, the wave's one forced deviation)**: Go shares the frozen
`*ast.SourceFile` config AST across worker threads freely; Rust's
`tsc_ast::SourceFile` carries parser-era `Cell` fields (not `Sync`, not
`Arc`-shareable) while `ParsedCommandLine` must stay `Send + Sync` (the
loader's worker groups share it) — and the workspace forbids `unsafe`.
The config AST therefore rides behind a `Mutex` inside `TsConfigSourceFile`,
read through `lock_source_file()` guards. Deadlock rule (both lock sites
documented): `to_related_info` computes the include processor's
`getCompilerOptionsObjectLiteralSyntax` memo BEFORE taking the guard (the
memo walk locks too).

**Validation**: tsc-tsoptions 45 (41 + 4 new configsyntax tests — Go's
parsedcommandline_test.go "Config file specs are owned by the parsed command
line" + the walk/position contracts); tsc-compiler 7 lib + 5 integration
(+1 removed-option arm test); clippy zero warnings both crates; 300-slice twin
divergence 81 → **1** (gap 2's asyncArrowFunction_allowJs TS8009 alone);
fixture twin byte-identical 9/9 match; program-matrix twin byte-identical
(15 records); see rust/CONFORMANCE.md for the climb re-measurement.

## M6 Wave 2 — the engines (relater + inference + tracer/tracing)

Two worker sessions (the first landed relater.rs + tracer.rs and drove the
compile loop from 52 errors to zero; the resumed session landed inference.rs
+ the tracing port + the trace-parity proof). Files rode the auto-checkpoints
where noted; final state is e8d9adc098.

- **relater.rs (~8,760 ln)** — the complete relater.go assignability engine:
  `isRelatedTo` + the structured-type relation ladder, variance, the
  mapper-parameterized recursions, comparability, excess-property checks.
  ~40 hand-applied borrow restructurings, all Go-shaped (pre-cloned
  constituent lists for closure iteration, hoisted nested calls, Option
  reborrows through `as_deref_mut()`, snapshot locals around simultaneous
  &mut/by-value args). Correctness finds: a not-yet-registered
  `SignatureId` guard (Go nil-signature semantics) in
  `get_type_predicate_of_signature`; the exports test flipped with the real
  relater wired.
- **inference.rs (3,368 ln)** — the full inference.go port: InferenceKey, the
  value-stack state pool (Go's intrusively-linked free list → `Vec`,
  `clear_for_pool` resets exactly Go's fields — priority and contravariant
  survive), the inferTypes/inferFromTypes ladder (union two-pass,
  intersection reduction, the structural switch, all tuple middle-arity arms
  incl. variadic middles), the newInferenceContext family, and the real
  `getInferredType` (covariant/contravariant + prefer-covariant policy;
  NO_DEFAULT→silentNever / ANY_DEFAULT→any / else unknown — and the
  signature-less fall-through to unknownType lives INSIDE the
  `n.signature != nil` branch, pinned by test).
- **InferencePriority is a signed i32, not a flag set** — CIRCULARITY=-1
  sorts below every value and the engine `min()`s over it. The W1-era
  provisional seams had wrong bits (NO_CONSTRAINTS|ALWAYS_STRICT are
  `1<<9|1<<10`); corrected when the real engine landed and the
  conditional-vs-conditional relater arm rewired through it.
- **tracer.rs (590) + tracing.rs (1,118)** — the trace machinery.
  `marshal_legend` had to adopt Go's `json.MarshalIndent` form (2-space
  indent, `": "` separators) — the compact form was the one byte-divergence;
  trace.json matched after the tracer test locked the observable contract.
- The W1 `InferenceContext`/`InferenceInfo` todo_shapes stubs are replaced
  with `pub use crate::inference::{…}` (the mapper's three-field contract —
  type_parameter/is_fixed/inferred_type — kept); `checker.rs` gains
  `free_inference_states` + the reverse-mapped caches and drops the W1
  `get_inferred_type`/`infer_from_intra_expression_sites` stubs.

**Stub directions bannered for the S waves**: S7 (getSimplifiedType,
instantiateTypeWorker, createTuple/TypeReference/ArrayType identities, the
reverse-mapped family, filterType), W3 (skipDirectInferenceNodes,
getContextualType drain), S5 (bigint literal parsing), S4
(declaration-ancestor walk), S10 (index info).

**Validation**: tsc-checker 74 lib + 7 integration (+13 new: the priority
ladder values incl. the CIRCULARITY sentinel, pool round-trip, invokeOnce
guard, clone/filter contracts, covariant+contravariant resolution through
getInferredType, the tracer fixture test); workspace clippy zero; workspace
tests all green; dump/symbols/program twins byte-identical; **Go-reference
trace byte-diff EMPTY** (trace.json + legend.json, deterministic mode, the
same event sequence as tracer_test.go's TestTracerPushPreservesEndArgMutations).

## M6 S1 — the checker entry & expression spine (checker.go 2240–2585 + 7539–8060 + 14222–14310 + NewChecker tail)

`rust/crates/checker/src/spine.rs` (new, ~3,590 ln) carries the slice (SPEC
§4.2's checker.go→checker.rs map is honored through the module-head PORT
note: checker.rs holds the spike core; the S1 wave rides spine.rs, later
S-waves continue per-slice). Small supporting edits rode it:
`utilities.rs` (`is_canceled`/`check_not_canceled` — Go's ctx becomes a
host-set `Checker::canceled` flag), `inference.rs`
(`is_skip_direct_inference_node` reads the now-real
`skip_direct_inference_nodes` OrderedSet; `is_from_inference_blocked_source`
made real), `todo_shapes.rs` (`FlowLoopInfo`).

- **checkSourceFile family + the unreachable machinery** (2240–2528):
  full statement dispatch (~55 arms, every leaf bannered with its owning
  slice — S3/W3 for statement arms, S4/W5 for expression arms);
  `checkSourceElementUnreachable`'s region scan (reportedUnreachableNodes
  dedupe, parent-statement forward scan), `withinUnreachableCode`
  save/restore through checkSourceElement.
- **Deferred nodes (M6-DISPATCH risk 2) — drain fidelity**: Go's drain is a
  LIVE index loop over `OrderedSet.Keys` ("We use a for loop here to ensure
  we enumerate new items added during iteration"), with the set REPLACED
  after the loop. The Rust OrderedSet can't mutate under a borrowed
  iteration, so `check_deferred_nodes` takes the set into a FIFO and, after
  each node, re-pulls whatever the drain queued into the fresh set —
  identical enumeration order and end state (empty set), pinned by test.
- **The expression spine** (7539–8060): getTypeOfExpression (flowTypeCache
  + flowInvocationCount gate — the flow engine is W3, so the cache stays
  cold exactly like Go on flow-free expressions), getQuickTypeOfExpression
  (literal arm real; call/new/await arms are optimizations routed through
  S-bannered stubs), checkNonNullExpression/Type/WithReporter (S10 PORT
  note: Go's getTypeFacts flow lattice → the pure-flags facts; the
  undefined/null SPLIT bits don't exist in the relater's carried TypeFacts
  yet — the reporter folds to the combined IS_UNDEFINED_OR_NULL message
  until S10 re-bases the bits), checkExpressionWithContextualType (the
  contextual/inference stacks push/pop Go-shaped),
  instantiateTypeWithSingleGenericCallSignature (reachable prefix exact;
  the deep tail bannered S8/S9), checkExpressionWorker (~45 arms; the
  spike's literal/keyword arms ride unchanged).
- **Diagnostics entries** (14222–14310): the REAL bodies for
  error/addDiagnostic/addSuggestionDiagnostic/errorOrSuggestion/
  errorAndMaybeSuggestAwait (the spike's W1 no-op stubs deleted from
  checker.rs); `DiagnosticsCollection` (ast/diagnostic.go) ported checker-
  locally (tsc-checker can't depend on tsc-compiler's copy — the compiler
  depends on the checker; hoist note: moves to ast when that port lands).
  PORT: the entries store the diagnostic's file NAME (resolved at Add
  time), keeping the comparator/Get methods checker-free — the call-site
  borrows need it; Go's diagnosticIndex map collapses to a same-location-
  key bucket scan (same observable Add semantics).
- **initializeChecker** (1309–1470): globals merge from script-file locals
  (ambient module symbols deferred; pattern ambient modules; module
  augmentations — global first, then non-global, Go's comment-ordering),
  addUndefinedToGlobalsOrErrorOnRedeclaration (the `globalThis`
  redeclaration diagnostic — DECLARATION_NAME_CONFLICTS_WITH_BUILT_IN_
  GLOBAL_IDENTIFIER_0), the valueSymbolLinks seeds (undefined/arguments/
  unknown/globalThis), and the special global types through
  getGlobalType/getGlobalStrictFunctionType (answering
  emptyObjectType/emptyGenericType per arity under the S6 resolveName
  stub). The 52-entry getGlobal* resolver table (Go's globalResolvers func
  fields, incl. the upstream `Contxt` typo) is memoized per resolver id
  through GLOBAL_RESOLVER_TABLE + accessor macros.
- **NewChecker tail wiring (checker.rs)**: the seeding lands AFTER the
  spike's intrinsic/marker creation (the type-id order pin holds);
  new_signature/new_object_type/new_anonymous_type are spine.rs's S7/S8
  seams (empty members → MEMBERS_RESOLVED, the note explains why that is
  Go's exact answer for these calls); the signatures arena gains the
  dummy slot 0 (id == index, matching the types arena + the existing
  `signatures[sig.0]` convention); noTypePredicate seeds
  `"<<unresolved>>"`/anyType; the for-check marker pair mirrors the
  relater pair; the booleanType union construction stays PANICKING (the
  S7 gate demo pin holds).
- **Gap 2 (TS8009) — CLOSED, and the dispatch note corrected**: Go emits
  TS8009 ONLY from the parser (checkJSSyntax's modifier arm, parser.go
  6873) and tsoptions — there is NO checker-side site (verified by grep);
  the condition is `ModifierToFlag(kind)&ModifierFlagsJavaScript == 0`
  (the .js-LEGAL modifiers — export/static/accessor/async/default — are
  exempt). The Rust parser had it INVERTED (`.intersects(JAVASCRIPT)`,
  erroring on exactly the legal set) — fixed in
  crates/parser/src/parser.rs's two modifier arms (PORT-noted).

**Validation**: tsc-checker 86 lib (+12 spine pins: typeChecked flip,
deferred queue/drain/live-addition ordering, globals merge + link seeds,
globalThis redeclaration, resolver memoization, unreachable report+dedupe
via getDiagnostics — TSUnknown options route TS7027 through
addErrorOrSuggestion's suggestion arm exactly as Go's code does —,
error-or-suggestion routing, the ctor-family pins, deferred-diagnostic
drain, checkExpressionEx literal) + 7 integration; tsc-parser 15 (+2
ignored); workspace tests all green; clippy zero on both crates; twins
byte-identical: fixture matrix 9/9, program 15, dump 13, symbols 15, and
the 300-slice **384/384 records byte-identical (gap-2 divergence 1 → 0)**;
climb stays 37.2% (parity first; the removed diagnostics were Rust-only
vs the reference — see rust/CONFORMANCE.md).

## M6 S2 — the getTypeOfSymbol spine + declared types + circularity

typeofsymbol.rs (~4,850 ln), final state 82cace033b. S2 owns the
circularity machinery (push/popTypeResolution, the
typeResolutions/resolutionStart stack — the S2×S7×S8 triangle's anchor)
and the declared-type family incl. the 381-ln getDeclaredTypeOfClassOrInterface,
the variable-like chain, binding patterns, widen* with the WideningContext
arena, the enum machinery, aliases, optionality.

**Three real bugs found by the wave's own gate demos** (the spike tests
grew 5 demos: class thisType promotion + instantiations seeding, thisless
interface, enum machinery, type-alias S5 seam, class anonymous value type):
1. Enum members were INVISIBLE — the member walk used the
   class/interface-only `getMembersOfDeclaration` subset where Go walks
   `declaration.Members()`; every enum took the computed-enum fallback.
2. `evaluate_stub`'s string-literal arm read the raw source text WITH
   quotes; Go reads `expr.Text()` (the parser-stored unquoted token).
3. The pub `get_symbol_of_declaration` twin in exports.rs was still the
   unconditional-None stub — masking every enum member symbol — while
   the real free fn existed unused.

**Stub directions pinned by tests**: S7 multi-constituent unions →
errorType (single-constituent identity), S5 type nodes → errorType, S6
merged/late-bound identity + is_thisless_interface conservative-false,
S9/S10 flow/facts identity, evaluator literal-only (numeric/string +
unary ±).

**Validation**: tsc-checker 90 lib + 12 integration; workspace green
(81 suites); clippy zero (11 new-code lints fixed); twins byte-identical
(dump 13, symbols 15, program 15, fixture matrix 9/9); the 300-slice
**384/384 byte-identical, divergence 0** — full driver parity at
parse+bind depth holds through S2.

## Full-port coverage ledger (cross-checked 2026-10-08)

Every Go package under `tsc/internal/` (51 dirs) maps to a SPEC crate —
no orphaned surface. Landed (33 crates): Layer 0 all 10 (collections,
stringutil, jsnum, nativepath, tspath, json, locale, debug, repo, core);
Layer 1 six of ten (diagnostics, vfs, contentmapper, glob, packagejson,
semver — sourcemap, symlinks, tracing, spanmap-landed*); Layer 2 all 5
(ast, scanner, parser, astnav-in-binder-shims*, binder); Layer 3 all 5
(module, tsoptions, compiler, outputpaths, bundled); Layer 4 one of five
(checker — the M6 long pole in flight; evaluator, pseudochecker,
nodebuilder, modulespecifiers pending); Layer 6 two of ~15 (testutil,
testrunner — the conformance gate tooling). Remaining: 22 crates across
Layer 4 tail, Layer 5 back end (printer, transformers, transpile,
diagnosticwriter), and Layer 6 drivers/services (execute, fswatch,
format, ls, project, lsp+lsproto, jsonrpc, ipc, api, fourslash, pprof,
osutil-landed*). (*spanmap landed as a crate; astnav + osutil landed
early; their Layer-table placement differs from this ledger's simple
count.) The full mapping is SPEC §3's layer tables; this ledger records
the cross-check ran and the plan covers the complete Go port.

## M6 S3 — statement/declaration/class/module/import-export checks

statements.rs (11,060 ln), five worker sessions (two killed by the local
box's reboot; the continuation-file discipline carried the wave across
them), final state eb7b49e811.

- The statement checks; the REAL IterationTypes/IterationTypeResolver
  machinery (replacing todo_shapes' shapes) with the fast/slow/worker
  paths, checkRightHandSideOfForOf, async-from-sync iteration types,
  and the return aggregation; the variable/decorator family with the
  unused-identifier machinery wired into checkSourceFile; the type-node
  checks incl. checkFunctionOrConstructorSymbolWorker (canonical-
  overload grouping, flag/question-token XOR agreement,
  reportImplementationExpectedError); the class/interface/enum/module
  checks; import/export + import attributes; the real checkAliasSymbol;
  checkExternalModuleExports.
- ~50 dispatch arms wired in spine's checkSourceElementWorker +
  checkDeferredNode; getMemberOverrideModifierStatus and
  getEffectiveDeclarationFlags stubs deleted (real bodies now).
- Go findings: checkModuleDeclaration's checkBody reads
  `SymbolFlagsTransient` (Flags, NOT CheckFlags); MethodSignature has no
  body; PropertySignature.initializer is a NodeId sentinel; the binder
  does not merge `enum E` with `const enum E` (TS 2473 unreachable from
  the checker).

**Validation**: tsc-checker 110 (90 unit + 8 S3 pins incl. TS
2392/2457/1147/1063 real-path pins + 12 spike); workspace green; clippy
zero; dump/symbols/program twins byte-identical; the conformance
300-slice **143/143, divergence 0** — MEASURED BY THE ORCHESTRATOR with
fresh binaries from the committed tree. (The worker's final report
claimed "269 match / 0 mismatch / 115 missing-baseline" for the same
run; that number did NOT reproduce with fresh builds of both drivers
from the same tree — the verified numbers above stand. The worker's
parity claim itself — identical drivers — is confirmed.)

## M6 gap-5 — verifyCompilerOptions (the FULL arm set) + verifyProjectReferences + the config-file AST routes

The wave the interrupted Droid session left mid-flight (the continuation
file carried the verified facts); landed from a fresh survey of Go
program.go:909-1449 + tsconfigparsing.go. The tsoptions side ports the
AST→value conversion chain and BOTH parse routes Go-shape; the compiler
side ports every remaining verifyCompilerOptions arm.

**tsoptions** (`tsconfigparsing.rs` + `parsedcommandline.rs`):
- The conversion chain: convertConfigFileToObject/convertToJson/
  convertPropertyValueToJson/convertObjectLiteralExpressionToJson/
  convertArrayLiteralExpressionToJson/isDoubleQuotedString — the
  nil-notifier shape (see below). The `?`-token and
  Property_assignment_expected diagnostics carry the RAW node loc (Go's
  `ast.NewDiagnostic`, not CreateDiagnosticForNodeInSourceFile's
  skip-trivia range); numeric literals go through
  `float64(jsnum.FromString)` with the Int/Float split the parsed twin
  proves; the non-object root reports
  The_root_value_of_a_0_file_must_be_an_object and the array-recovery arm
  converts the first object element (Go DROPS the root diagnostic there —
  exact mirror).
- `ParseConfigFileTextToJson` is the AST route with Go's parse-diag
  override (the FIRST parse diagnostic REPLACES the conversion errors);
  `GetParsedCommandLineOfConfigFilePath` is the sourceFile route
  (`ParseJsonSourceFileConfigFileContent`): values from the conversion,
  conversion diagnostics ride `errors`, JSON parse diagnostics ride the
  `TsConfigSourceFile` and surface ONLY through
  `GetConfigFileParsingDiagnostics` (Go's concat — never duplicated into
  `errors`; the DoesNotDuplicateUnquotedKey test pins the split).
- `getExtendedConfig` is the AST route with ParseExtendedConfig's exact
  semantics: the extended file's parse diagnostics replace the whole
  conversion (ALL of them, each paired with the EXTENDED file), the
  circularity diagnostic is Go's fileless
  Circularity_detected_while_resolving_configuration_Colon_0 (no args),
  unreadable extended files report Cannot_read_file_0 (tryReadFile).
- The worker (`parseJsonConfigFileContentWorker`): getPropFromRaw's
  wrong-value markers (the "" marker is config-file-route-only — the
  json-route diagnostics are Go's); the empty-files-list diagnostic
  positions at the `files` property initializer on the sourceFile route;
  the references arm ports Go's three positioned diagnostics
  (createDiagnosticAtProjectReferenceProperty); validateSpecs positions
  at the matching include/exclude array element through
  GetTsConfigPropArrayElementValue; the worker sets
  `options.ConfigFilePath = configFileName` on BOTH routes (was missing
  — feeds the tsBuildInfoFile arm + the rootDir-inferred check);
  CreateDiagnosticAtReferenceSyntax ports for verifyProjectReferences.
- **File pairing** (`ParsedCommandLine::error_files`): Go's
  config-parsing diagnostics carry their own `File()` (the main config,
  an extended file, or nil); the port pairs at push time
  (`WorkerErrors`) — GetConfigFileParsingDiagnostics returns the pairs,
  and the compiler's `GetConfigFileParsingDiagnostics` delegates (it
  previously paired everything with the main config name — an
  extended-file's parse errors rendered under the wrong file).
  `ParsedCommandLine::GetBuildInfoFileName` ports through the new
  outputpaths dep.
- **NOTIFIER RESIDUAL** (deliberate): Go's per-key option conversion
  (parseOwnConfigOfJsonSourceFile's jsonConversionNotifier — positioned
  unknown-option diagnostics, the extends/excludes/root-compiler-option
  arms) stays deferred; both routes pass the nil notifier and the shared
  worker's convertOptionsFromJson arm parses the options (the diagnostics
  pair with the config file, positions zero — 382 full-corpus twin
  records still diverge on exactly this, all pre-existing).
- `ResolvedProjectReferencePaths` ports (core.ResolveProjectReferencePath
  — a `.json` path as-is, else the path with DIRECTORY INTENT (Go's
  RootedDirectoryPathFromPath is an identity cast, no stripping)
  resolved against tsconfig.json).

**compiler** (`program.rs` + `file_loader.rs` +
`project_reference_file_mapper.rs` + `project_reference_parser.rs` +
`host.rs`):
- verifyCompilerOptions: every remaining arm in Go order — the
  strict/exactOptionalPropertyTypes checks (GetStrictOptionValue's
  strict-family default is the arm's real gate — TS5052 needs
  `strict: false`), isolatedDeclarations, inlineSourceMap, the composite
  declaration/incremental checks (args ["declaration", ""], Go's
  createDiagnosticForOptionName quirk), the tsBuildInfoFile arm,
  verifyProjectReferences, the composite root-files walk (processing
  diagnostics), the paths validation (forEachOptionPathsSyntax/
  createDiagnosticForOptionPaths/createDiagnosticForOptionPathKeyValue —
  the nil-vs-empty substitution collapse PORT-noted), the
  sourcemap/declarationDir/lib/jsx/moduleKind arms (ModuleKind.String
  stringer local — core has no Display), the rootDir-inferred check, and
  the emit-path-uniqueness loop (the OutputPathsHost adapter; the loop
  inlines over get_output_paths_for — outputpaths::for_each_emitted_file
  cannot express the memoized indexes, the orphan rule).
- PORT (flush boundaries): Go appends into programDiagnostics across
  closures that borrow `p` every which way; the port collects per Go
  append-boundary (block A, verifyProjectReferences' own collector —
  the range walk borrows `&self`, the composite walk, block B, the
  emit-path findings applied after) and extends in Go's order.
- **Fixed**: `getSourceFilesToEmit(nil)` previously returned the
  UNFILTERED `files` — Go memoizes the sourceFileMayBeEmitted-filtered
  walk; now `OnceLock<Vec<usize>>` (the emit-path loop depends on it).
- **Fixed** (pre-existing, found by the wave's tests): the
  project-reference resolution used the DECLARED paths (the reference
  `path` — `/src/ref`) as the parse-task config names where Go uses the
  RESOLVED config file paths; and `GetResolvedProjectReference` returned
  an empty-config-with-error where Go returns NIL on an unreadable
  config (verifyProjectReferences' config==nil arm reports
  File_0_not_found). The loader/parser/mapper now ride
  `ResolvedProjectReferencePaths` and the host nil-propagates read
  failures. Also fixed: the range walks' root parent was `None` where Go
  passes `mapper.config` (the callback's `parent.ProjectReferences()[index]`
  read needed it — wrapped in the ResolvedProjectReference newtype,
  memoized).

**Tests**: tsoptions configfileroute_test (10: the Go
TestParseConfigFileTextToJson value cases, the array-recovery root arm,
DoesNotDuplicateUnquotedKeyDiagnostics (2 @ (1,2)/(2,4)),
ReportsQuestionTokenDiagnostics (2 @ (1,17)/(2,10)),
ReportsInvalidExtendedConfig (4 X_0_expected at 7/10/14/18 in
bad.json, each paired with the extended file), WithEmptyExtendedConfig,
ConfigFilePath-set) + compiler program_test (+7 PORT-marked: the strict
arm, the composite arms, the incremental arm's non-firing, the paths
validation, the bundler moduleKind arm (TS5095), the emit-path
overwrite (TS5055 + IsEmitBlocked), verifyProjectReferences
(TS6306 positioned at the parent's references element, args the declared
path)).

**Validation**: workspace 83 suites green; clippy zero; twins with fresh
binaries: conformance 300-slice **384/384 byte-identical (divergence 0,
climb stays 143/384 = 37.2%)**, program-matrix 15/15, tsconfig-matrix
showconfig 18/18, resolver-matrix, conformance-matrix 9/9, and the ten
arm-firing filter slices (bundlerOptionsCompat, resolutionModeTripleSlash1,
nodeAllowJsPackageSelfName2, typingsSuggestion1, typesVersions.multiFile,
declarationEmitWithComposite, noEmitAndComposite,
incrementalTsBuildInfoFile, optionsTsBuildInfoFileWithoutIncremental-
AndComposite, declarationEmitPathMappingMonorepo) all identical. The
FULL conformance corpus (7,567 records): twin divergence drops
**528 → 382 with ZERO new divergences** (146 out-of-slice records fixed
by the routes/arms — the residual is the notifier wave's positioned
unknown-option diagnostics).

## Program-layer perf wave — collectFiles determinism + parallel BindSourceFiles

Two fixes from the Phase B program race (see BENCHMARKS.md's first Phase B
entries):

- **`files_parser.rs` — collectFiles now follows Go's `if
  task.loadedTask != nil { task = task.loadedTask }`** (the claim task is
  the group's loaded winner, so a multi-parent file claims at the FIRST
  DFS site). The pre-fix form read the ALIAS task's own `loaded` (false),
  skipped, and left the claim to whichever parent's task object won the
  PARALLEL LOAD RACE — the program order was run-to-run nondeterministic
  (5 runs / 5 outputs on packages/typescript/src) and diverged from Go on
  every multi-import corpus (the 125-record program twin never matched;
  the small program-matrix fixtures never exposed it — their import graphs
  have no multi-parent files). The include-reason redirect keeps the
  pre-follow task (Go captures `includeReason` before the reassignment,
  mirrored). Twins after the fix: program-matrix 15/15, symbols
  5,935-file conformance sweep, conformance 300-slice 384/384, and the
  packages/typescript/src program twin — ALL byte-identical.
- **`program.rs` — `BindSourceFiles` is parallel** (Go's
  `core.NewWorkGroup(p.SingleThreaded())` → scoped threads over
  text-weighted contiguous chunks of `files`; Go's queue balances per
  file on the scheduler, so the chunk cut points follow cumulative TEXT
  SIZE — equal-count chunks straggled on the big lib `.d.ts` files).
  Determinism is Go's own (per-file binder, files order unchanged, no
  twin output reads the raw lazy node/symbol ids — symbols_seam
  canonicalizes); the SingleThreaded option and the <8-file break-even
  gate the parallel arm; `IsBound` skips mirror Go. Bind on the program
  corpus: 3108 ms serial → 1900 ms parallel (Go parallel 1134 ms — the
  remaining gap is allocator-bound, see BENCHMARKS.md's tcmalloc data).
- **bench-harness + go-driver — `TSC_BENCH_PROFILE=1`** accumulates the
  five phase timers (seed/config/program/bind/record) to STDERR on both
  drivers (identical phases, same env var; the NDJSON contract
  untouched). PORT-noted at the instrumentation site.

## M6 S4 (tranche 1) — expression leaves I: operators/literals/this/property-access (2026-10-10, wip-checkpointed + tail commit)

`expressions.rs` ~4,200 lines: truthiness/nullish semantics, operator
error family, REAL is_type_assignable_to_kind, full check_binary_like_
expression + destructuring-assignment, unary/await/conditional/spread/
yield/meta-property leaves, property-access core incl. private
identifiers + accessibility, the this family, 18 spine arms wired.
checker.rs boolean_type no longer panics (S7 seam lazy build).
Worker gates, ALL re-verified by the orchestrator with fresh binaries:
115 checker tests, workspace clippy 0, fixture matrix 9/9 identical,
300-slice twins byte-identical (143/143 match, 0 divergent — baseline
preserved), program twin 15/15 identical. Remaining S4 families
(identifier/super/template/regex/array-literal/object-literal full/
expando/JS-assignment/contextual signatures) bannered; continuation at
$HOME/.s4-continuation.md; tranche 2 dispatched.

## M6 S4 complete (tranche 2, 2026-10-10) — expression leaves I closed

Tranche 2: checkIdentifier tail to Go's exact shape (2454/7022 dormant
under W3), checkPrivateIdentifierExpression + cached resolution, the
computed-property-name fold fix in checkObjectLiteral (name type, never
node_text — Node.Text() panicked on ComputedPropertyName), spine arms
ObjectLiteral/Class/Function|Arrow/PrivateIdentifier + 3 deferred arms.
Cross-file REAL fixes: relater is_readonly_symbol (full Go body),
typeofsymbol get_type_of_initializer (Go passes the node directly; the
re-read misport expect-panicked on binding-pattern params — caught by 2
s3 tests), new_anonymous_type_with_members extended + dedup,
is_valid_spread_type real. Orchestrator re-verified fresh: 120 checker
tests, workspace 689/0, clippy 0, fixture 9/9, 300-slice twins
identical (143/143, 0 divergent), program twin 15/15. Remaining stubs
by slice at $HOME/.s4t2-continuation.md; CallExpression family is S6.
Next: S5 type-nodes.

Rig validation (2026-10-10, post-9fd24bb2f3): fresh release build, all
three twin gates green on the rig — fixture matrix 9/9 identical,
300-slice byte-identical (143/143), program twin 15/15 identical.
S4 CLOSED.

## M6 S5 — type-nodes landed (2026-10-10)

typenodes.rs 3,351 lines: get_type_from_type_node 30+ arm routing,
conditional-flow/implied-constraint, type-reference + class/interface
reference ladders (2314/2315, deferred-reference, fill-missing-args),
TupleNormalizer + normalized tuples, alias instantiation + intrinsics,
mapped (S8 seam)/conditional (ConditionalRoot real; S9 get_conditional
deferred)/infer/import chains, defaults machinery, real
get_generic_object_flags. Cross-file: empty_type_literal_type seeded in
new_checker (Go 1044 — fixed a walk panic), is_array_type real,
IntersectionFlags iota mispin corrected (behavior-neutral under S7),
~20 relater predicates real, check_expression_with_type_arguments
fixed for TypeQuery data (Go 10868 dual-form read; the old expect
panicked on typeof queries). S6 resolve_entity_name stays permissive —
the 2314/2315 ladders dormant by design until S6. Orchestrator
re-verified fresh: 128 tests (8 new s5), clippy 0, fixture 9/9,
300-slice twins identical 143/143, program twin 15/15. Next: S6
resolution.

Rig validation (S5, 2026-10-10): forced `cargo clean -p tsc-checker -p
bench-harness` + full rebuild (32s) — all three twin gates green
(fixture 9/9, 300-slice byte-identical 143/143, program twin 15/15).
Rig gotcha recorded: `target/release/tsc-bench` is a HARDLINK of the
deps artifact, so its mtime survives `rm` + relink — a "0.01s Finished"
build with an unchanged binary mtime does NOT prove staleness; when in
doubt, `cargo clean -p` the crate and rebuild before trusting rig twin
runs. S5 CLOSED.

## M6 S6 — resolution landed (2026-10-11)

resolution.rs ~5,020 lines (resolve_name_worker walk + error ladder,
merged/late-bound symbols, resolve_alias + 13-arm
get_target_of_alias_declaration, resolve_entity_name/qualified_name,
exports machinery, clone_symbol) + resolvecall.rs 2,556 lines (the
full call-resolution family: get_resolved_signature_ex with cache +
resolution-start, resolve_call + CallState, choose_overload,
is_signature_applicable, infer_type_arguments, the arity/constraint
checks, untyped/error-call paths, check_call_expression +
check_import_call_expression, decorator/tagged-template/instanceof
resolvers, the TS18047/18049/18050 null/undefined ladder). Live wiring:
Call/New expression arms, resolve_untyped_call real, named-import arm
ignoreErrors=false (TS2307 fires), get_resolved_symbol passes Go's
exact args (TS2304 fires), get_effective_check_node real. S8-seam guard
in get_base_constructor_type_of_class (false TS2507 without it —
remove with S8). Option-vs-nil convention footgun recorded (Go compares
non-nil resolvingSignature). The wave survived a mid-edit session
cutoff (tree left non-compiling; resumed session finished it).
Orchestrator re-verified fresh: 138 tests (10 new s6), clippy 0,
fixture 9/9, 300-slice twins identical 143/143, program twin 15/15.
Next: S7 BEFORE S8.

Rig validation (S6, 2026-10-11): forced clean + rebuild — all three
twin gates green (fixture 9/9, 300-slice 143/143 byte-identical,
program twin 15/15). S6 CLOSED.

## M6 S7 — the union/type-construction engine landed (2026-10-11)

uniontypes.rs ~3,900 lines: getUnionType + unionReduction (subtype
reduction, exact cache-key discipline), getIntersectionType +
isReducibleIntersection, the full instantiate* family with the
mapper-driven substitution engine, template/string-mapping ctors,
get_reduced_type. Banner sweep: every S7-owned constructor seam removed
(~120 -> 101 banners; survivors are adjacent-wave deps or verified
cross-wave lazy-readers); anyArrayType/typeofType/unknownUnion seeded
per Go's NewChecker order, autoArrayType through the interned ctor,
create_type_reference_ex real in the FROM_TYPE_NODE arm. Orchestrator
re-verified fresh: 153 tests (15 new s7), clippy 0, fixture 9/9,
300-slice twins identical 143/143, program twin 15/15. Next: S8
(removes S6's TS2507 guard; owns resolveStructuredTypeMembers).

Rig validation (S7, 2026-10-11): forced clean + rebuild — all three
twin gates green (fixture 9/9, 300-slice 143/143 byte-identical,
program twin 15/15). S7 CLOSED.

## M6 S8 — structured-members machinery landed (2026-10-11)

members.rs 4,936 lines: resolveStructuredTypeMembers (dispatch per Go
19430: OBJECT flags first, Reference->ClassOrInterface->ReverseMapped-
>Anonymous->Mapped), real getSignaturesOfType, getPropertyOfType
family, the deferred-members protocol, relater member-comparison arms
on real members, S6's TS2507 guard removed. REAL BUG root-caused:
payload_symbol was missing TypeLiteralNode/MappedTypeNode arms (binder
sets those symbols; getSymbolOfDeclaration returned None) — every
type-literal annotation collapsed onto emptyTypeLiteralType and unions
of object literals deduped to one OBJECT; two payload arms fixed it.
TS2339 union-missing-property gate retired ( getPropertyOfType None is
now Go-faithful). Orchestrator re-verified fresh: 166 tests (13 new
s8), clippy 0, fixture 9/9, 300-slice twins identical 143/143, program
twin 15/15. Remaining S8 banners (65) are forward-references to
S9/S10 consumers. Next: S9.

Rig validation (S8, 2026-10-11): forced clean + rebuild — all three
twin gates green (fixture 9/9, 300-slice 143/143 byte-identical,
program twin 15/15). S8 CLOSED.

## M6 S9 — conditionals + contextual signatures/inference landed (2026-10-11, resumed after model-usage cutoff)

contextual.rs ~1,540 lines: get_contextual_type dispatcher real (20+
arms: variable-like/arrow/return/yield/await/call-arg/assertion/binary/
property/spread/array/conditional/template/satisfies; JSX W5 + decorator
W4 + import-attribute bannered), contextual-signature family (union
walk, compareSignaturesIdentical, createUnionSignature, intersected
signatures, IIFE arm bannered), get_contextual_return_type (generator
filter + Awaited|PromiseLike union), instantiate_contextual_type
(nonFixingMapper + #48363 boolean strip), isContextSensitive family,
createPromiseLikeType real. Plus the pre-cutoff typenodes conditional
work (is_simple_tuple_type 24968, forConstraint probe 24893, awaited/
promise pins, getConditionalType machinery; 8 conditionals tests).
Orchestrator re-verified fresh: 182 tests (16 new s9), clippy 0,
fixture 9/9, 300-slice twins identical 143/143, program twin 15/15.
Honest remainders bannered to S10/W4/W5 (inferTypeArguments
contextual-return links, discriminate/literal-widening sub-arms).
Next: S10.

Rig validation (S9, 2026-10-11): forced clean + rebuild — all three
twin gates green (fixture 9/9, 300-slice 143/143 byte-identical,
program twin 15/15). S9 CLOSED.

## M6 S10 — facts lattice + pattern/index machinery landed (2026-10-12) — S-SLICES COMPLETE

facts.rs ~440 (the TypeFacts bitset family — NOTE: the dispatch doc's
checkFacts/ExprFact/setFact names do not exist in this Go snapshot; the
real lattice is getTypeFacts/hasTypeWithFacts; the prior contains-all
stub was a misport; doc corrected) + index.rs ~1,730 (keyof family with
NoInfer re-wrap + deferred generic/mapped forms, the interned
indexed-access ctor, getPropertyTypeForIndexType with Levenshtein
suggestions, substitution/distribution families). Stub->real flips
across 12 modules: relater get_constraint_of_type to Go's exact switch,
typeofsymbol computeBaseConstraint arms + the Go-17981 widen-tail fix,
uniontypes instantiateType INDEX, contextual discriminate arms with
ObjectLiteralDiscriminator, statements checkIndexConstraints with the
(a)/(b)/(c) errorNode selection, checkMappedType, expressions TS2464 +
spread index-infos. One honest open banner: resolveNameForSymbolSuggestion
(lookup-hook refactor; affects only did-you-mean). Orchestrator
re-verified fresh: 188 tests (6 new), clippy 0, fixture 9/9, 300-slice
twins identical 143/143, program twin 15/15. ALL TEN S-SLICES CLOSED.
Next: W3 flow/grammarchecks/services + check-stage wiring (climb %).

Rig validation (S10, 2026-10-12): forced clean + rebuild — all three
twin gates green (fixture 9/9, 300-slice 143/143 byte-identical,
program twin 15/15). S10 CLOSED. ALL TEN S-SLICES (S1-S10) CLOSED AND
RIG-VALIDATED. W3 (flow/grammarchecks/services + check-stage wiring)
dispatched.

## M6 W3 tranche 1 — flow engine + the check stage goes LIVE (2026-10-12)

Flow engine landed (per the continuation file); 6 flow pins drive the
real spine (typeof guard, assignment reduction, truthiness, branch-label
recovery, equality literal, function walk). CHECK STAGE WIRED
Rust-side: semantic_diagnostics (GetSemanticDiagnostics read side),
CheckerUnitProgram adapter (§5.5 symbol split, case-marshaled options,
skip tsconfig/json/tsbuildinfo units), --stage check accepted. REAL
PORT-BUG: get_effective_declaration_flags dereferenced parent.parent
unconditionally (Go 3736-3745 gates it behind IsModuleBlock &&
IsGlobalScopeAugmentation) — fixed; check-stage panics 70 -> 7
(residual: 4 MappedType, 3 InterfaceType — genuine unported arms,
contained as -1 sentinels). Orchestrator re-verified fresh: 217 tests
(checker + testrunner), fixture 9/9, parse+bind twins identical
143/163/78/0, program twin 15/15, CHECK-STAGE HISTOGRAM MOVED:
69 match / 153 mismatch / 162 missing / 7 panic of 384 (18.0% — the
climb is live). Tranche 2: grammarchecks + services + the GO DRIVER
check stage (scope granted) + the 7 panic arms.

Rig validation (W3 tranche 1, 2026-10-12): forced clean + rebuild —
all three twin gates green at parse+bind (fixture 9/9, 300-slice
143/143 byte-identical, program twin 15/15). The check stage is
Rust-side live (69/153/162/7); tranche 2 (grammarchecks + services +
the Go driver check stage + the 7 panic arms) dispatched with the
go-driver scope granted.

## M6 W3 tranche 2 landed (902475a583, 2026-10-12)

grammarchecks ~5.3k + Go driver check stage + the 7 panic arms fixed
(three real shape bugs). Orchestrator re-verified fresh: 16 suites ok,
clippy 0, fixture identical at all three stages, bind slice 143/143,
program twin 15/15; check slice: Rust 85 vs Go 128 of 384, 126
divergent (my measured count; the worker reported ~179 by a different
fold — both recorded) — the check-stage climb queue is now concrete:
services slice + the leading divergence families (ambient/async
rust-fewer), the special-globals arm, checkUnused semantics. Tranche 3
dispatched.

Rig validation (W3 tranche 2, 2026-10-12): forced clean + rebuild —
fixture twins identical at parse, bind, AND check; bind 300-slice
143/143 byte-identical; program twin 15/15. Rig /tmp tmpfs hit its
12G quota mid-run (stale tsc-target 3.6G + a go-build dir from
earlier waves freed; other large /tmp files belong to other rig users
— the root btrfs balance recommendation stands). W3 tranche 2
RIG-VALIDATED.

## M6 W3 tranche 3 landed (2026-10-12) — services + special-globals + the isErrorType misport

services.rs ~1.9k (62 funcs in Go order); special-globals arm REAL on
both drivers (Go filter deleted); checkUnused aligned (Go 14232 rule);
isErrorType misport FIXED — the port had only t==errorType, Go's
second arm (Any flags + alias) was missing — killed the 105-record
spurious TS2318-Promise family; get_file_and_range NONE guard;
--dump-filter bisect tooling both drivers. Counting reconciled
(divcount.py; the 126-vs-179 was overlapping sub-folds). Check 300-
slice fresh: BOTH drivers 0/174/210/0 histograms, verdict flips 45->0,
divergent 131 (64 rust-fewer / 22 rust-more / 45 count-eq-hash-diff).
Climb note: match-vs-baseline is ~0 on BOTH drivers now (the lib-less
twin cannot match lib-generated reference baselines; twin identity is
the strong rail — the lib-file support gap is recorded in
CONFORMANCE.md as the path back to match%). Orchestrator re-verified
fresh: 16 suites ok, clippy 0, fixture identical parse/bind/check,
bind slice 143/143, program twin 15/15, check divergent 131 confirmed.
Remaining families (next tranche; S6-S9 depth): go-extra TS2454 x47,
async-2697 x37, TS2304 x33; rust-extra TS2339 x54, 2425/2417/2719 x56.

Rig validation (W3 tranche 3, 2026-10-12): forced clean + rebuild —
fixture twins identical at parse, bind, AND check; bind 300-slice
143/143 byte-identical; program twin 15/15. W3 tranche 3 RIG-VALIDATED.
Tranche 4 (the divergence-family chase) dispatched.

## M6 W3 tranche 4 landed (2026-10-12) — divergence 131 -> 111

Six Go-exact fixes (the check_identifier definite-assignment arms —
killed both the 2454 x47 and 2339 x54->18 families — plus
contains_undefined_type, is_mutable_local off-by-one,
is_prototype_property, get_type_without_signatures, declaration
modifier flags); 6/6 pins. Orchestrator re-verified fresh: 16 suites
ok, clippy 0, fixture x3 identical, bind 143/143, program 15/15,
check divergent 111 confirmed. W4 dispatched (collapses the 41
type_to_string hash-diffs; then tranche 5 chases async-2697/2304/2683
with the printer real).

Rig validation (W3 tranche 4, 2026-10-12): forced clean + rebuild —
all rails green (fixture x3 identical, bind 143/143, program 15/15).
W3 tranche 4 RIG-VALIDATED. W4 (nodebuilder/type_to_string printer)
dispatched.

## M6 W4 — the nodebuilder/type-to-string printer landed (2026-10-13)

nodebuilder.rs 3,384 lines: the full nodebuilderimpl.go + printer.go
family (type/symbol/signature/predicate to_string, the ttn precedence
dispatch, map_to_type_nodes truncation + collision re-render,
object/mapped/tuple/conditional/keyof/template/indexed arms,
get_expanded_parameters; 4 approximation seams PORT-noted). relater's
printer stubs deleted (real delegation, Go's context-sensitive
enclosing-name mirror at relater.go:1270-1290). type_of_node gained
Go's FunctionTypeNode/ConstructorTypeNode default arms (ast.go:679) —
unblocked return-type rendering. Check divergent 111 -> 99 (the 11
type_to_string arg-diff records collapsed + the rest-dot fix; the
remaining 31 count-eq records carry ZERO printer diffs — they are the
async/2307/2683/2703/2322 families). Four upstream gaps found by the
byte-verified render probes (union literal reduction in addTypesToUnion,
declared type predicates, a symbol-links aliasing suspect, TS2322 span
selection) — recorded for the next tranche. Orchestrator re-verified
fresh: 16 suites ok, clippy 0, fixture x3 identical, bind 143/143,
program 15/15, check divergent 99 confirmed, double-run deterministic.

Rig validation (W4, 2026-10-13): forced clean + rebuild — all rails
green (fixture x3 identical, bind 143/143, program 15/15). W4
RIG-VALIDATED. W3 tranche 5 (async-2697 + the four printer-probed
gaps + 2304/2683) dispatched.

## M6 W3 tranche 5 landed (2026-10-13) — divergence 99 -> 83; async-2697 collapsed

Root cause CORRECTED (W3T4's hypothesis falsified): Go forces
unannotated function-expression/arrow return types in
checkFunctionExpressionOrObjectLiteralMethodDeferred (checker.go:10449),
NOT checkFunctionOrMethodDeclaration (that one forces only generators,
3487). Landed: the contextuallyCheckFunctionExpressionOrObjectLiteral-
Method family REAL (infer_from_annotated_parameters_and_return,
assign_contextual/non_contextual_parameter_types, assign_parameter_
type, assign_binding_element_types), Go's resolvingSignature recursion
guard (30330 — stack-overflowed without it), checkComputedPropertyName
miswiring (both arms called the grammar stub — TS2304s now fire),
getLiteralTypeFromPropertyName computed arm, is_object_literal_type
misport (symbol-flags -> Go's object-flags, utilities.go:867). The
worker root-caused 2537 x11 via a TEMPORARY probe patch of the Go
checker (restored; orchestrator VERIFIED the tsc/packages/tools trees
are untouched). Clippy: the worker's "zero" did not fully reproduce (1
empty-lines-after-doc-comment warning at spine.rs:4162 — fixed by the
orchestrator; recorded as a verify catch). Orchestrator re-verified
fresh: 16 suites ok, fixture x3 identical, bind 143/143, program
15/15, check divergent 83 confirmed (rust-fewer 55->20; async-2697 now
count-equal with 23 span-only diffs in createPromiseReturnType error-
node choice). Remaining census: 2683 x17, 2322 spans, 2355 x9, 7033 x7,
2697 spans, 2537 (fix direction documented), + the four W4-probed gaps.

Rig validation (W3 tranche 5, 2026-10-13): forced clean + rebuild — all
rails green (fixture x3 identical, bind 143/143, program 15/15). W3
tranche 5 RIG-VALIDATED. Tranche 6 (2537 marker + the four probed gaps
+ 2683/2697/2322 spans) dispatched.

## M6 W3 tranche 6 landed (2026-10-13) — divergence 83 -> 27; eleven families

Falsified ANOTHER hypothesis honestly (2537's W3T5 "resolvingSignature
marker" was wrong; Go-probe showed ctxSig=false — the real cause was
Go's IIFE arm in getContextuallyTypedParameterType 30025). The span-
clamp splice (get_file_and_range never called the ported
get_error_range_for_node — Go's NewDiagnosticForNode clamp table) fixed
17 records at once (2697 spans/2322/2515/2654/2703). try_get_this_type
discarded the declared type it computed; function_has_implicit_return
real (EndFlowNode reachability); getTypeOfAccessors skipped
getReturnTypeFromBody(getter); Label() accessor; merge_module_
augmentation never resolved with isForAugmentation; the index-info
property-access arm (2542/4111); get_non_nullable_type real (not
identity); check_non_null_type via the facts WORKER; await TS1320
message; checkAwaitedType wrapper. Orchestrator re-verified fresh: 123
lib tests, clippy 0 real warnings, fixture x3 identical, bind 143/143,
program 15/15, check divergent 27 confirmed, tsc/packages/tools CLEAN.
Remaining census: the S8 generic-member instantiation root (T->string
mapping in E3<T>.m() — explains 2322/2719 residuals), 2307 x4 ambient
program-shape, 2414 x4 checkCollisionsForDeclarationName unwired.

Rig validation (W3 tranche 6, 2026-10-13): forced clean + rebuild — all
rails green (fixture x3 identical, bind 143/143, program 15/15). W3
tranche 6 RIG-VALIDATED. Tranche 7 (S8 generic-member instantiation +
2414 collisions wiring + 2307 ambient seam-shape + the final hash-diffs)
dispatched — the zero-divergence push.

## M6 W3 CLOSED — check-stage twin divergence ZERO (2026-10-14)

Tranche 7 (survived a box-reboot cutoff via continuation discipline):
27 -> 25 (S8 generic-member instantiation landed — the E3<T>.m() ->
string mapping, EPromise.then) -> 0. Final fixes: the EnumDeclaration
members_of arm, the real const-evaluator, is_external_module_
augmentation, the async-generator 2318 nested-stub pair, the @ts-check
directive gate. Orchestrator re-verified fresh: check twin IDENTICAL,
all rails green, 256 tests. W3 COMPLETE after seven tranches.

Rig validation (W3 COMPLETE, 2026-10-14): forced clean + rebuild —
RIG-CHECK-IDENTICAL (zero divergence confirmed on the rig), fixture
x3 identical, bind 143/143, program twin 15/15. W3 CLOSED ON BOTH
MACHINES. W5 (jsx, the final M6 wave) dispatched.

## M6 W5 COMPLETE — jsx landed; M6 CLOSED (2026-10-14)

jsx.rs 3,409 lines: the full jsx.go family (checkJsxElement/Fragment/
attributes, element-type resolution, JSX namespace walks, JsxElementLinks,
the checker jsx fields, Program::get_jsx_runtime_import_specifier;
parse_isolated_entity_name via the subtree-copy PORT). Two real bugs
fixed by its gates: the FRESH_LITERAL flag loss (Go's closure mutates
the CAPTURED objectFlags — the port's macro copied it; excess-property
checks missed) and the lost diagnostic-chain elaboration (relater.go:
4733 pushes next onto the head's message chain — the TS2339 under
TS2322 vanished). All W5 banners wired. PLUS the orchestrator's
full-corpus finding: an infinite loop in
lookup_symbol_for_private_identifier_declaration — Go's GetContaining-
Class is FindAncestor(node.PARENT, IsClassLike) (utilities.go:1739) but
the port's helper was INCLUSIVE, so the class cursor returned itself
forever (the full-corpus check sweep hung at record 410,
classStaticBlock15). Fixed Go-exact in utilities.rs; the sweep now
passes 410 (777 records in 4 min). NEW FRONTIER: the full corpus
exposes a pre-existing stack overflow on classes/members/privateNames
(the pre-fix binary HUNG on the same dir — a separate recursion bug
between private-identifier resolution and property-access checking on
nested classes; next tranche). Orchestrator re-verified fresh (W5 +
hang fix together): 17 suites ok, clippy 0 real warnings, fixture x3
IDENTICAL, bind 143/143, CHECK 300-SLICE IDENTICAL (zero divergence
held through W5), program twin 15/15, tsc/packages/tools clean.
M6 — the checker, the port's long pole — IS CLOSED. Next: the
full-corpus check sweep tranche, then M7 (emit back end).

## M6+ full-corpus sweep tranche 1 landed (2026-10-14) — sweep COMPLETES; divergent 3,068 -> 2,532

The sweep now runs 7,567/7,567 exit 0 (the privateNames recursion
resolved earlier in the tranche series). Six Go-exact fixes: the REAL
using/await-using Disposable initializer check (checker.go:6088-6126,
the TS2318s + the asyncDispose hint chain), the fast-path iteration
resolver pair un-collapsed (6565 reportErrors=false — spurious 2318s +
downstream 2495s on every fast-path probe), is_array_like_type stub ->
real body delegation, getBindingElementTypeFromParentType Array arm
(the 2461s), hasExcessProperties' errorNode reassignment (2802), and
includeUndefinedInIndexSignature nil-tolerance (flow.go:2339 — a 300-
slice gate regression caught by the fix itself). Census: usingDeclar-
ations (323) ELIMINATED, statements/for-ofStatements (104) eliminated,
destructuring 119->77; panics 513->502. Orchestrator re-verified fresh:
18 suites ok, clippy 0, fixture x3 identical, bind 143/143, CHECK
ZERO divergence held, program twin 15/15. Next tranche: the
NodeId::NONE-is-foreign-to-file-0 panic family (320 cases —
check_identifier -> get_control_flow_container on NONE parents,
dominating jsdoc/declarations + node/allowJs), then the 2354 tslib
family + the remaining panic families.

Rig validation (full-corpus sweep tranche 1, 2026-10-14): forced clean
+ rebuild — all rails green (fixture x3 identical, bind 143/143,
CHECK zero divergence, program 15/15). Tranche 1 RIG-VALIDATED. Tranche
2 (the 320-case NodeId::NONE panic family + the tslib/panic census)
dispatched.

## Full-corpus sweep tranche 2 landed (2026-10-14, dc61acbfa9) — sentinels 502 -> 191, divergent 2,532 -> 2,423

The NodeId::NONE family (~320 cases) fell to the real root cause:
Go's getControlFlowContainer is a FindAncestor that legitimately
returns nil (the binder's CommonJS module/exports symbols use the
SourceFile as valueDeclaration — parent Go-nil); the port's
Option-aversion turned that into a panic. Now Option<NodeId> Go-exact
at every call site. Nine further sibling fixes (is_alias_symbol_
declaration completed, MODULE_EXPORTS through resolveExternalModule-
Symbol, LOC-based synthesized guards, resolve_signature left.PARENT,
nil-tolerant find_constructor_declaration, get_resolved_symbol_or_nil
raw-link, get_parameter_count TARGET read). Panics 502->191; divergent
2,532->2,423 (3,068 pre-tranche-1). 19 suites + 5 fcs2 pins; all
300-slice gates held; rig validation queued. Plus the tsoptions
UndefinedTextRange span fix (orchestrator, the tranche's flagged
follow-up) — all gates re-verified fresh.

Rig validation (sweep tranche 2 + tsoptions span fix, 2026-10-14):
forced clean + rebuild — all rails green (fixture x3 identical, bind
143/143, CHECK zero divergence, program 15/15). Tranche 2 RIG-
VALIDATED on both machines. Tranche 3 in flight: type_of_node sentinel
leak (51+2 cases) + try_get_this_type_at_ex nil-parent guard (32) —
85/181 unique panic cases now produce real records; all gates held
locally; the remaining 96 re-censused by backtrace (nodebuilder
typenode 19, InterfaceType data 15, Type.Target 11, mapper OOB 10,
resolveAlias export= 7, ...). Worker resumed after a mid-tranche
cutoff; nodebuilder/relater in-flight edits verified compiling + 19
suites green before resume.

## Full-corpus sweep tranche 3 landed (2026-10-15) — panics 191 -> 26, divergent 2,423 -> 2,143

Seventeen worker fixes (FCS-noted) cleared every in-scope backtrace family:
the type_of_node NodeId::NONE sentinel leak (51+2), the try_get_this_type_at
nil-parent guard (32), the nodebuilder typenode ladder, InterfaceType data,
Type.Target/Types arms, mapper/inference/relater OOB clamps per Go's
indexing, resolveAlias export=, the jsx elaboration families, late_bind_
member, evaluate_entity, get_type_of_dotted_name, and the typenodes range
clamp. Orchestrator fixes for the out-of-scope stragglers: node_text_owned
(Go's ALLOCATING Text() arms — KindJsxNamespacedName "ns:name" + the four
JSDoc comment kinds; wired into check_grammar_jsx_element + check_
deprecated_property) and the runner's global-diagnostic pairing (Go's
fileless NewCompilerDiagnostic goes to the baseline's GLOBAL block with
no location; the per-unit seam was attaching them to the unit file and,
once spans went Go-exact {-1,-1}, panicking the column slice). Full-corpus
twin (rig, fresh): Rust 0/3,852/3,715/26 vs Go 0/3,826/3,741/0; divergent
2,143 (26 verdict / 1,828 count / 289 hash-only), identical 5,424/7,567.
20 suites + the fcs3 pin suite green; clippy 0; all 300-slice gates green
(fixture x3, bind 143/163/78/0, CHECK ZERO divergence, program 15/15) on
the rig. All heavy validation now runs on the rig per user directive.

## Full-corpus sweep tranche 4 landed (2026-10-16, RIG-VALIDATED) — panics 26 -> 0, divergent 2,143 -> 2,095

The last 26 panics were ONE family: case-meta option wildcards (@strict: *,
@target: *,-es3 etc.) marshal a non-boolean "*" into the synthetic tsconfig,
and the Rust tsoptions dropped the AST params from validate_json_option_
value/convert_json_option_of_enum_type — the 5024/6046 diagnostics arrived
fileless with (-1,-1) yet paired to the config path, panicking the baseline's
column slice (Go renders them with the real "*"-initializer span via
convertJsonOption(..., propertyAssignment.Initializer, sourceFile)). Fix in
the runner (tsoptions is read-only this tranche): file_diagnostic_view
re-derives Go's valueExpression loc from the byte-identical config text; no
matching node -> global block (Go's nil-node arm). Upstream fix queued in the
continuation. Plus a count-diff family: check_function_or_method_declaration
ran the GRAMMAR-only computed-name check where Go (checker.go:3450) runs the
full checkComputedPropertyName — the entire TS2465 family was missing.
VERDICT-DIFF AND PANICS ARE NOW ZERO on the full corpus: both drivers
0/3,826/3,741/0. Divergent 2,095 (1,806 count-diff / 289 hash-only),
identical 5,472/7,567. 22 suites (incl. fcs4_config_span + fcs4_computed_
property_names pins); clippy 0; all 300-slice gates green on the rig, fresh.
Next: the 2300 Duplicate_identifier family (binder flags probe — binder
crate may need opening), the S7/S10 object-literal index-info gap, the
proper tsoptions upstream fix, and the per-dir count-diff census.

## Full-corpus sweep tranche 5 landed (2026-10-17, RIG-VALIDATED) — divergent 2,095 -> 1,963

Three families: (1) the 2300 Duplicate_identifier family root-caused to
get_property_name_for_property_name_node returning "" where Go returns
the InternalSymbolNameMissing sentinel — every non-literal computed name
collided on the "" key; plus the real try_get_name_from_type
(checker.go:19126/19128 + computed_name_links caching) and AnyToString
rendering in flow.go:1782's shape (binder hypothesis disproven, flags
match Go). (2) tsoptions upstream: validate_json_option_value/convert_
json_option_of_enum_type now carry Go's (valueExpression, sourceFile)
triple via OptionDiagSite — 5024/6046 anchor at the initializer natively;
the tranche-4 runner workaround now no-ops (kept as a safety net).
(3) the TS2711 dynamic-import family: check_import_call_expression now
funnels every exit through createPromiseReturnType (checker.go:8486)
with Go's IfElse message split — 824 missing -> 76. Count-diff census
banked: rust-extra TS2318 532 / TS2307 522 / TS2367 397 / TS2683 218;
rust-missing TS2711 76 / TS7053 314 / TS2834 300 / TS2300 125. Both
drivers 0/3,826/3,741/0; 39 test-result-ok lines; clippy 0; all
300-slice gates green fresh on the rig.

## Full-corpus sweep tranche 6 landed (2026-10-18, RIG-VALIDATED) — divergent 1,963 -> 1,880, panics held at 0

Worker fixes: the full module-not-found ladder (checker.go:15690-15746 tail:
json arm, extensionless-ESM 2834/2835 with real suggestion derivation, plain
2307) + ~15 mode-derivation helpers; get_script_kind_from_file_name in the
runner (JSON/Unknown units skipped per conformance.go:1202); the real
is_type_equality_comparable_to (13105) and get_contextual_this_parameter_type
(12260); is_import_call's IsImportPhaseMetaProperty arm. ORCHESTRATOR CATCH:
the worker's final tree had 13 NEW panics (NodeId::NONE down resolveExternal-
Module) on react-jsx/jsxdev variants — its 0-panic claim was measured on the
dump sweep, not the plain conformance sweep. Root cause (gdb-style walk
trace): the implicit-import first-JSX-tag walk had a `!` visitor inversion —
it stopped on the first child subtree WITHOUT a JSX tag, so files whose first
statement isn't JSX found no canonical error tag and passed NONE as the
resolveExternalModule location. Fixed Go-exact (the visitor answers true to
STOP, i.e. on found). Plain-sweep census after the fix: divergent 1,880
(1,622 count-diff / 258 hash-only), identical 5,687/7,567, both drivers
0/3,826/3,741/0, verdict-diff 0. 41 test-result-ok lines (2 new suites, 10
pins); clippy 0; all 300-slice gates green fresh on the rig. Next tranche's
top families: the per-unit resolveModuleName seam (root of remaining TS2307
246/TS2318), the widened-literal layer, TS7053/TS2300/TS2345/TS2564/TS2571.

## Full-corpus sweep tranche 7 landed (2026-10-19, RIG-VALIDATED) — the resolveModuleName seam; divergent 1,880 -> 1,804

The top family: the per-unit resolveModuleName seam. Go's per-unit check twin
builds a 1-file map-FS program and runs the REAL file loader/module resolver
against it (node16 .js->.ts self-substitution, CJS directory/index
resolution), filling resolvedModules. The Rust seam never resolved — every
self-import hit the module-not-found ladder. Now ported Go-exactly: Program
trait get_resolved_module/get_source_file_for_resolved_module/source_file_
may_be_emitted; resolveExternalModule's resolved-module half (checker.go:
15508-15746: resolution-diagnostic gate, TS2305/5097/2306 arms, node16 mode
mismatch, untyped-module 2666/7016, AlternateResult chain); the runner builds
the same 1-file twin program and threads the cache. Plus the import.source
tail (Go's AbstractModuleSource short-circuit at 8523, ImportCallOptions,
assert/with attributes) — 24 records to identical, zero moved away from Go.
PLAIN-sweep census: divergent 1,804 (1,555 count / 249 hash), identical
5,763/7,567; both drivers 0/3,826/3,741/0; panics 0. 43 suites (2 fcs7 pin
suites); clippy 0; all 300-slice gates green fresh. TS2318 extra 436->16,
TS2307 189->57. Next: the widened-literal layer (getWidenedType), then
TS7053/TS2345/TS2564/TS2300/TS2571 by census size.

## Full-corpus sweep tranche 8 landed (2026-10-20, RIG-VALIDATED) — widened-literal layer; divergent 1,804 -> 1,688

Five Go-exact fixes: the object-literal-method contextual seam (the branch
passed the PARENT node, not the method node — Go checker.go:31238/30513 —
killing the TS7006 family), the widened-literal return layer (get_return_
type_from_body skipped Go's isUnitType contextual-signature block where
fresh "hi" widens to string — the 2339-arg diffs + TS2571 extras), the real
intra-expression inference body (inference.go:1317-1331), the indexed-access
stub routed to index.rs's real S10 resolver (TS7053 missing 314->49), and
the computed-property index-info synthesis in check_object_literal
(checker.go:13439-13452 + 13515-13528 — the fcs7 handoff's predicted gap).
PLAIN-sweep census: divergent 1,688 (1,461 count / 227 hash), identical
5,879/7,567; both drivers 0/3,826/3,741/0; panics 0; verdict-diff 0.
44 suites (fcs8 pin suite, 7 pins); clippy 0; all 300-slice gates green
fresh on the rig. Honest residuals: TS2532 +12 (flow-narrowing now visible
under correct contextual typing), TS2345 +7, TS7053 stragglers. Next by
census: TS2345 (213 extra/59 missing), TS2564 (154), TS2300 (125), TS2532.

## Full-corpus sweep tranche 9 landed (2026-10-21, RIG-VALIDATED) — divergent 1,688 -> 1,574

Three families: (1) TS2345 — get_inferred_type's prefer-covariant short-
circuit ran the some(contraCandidates) test even with NO contravariant
inference, discarding the good covariant inference (T={Shape} fell to
unknown, keyof T collapsed to never — every keyof-constrained call reported
a bogus never-assignability); Go-exact nested if/else (extra 213->192).
(2) TS2564 — is_property_without_initializer ignored the definite-
assignment ! token; the containsUndefinedType gate was a permissive stub;
and isPropertyInitializedInConstructor ported IN FULL (Go checker.go:5085:
synthetic this.<name> reference via the node_store_mut alloc seam, wired to
the constructor's ReturnFlowNode through the real getFlowTypeOfReferenceEx
walk) — total 261 -> 7. (3) TS2300 — the checkPropertyOrAccessor closure's
error arm was a no-op (report_duplicate_memberErrors now real) + Go's
len(declarations)>1 gate (missing 125 -> 18). PLAIN-sweep census: divergent
1,574 (1,346 count / 228 hash), identical 5,993/7,567; both drivers
0/3,826/3,741/0; panics 0. 47 suites (3 fcs9 pin suites); clippy 0; all
300-slice gates green fresh. Next by census: TS2322 (187/174), TS2339
(156/316), TS2345 residuals, TS2300 computed-name statics seam, TS2532 61.

## Full-corpus sweep tranche 10 landed (RIG-VALIDATED) — divergent 1,574 -> 1,503

Four families: (1) import.meta — check_import_meta_property's W3 stub
returned errorType; Go answers getGlobalImportMetaType (checker.go:11029,
the spine resolver) so the property checks fire (48 missing TS2339s +
the 1343 gate runs AFTER the resolver — property checks still fire below
ES2020). (2) es6/destructuring (113 missing TS2339s) — two stacked roots:
check_array_literal's inTupleContext read is_array_like_type where Go
reads isTupleLikeType (checker.go:8218 — under the no-libs seam EVERY
non-nullable contextual type is "array-like", so IIFE `[1]` arguments
built fresh tuples instead of Array<number> = the {} fallback), and
get_binding_element_type_from_parent_type's indexed access used the
node-less _writing stub instead of the full resolver with the binding
element as the access node (Go checker.go:18195's synthetic expression)
so the Property-does-not-exist ladder never ran. (3) TS2322 call/
construct/generic-signature family (30+ records across the whole
assignmentCompat directory) — instantiate_signature_in_context_of was
the M6 S6 identity stub; now the real Go checker.go:19813 body
(newInferenceContext over the source's type params, applyToParameterTypes/
applyToReturnTypes inference from the canonical target, getSignatureInstantiation
with the inferred types), with compare_types/relater/intersection_state
threaded from compare_signatures_related into the InferenceComparer.
(4) plumbing for (3): apply_to_parameter_types/apply_to_return_types/
InferenceState.inferences/get_signature_instantiation/get_type_parameters_
for_mapper widened to pub(crate) + pub(crate) inference-state-for-context
wrappers (Go's callbacks close over context.inferences; Rust fn pointers
can't capture). PLAIN-sweep census: divergent 1,503 (1,276 count / 227
hash), identical 6,064/7,567; panics 0. 48 suites (fcs10 pin suite, 5
pins); clippy 0; all 300-slice gates green fresh. Honest residuals: 3
newly-divergent importMetaNarrowing records expose the declare-global
augmentation seam (Rust's binder does not merge `declare global`
interfaces into the global scope, so the ImportMeta resolver reports
2318 where Go resolves foo) — the moduleExportAlias/prototype/globalThis
TS2339 families, TS2532 flow-narrowing 61, TS2345 residuals, TS2300
computed-name statics 12, TS2564 residual 7, and the 2322 optional-param
elaboration string seam (assignmentCompatWithCallSignaturesWithRestParameters,
26 diff lines, positions identical) remain for tranche 11.

## Full-corpus sweep tranche 11 landed (2026-10-22, RIG-VALIDATED) — divergent 1,503 -> 1,482

Four families: (1) removeOptionalTypeMarker real (the optional-call-chain
root of the TS2532 o3.b?.().c family — partial, 45 extras remain with the
this-flow-narrowing root diagnosed in the continuation). (2) The template-
literal pattern validators: local always-false stubs is_valid_number_string/
is_valid_big_int_string were SHADOWING the real utilities.rs twins — the
${number}/${bigint} TS2345 family (21x) + large collateral (es6/destructuring
28x, variadicTuples, restTuples). (3) The TS2322 optional-param elaboration
seam: symbol_value_declaration_is_context_sensitive was inverted (missing
Go's IsExpression && !isContextSensitive gate) + serialize_type_for_
declaration's annotation-reuse arm renders the written annotation slice
(x?: number, not x?: number | undefined). (4) The declare-global binder seam:
Go's mergeSymbolTable/mergeSymbol/reportMergeSymbolError (checker.go:
14380-14474) ported in full into spine.rs — collapsed the whole runway
probe list (moduleExportAlias 20, typeFromPrototypeAssignment 14,
globalThis 18, staticPropertyNameConflicts, umd-augmentation, ...). PLAIN-
sweep census: divergent 1,482 (1,259 count / 223 hash), identical 6,085;
both drivers 0/3,826/3,741/0; panics 0. 49 suites (fcs11 pins); clippy 0;
all 300-slice gates green fresh. Honest caveat: the worker's earlier probe
battery claiming the whole TS2532 family fixed was FAULTY (self-corrected
in the continuation) — the remaining root is this-property flow narrowing
(if (this.z && dz) doesn't narrow the later this.z).

## Full-corpus sweep tranche 12 landed (2026-10-23, RIG-VALIDATED) — the TS2532 family ELIMINATED; divergent 1,482 -> 1,455

The fcs11 diagnosis was right in location, but the root was more basic:
get_flow_type_of_access_expression was a permissive seam (prop_type passed
through, TODO M6 W3) — NO property access in the corpus ever ran the flow
walk. Four Go-exact fixes: the real flow call (checker.go:11651); this-type
answers wrapped in getFlowTypeOfReference in both class arms (12401-12412);
report_cannot_invoke... real (Go emits TS2722/2721/2723, not 2531/2532/2533 —
the logicalAssignment5 root); and get_type_of_accessors' implicit-any ladder
(18944-18952: setter 7032 / getter 7033 / auto 7008 with the isPrivateWithin-
Ambient gate). Census: TS2532 45->0 (family gone), TS18046 28->19, TS18048
28->15, TS2339 172->140, TS2322 123->106. PLAIN-sweep census: divergent 1,455
(1,242 count / 213 hash), identical 6,112/7,567; both drivers 0/3,826/
3,741/0; panics 0. 50 suites (fcs12 pins); clippy 0; all 300-slice gates
green fresh. Honest: TS7032 0->12 extras on esDecorators accessor records —
a PRE-EXISTING per-case noImplicitAny options seam (already divergent
pre-fix with same-position 7006s); the TS7006-family lead (47 extras) is
the next tranche's likely root, alongside TS2339 (140/130), TS2345 (132),
TS2322 (106), TS2699 (84), TS2556 (66).

## Full-corpus sweep tranche 13 landed (2026-10-24, RIG-VALIDATED) — divergent 1,455 -> 1,418

The fcs12 options-seam theory was DISPROVED (an 8-case probe battery
showed both twins resolve per-case options identically — Rust's
GetStrictOptionValue quirk matches Go compileroptions.go:122 byte-same).
The real roots: six checker fixes — isParameterOfContextSensitiveSignature
used the broad helper (Go checker.go:16959 narrows to isContextSensitive-
FunctionOrObjectLiteralMethod, itself missing the 30044 conjunction — the
missing-TS7006 family); addDeclarationToLateBoundSymbol missed Go 16484's
lateSymbol write (paired dynamic accessors never merged — TS7032 12->0);
hasBindableName was a !hasDynamicName stub (Go 20345's late-bindable half);
the set-accessor parameter arm read raw symbols instead of merged+late-
bound; the object-literal index arm poisoned with unknownType instead of
Go's nil-dropping mapTypeEx; and getTypeOfPropertyOfContextualTypeEx got
its REAL body (checker.go:31104: the index-info fallback ladder). PLAIN-
sweep census: divergent 1,418 (1,199 count / 219 hash), identical
6,149/7,567; both drivers 0/3,826/3,741/0; panics 0. TS7006 missing
61->13 / extra 47->36. 51 suites (fcs13 pins); clippy 0; all 300-slice
gates green fresh. Next: TS2339 sub-shapes (globalThis expando, strict-
BindCallApply, intrinsic-method both-ways), residual TS7006 contextual
worker stubs (typeofsymbol.rs:837-848), the blanket merged-symbol swap
over ~80 raw get_symbol_of_declaration sites.

## Full-corpus sweep tranche 14 landed (2026-10-25, RIG-VALIDATED) — divergent 1,418 -> 1,393

Three families: (1) the globalThis expando family — Go's globals is ONE map
aliased into globalThisSymbol.Exports (checker.go:974; Rust's SymbolTable is
a value type) — new globals_set mirrors all 5 post-init writes; plus the
property-access arm ported real (11573-11584: block-scoped -> 2339, else
noImplicitAny -> 7017) and the element-access 4-leg conjunction flattened
(27679). TS2339 extra 142->86. (2) The typeofsymbol worker stubs made real
(PropertyAssignment/Shorthand/MethodDeclaration arms — byte-identical on
the full corpus; the residual TS7006 36 is the deeper computed-name-method
contextual-signature seam + JS expando-assignment params). (3) The merged-
symbol sweep: per-site audit of all 106 raw Rust sites — ~60 swapped to
merged+late-bound per Go's own bodies, ~19 deliberately kept raw (Go reads
node.Symbol() inline); TS2699 missing 12->0. PLAIN-sweep census: divergent
1,393 (1,182 count / 211 hash), identical 6,174/7,567; both drivers
0/3,826/3,741/0; panics 0; newly-divergent 0. 52 suites (fcs14 pins);
clippy 0; all 300-slice gates green fresh. Next by census: TS2345 (131),
TS2322 (103/173), TS2699 (96 extra), TS2556 (66), TS2339 missing 127.

## Sweep micro-tranche 15 landed (2026-10-26, RIG-VALIDATED) — TS2345 residual: divergent 1,393 -> 1,346

Three co-masking roots in the call-arity/relater chain: (1) the relater's
any_function_type() helper was a None STUB — dead-coding Go's anyFunctionType
wildcard arms in signatures_related_to (relater.go:4478/4481), so the
context-sensitive first applicability pass (typed anyFunctionType) never
matched any function type — multi-overload calls resolved against later
overloads (the bogus number->FuncType 2345s). (2) has_correct_arity's
too-few ladder probed raw t.flags instead of Go's filterType(t, acceptsVoid)
(checker.go:9389) — every under-supplied call with non-void params was
accepted. (3) get_effective_call_arguments' TaggedTemplateExpression arm was
an empty Vec stub (30636) — dormant while (2) accepted everything; the arity
fix surfaced TS2554/2555 on ~265 instances. Plus get_error_node_for_call_
node's missing main body (10073). TS2554 missing 78->0; TS2345 missing off
the top-50. The live wildcard EXPOSED the next seam honestly: get_regular_
type_of_object_literal lacks Go's context-sensitive-member regularization
(records-with-2345-extra 131->176 from previously-masked sites; net records
still -47; newly-divergent 4 vs newly-identical 51). PLAIN-sweep census:
divergent 1,346 (1,128 count / 218 hash), identical 6,221/7,567; both
drivers 0/3,826/3,741/0; panics 0. 53 suites (fcs15 pins); clippy 0; all
300-slice gates green fresh. NEW CADENCE: single-family micro-tranches,
one commit each.

## Sweep micro-tranche 16 landed (2026-10-27, RIG-VALIDATED) — object-literal regularization real; lead honestly DISPROVEN; divergent 1,346 -> 1,345

get_regular_type_of_object_literal was an identity function (the symbol-
flags guard could never fire); Go's real body ported (checker.go:28693 +
transformTypeOfMembers 28710: OBJECT_LITERAL+FRESH_LITERAL guard, the
CachedTypeKindRegularObjectLiteral cache, the recursive member walk with
createSymbolWithType rebinds, the anonymous type carrying members/
signatures/index-infos). HONEST VERDICT: the tranche-15 "+45 TS2345" lead
did NOT collapse — a six-case both-driver probe showed all pre-existing
divergences byte-identical pre/post; intraExpressionInferences was already
divergent pre-tranche-15. The port is live and Go-shaped (instrumented
check: regular types carry ANYFN members). Net effect: -1 record
(contextualTypeWithUnionTypeMembers — the union-target arm now regularizes
the fresh literal source per relater.go:2909). The REAL residual root,
diagnosed for the next micro-tranche: the final pass relates the fully-
contextualized argument against the UNINSTANTIATED candidate signature
(target shows bare A/T) — the inference second-pass / signature-
instantiation wiring, shared with the restTuples args_0/args_1 seam.
PLAIN-sweep census: divergent 1,345 (1,126 count / 219 hash), identical
6,222/7,567; both drivers 0/3,826/3,741/0; panics 0; newly-divergent 0.
54 suites (fcs16 pins); clippy 0; all 300-slice gates green fresh.
(Cargo.lock: stale-lock regen adding the tranche-7 tsc-module entries.)

## Sweep micro-tranche 17 landed (2026-10-28, RIG-VALIDATED) — the inference-context ancestor match; divergent 1,345 -> 1,341

ONE 21-line root for the fcs16 diagnosis: get_inference_context required an
EXACT node match where Go's getInferenceContext (checker.go:31513) matches
the first stack entry whose node is an ANCESTOR (isNodeDescendantOf,
utilities.go:912), returning that entry's context (even nil) on the first
ancestor hit. With the exact match, context-sensitive members inside call
arguments never resolved the argument's inference context -> the nonFixing
substitution was skipped -> the second pass recorded a self-referential
contravariant candidate (T:=T) -> the final applicability pass related
against the UNINSTANTIATED candidate (instrumented-confirmed). Fix: Go's
ancestor match via the existing is_node_descendant_of. Repro deltas:
intraExpressionInferences 67/35 -> 36/35; typeArgumentInferenceWith-
ObjectLiteral 21/9 -> 9/9 DIAG-IDENTICAL; minimal repros k1-k4 IDENTICAL.
TS2345 records-with-extra 176 -> 131 (-45); TS2322 105->99. Honest split:
two distinct sub-roots remain in the contextual-signature instantiation
chain — restTuplesFromContextualTypes 42/18 (Go instantiates the
contextual rest signature with renamed params args_0/...args; the port
relates against uninstantiated ...arg) and parenthesizedContexualTyping2
sub-shape B (ternary/comma arrows, 7006+2347 vs 2345). PLAIN-sweep
census: divergent 1,341 (1,121 count / 220 hash), identical 6,226/7,567;
newly-divergent 0, newly-identical 4. 55 suites (fcs17 pins); clippy 0;
all 300-slice gates green fresh.

## Sweep micro-tranche 18 landed (2026-10-29, RIG-VALIDATED) — the rest-tuple family COLLAPSED; divergent 1,341 -> 1,326

Four Go-exact port gaps behind restTuplesFromContextualTypes (42 -> 18 ==
Go, same shape_hash — fully identical): (1) get_effective_call_arguments'
spread-of-TUPLE expansion was an explicit TODO (Go checker.go:30615:
synthetic per-element args, REST->createArrayType, VARIABLE->isSpread,
labeledDeclaration->tuple_name_source) — unexpanded spreads made every
fixed-arity call fall to the TS2556 fallback: TS2556 66 -> 0 corpus-wide.
(2) try_get_type_at_position inverted Go's ORed gate (relater.go:1803:
!isTuple || Variable || index<fixedLength — the port dropped rest-tail
positions, leaving contextual ...string[]-tail params implicitly any).
(3) get_expanded_parameters hard-coded "arg" instead of Go's
getUniqAssociatedNamesFromTupleType (nodebuilderimpl.go:2017: args_0/
args_1/...args + Go's duplicate-suffix counter quirk). (4) slice_tuple_
type was an IDENTITY STUB (minimal probe caught doubled fixed elements) —
ported relater.go:1912 (rest-array/empty-tuple/sliced arms). Sibling
checked honestly: parenthesizedContexualTyping2 sub-shape B does NOT
share the root (byte-identical pre/post) — distinct family, next queue.
PLAIN-sweep census: divergent 1,326 (1,106 count / 220 hash), identical
6,241/7,567; newly-divergent 0, newly-identical 15. 56 suites (fcs18
pins); clippy 0; all 300-slice gates green fresh.

## Sweep micro-tranche 19 landed (2026-10-30, RIG-VALIDATED) — parenthesizedContexualTyping2 COLLAPSED; divergent 1,326 -> 1,284

Root: Go's addImplementationSuccessElaboration (checker.go:9917) re-runs
chooseOverload on the overload IMPLEMENTATION signature after a failed-
overload report; its rest arm's getSpreadArgumentType (9536) re-checks
rest args with anyType context, leaving rest-position arrows NON-
contextually typed (Go emits 7006+2347 there). Rust gaps: the S8 seam in
is_signature_applicable's rest arm (rest compared to ITSELF), no
add_implementation_success_elaboration at all, S4-stubbed elaborateArrow-
Function/elaborateObjectLiteral, and missing elaborateError recursion
arms. Regression caught mid-tranche: the SAME S8 seam in infer_type_
arguments' rest arm (infer_types(rest_type, rest_type) -> Go 9716's
spread-type inference) — fixing it collapsed genericRestParameters1 too.
One fcs3 pin honestly updated (it asserted a spurious 2345; Go reports
2769). A latent jsx panic (JsxAttribute guard order vs Go jsx.go:272)
fixed. Target now rust 50 == go 50 DIAG-IDENTICAL. TS7006 missing 13->4.
PLAIN-sweep census: divergent 1,284 (1,090 count / 194 hash), identical
6,283/7,567; newly-divergent 0, newly-identical 42 (constAssertions,
typeSatisfaction x4, tagged-template/destructuring cases). 57 suites
(fcs19 pins); clippy 0; all 300-slice gates green fresh. Honest residual:
9 rust-extra 7006s inside 4 variadic-tuple records — the variadic-tuple S8
seam is the next candidate family.

## Sweep micro-tranche 20 landed (2026-10-31, RIG-VALIDATED) — the variadic-tuple family COLLAPSED; divergent 1,284 -> 1,282

Two Go-exact gaps: (1) get_contextual_type_for_element_expression's tuple
arm was the S9 TODO stub — it answered the fixed constituent at index and
ignored length/lastSpreadIndex, dropping Go checker.go:30526-30556's
ending-fixed arm (the arm that contextually types the tail function
element of ...args: [...T, F] rest tuples) and the variadic slice-union
fallback — ported Go-exactly (removeMissingType + fixedEndLength geometry
+ get_element_type_of_slice_of_tuple_type noReductions). (2) get_start/
end_element_count were S7 TODO stubs returning 0 — inferFromObjectTypes's
endLength consumed them, so Go inference.go:805's single-variadic-middle
arm never fired and T inferred as {} instead of the source slice — ported
as real prefix/suffix scans. All 9 rust-extra 7006s + 2 7019s collapsed
(census: TS7006 -9 exactly, TS2345 -22 collateral); contextualTypeTupleEnd
30->17==17, variadicTuples1 62->53==53. intraExpressionInferences'
mapped-type residual confirmed NOT shared (still 36/35). PLAIN-sweep
census: divergent 1,282 (1,083 count / 199 hash), identical 6,285/7,567;
newly-divergent 0, newly-identical 2. 58 suites (fcs20 pins); clippy 0;
all 300-slice gates green fresh.

## Sweep micro-tranche 21 landed (2026-11-01, RIG-VALIDATED) — the JS expando assignment-declaration seam; divergent 1,282 -> 1,251

The TS2339-missing family's huge-radius root: EVERY JS expando assignment-
declaration symbol (this.x=..., C.prototype={...}, C.prototype.m=...,
exports.x=..., module.exports, Object.defineProperty) typed as ANY in the
port. Two roots: (1) get_widened_type_for_assignment_declaration + is_
constructor_declared_this_property were the "TODO M6 S4" permissive stubs —
Go checker.go:18478-18647 REAL bodies ported (kind ladder, declarations
scan, get_assignment_declaration_initializer_type union with the Module-
Exports/ExportsProperty/ThisProperty/defineProperty arms, the JS all-
nullable implicit-any report) + the missing ast_shims type_of_node
Kind::BinaryExpression arm. (2) get_contextual_type_for_assignment_
expression was the "TODO M6 W2/W4" permissive stub (get_type_of_expression
(left) — Go's TAIL only): the REAL body (Identifier module.exports -> nil,
binary.Symbol set -> annotated-VariableDeclaration-only else nil (THE
circularity guard — the port's permissive stub hit tvpp(prototype)
re-entry, cached errorType, and poisoned the outer resolution), ThisKeyword
ladder, prop/element-access arms) + binary.Type (@type) precedence in
get_contextual_type_for_binary_operand. Instrumented Go ground truth: ONE
tvpp(prototype) per run, no re-entrancy. Recovered: moduleExportAlias ALL
22, typeFromPrototypeAssignment 1/2/3 all 15, jsDeclarations x10,
jsdocTemplateTag4/5, salsa/jsdoc families broadly. Honest splits left:
moduleExportAlias's 12 extra 2323 redeclares (separate binder family),
computedPropertyNamesContextualType ES5/ES6 x18 (S10 contextual-signature
residual), a 2318 lib-family root. PLAIN-sweep census: divergent 1,251
(1,058 count / 193 hash), identical 6,316/7,567; both drivers 0/3,826/
3,741/0; panics 0. 59 suites (fcs21 pins, 4); clippy 0; all 300-slice
gates green fresh. (Worker cut off twice silently; orchestrator finished
the battery + fixed its doc-test compile slip.)

## Sweep micro-tranche 22 landed (2026-11-02, RIG-VALIDATED) — the S10 computed-name contextual-signature family; divergent 1,251 -> 1,242

The computedPropertyNamesContextualType ES5/ES6 family (18 missing 2339s
+ 18 extra 7006s across 6 records — the fcs21 honest split): the
contextual-signature lookup for computed-name object-literal methods never
matched its index-signature contextual type. Root fixes per the worker's
[s2]-[s4]: the element-impl name reads now use the generated Go-exact
node().name() (Go line cites), the map_type_ex arm restored, and the
instrument/restore formatting mangle fixed. DUMP DIAGS IDENTICAL through
both drivers on all 6 records; pin 3 (no index sig -> still 7006, no
2339) Go-verified through BOTH drivers to guard against over-application.
Newly-identical 9 (all 6 records' variants), newly-divergent 0. The
BindingElement property_name.or(name) latent divergence NOTED for a
future tranche (Go's ArrayBindingPattern check at 30151 reads plain
Name()). PLAIN-sweep census: divergent 1,242 (1,058 count / 184 hash),
identical 6,325/7,567; both drivers 0/3,826/3,741/0; panics 0. 60 suites
(fcs22 pins); clippy 0; all 300-slice gates green fresh. Remaining
missing-2339 census: 49 instances across 18 records (assertionType-
Predicates1 x10, intraExpressionInferences x11, privateNameAndObjectRest-
Spread x4, jsx clusters) — next micro-tranches. (Worker died mid-
battery; the rig-side battery script completed + orchestrator re-verified
tests fresh.)
