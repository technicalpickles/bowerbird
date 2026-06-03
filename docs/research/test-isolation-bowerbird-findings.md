# Bowerbird test-isolation findings (concrete reproduction)

Companion to `test-isolation-parallelism-research-prompt.md` /
`-results.md` (which are general, codebase-agnostic best-practices). This file
records the **concrete** flakiness observed in `crates/daemon/tests/contract_daemon.rs`
while resolving Story 5.8 code-review pass-4, so a focused investigation has a
ready reproduction instead of re-deriving it.

Captured: 2026-06-03, during Story 5.8 pass-4 (the broadcast-lag snapshot-coverage
fix, `crates/daemon/src/api/ws.rs`).

## Two distinct symptoms

### Symptom A — pre-existing intermittent hang (NOT new)

`story_1_7_rest::status_returns_none_last_event_when_only_sentinels` hangs
intermittently. Measured **~1 in 5** even when run **in complete isolation**:

```
for i in 1 2 3 4 5; do
  RTK_DISABLED=1 timeout 60 cargo test -p bowerbird-daemon --test contract_daemon -- \
    --test-threads=1 --exact story_1_7_rest::status_returns_none_last_event_when_only_sentinels
done
# → 4 pass in 0.00s, 1 times out (hung)
```

This is the "intermittent workspace-test hang documented as known issue" recorded
in `sprint-status.yaml` (2026-05-28, Story 5.3). It is a REST `/status` test — it
does not touch WebSockets, the broadcast hub, or anything Story 5.8 changed. It
hangs the whole `--workspace` run when it fires (libtest `--test-threads=1` blocks
on the stuck test), which is what makes a single clean full-suite run unreliable.
Because it reproduces in isolation, the cause is process-local to that one test
(or its daemon spawn), not cross-test contention.

### Symptom B — an e2e test that passes alone but flakes under `--workspace`

`story_2_4_dropped::lag_invalidates_snapshot_coverage_resubscribe_resnapshots`
(added in 5.8 pass-4) reliably passes when the `contract_daemon` binary runs
**alone**, but reliably **fails** under `cargo test --workspace`:

```
# Binary alone — PASSES (3/3), ~9s:
RTK_DISABLED=1 cargo test -p bowerbird-daemon --test contract_daemon -- \
  --test-threads=1 --skip status_returns_none_last_event_when_only_sentinels
# → 184 passed; 0 failed

# Whole workspace — FAILS (3/3), the daemon binary takes ~14s instead of ~9s:
RTK_DISABLED=1 cargo test --workspace -- \
  --test-threads=1 --skip status_returns_none_last_event_when_only_sentinels
# → lag_invalidates_... FAILED  (183 passed; 1 failed)
```

Key facts established by tracing/experiment:

- The **fix is correct**: file-trace instrumentation of `api/ws.rs` shows that on
  the post-lag re-subscribe the daemon clears `snapshotted_keys`, re-snapshots
  `sess-A`, and calls `socket.send` for it. The failure is purely that the client
  side never observes the frame within the deadline.
- It is **not the runtime flavor.** Both `#[tokio::test(flavor = "current_thread")]`
  and `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` pass when the
  binary runs alone and fail under `--workspace`.
- It is **not the read strategy.** A single blocking `read_text_frame_or_close`, a
  short-timeout poll loop, and an active "drive-the-runtime" loop (republish a live
  `state` probe + drain each iteration, à la `wait_subscribe_live`) all pass alone
  and flake under `--workspace`.
- The differentiator is the `cargo test --workspace` invocation itself. The daemon
  binary's wall time rises (~9s → ~14s) under `--workspace`, consistent with extra
  system load (other crates' test binaries building/running) perturbing the timing
  of WS frame delivery for a connection that just survived a broadcast-lag burst.

The test's job is to observe a snapshot frame the daemon emits after a deliberate
broadcast-lag flood. Forcing the lag leaves the per-connection task busy draining
flood residue; delivering the subsequent re-subscribe snapshot then competes with
that work and with whatever else the box is doing. Under added `--workspace` load
the 5s observation deadline is missed.

## What was tried (and didn't fully resolve B)

- `current_thread` + single read; + short-poll; + active-pump drive loop.
- `multi_thread` (worker_threads = 2) + caught-up gate + single read; + active-pump.
- A `wait_subscribe_live` "caught-up" barrier before re-subscribe (drain flood
  residue / confirm the task is idle) — helps alone, not under `--workspace`.

All pass alone; all flake under `--workspace`.

## Leads for the focused investigation

1. **Confirm the load hypothesis.** Does `--workspace` run test binaries (or their
   builds) concurrently with `contract_daemon`? If so, cap binary-level concurrency
   (nextest, or `cargo test --workspace --jobs 1`) and see whether B disappears —
   that would confirm it is contention, not a logic bug.
2. **nextest test groups** (the `-results.md` recommendation): put the heavyweight
   WS/daemon contract tests in a `max-threads = 1` group and let the rest run
   parallel. Check whether running B in its own serialized group removes the flake.
3. **Decouple the assertion from wall-clock WS delivery.** The fragile part is
   observing a single snapshot frame over a real socket after a lag burst. A less
   timing-sensitive observation (e.g. assert on the per-connection coverage set via
   a test hook, or a deterministic in-process harness around `connection_task`
   rather than a real TCP socket) would make the regression test robust regardless
   of system load. This likely needs a small testability seam in `api/ws.rs`.
4. **Symptom A is probably the higher-value fix** — it hangs CI intermittently and
   is independent of A/B. Worth root-causing the `/status`-only-sentinels path
   (signal-handler registration? a daemon spawn that doesn't always come up?).

## Current state of the 5.8 test

`lag_invalidates_snapshot_coverage_resubscribe_resnapshots` is left on
`current_thread` (matching the file's other 169 tests) with the active-pump
observation and a doc-comment flagging the `--workspace` flake. The 5.8 fix is
verified by: daemon tracing, binary-alone runs (3/3 green), and the without-fix
negative check (fails 3/3). The `--workspace` robustness of this one test is the
open item handed to this investigation.
