# options-model.json

Hand-mirrored, minimal extraction of the subset of
`tools/scripts/tsc/options.ts` (the hand-maintained `OptionsModel` literal,
commit `ec47d33c23e464a17cdf2475632cba629bee8763`) that
`tools/scripts/tsc/generate-options.ts` emits into
`tsc/internal/core/options_generated.go`:

- `enums` — the 6 numeric enums (`ModuleDetectionKind`, `ModuleKind`,
  `ModuleResolutionKind`, `NewLineKind`, `ScriptTarget`, `JsxEmit`) with exact
  member names, values, and the comments/trailing comments the Go output
  carries. Members with string values (e.g. `ScriptTarget.Latest`) are Go
  value aliases (`ScriptTargetLatest ScriptTarget = ScriptTargetESNext`).
- `stringer` — mirrors `OptionEnum.stringer`: `"name"` stringifies by member
  name (Go `ModuleResolutionKind.String()`); an object maps member name →
  display string, pre-resolved from the `enumMaps.jsx` entries in options.ts
  (Go `JsxEmit.String()`).
- `pluginImportFields` / `compilerOptions` / `typeAcquisition` / `buildOptions`
  (+ `buildOptionFieldOrder`) — exactly the rows `coreOptions()` and
  `storedOptions()` consume: `name`, `goName` (only where options.ts overrides
  the default `fieldName()` capitalization), the Go field `type` (after
  `goType()` path-kind mapping), and `deprecated`/`internal`/`section`/
  `comment` where the Go output reflects them. `buildOptions` includes the
  declaration-only rows (e.g. `build`); rows without `goName` have no stored
  field and are skipped, matching `generateOptions()`'s
  `option.field !== undefined` filter, and stored fields are ordered by
  `buildOptionFieldOrder` (`orderByName`).

## Deliberately excluded (future `tsc-tsoptions` crate)

Everything in options.ts that only feeds the other 6 generator outputs:
`schemaOnlyOptions`, declaration metadata (`declaration`,
`declarationOrder`, `elements`), `rootOptions`, `enumMaps` proper (except the
pre-resolved jsx stringer strings), `jsconfigDefault`, `parseAliases`,
`transpile` configs, `lib`/`libComment` (→ `targetToLibMap`), and the
`api`/`excludeFromAPI` flags (→ the TS `api/*.generated.ts` outputs).

## Re-verification

The Go output (`tsc/internal/core/options_generated.go`) is never a
generation input (SPEC §6) — it may be used only as a parity baseline. To
re-verify this mirror:

1. Compare against `tsc/internal/core/options_generated.go` (the same commit):
   struct field names/order/types, enum members/values/comments, stringer
   outputs, and the `ModuleKindToModuleResolutionKind` entries must match.
2. Or, in a TypeScript checkout at the same commit, re-run
   `npx hereby generate:compileroptions` (which runs
   `tools/scripts/tsc/generate-options.ts`) and diff the core output against
   the struct/enum tables this file produces via
   `cargo run -p gen-options -- --check`.

If options.ts changes upstream, update this file by hand (it is the single
input to `rust/tools/gen-options`) and re-run the generator.
