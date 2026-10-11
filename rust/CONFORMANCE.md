# Conformance Tracker

The M6 conformance runner (SPEC §8): a port of tsc's `testrunner` +
`testutil` baseline machinery driving the Rust compiler at the wired
depth (**parse + bind + check** — the check stage since the M6 W3 checker
wave), with a byte-identical Go twin proving the
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
  `Bind` (default) additionally calls `bind_source_files`, and — since
  the M6 W3 checker wave — `Check` additionally drives the ported
  checker spine per in-test unit (`semantic_diagnostics_of_unit`: parse +
  bind + `check_source_file` over the unit's own bytes, the case's
  marshaled `CompilerOptions`, the checker's `semantic_diagnostics()`
  read). Go's counterpart is program.go's `GetSemanticDiagnostics`; the
  single-file seam is the §5.1 NodeId space (one file per checker, as in
  Go's CheckerPool) — PORT-noted: cross-file symbols resolve with the M5
  program merge.
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

## Measured numbers

### 2026-10-08 — gap-2 closure (the TS8009 .js-unit parse diagnostics)

**Twin parity (the wave's gate): the 300-case slice is now byte-identical
on all 384 records (100%)** — the gap-2 residual (1 divergent record,
`asyncArrowFunction_allowJs.ts`) is closed.

**Root cause** (corrected by this wave): the M6-DISPATCH note said "Go's
CHECKER emits TS8009"; verified false — Go emits
`The_0_modifier_can_only_be_used_in_TypeScript_files` ONLY from the
parser's `checkJSSyntax` (parser.go:6873, the modifier arm) and from
tsoptions, with the condition
`ast.ModifierToFlag(kind)&ast.ModifierFlagsJavaScript == 0` (the .js-LEGAL
modifiers — export/static/accessor/async/default — are EXEMPT). There is
no checker-side emission site. The Rust parser's port had the condition
**inverted** (`ModifierFlags.intersects(JAVASCRIPT)`, erroring on exactly
the JS-legal set), producing 4 spurious TS8009s on
`async/es2017/asyncArrowFunction/asyncArrowFunction_allowJs.ts` (an
`async` arrow in a `.js` unit). The fix inverts the two conditions in
`crates/parser/src/parser.rs` (class/method + variable/property arms,
PORT-noted against conformance gap 2).

| driver | records | match | mismatch | missing-baseline | panic |
|--------|---------|-------|----------|------------------|-------|
| Rust   | 384 | 143 (37.2%) | 163 | 78 | 0 |
| Go twin| 384 | 143 (37.2%) | 163 | 78 | 0 |

Twin records byte-identical: **384/384 (100%)**; both drivers double-run
deterministic. The fixture matrix twin (9 records), the program gate
(15 records), and the `dump` (13 records) / `symbols` (15 records) twins
all stay byte-identical. Climb % unchanged (37.2%): the closed gap
removed divergent Rust-only diagnostics the *reference baselines* never
carried (they are rendered from full checker depth, still the unwired
boundary) — parity first, exactly as the gap-1 wave traded climb for
truthfulness.

### 2026-10-08 — gap-1 closure (removed-option diagnostics + the
### AST-config-node surface)

**Twin parity (the wave's gate)**: the 300-case slice now runs
**byte-identical on 383/384 records (99.7%)** — down from 81 divergent.
The single residual is gap 2 exactly (`asyncArrowFunction_allowJs.ts`:
Rust reports 4 TS8009-family parse diagnostics where Go scopes them to
the checker).

| driver | records | match | mismatch | missing-baseline | panic |
|--------|---------|-------|----------|------------------|-------|
| Rust   | 384 | 143 (37.2%) | 163 | 78 | 0 |
| Go twin| 384 | 143 (37.2%) | 163 | 78 | 0 |

Both drivers double-run deterministic; the fixture matrix twin
(`rust/bench/gates/conformance-matrix`) stays byte-identical at
9/9 `match`; the program gate (`rust/bench/gates/program-matrix`)
regression stays byte-identical (15 records).

**Why the climb % dropped from 55.5% to 37.2%**: closing the gap makes
Rust emit Go's removed-option diagnostics (TS5102/TS5108) with the same
config-node positions — twin parity first (the dispatch note's
expectation: the pre-wave Rust "matched more" only because it emitted
nothing where Go emitted config-positioned diagnostics the reference
baselines don't carry at parse+bind depth). The ≥90% M6-done gate
re-measures at full depth once the checker waves land.

### 2026-10-08, first green slice (pre gap-1)

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

Twin parity on the slice: 304/384 records byte-identical (79.2%). All 80
divergent records traced to the program-layer gaps (below), none to the
runner structure. Runtime: ~69 s Rust (dev profile) vs ~10 s Go.

**M5 program gate regression**: `tsc-bench program` vs `go-bench
program` over `rust/bench/gates/program-matrix` — byte-identical
(15 records), no regression from the M6 work.

## Gate table

| gate | target | measured | state |
|------|--------|----------|-------|
| first green slice | fixture matrix twin identical + climb ≥ 50% | matrix identical; 55.5% (pre gap-1) | **PASS** |
| gap-1 closure | twin divergence ≤ gap 2's residual on the 300-slice | 1 divergent record (gap 2 only) | **PASS** |
| gap-2 closure (M6 S1) | twin divergence 0 on the 300-slice | 384/384 byte-identical | **PASS** |
| gap-5 closure (verifyCompilerOptions full arms + the config-file AST routes) | 300-slice divergence 0 + the arm-firing filter slices identical | 384/384; 10/10 filter slices identical; full-corpus divergence 528 → 382, 0 new | **PASS** |
| M6 done | climb ≥ 90% on the 300-case slice | 37.2% at parse+bind (twin-parity first; re-measures at full depth with the checker waves) | blocked (below) |

A test is counted "pass" in the legacy table only when its full baseline
set is byte-identical to the reference tree; the climb % above is the
single-`.errors.txt` slice at parse+bind depth.

## Known gaps

1. ~~**Removed-option diagnostics (TS5102/TS5108)**~~ — **CLOSED (gap-1
   wave)**: `verifyCompilerOptions`'s removed-option arm is ported
   (`crates/compiler/src/program.rs`, the `OptionDiagnosticContext`
   machinery) over the config-node surface (`tsoptions`'s
   `TsConfigSourceFile` + the config-syntax walks +
   `CreateDiagnosticForNodeInSourceFile`'s positioned form). The
   REMAINDER of Go's `verifyCompilerOptions` (the strict/composite/
   paths/jsx/moduleKind arms, `verifyProjectReferences`, the
   emit-path-uniqueness loop) and the AST→value config conversion
   (`convertConfigFileToObject`) stay deferred to the full AST-config
   wave — the plug points were PORT-noted in
   `crates/compiler/src/program.rs::verify_compiler_options` (the
   gap-5 wave landed them all — see gap 5).
2. ~~**JS parse-diagnostic scope (TS8009)**~~ — **CLOSED (M6 S1 wave)**:
   the divergence was not a checker/parser scope split (Go's checker never
   emits TS8009; verified — the only emission sites are the parser's
   `checkJSSyntax` modifier arm and tsoptions). The Rust parser's port had
   the JS-legal-modifier exemption **inverted**
   (`ModifierFlags.intersects(JAVASCRIPT)` vs Go's `& JavaScript == 0`),
   erroring on exactly the .js-LEGAL modifiers (async/export/static/
   accessor/default). Fixed in `crates/parser/src/parser.rs` (both
   modifier arms, PORT-noted); the 300-slice twin is now 384/384
   byte-identical.
3. ~~**Include-reason matched-spec arms**~~ — **CLOSED (rode the same
   wave)**: `GetMatchedFileSpec`/`GetMatchedIncludeSpec` are ported on
   `ParsedCommandLine` (the as-written specs: `file_spec_by_path`,
   before-substitution lists), and `fileInclude.go`'s
   `computeDiagnostic` RootFile arm + `toRelatedInfo`'s
   files/include/types/lib/target arms read them through the config AST.
4. **Related-info location** — the Rust `ast::Diagnostic` carries
   related files as node ids (the PORTING-NOTES file-id gap), so both
   conformance drivers render `!!! related TSxxxx: <message>` without
   the `file(line,col)` prefix. Parse-level diagnostics do not carry
   related information in practice.
5. ~~**verifyCompilerOptions' non-removed arms**~~ — **CLOSED (the
   gap-5 wave)**: every remaining Go arm is ported — the strict/
   isolatedDeclarations/inlineSourceMap/composite/tsBuildInfo checks,
   `verifyProjectReferences` (+ the reference-path resolution chain it
   reads: `ResolvedProjectReferencePaths`, the host's nil-on-unreadable
   propagation, the range walks' root parent), the composite root-files
   walk, the `paths` validation, the sourcemap/declarationDir/lib/jsx/
   moduleKind arms, and the emit-path-uniqueness loop — plus the
   config-file AST routes in tsoptions
   (`ParseConfigFileTextToJson`'s parse-diag override,
   `GetParsedCommandLineOfConfigFilePath`'s route split,
   `getExtendedConfig`'s AST semantics, the per-diagnostic file pairing).
   Measured on the ten arm-firing filter slices (bundlerOptionsCompat,
   the composite/tsBuildInfo/paths compiler-suite cases): all identical;
   the full-corpus twin divergence drops 528 → 382 with zero new. The
   residual (382 records, all pre-existing) is the
   `jsonConversionNotifier` wave — Go's per-key route diagnostics are
   POSITIONED by `parseOwnConfigOfJsonSourceFile`; the port's are
   route-paired with the zero range until that wave lands (PORTING-NOTES).

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

## Full-corpus sweep — 2026-10-10, post-S4 (rig, fresh release builds at 9fd24bb2f3)

7,567 cases: 2,782 match / 3,681 mismatch / 1,104 missing-baseline at
parse+bind depth (the checker stage is still deliberately unwired — the
climb % starts moving at W3). Twin divergence: **382 records — the
identical pre-existing set recorded at gap-5** (the positioned per-key
option diagnostics class). **S4 introduced zero new divergences.** Each
side shows one panic-sentinel stderr line for the same case (part of
the identical-stream contract, not a regression). Streams on the rig at
`/tmp/full_rust.ndjson` / `/tmp/full_go.ndjson`.

## 2026-10-11 — W3: the check stage goes live

`Stage::Check` is wired (the M6 W3 checker wave): per in-test unit the
runner parses + binds the unit's own bytes and drives the ported
checker spine (`check_source_file`), reading
`Checker::semantic_diagnostics()` into the `.errors.txt` render — Go
`GetSemanticDiagnostics`'s harness read, at the single-file seam
(§5.1 NodeId space; cross-file symbols resolve with the M5 program
merge — PORT-noted above).

- **300-slice at check** (384 records): **69 match (18.0%) / 153
  mismatch / 162 missing-baseline / 7 panic-sentinel**. The histogram
  moved exactly as the climb metric predicts: cases that rendered
  NoContent at parse+bind (auto-`match`) now produce real semantic
  content — matched when the ported checker agrees with the reference,
  `missing-baseline` where the reference itself is absent for the
  synthetic configs, `mismatch` where the checker's partial depth
  (grammarchecks, services, emit-resolver diagnostics unported) shows.
- **Panic containment**: 70 → 7 sentinels. The dominant
  `getEffectiveDeclarationFlags` crash was a port bug — the body
  dereferenced `parent.parent` unconditionally (Go reads
  `container.Parent` behind an `&&` short-circuit) and the source file's
  NONE-parent sentinel panicked the store; fixed to Go's exact shape
  (PORT-noted at `statements.rs`). The residual 7 are genuine unported
  arms (MappedType/InterfaceType structure resolution), contained as
  `errors_count: -1` sentinel records per the contract.
- **Twin discipline at check**: the Rust driver now accepts
  `--stage check`; the Go driver's check stage is a separate wave
  (rust/bench/go-driver — out of the W3 writable scope, flagged to the
  parent), so the check stage is Rust-only for now — the parse+bind
  twins are unchanged and byte-identical (fixture 9/9, 300-slice
  384/384, program 15/15 all re-verified after the wiring).

## W3 tranche 2 — the check stage is live on BOTH drivers (2026-10-12, 902475a583)

grammarchecks.rs ~5.3k lines + call-site wiring; the GO driver's check
stage wired (single-file map FS, real Go checker, GetDiagnostics +
GetGlobalDiagnostics; TS2316/2317/2318/2468 special-globals arm
filtered to match the Rust S2 seam; panic recovery prints to stderr).
The 7 residual panic arms fixed at the root (three §5.1 shape/reorder
bugs: the relater TYPE_PARAMETER hoist vs Go's short-circuit, the
tuple-embeds-interface layer for instantiations, the InterfaceType
data layer in get_base_types). Orchestrator fresh-binary verification:
fixture twins IDENTICAL at parse, bind, AND check; bind 300-slice
identical 143/143; program twin 15/15; check 300-slice (fresh
measurement): Rust 85 match / 135 mismatch / 164 missing-baseline vs
Go 128 / 96 / 160 — **126 records divergent driver-to-driver** (the
remaining checker gap: services slice, special-globals arm, W4/W5
families, the documented checkUnused semantics difference). The climb
targets: drive Rust 85 -> 128 with twin convergence.

## W3 tranche 3 — services slice, real special-globals, isErrorType (2026-10-12)

Landed (rust/crates/checker/** + rust/bench/go-driver):

1. **Counting reconciled**: the canonical divergence metric is
   "records where ANY of verdict/errors_count/shape_hash differ
   driver-to-driver" — **126 at baseline** (the worker's ~179 was a
   multi-fold double-count; non-overlapping folds: 57 rust-fewer /
   30 rust-more / 39 count-eq-hash-diff, 45 verdict flips, 28
   hash-only, 258 identical). Script $HOME/.w3t3-scratch/divcount.py.
2. **checkUnused aligned** (compiler_runner.rs): the seam passes
   `noUnusedLocals||noUnusedParameters` to check_source_file (Go
   GetDiagnostics checker.go:14232's rule).
3. **Special-globals arm REAL on both drivers**: spine.rs
   get_global_symbol → resolve_name_worker(None, ...) (the ported
   NameResolver walk) + real get_global_type_alias_symbol (declared
   type, typeAliasLinks arity check, TS2317); resolution.rs's
   on_failed_to_resolve_symbol takes Option<NodeId> with Go's exact
   nil gates (Go's getGlobalSymbol passes nil location WITH a
   diagnostic — location-less global diagnostics); utilities.rs
   get_file_and_range gained the NodeId::NONE guard. The Go driver's
   confRustSilentGlobalArmKeys filter DELETED — both drivers now emit
   the machinery TS2316/2318 families raw.
4. **isErrorType misport FIXED** (the big one): relater.rs /
   typeofsymbol.rs is_error_type carried only `t == errorType`; Go's
   second arm (`flags&Any != 0 && t.alias != nil` — the any-flagged
   error types getTypeFromTypeAliasReference manufactures for
   UNRESOLVED symbol references) was missing. Without it every
   annotated-async no-lib case ran past checkAsyncFunctionReturnType's
   gate and emitted a spurious TS2318-Promise (105 records). Pin:
   spine.rs w3t3_annotated_async_emits_no_promise_2318.
5. **services.go ported** (62 funcs / 1,149 Go lines → checker/src/
   services.rs, ~1.9k lines with PORT notes): GetSymbolsInScope +
   the copy closures (CombinedLocalAndExportSymbolFlags via
   ast/symbol.rs), IsValidPropertyAccess(+WithType), apparent-
   properties/signatures/index twins, TryGetMemberInModuleExports,
   GetContextualType (S9 permissive), inference-block/caching
   runners, GetRootSymbols, GetShorthandAssignmentValueSymbol,
   GetReferencesToSymbolInFile (real text scan; astnav nodes arm
   bannered M7), GetTypeArgumentConstraint,
   IsTypeInvalidDueToUnionDiscriminant (M7 getTypeOfNode dormant),
   GetSignatureFromDeclaration, lib hover verbosity twins (new
   permissive `is_source_file_default_library` on the Program trait),
   destructuring property/type composition, and the kind-set
   predicates. Pins: spine.rs w3t3_services_get_symbols_in_scope_
   reports_function_locals + w3t3_services_parameter_property_
   declaration_symbols.
6. **--dump-filter bisect tooling on BOTH drivers**: shape-hash fold
   lines printable per matching case (drove the isErrorType root-cause).

Fresh measurement (both drivers, check 300-slice, 384 records):
- Regression gates: fixture twins IDENTICAL at parse/bind/check (9/9);
  bind 300-slice byte-identical 143/163/78/0; program twin 15/15.
- Histograms IDENTICAL driver-to-driver: Rust 0/174/210/0, Go
  0/174/210/0 (match/mismatch/missing-baseline/panic). Divergent:
  **232 after the raw special-globals fix, 131 after isErrorType**
  (64 rust-fewer / 22 rust-more / 45 count-eq-hash-diff; verdict flips
  45 → **0**). The raw-fix regression was fully recovered by
  isErrorType.
- SEMANTICS DECISION (the parent's call recorded): dropping the
  special-globals filter collapses baseline-match% to ~0 on BOTH
  drivers identically — the lib-less twin shape emits the machinery
  TS2316/2318 noise the reference baselines (generated WITH libs)
  never contain. The twin discipline (driver-to-driver identity) is
  stronger than baseline (verdict flips 0), but the climb-vs-baseline
  metric stays ~0 until the twin grows lib-file support. The dispatch
  chose to keep the filter dropped on both drivers.
- Remaining divergent families (next tranches, mostly S6-S9 depth,
  not shallow misports): TS2454 definite-assignment ×47,
  async-2697 ×37, TS2304 ×33 (go-extra); TS2339 ×54 + 2425/2417/2719
  ×56 (rust-extra, S8 member/relate depth).
- Validation: cargo test -p tsc-checker -p tsc-testrunner green (93
  lib tests incl. 3 w3t3 pins); clippy zero on checker + testrunner
  + bench-harness; go vet + go build clean.

## W3 tranche 4 — the divergence-family chase, first blood (2026-10-12)

Divergent 131 -> 111. One port killed two census families:
check_identifier's missing live definite-assignment arms (container-hop
loop, is_never_initialized, TS2454 with Go's declared-type fallback,
TS7022/7005 autoType) — go-extra 2454: 47->0 AND rust-extra 2339:
54->18 (the leaked T|undefined flow types were the property-access
noise). Also: contains_undefined_type misport (Types()[0] probe),
is_mutable_local_variable_declaration off-by-one, is_prototype_property
real (Method-flag check; 2425: 21->0), get_type_without_signatures
real (2417: 17->0, TS2515 now fires both sides), declaration-modifier
flags real. Progression: 131 -> 115 -> 113 -> 111. Remaining census:
async-2697 x37 (machinery mirrors Go but the forced signature return-
type resolution in checkFunctionOrMethodDeclaration isn't — next
tranche), TS2304 x33, 2683 x17, and 41 hash-only diffs dominated by the
type_to_string W4 stub (collapse when W4's printer lands). All gates
held: fixture x3 stages identical, bind 143/143, program 15/15,
verdict flips 0.
