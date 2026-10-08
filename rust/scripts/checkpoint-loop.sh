#!/usr/bin/env bash
# checkpoint-loop.sh — the 15-minute commit cadence (rust/SPEC.md §10).
#
# Every 900s: if the working tree is dirty, `git add -A` and commit
#   wip(auto): checkpoint HH:MM — N files changed
# then push to origin. Single instance (flock on .git/tsrs-checkpoint.lock).
# Never amends or rebases. Started by the orchestrator via fireAndForget;
# stopped at session end after a final real commit.
set -u

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BRANCH="${TS_CHECKPOINT_BRANCH:-rust-perf}"
INTERVAL="${TS_CHECKPOINT_INTERVAL:-900}"

cd "$REPO_ROOT" || exit 1
exec 9>"$REPO_ROOT/.git/tsrs-checkpoint.lock"
flock -n 9 || { echo "checkpoint loop already running; exiting"; exit 0; }

echo "checkpoint loop: every ${INTERVAL}s on branch ${BRANCH} (pid $$, start $(date +%H:%M:%S))"
while :; do
  sleep "$INTERVAL"
  now=$(date +%H:%M)
  if git diff --quiet HEAD --ignore-submodules 2>/dev/null && \
     [ -z "$(git ls-files --others --exclude-standard)" ]; then
    echo "$now clean"
    continue
  fi
  git add -A 2>/dev/null || { echo "$now add failed; retry next tick"; continue; }
  if git diff --cached --quiet 2>/dev/null; then
    echo "$now nothing staged (race with orchestrator commit); skipping"
    continue
  fi
  n=$(git diff --cached --numstat | wc -l)
  if git commit -m "wip(auto): checkpoint ${now} — ${n} files changed" >/dev/null 2>&1; then
    echo "$now committed (${n} files)"
    git push origin "$BRANCH" >/dev/null 2>&1 \
      || echo "$now push failed (will retry next tick)"
  else
    echo "$now commit failed (likely index.lock race); retry next tick"
  fi
done
