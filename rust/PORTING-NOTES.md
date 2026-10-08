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
