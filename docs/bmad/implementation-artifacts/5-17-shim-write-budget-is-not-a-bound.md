# Story 5.17: The shim's socket budgets bound each wait, not the round-trip, so a slow-draining daemon stalls Claude

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As the bowerbird maintainer,
I want the shim's ingest round-trip to be bounded in **total** elapsed time, not merely per socket wait,
so that a slow-draining or starved daemon cannot stall Claude Code for hundreds of milliseconds inside a hook while the shim reports success.

**Origin.** Found by the pass-1 code review of Story 5.16, attempted there, and **backed out** after the pass-2 review measured the attempt and showed it did not work. Story 5.16 ships the diagnosability improvement it was scoped for; this story owns the behavior. Maintainer decision (pickles, 2026-07-29): file separately rather than retry inside a hotfix, on the grounds that the bug already defeated one careful attempt and 5.16's own scope boundary reserves behavior work for its own story.

**This is a real Axiom 3 violation, not a theoretical one.** The shim runs inside Claude's process tree, where the performance contract is *hard*. Today a large hook payload plus a slow-draining daemon blocks Claude for as long as it takes, and returns `Ok`.

## Measurements (already done, do not re-derive)

`SO_SNDTIMEO` / `SO_RCVTIMEO` bound how long the kernel will **wait** for buffer space or data. They do not bound a syscall that keeps making progress, and they do not bound a loop of syscalls. macOS's `sosend` re-waits per buffer refill, so a single `write(2)` continues as long as the peer drains anything.

1 MiB payload (the real `MAX_STDIN_BYTES` cap), socket armed at 2ms, peer draining 8 KiB per interval, measured on macOS arm64 against `net.local.stream.sendspace: 8192`:

| peer drain interval | result | elapsed | `write(2)` calls |
| --- | --- | --- | --- |
| flat out | `Ok` | 1.0ms | 1 |
| 200µs | **`Ok`** | **40ms** | 1 |
| 500µs | **`Ok`** | **97ms** | 1 |
| 1000µs | **`Ok`** | **189ms** | 1 |
| 1400µs | `Err(Timeout)` | 48ms | 1 |
| 2000µs | `Err(Timeout)` | 2.5ms | 1 |

Note the shape: the *worst* cases return **success**. A peer slow enough to trip the timeout is the lucky case; a peer that dribbles just fast enough keeps the syscall alive for hundreds of milliseconds. The read side has the same shape (`read_line` loops `fill_buf` until `\n`), measured at 12ms and 24ms against its 3ms value with a dribbling peer.

**A verified candidate fix exists.** Capping each `write(2)` at the send-buffer size (`buf.len().min(8192)`) so the syscall cannot loop in the kernel, then checking a deadline between chunks, measured **2.01ms to 2.50ms at every drain rate above**, with a 400-byte hook payload still completing in one syscall (7.6µs vs 8.1µs, no regression). Treat this as a starting point, not a settled design: it was measured in a harness, not in the shim.

**Why Story 5.16's attempt failed, so it is not repeated.** It re-armed the socket with the remaining budget after a partial write, which is correct in principle but unreachable in practice: the first match arm returned `Ok(())` whenever the syscall wrote everything, and as the table shows a single syscall *does* write everything even while taking 189ms. Measured on the shipped code: **1 syscall, 0 re-arms.** The deadline was never consulted. Its test missed this because the test's peer drained *slower* than the budget, so the write always failed on its first wait and the re-arm path never executed.

## Acceptance Criteria

1. **Given** the shim's ingest round-trip **When** the daemon drains the socket slowly at any rate **Then** the total time the shim blocks is bounded by a documented budget, and the bound is verified by measurement across a range of drain rates rather than at a single one.

2. **Given** a normal hook payload (a few hundred bytes, comfortably inside the 8 KiB send buffer) **When** the daemon is healthy **Then** the success path still completes in **one** `write(2)` with no added syscalls, and `shim/benches/hot_path.rs` shows no p99 regression beyond the CI gate. The success path is the path that must stay invisible.

3. **Given** the bound is exceeded **When** the shim gives up **Then** the resulting `Error::Timeout` reports a figure consistent with the time actually spent, because Story 5.16's `budget_ms` is the *configured per-wait* value and currently under-reports elapsed time by up to 100x (it logs "timed out after 2ms" after 48ms).

4. **Given** a write that stops mid-payload **When** the daemon parses the truncated line **Then** the documented consequence matches reality: the daemon logs `ingest: invalid JSON` and replies `400`, and the event is lost **except** when only the trailing `\n` was unsent, in which case the event IS recorded. Story 5.16 documented this boundary case; keep it accurate if the write path changes.

5. **Given** the read half has the same unbounded shape **When** this story lands **Then** either the read is bounded too, or the code states plainly that it is not and why that is acceptable. No silent asymmetry.

