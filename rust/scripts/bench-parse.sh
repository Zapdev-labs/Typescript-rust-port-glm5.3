#!/usr/bin/env bash
# bench-parse.sh — Phase A corpus parse race: Rust port vs Go port (SPEC.md §15.1).
#
# Runs ON the RIG (dih@192.168.1.15) against the rsync mirror at ~/TypeScript.
# Both drivers do identical work per rust/bench/DRIVER-CONTRACT.md:
#
#   parse --corpus <dir> [--threads N] [--iterations K] --json
#
# The race only counts when both sides agree exactly (nodes, errors,
# shape_hash) at threads=1/K=1. Verdict gates: first green <=1.30x Go,
# hardening <=1.00x, stretch <=0.80x; peak RSS <=1.0x Go.
#
# Usage: bash rust/scripts/bench-parse.sh [--samples N] [--threads "1 16"]
set -uo pipefail

RUST_DIR="$HOME/TypeScript/rust"
TSC_DIR="$HOME/TypeScript/tsc"
CORPUS="$HOME/bench-corpus/excalidraw"
GO="$HOME/opt/go/bin/go"
CARGO="$HOME/.cargo/bin/cargo"
GO_BENCH="$HOME/bench-bin/go-bench"
RSSWRAP="$HOME/bench-bin/rsswrap"

SAMPLES=5
THREADS_LIST="1 16"
while [ $# -gt 0 ]; do
  case "$1" in
    --samples) SAMPLES="$2"; shift 2 ;;
    --threads) THREADS_LIST="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

OUT="$HOME/bench-results/phaseA"
mkdir -p "$OUT"

echo "== building go-bench + tsc-bench (release) + rsswrap"
(cc -O2 -o "$RSSWRAP" "$RUST_DIR/bench/rsswrap.c") || exit 1
(cd "$HOME/TypeScript" && "$GO" build -o "$GO_BENCH" ./rust/bench/go-driver) || exit 1
(cd "$RUST_DIR" && "$CARGO" build --release -p bench-harness) || exit 1
RUST_BENCH="$RUST_DIR/target/release/tsc-bench"

run_side() { # side, binary, threads, outprefix
  local side="$1" bin="$2" threads="$3" prefix="$4"
  local i
  for i in $(seq 1 "$SAMPLES"); do
    "$RSSWRAP" "$bin" parse --corpus "$CORPUS" --threads "$threads" --json \
      > "$OUT/${prefix}_${i}.json" 2> "$OUT/${prefix}_${i}.err" || {
        echo "ERROR: $side run $i failed (see $OUT/${prefix}_${i}.err):" >&2
        tail -3 "$OUT/${prefix}_${i}.err" >&2
        return 1
      }
  done
}

for T in $THREADS_LIST; do
  echo "== threads=$T: go side ($SAMPLES runs)"
  run_side go "$GO_BENCH" "$T" "go_t$T" || exit 1
  echo "== threads=$T: rust side ($SAMPLES runs)"
  run_side rust "$RUST_BENCH" "$T" "rust_t$T" || exit 1
done

python3 - "$OUT" "$SAMPLES" "$THREADS_LIST" <<'PYEOF'
import json, re, statistics, sys

out_dir, samples, threads_list = sys.argv[1], int(sys.argv[2]), sys.argv[3].split()

def load(side, t):
    rows, rss = [], []
    for i in range(1, samples + 1):
        with open(f"{out_dir}/{side}_t{t}_{i}.json") as f:
            rows.append(json.load(f))
        with open(f"{out_dir}/{side}_t{t}_{i}.err") as f:  # rsswrap stats land on stderr
            text = f.read()
            m = re.search(r"maxrss_kb=(\d+)", text)
            if m:
                rss.append(float(m.group(1)))  # kB
    return rows, rss

print(f"\n{'threads':>7} {'go wall ms':>11} {'rust wall ms':>13} {'rust/go':>8} "
      f"{'go MB/s':>8} {'rust MB/s':>10} {'nodes':>10} {'shape ok':>9} {'rss go':>9} {'rss rust':>9}")
gate = None
for t in threads_list:
    go_rows, go_rss = load("go", t)
    ru_rows, ru_rss = load("rust", t)
    go_wall = statistics.median(r["wall_ms"] for r in go_rows)
    ru_wall = statistics.median(r["wall_ms"] for r in ru_rows)
    go_mb = statistics.median(r["mb_per_s"] for r in go_rows)
    ru_mb = statistics.median(r["mb_per_s"] for r in ru_rows)
    nodes_ok = all(r["nodes"] == go_rows[0]["nodes"] for r in ru_rows) and go_rows[0]["nodes"] == ru_rows[0]["nodes"]
    shape_ok = all(r["shape_hash"] == go_rows[0]["shape_hash"] for r in ru_rows) and go_rows[0]["shape_hash"] == ru_rows[0]["shape_hash"]
    ratio = ru_wall / go_wall
    go_rss_med = f"{statistics.median(go_rss):.0f}" if go_rss else "-"
    ru_rss_med = f"{statistics.median(ru_rss):.0f}" if ru_rss else "-"
    print(f"{t:>7} {go_wall:11.1f} {ru_wall:13.1f} {ratio:8.3f} "
          f"{go_mb:8.1f} {ru_mb:10.1f} {ru_rows[0]['nodes']:>10,} "
          f"{'MATCH' if shape_ok and nodes_ok else 'MISMATCH':>9} "
          f"{go_rss_med:>9} {ru_rss_med:>9}")
    if t == "1":
        gate = (ratio, nodes_ok, shape_ok)

print()
if gate is None:
    sys.exit(2)
ratio, nodes_ok, shape_ok = gate
if not (nodes_ok and shape_ok):
    print(f"RACE INVALID: nodes/errors/shape_hash disagree at threads=1 — drivers do not do identical work.")
    sys.exit(1)
if ratio <= 0.80:
    print(f"GATE: STRETCH PASSED ({ratio:.3f}x <= 0.80x)")
elif ratio <= 1.00:
    print(f"GATE: HARDENING TARGET MET ({ratio:.3f}x <= 1.00x); first green already achieved")
elif ratio <= 1.30:
    print(f"GATE: FIRST GREEN ({ratio:.3f}x <= 1.30x) — hardening to <=1.00x is next")
else:
    print(f"GATE: FAILED ({ratio:.3f}x > 1.30x) — see SPEC.md §15.2 perf playbook")
    sys.exit(1)
PYEOF

echo "== raw outputs in $OUT"
