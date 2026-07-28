#!/usr/bin/env bash
# Serialized, timeout-guarded cargo test runner.
#
# Concurrent cargo test invocations against this worktree have been the
# reproducible trigger for the workspace's intermittent test hangs: every
# hang observed under controlled conditions correlated with a second cargo
# test process contending for CPU/the cargo build lock at the same time, and
# neither symptom reproduced on a quiet machine. See
# docs/research/test-isolation-bowerbird-findings.md and
# docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md
# for the investigation.
#
# This script:
#   1. Takes an exclusive lock (mkdir-based; portable, no flock(1) dependency
#      since macOS doesn't ship one) so a second invocation queues instead of
#      racing with the first.
#   2. Runs cargo test under a timeout so a genuine hang fails loudly instead
#      of hanging forever.
#
# Usage:
#   scripts/test.sh                        # cargo test --workspace -- --test-threads=1
#   scripts/test.sh -p bowerbird-daemon --test contract_daemon -- --exact some_test
#
# Env overrides:
#   BOWERBIRD_TEST_TIMEOUT_SECS   per-run timeout, seconds (default 300)
#   BOWERBIRD_TEST_LOCK_WAIT_SECS max time to wait for the lock (default 900)
#
# Compatibility: portable bash (mkdir-based lock, no `mapfile`/flock); works
# on macOS bash 3.2 and Ubuntu bash 5.

set -eu

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
LOCK_DIR="$TARGET_DIR/.bowerbird-test-lock"
TIMEOUT_SECS="${BOWERBIRD_TEST_TIMEOUT_SECS:-300}"
LOCK_WAIT_SECS="${BOWERBIRD_TEST_LOCK_WAIT_SECS:-900}"
STALE_AFTER_SECS=$((TIMEOUT_SECS * 2 + 60))

mkdir -p "$TARGET_DIR"

lock_holder_pid() {
  cat "$LOCK_DIR/pid" 2>/dev/null || echo "unknown"
}

lock_is_stale() {
  local pid
  pid="$(lock_holder_pid)"

  if [ "$pid" != "unknown" ] && ! kill -0 "$pid" 2>/dev/null; then
    return 0
  fi

  local created now age
  created="$(cat "$LOCK_DIR/created" 2>/dev/null || echo 0)"
  now="$(date +%s)"
  age=$((now - created))
  [ "$age" -gt "$STALE_AFTER_SECS" ]
}

acquire_lock() {
  local waited=0
  local announced=0

  while ! mkdir "$LOCK_DIR" 2>/dev/null; do
    if lock_is_stale; then
      echo "test.sh: clearing a stale lock left by pid $(lock_holder_pid)" >&2
      rm -rf "$LOCK_DIR"
      continue
    fi

    if [ "$announced" -eq 0 ]; then
      echo "test.sh: another cargo test run (pid $(lock_holder_pid)) holds the lock; waiting..." >&2
      announced=1
    fi

    if [ "$waited" -ge "$LOCK_WAIT_SECS" ]; then
      echo "test.sh: waited ${LOCK_WAIT_SECS}s for the test lock and gave up; is a run stuck?" >&2
      exit 1
    fi

    sleep 1
    waited=$((waited + 1))
  done

  echo "$$" >"$LOCK_DIR/pid"
  date +%s >"$LOCK_DIR/created"
}

release_lock() {
  rm -rf "$LOCK_DIR"
}

acquire_lock
trap release_lock EXIT

timeout_cmd=""
if command -v timeout >/dev/null 2>&1; then
  timeout_cmd="timeout"
elif command -v gtimeout >/dev/null 2>&1; then
  timeout_cmd="gtimeout"
else
  echo "test.sh: warning: no 'timeout'/'gtimeout' found; running WITHOUT a hang timeout (brew install coreutils for one)" >&2
fi

args=("$@")
if [ "$#" -eq 0 ]; then
  args=(--workspace -- --test-threads=1)
fi

echo "test.sh: cargo test ${args[*]}" >&2

if [ -n "$timeout_cmd" ]; then
  "$timeout_cmd" "$TIMEOUT_SECS" cargo test "${args[@]}"
else
  cargo test "${args[@]}"
fi
