#!/usr/bin/env bash
# bench-micro.sh — Phase 0 micro-race: Rust port vs Go port (SPEC.md §15.1).
#
# Runs ON the rig (dih@192.168.1.15) against the rsync mirror at ~/TypeScript.
# Races the ported Rust crates against Go's own benchmarks with identical
# inputs (mirrored by `tsc-bench micro`), then prints a joined table:
#
#   sub-bench | go ns/op (median) | rust ns/op (median) | rust/go | verdict
#
# Join rule: by sub-bench name first (Rust names mirror Go's, spaces → "_");
# names that do not match fall back to positional pairing within the same
# benchmark family, printing a WARNING (so a name mismatch is visible,
# never silent).
#
# Usage: bash rust/scripts/bench-micro.sh [--samples N]
set -uo pipefail

RUST_DIR="$HOME/TypeScript/rust"
TSC_DIR="$HOME/TypeScript/tsc"
GO="$HOME/opt/go/bin/go"
CARGO="$HOME/.cargo/bin/cargo"
SAMPLES=10
[ "${1:-}" = "--samples" ] && [ -n "${2:-}" ] && SAMPLES="$2"

OUT="$HOME/bench-results/phase0"
mkdir -p "$OUT"

echo "== building tsc-bench (release)"
(cd "$RUST_DIR" && "$CARGO" build --release -p bench-harness) || exit 1
BENCH="$RUST_DIR/target/release/tsc-bench"

# Families: <pkg> <Go benchmark name> <rust op name>
FAMILIES=(
  "tspath CombinePaths tspath/CombinePaths"
  "tspath GetNormalizedAbsolutePath tspath/GetNormalizedAbsolutePath"
  "tspath ToFileNameLowerCase tspath/ToFileNameLowerCase"
  "tspath HasRelativePathSegment tspath/HasRelativePathSegment"
  "tspath PathIsRelative tspath/PathIsRelative"
  "tspath RootedDirectoryPathResolveFile tspath/RootedDirectoryPathResolveFile"
  "tspath RootedFilePathToPathKey tspath/RootedFilePathToPathKey"
  "jsnum ToInt32 jsnum/ToInt32"
  "jsnum Exponentiate jsnum/Exponentiate"
)

echo "== Go side: go test -bench (-count=$SAMPLES) — this also compiles the packages"
for fam in "${FAMILIES[@]}"; do
  read -r pkg goname rustop <<<"$fam"
  echo "  go: $pkg.$goname"
  (cd "$TSC_DIR" && "$GO" test "./internal/$pkg" -run '^$' -bench "^${goname}\$" \
     -count="$SAMPLES" -benchmem) > "$OUT/go_${pkg}_${goname}.txt" 2>&1 &
done
wait

echo "== Rust side: tsc-bench micro (--samples $SAMPLES)"
for fam in "${FAMILIES[@]}"; do
  read -r pkg goname rustop <<<"$fam"
  echo "  rust: $rustop"
  "$BENCH" micro --op "$rustop" --samples "$SAMPLES" > "$OUT/rust_${rustop//\//_}.tsv" 2>&1
done

echo "== joining"
python3 - "$OUT" <<'PYEOF'
import re, sys, glob, os, statistics

out_dir = sys.argv[1]
FAMILIES = [
    ("tspath", "CombinePaths"), ("tspath", "GetNormalizedAbsolutePath"),
    ("tspath", "ToFileNameLowerCase"), ("tspath", "HasRelativePathSegment"),
    ("tspath", "PathIsRelative"), ("tspath", "RootedDirectoryPathResolveFile"),
    ("tspath", "RootedFilePathToPathKey"), ("jsnum", "ToInt32"),
    ("jsnum", "Exponentiate"),
]

go_line = re.compile(r"^(Benchmark\S+?)(?:-\d+)?\s+([\d.]+) ns/op")
rows = []
for pkg, goname in FAMILIES:
    go_path = os.path.join(out_dir, f"go_{pkg}_{goname}.txt")
    rust_path = os.path.join(out_dir, f"rust_{pkg}_{goname}.tsv")
    go_samples = {}
    go_order = []
    with open(go_path, errors="replace") as f:
        for line in f:
            m = go_line.match(line.strip())
            if not m:
                continue
            name, ns = m.group(1), float(m.group(2))
            if "_(old)" in name:
                continue  # Go also benches a legacy variant; not part of the race
            if name not in go_samples:
                go_order.append(name)
                go_samples[name] = []
            go_samples[name].append(ns)
    go_med = {n: statistics.median(v) for n, v in go_samples.items() if len(v) > 1}

    rust_med = {}
    rust_order = []
    with open(rust_path, errors="replace") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) != 5:
                continue
            median, _, _, op, _ = parts
            if op not in rust_med:
                rust_order.append(op)
                rust_med[op] = float(median)

    # name join: Go "Benchmark<Op>/<sub>" -> Rust "<pkg>/<Op>/<sub>"
    def go_to_rust(gname):
        assert gname.startswith("Benchmark")
        return f"{pkg}/" + gname[len("Benchmark"):]

    matched_rust = set()
    pairs = []
    for gname in go_order:
        key = go_to_rust(gname)
        if key in rust_med:
            pairs.append((gname, go_med.get(gname), rust_med[key]))
            matched_rust.add(key)
    unmatched_go = [g for g in go_order if go_to_rust(g) not in rust_med]
    unmatched_rust = [r for r in rust_order if r not in matched_rust]
    if unmatched_go or unmatched_rust:
        if len(unmatched_go) == len(unmatched_rust):
            print(f"WARNING: name mismatch in {pkg}/{goname}; pairing positionally:")
            for g, r in zip(unmatched_go, unmatched_rust):
                print(f"  go: {g}\n  rust: {r}")
                pairs.append((g, go_med.get(g), rust_med[r]))
        else:
            print(f"WARNING: unpairable rows in {pkg}/{goname}:")
            print(f"  go:   {unmatched_go}")
            print(f"  rust: {unmatched_rust}")

    for gname, g, r in pairs:
        ratio = (r / g) if g else float("nan")
        verdict = "rust faster" if ratio < 0.95 else ("parity" if ratio <= 1.05 else "go faster")
        rows.append((f"{gname}", g, r, ratio, verdict))

print(f"\n{'sub-bench':70} {'go ns/op':>12} {'rust ns/op':>12} {'rust/go':>8}  verdict")
for name, g, r, ratio, verdict in rows:
    print(f"{name:70} {g:12.1f} {r:12.1f} {ratio:8.3f}  {verdict}")

if rows:
    import math
    ratios = [x[3] for x in rows if x[3] == x[3]]
    geo = math.exp(sum(math.log(r) for r in ratios) / len(ratios))
    print(f"\nGEOMEAN rust/go across {len(ratios)} sub-benches: {geo:.3f}")
PYEOF

echo "== raw outputs in $OUT"
exit 0
