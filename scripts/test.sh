#!/usr/bin/env bash
# Lock-guarded, timeout-guarded cargo test runner.
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
#      since macOS doesn't ship one). If another run already holds it, this
#      exits immediately (does NOT block/poll) so a caller (human or agent)
#      gets a fast, clear answer instead of a tool call sitting open for
#      minutes. Re-run once the other one finishes, or use --unlock.
#   2. Runs cargo test under a timeout so a genuine hang fails loudly instead
#      of hanging forever.
#   3. Traps Ctrl-C/SIGTERM: kills the cargo test process tree and releases
#      the lock immediately instead of leaving orphaned processes behind.
#
# Usage:
#   scripts/test.sh                        # cargo test --workspace -- --test-threads=1
#   scripts/test.sh -p bowerbird-daemon --test contract_daemon -- --exact some_test
#   scripts/test.sh --unlock               # force-kill a stuck/old run and clear its lock
#
# Env overrides:
#   BOWERBIRD_TEST_TIMEOUT_SECS   per-run timeout, seconds (default 300)
#
# Compatibility: portable bash (mkdir-based lock, no `mapfile`/flock); works
# on macOS bash 3.2 and Ubuntu bash 5.

set -eu

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
LOCK_DIR="$TARGET_DIR/.bowerbird-test-lock"
TIMEOUT_SECS="${BOWERBIRD_TEST_TIMEOUT_SECS:-300}"
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

write_lock() {
  echo "$$" >"$LOCK_DIR/pid"
  date +%s >"$LOCK_DIR/created"
}

# Fails fast (no polling/blocking) — see header comment #1.
acquire_lock() {
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    write_lock
    return 0
  fi

  if lock_is_stale; then
    echo "test.sh: clearing a stale lock left by pid $(lock_holder_pid)" >&2
    rm -rf "$LOCK_DIR"
    if mkdir "$LOCK_DIR" 2>/dev/null; then
      write_lock
      return 0
    fi
  fi

  echo "test.sh: another cargo test run (pid $(lock_holder_pid)) already holds the lock (${LOCK_DIR})." >&2
  echo "test.sh: not waiting. Re-run once it finishes, or run 'scripts/test.sh --unlock' to force-clear a stuck run." >&2
  exit 2
}

release_lock() {
  rm -rf "$LOCK_DIR"
}

# Descendants of $1, in post-order (children before parent), one pid per line.
# Best-effort: if `pgrep` isn't available, prints nothing and the caller falls
# back to killing just the given pid.
collect_descendants() {
  local parent="$1"
  local child
  if ! command -v pgrep >/dev/null 2>&1; then
    return 0
  fi
  for child in $(pgrep -P "$parent" 2>/dev/null || true); do
    collect_descendants "$child"
    echo "$child"
  done
}

# SIGTERM the given pid + its descendants, SIGKILL any stragglers after a
# short grace period. Used both by --unlock (on a lock file's recorded pid)
# and by the INT/TERM trap below (on the run we just started).
kill_tree() {
  local root="$1"
  local pids
  pids="$(collect_descendants "$root")
$root"

  local p
  for p in $pids; do
    kill -TERM "$p" 2>/dev/null || true
  done

  local waited=0
  while [ "$waited" -lt 5 ]; do
    local any_alive=0
    for p in $pids; do
      if kill -0 "$p" 2>/dev/null; then
        any_alive=1
      fi
    done
    [ "$any_alive" -eq 0 ] && break
    sleep 1
    waited=$((waited + 1))
  done

  for p in $pids; do
    if kill -0 "$p" 2>/dev/null; then
      echo "test.sh: pid ${p} still alive after SIGTERM; sending SIGKILL" >&2
      kill -KILL "$p" 2>/dev/null || true
    fi
  done
}

force_unlock() {
  if [ ! -d "$LOCK_DIR" ]; then
    echo "test.sh: no lock held (${LOCK_DIR} does not exist); nothing to do" >&2
    exit 0
  fi

  local pid
  pid="$(lock_holder_pid)"

  if [ "$pid" = "unknown" ] || ! kill -0 "$pid" 2>/dev/null; then
    echo "test.sh: lock holder (pid ${pid}) is not running; clearing stale lock" >&2
    rm -rf "$LOCK_DIR"
    exit 0
  fi

  echo "test.sh: killing pid ${pid} and its descendants" >&2
  kill_tree "$pid"

  rm -rf "$LOCK_DIR"
  echo "test.sh: lock cleared" >&2
  exit 0
}

if [ "${1:-}" = "--unlock" ] || [ "${1:-}" = "--force-unlock" ]; then
  force_unlock
fi

acquire_lock
trap release_lock EXIT

run_pid=""

on_interrupt() {
  local sig="$1"
  echo "test.sh: received ${sig}; stopping the test run..." >&2
  if [ -n "$run_pid" ]; then
    kill_tree "$run_pid"
  fi
  exit 130
}

trap 'on_interrupt INT' INT
trap 'on_interrupt TERM' TERM

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
  "$timeout_cmd" "$TIMEOUT_SECS" cargo test "${args[@]}" &
else
  cargo test "${args[@]}" &
fi
run_pid=$!
wait "$run_pid"
