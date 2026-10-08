#!/usr/bin/env bash
# rig-setup.sh — provision the remote test rig (dih@192.168.1.15).
#
# Idempotent, no sudo. Run ON the remote after remote-sync.sh, e.g.:
#   ssh dih@192.168.1.15 'bash ~/TypeScript/rust/scripts/rig-setup.sh'
#
# Provisions (SPEC.md §9.2 + §15):
#   ~/opt/go                       Go 1.27 (tsc/go.mod requires go 1.27)
#   ~/.cargo/bin/hyperfine         benchmark runner (cargo install)
#   ~/bench-bin/rsswrap            wall time + peak RSS wrapper (gcc)
#   ~/bench-bin/tsgo-go            baseline binary built from ~/TypeScript/tsc
#   ~/bench-corpus/excalidraw      benchmark corpus (excalidraw.com), pinned SHA
set -euo pipefail

OPT="$HOME/opt"
BIN="$HOME/bench-bin"
CORPUS="$HOME/bench-corpus"
RUST="$HOME/TypeScript/rust"
mkdir -p "$OPT" "$BIN" "$CORPUS"

echo "== 1/5 Go toolchain"
if [ -x "$OPT/go/bin/go" ]; then
  echo "go already installed: $("$OPT/go/bin/go" version)"
else
  fname=$(curl -sL 'https://go.dev/dl/?mode=json' | grep -oE 'go1\.27\.[0-9]+\.linux-amd64\.tar\.gz' | head -1)
  if [ -z "$fname" ]; then
    echo "FATAL: could not resolve a go1.27 linux-amd64 tarball from go.dev"
    exit 1
  fi
  echo "downloading https://go.dev/dl/$fname"
  curl -sL "https://go.dev/dl/$fname" | tar -C "$OPT" -xz
  "$OPT/go/bin/go" version
fi

echo "== 2/5 hyperfine"
if [ -x "$HOME/.cargo/bin/hyperfine" ]; then
  "$HOME/.cargo/bin/hyperfine" --version
elif command -v hyperfine >/dev/null 2>&1; then
  hyperfine --version
else
  "$HOME/.cargo/bin/cargo" install hyperfine --locked
  "$HOME/.cargo/bin/hyperfine" --version
fi

echo "== 3/5 rsswrap"
if [ ! -x "$BIN/rsswrap" ]; then
  cc -O2 -o "$BIN/rsswrap" "$RUST/bench/rsswrap.c"
fi
"$BIN/rsswrap" true

echo "== 4/5 tsgo baseline binary"
if [ ! -x "$BIN/tsgo-go" ]; then
  (cd "$HOME/TypeScript/tsc" && "$OPT/go/bin/go" build -o "$BIN/tsgo-go" ./cmd/tsc)
fi
"$BIN/tsgo-go" --version 2>&1 || echo "tsgo-go --version exited nonzero (check binary)"

echo "== 5/5 corpus (excalidraw)"
if [ ! -d "$CORPUS/excalidraw/.git" ]; then
  git clone --depth 1 https://github.com/excalidraw/excalidraw "$CORPUS/excalidraw"
fi
CORPUS_SHA=$(git -C "$CORPUS/excalidraw" rev-parse HEAD)
TS_FILES=$(find "$CORPUS/excalidraw" \( -name node_modules -o -name dist -o -name .git \) -prune -o \( -name '*.ts' -o -name '*.tsx' \) -print | wc -l)
TS_BYTES=$(find "$CORPUS/excalidraw" \( -name node_modules -o -name dist -o -name .git \) -prune -o \( -name '*.ts' -o -name '*.tsx' \) -print0 | du -cb --files0-from=- 2>/dev/null | tail -1 | cut -f1)

cat <<EOF
== RIG READY ==
go:          $($OPT/go/bin/go version)
rustc:       $($HOME/.cargo/bin/rustc --version)
hyperfine:   $("$HOME/.cargo/bin/hyperfine" --version 2>/dev/null || hyperfine --version 2>/dev/null || echo n/a)
tsgo:        $($BIN/tsgo-go --version 2>&1 | head -1 || echo n/a)
rsswrap:     $($BIN/rsswrap true | head -1)"
corpus:      $CORPUS/excalidraw
corpus_sha:  $CORPUS_SHA
ts_files:    $TS_FILES
ts_bytes:    $TS_BYTES
EOF
echo "== done =="
