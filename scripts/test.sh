#!/usr/bin/env bash
# Lock-guarded, timeout-guarded cargo test runner with hang diagnostics.
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
#   2. Tees all cargo test output to target/test-logs/<run>/run.log. With
#      --test-threads=1, libtest prints "test name ... " BEFORE running each
#      test and the result after, so on a hang the log's last line names the
#      stuck test.
#   3. Watches a deadline (BOWERBIRD_TEST_TIMEOUT_SECS) itself instead of
#      wrapping cargo in timeout(1). On timeout it captures diagnostics
#      BEFORE killing anything: the likely hung test, the live process tree,
#      and a `sample` backtrace of each live test/daemon process, all into
#      the run's log dir. Then it kills the tree and exits 124.
#   4. Traps Ctrl-C/SIGTERM: records the kill in run.log, kills the cargo
#      test process tree, and releases the lock immediately instead of
#      leaving orphaned processes behind.
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
# on macOS bash 3.2 and Ubuntu bash 5. `sample` backtrace capture is
# macOS-only and skipped elsewhere.

set -eu

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
LOCK_DIR="$TARGET_DIR/.bowerbird-test-lock"
LOG_ROOT="$TARGET_DIR/test-logs"
KEEP_RUNS=10
TIMEOUT_SECS="${BOWERBIRD_TEST_TIMEOUT_SECS:-300}"
STALE_AFTER_SECS=$((TIMEOUT_SECS * 2 + 60))

mkdir -p "$TARGET_DIR"

timestamp() {
  date '+%Y-%m-%d %H:%M:%S'
}

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

# Oldest run dirs beyond KEEP_RUNS get deleted. Dir names are sortable
# timestamps, so lexical sort == chronological.
prune_old_runs() {
  local total prune d
  total="$(ls -1 "$LOG_ROOT" 2>/dev/null | wc -l | tr -d ' ')"
  if [ "$total" -le "$KEEP_RUNS" ]; then
    return 0
  fi
  prune=$((total - KEEP_RUNS))
  ls -1 "$LOG_ROOT" | sort | head -n "$prune" | while read -r d; do
    rm -rf "${LOG_ROOT:?}/$d"
  done
}

# One line describing the log's final line: with --test-threads=1, an
# in-flight test's start line ("test name ... ", no result, no newline) is
# always the last thing in the log, so that line IS the hung test.
describe_last_line() {
  local last="$1"
  case "$last" in
    test*"... ")
      printf 'likely hung test (started, never finished): %s\n' "$last"
      ;;
    *)
      printf 'no test was mid-run per run.log; last log line: %s\n' "$last"
      ;;
  esac
}

# Capture everything a post-mortem needs while the processes are still
# alive: hung test name, process tree, and a `sample` backtrace per live
# test/daemon process. Called BEFORE kill_tree on the timeout path.
capture_hang_diagnostics() {
  echo "test.sh: TIMEOUT after ${TIMEOUT_SECS}s; capturing diagnostics into ${RUN_DIR} before killing" >&2

  local pids p cmd name last
  pids="$(collect_descendants "$run_pid")
$run_pid"
  last="$(tail -n 1 "$RUN_LOG" 2>/dev/null || true)"

  {
    echo ""
    echo "=== test.sh: TIMEOUT after ${TIMEOUT_SECS}s at $(timestamp) ==="
    describe_last_line "$last"
    echo ""
    echo "live processes at timeout (pid ppid etime command):"
    for p in $pids; do
      ps -o pid=,ppid=,etime=,command= -p "$p" 2>/dev/null || true
    done
  } >>"$RUN_LOG" 2>/dev/null || true

  if command -v sample >/dev/null 2>&1; then
    for p in $pids; do
      kill -0 "$p" 2>/dev/null || continue
      cmd="$(ps -o comm= -p "$p" 2>/dev/null || true)"
      name="$(basename "${cmd:-unknown}")"
      case "$name" in
        cargo | bash | sh | zsh | tee) continue ;;
      esac
      echo "test.sh: sampling pid ${p} (${name})" >&2
      sample "$p" 2 -file "$RUN_DIR/sample-${p}-${name}.txt" >/dev/null 2>&1 || true
    done
  else
    echo "(no 'sample' command on this host; skipped backtrace capture)" >>"$RUN_LOG" 2>/dev/null || true
  fi

  echo "test.sh: diagnostics captured in ${RUN_DIR}" >&2
}

if [ "${1:-}" = "--unlock" ] || [ "${1:-}" = "--force-unlock" ]; then
  force_unlock
fi

acquire_lock
trap release_lock EXIT

RUN_DIR="$LOG_ROOT/$(date +%Y%m%d-%H%M%S)-$$"
RUN_LOG="$RUN_DIR/run.log"
mkdir -p "$RUN_DIR"
prune_old_runs

run_pid=""

on_interrupt() {
  local sig="$1"
  echo "test.sh: received ${sig}; stopping the test run..." >&2
  if [ -n "$run_pid" ]; then
    local last
    last="$(tail -n 1 "$RUN_LOG" 2>/dev/null || true)"
    {
      echo ""
      echo "=== test.sh: killed by SIG${sig} at $(timestamp) ==="
      describe_last_line "$last"
    } >>"$RUN_LOG" 2>/dev/null || true
    kill_tree "$run_pid"
  fi
  exit 130
}

trap 'on_interrupt INT' INT
trap 'on_interrupt TERM' TERM

args=("$@")
if [ "$#" -eq 0 ]; then
  args=(--workspace -- --test-threads=1)
fi

echo "test.sh: cargo test ${args[*]}" >&2
echo "test.sh: logging to ${RUN_LOG}" >&2

{
  echo "=== test.sh run started at $(timestamp) ==="
  echo "command: cargo test ${args[*]}"
  echo "timeout: ${TIMEOUT_SECS}s"
  echo ""
} >>"$RUN_LOG"

cargo test "${args[@]}" > >(tee -a "$RUN_LOG") 2>&1 &
run_pid=$!
printf 'wrapper pid: %s\ncargo pid: %s\n' "$$" "$run_pid" >"$RUN_DIR/pids"

start_ts="$(date +%s)"
timed_out=0
while kill -0 "$run_pid" 2>/dev/null; do
  if [ $(($(date +%s) - start_ts)) -ge "$TIMEOUT_SECS" ]; then
    capture_hang_diagnostics
    timed_out=1
    kill_tree "$run_pid"
    break
  fi
  sleep 2
done

status=0
wait "$run_pid" || status=$?

if [ "$timed_out" -eq 1 ]; then
  echo "=== test.sh: process tree killed after timeout; exiting 124 ===" >>"$RUN_LOG" 2>/dev/null || true
  echo "test.sh: run timed out after ${TIMEOUT_SECS}s; diagnostics in ${RUN_DIR}" >&2
  exit 124
fi

{
  echo ""
  echo "=== test.sh: finished at $(timestamp) with exit status ${status} ==="
} >>"$RUN_LOG" 2>/dev/null || true

exit "$status"