6. **Given** any test claiming to verify a bound **When** it runs **Then** it must exercise the mechanism it guards. Two previous tests went green while the stall was live: one asserted a sum of constants, the other used a peer slower than the budget so the loop never iterated. A valid test drives a peer that drains **faster** than the budget but still slowly, and asserts on both elapsed time and the number of syscalls or re-arms.

## Tasks / Subtasks

- [ ] **Task 1: Reproduce the measurements above in-tree (AC: 1)**
  - [ ] Port the harness shape into a test or bench so the numbers are reproducible from the repo rather than from a scratchpad. The Story 5.16 pass-2 review noted, correctly, that its measurement basis was uncommitted prose.
  - [ ] Confirm the drain-rate sweep on Linux as well as macOS. The `sosend` re-wait behavior was measured on macOS; Linux's `unix_stream_sendmsg` may differ, and the fix must not assume one platform.

- [ ] **Task 2: Bound the write (AC: 1, 2, 3)**
  - [ ] Implement a bound. The chunk-at-send-buffer approach is measured and is the suggested starting point; a deadline-aware loop alone is proven insufficient.
  - [ ] Verify the success path is still one syscall for a normal payload, by counting syscalls, not by reasoning about them.
  - [ ] Make the reported figure consistent with elapsed time (AC #3), or state why the configured value is the more useful thing to log.

- [ ] **Task 3: Decide the read half (AC: 5)**
  - [ ] Bound it, or document that it is unbounded and why that is tolerable (the daemon's reply is 4 bytes written in one call, so reachability is low). No silent asymmetry.

- [ ] **Task 4: Tests that cannot go green while the stall is live (AC: 6)**
  - [ ] Drive a peer that drains faster than the budget but slowly. Assert elapsed time AND syscall/re-arm count.
  - [ ] Verify the test fails against the current unbounded code before keeping it. Story 5.16 did this for its (wrong) fix and it was still the right instinct.
  - [ ] Respect the parallel-test discipline: no sleeps for synchronization, hang guards at 30s. Note the tension with AC #1, which needs wall-clock assertions; keep those as generous bound checks with a wide margin, not tight latency assertions, and say so in a comment.

- [ ] **Task 5: Reconcile every claim site (AC: 1, 5)**
  - [ ] `crates/shim/src/socket.rs` currently states the honest unbounded shape in two places and points here. Update both when the bound becomes real.
  - [ ] `crates/shim/src/error.rs`'s `Error::Timeout` doc says `budget_ms` is a per-wait figure that under-reports elapsed time. Update if AC #3 changes that.
  - [ ] `docs/bmad/project-context.md` §Performance bars describes the shim budget; check whether it needs the aggregate-versus-per-wait distinction.

## Dev Notes

### Do not "fix" this by shrinking the payload cap

Capping `MAX_STDIN_BYTES` below the send buffer would make the budget hold by construction, but it drops large `PostToolUse` events, which is a product decision (event completeness) masquerading as a perf fix. If that is the chosen path it needs its own justification, not a side effect.

### Dropping the event remains legitimate

Per Axiom 3 and Story 5.16's conclusion, giving up on an event to protect the budget is a correct outcome, not a bug to retry away. Do not add a retry: Story 5.16 established that the daemon `try_send`s before it replies and `events` has no idempotency key, so a retry can duplicate events in the append-only log.

### Interaction with the daemon-starvation work

taskwarrior `719e7027` covers insulating daemon ingest from OS scheduler starvation. That reduces how often the daemon drains slowly; it does not bound the shim, which must hold regardless of daemon health. Independent fixes, related symptom.

### Test execution

`scripts/test.sh`, never raw `cargo test` (project rule, `CLAUDE.md`). Shim-scoped: `scripts/test.sh -p bowerbird-shim`. The `hot_path` bench is only meaningful on an idle machine and never immediately after a test run; A/B it against a stash rather than trusting a single delta. Two readings in Story 5.16 misled on machine state alone.

### References

- [Source: crates/shim/src/socket.rs] `WRITE_BUDGET_MS` doc carries the measurement table and the per-wait-versus-aggregate explanation; `send` states the honest bound shape.
- [Source: crates/shim/src/error.rs] `TimeoutOp` and `Error::Timeout`, including the one-byte truncation boundary case.
- [Source: docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md] Pass-1 review finding (HIGH, `socket.rs:72`), the backed-out attempt, and the pass-2 review that measured it. Completion Note 9 records the backout.
- [Source: docs/bmad/project-context.md] Axiom 3, shim hot-path discipline, deterministic test discipline.

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

## Change Log

- 2026-07-29: Story created as the Story 5.16 pass-2 escalation. Pass 1 found that `socket.rs`'s "write + read <= 5ms" claim was false; the fix attempted in 5.16 was measured by pass 2 and did not work (1 syscall, 0 re-arms, 189ms returning `Ok`), so it was backed out and the behavior filed here with the measurements attached. Story 5.16 keeps the diagnosability work it was scoped for. Deliberately carries the full measurement table, the reason the first attempt failed, and a verified candidate fix, so this does not start from scratch. Status -> ready-for-dev.
