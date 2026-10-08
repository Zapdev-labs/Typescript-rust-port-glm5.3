# Benchmark Driver Contract — `go-bench` (Go) ↔ `tsc-bench` (Rust)

The Phase A corpus parse race (SPEC.md §15.1) requires both drivers to do
IDENTICAL work and emit IDENTICAL output apart from timing. The Go driver is
`rust/bench/go-driver` (module `github.com/microsoft/TypeScript/tsc/rust-bench`
— the `tsc/` prefix is REQUIRED so Go's internal-package rule permits
importing `tsc/internal/...` from outside the `tsc/` tree). The Rust driver is
`tsc-bench parse` in `rust/crates/bench-harness`. The Rust side MUST implement
every rule below byte-for-byte.

## CLI

```
parse --corpus <dir> [--threads N] [--iterations K] [--json]
```

- Corpus walk: extensions `.ts`, `.tsx`, `.mts`, `.cts`, `.d.ts`; files sorted
  by path; SKIP dirs `node_modules`, `.git`, `dist`, `build`, `out`, `.yarn`.
- Defaults: threads 1, iterations 1. `--iters` is accepted as an alias of
  `--iterations` on both sides.

## Output (JSON, key order fixed)

```json
{"driver":"go-bench","op":"parse","files":N,"bytes":B,"iterations":K,"threads":T,
 "wall_ms":X,"mb_per_s":Y,"nodes":Z,"errors":E,"shape_hash":"<16-hex>"}
```

(`driver` is `"tsc-bench"` on the Rust side; everything else identical.)

## Shared rules

1. **scriptKind** (from `core.GetScriptKindFromFileName`): `.tsx` → TSX (4),
   every other indexed extension (`.ts` incl. `.d.ts`, `.mts`, `.cts`) → TS (3).
2. **Node count**: the SourceFile root node + every node reachable via the
   non-JSDoc `ForEachChild` walker, INCLUDING `EndOfFileToken`, EXCLUDING
   NodeList wrappers (walkers visit list children, not the list itself).
   The walker must return false to continue (self-recursing visitor).
3. **Errors**: `len(source_file.Diagnostics())` — parse diagnostics only.
4. **Parse options**: `{file_name: tspath.RootedFilePath, path_key:
   tspath.PathKey}` from the resolved path; external-module indicator options
   zeroed.
5. **Totals are run totals**: nodes/errors/histogram sum over ALL K iterations
   and all files (they scale with K); `mb_per_s = bytes*K / wall`.
6. **Unreadable files**: +1 to errors (and thus to shape_hash), excluded from
   `files`/`bytes`; the walk continues. `files` = successfully read + parsed.
7. **Determinism**: workers write per-file-index slots; merge in file-index
   order after each pass. JSON counts + shape_hash are identical at any
   `--threads`; only `wall_ms`/`mb_per_s` vary (rounded to 3 decimals).
8. **Timing**: file IO happens before timing starts; the timed section covers
   parse + walk over all K passes.

## shape_hash recipe (must match byte-for-byte)

FNV-1a 64-bit: `h = 0xcbf29ce484222325`; per byte: `h ^= b`;
`h *= 0x100000001b3` (wrapping). Fold the bytes of, in order:

1. `"nodes=" + decimal(total_nodes) + "\n"`
2. `"errors=" + decimal(total_errors) + "\n"`
3. for each kind ordinal in ASCENDING numeric order with count > 0:
   `decimal(ordinal) + ":" + decimal(count) + "\n"`

Output: `%016x` lowercase hex. Kind ordinals are the Go `Kind int16` values
(`KindUnknown` = 0, `KindEndOfFile` = 1, ...); the Rust generated kind table
must use the same integers (M2 gen-ast emits them from the same ast.json —
spot-check a sample against `tsc/internal/ast/kind_generated.go`).
