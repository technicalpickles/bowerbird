# Story 5.17: The shim's socket budgets bound each wait, not the round-trip, so a slow-draining daemon stalls Claude

Status: done

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

**Confirmed on Linux too, so this is not a macOS quirk** (2026-07-30, `rust:slim`/glibc container). An expired `SO_SNDTIMEO` armed at 2ms returned after **6.80ms** having written **255,360 bytes in a single `write(2)`** (macOS managed 8,192 in the comparable run). The read side matched macOS exactly: `WouldBlock` / `raw_os_error == Some(11)` (`EAGAIN`), so the classification in Story 5.16 holds on both platforms.

That settles Task 1's open question about whether Linux's `unix_stream_sendmsg` behaves like macOS's `sosend`: **it does, and if anything it is worse**, since it will push ~31x more data inside one syscall before the timeout is consulted. Any fix must therefore be cross-platform rather than a macOS special case, and a chunk size tuned to macOS's 8 KiB send buffer should not be assumed correct for Linux without re-measuring.

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

- [x] **Task 1: Reproduce the measurements above in-tree (AC: 1)**
  - [x] Port the harness shape into a test or bench so the numbers are reproducible from the repo rather than from a scratchpad. The Story 5.16 pass-2 review noted, correctly, that its measurement basis was uncommitted prose.
  - [x] Confirm the drain-rate sweep on Linux as well as macOS. **Partly answered already:** Linux was measured (2026-07-30) and behaves the same way, returning after 6.80ms against a 2ms budget with 255,360 bytes written in one `write(2)`, so `unix_stream_sendmsg` re-waits like `sosend` and is ~31x more permissive per syscall. What remains is the full drain-rate sweep on Linux, not the existence of the bug. Do not assume a macOS-tuned chunk size transfers.

- [x] **Task 2: Bound the write (AC: 1, 2, 3)**
  - [x] Implement a bound. The chunk-at-send-buffer approach is measured and is the suggested starting point; a deadline-aware loop alone is proven insufficient.
  - [x] Verify the success path is still one syscall for a normal payload, by counting syscalls, not by reasoning about them.
  - [x] Make the reported figure consistent with elapsed time (AC #3), or state why the configured value is the more useful thing to log.

- [x] **Task 3: Decide the read half (AC: 5)**
  - [x] Bound it, or document that it is unbounded and why that is tolerable (the daemon's reply is 4 bytes written in one call, so reachability is low). No silent asymmetry.

- [x] **Task 4: Tests that cannot go green while the stall is live (AC: 6)**
  - [x] Drive a peer that drains faster than the budget but slowly. Assert elapsed time AND syscall/re-arm count.
  - [x] Verify the test fails against the current unbounded code before keeping it. Story 5.16 did this for its (wrong) fix and it was still the right instinct.
  - [x] Respect the parallel-test discipline: no sleeps for synchronization, hang guards at 30s. Note the tension with AC #1, which needs wall-clock assertions; keep those as generous bound checks with a wide margin, not tight latency assertions, and say so in a comment.

- [x] **Task 5: Reconcile every claim site (AC: 1, 5)**
  - [x] `crates/shim/src/socket.rs` currently states the honest unbounded shape in two places and points here. Update both when the bound becomes real.
  - [x] `crates/shim/src/error.rs`'s `Error::Timeout` doc says `budget_ms` is a per-wait figure that under-reports elapsed time. Update if AC #3 changes that.
  - [x] `docs/bmad/project-context.md` §Performance bars describes the shim budget; check whether it needs the aggregate-versus-per-wait distinction.

### Review Findings

Three-layer adversarial review (Blind Hunter / Edge Case Hunter / Acceptance Auditor, all on Opus per maintainer instruction), 2026-07-31. 30 raw findings, deduplicated to 1 decision-needed + 16 patch + 0 defer; 5 dismissed as noise after verification.

- [x] [Review][Decision] Multi-chunk payload delivery profile under the 2ms budget is an emergent product cliff nobody signed off on. Payloads over 8 KiB now need the daemon to drain nearly everything inside 2ms: healthy-idle delivers 1 MiB in ~1ms, but under load (5.16 measured daemon wakeup tails of 199µs p50 / 2.4ms p99) mid-size payloads shift from delivered-late to dropped at exit-0, and the wall-clock deadline also fires when the SHIM (not the daemon) is descheduled between chunks. No test or measurement exists between 8193 bytes and 1 MiB, and the loss-surface disclosure says "starved daemon" only. Options: (1) accept + measure the cliff + document it as the Axiom 3 trade (recommended); (2) widen the write budget for multi-chunk payloads (weakens the hard bound); (3) shrink MAX_STDIN_BYTES (needs its own product justification per Dev Notes); (4) split the behavior question into its own story with measurements attached.

- [x] [Review][Patch] SEND_CHUNK_BYTES doc overclaims "at most one buffer-refill wait" and the sweep never varies drain quantum; add a small-quantum sweep row, measure the trailing-chunk residual, correct the doc [crates/shim/src/socket.rs:53]
- [x] [Review][Patch] Claim sites contradict each other: send() says "one trailing chunk" then justifies no-re-arm with "one trailing wait"; project-context.md states "one trailing wait" for both halves, understating the write residual [crates/shim/src/socket.rs:268, docs/bmad/project-context.md]
- [x] [Review][Patch] The shipped bound has no in-tree elapsed measurement and the "2.01-2.50ms" figures are misattributed to "the Story 5.17 harness" (they are the 5.16 pass-2 review scratchpad numbers the story said not to transfer); extend the manual repro to measure write_bounded rows and fix the attribution [crates/shim/src/socket.rs:26]
- [x] [Review][Patch] read_bounded doc claims "later bytes are left unread" but bytes past the newline in the same read(2) are consumed and included in the returned slice [crates/shim/src/socket.rs:293]
- [x] [Review][Patch] read_eof_before_newline test runs the production 3ms budget across two required syscalls with a deadline check between them; a starved runner flakes it (same trap already fixed in two sibling tests) [crates/shim/src/socket.rs:673]
- [x] [Review][Patch] elapsed_ms >= budget assertions can trip via the classify path if the kernel timer expires a hair early plus as_millis floor-truncation; relax by 1ms with a comment [crates/shim/src/socket.rs:472, 625]
- [x] [Review][Patch] Buffer-full BadResponse carries the raw 4096-byte reply into a single log line; truncate to a short prefix (the reviewer's embedded-newline claim is false, the arm is newline-free by construction, but the 4 KiB line is real) [crates/shim/src/socket.rs:199]
- [x] [Review][Patch] read_bounded full-buffer guard uses == and panics on a Read impl that over-reports n; use >= [crates/shim/src/socket.rs:197]
- [x] [Review][Patch] Empty reply (daemon accepts then closes) logs bare "unexpected daemon response: " with no information; give the empty case explicit wording [crates/shim/src/socket.rs:224]
- [x] [Review][Patch] spawn_draining_peer treats EINTR as EOF, killing the drainer and flaking chunk_boundary_arithmetic under signals; continue on Interrupted [crates/shim/src/socket.rs:397]
- [x] [Review][Patch] The sweep cannot detect a dead drainer (it would degenerate into the 5.16 slower-than-budget shape and still pass); assert drained bytes > 0 [crates/shim/src/socket.rs:463]
- [x] [Review][Patch] Sweep and boundary tests silently assume the send buffer is smaller than the payload (Linux wmem_default is tunable); document the assumption where the tests rely on it [crates/shim/src/socket.rs:431]
- [x] [Review][Patch] Read-timeout test's calls >= 1 assertion is vacuous and Completion Note 3 over-claims it as "a syscall count"; reword the note and comment why the count cannot be tight (a starved runner legitimately produces 1) [crates/shim/src/socket.rs:775]
- [x] [Review][Patch] deferred-work strike hygiene: pass-2 item 1 was never struck through and item 2's half-strike leaves the stale "residual questions moved to 5.17" sentence reading as live [docs/bmad/implementation-artifacts/deferred-work.md]
- [x] [Review][Patch] Completion Note 2's "ubuntu CI re-runs it on every PR" reads as the ignored manual-repro test (CI skips ignored tests); Note 4 and sprint-status should also state the authoritative CI gate has not yet run (work is uncommitted, pre-PR) [story file]
- [x] [Review][Patch] sprint-status comment-trail ordering: the new 5.17 entry was inserted above the demoted 5.18 comment, breaking the append-below convention [docs/bmad/implementation-artifacts/sprint-status.yaml:129]

Dismissed after verification (5): tautological elapsed<=wall assertion (harmless tripwire, monotone by construction); fractional-budget as_millis truncation (unreachable at 2/3ms); EINTR byte-accounting assumption in write_bounded (identical platform contract std::write_all relies on); manual repro not CI-gated (by design and documented; the record wording is covered by a patch above); deadline-before-buffer-full ordering (budget legitimately governs, reordering only changes an exact tie).

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

claude-fable-5 (Fable 5)

### Implementation Plan

Chunk-cap plus deadline, per the story's verified candidate: `write_bounded` hands each `write(2)` at most `SEND_CHUNK_BYTES` (8192, macOS `net.local.stream.sendspace`, the smallest send buffer in play) so the kernel's sosend/sendmsg re-wait loop has nothing to loop over, and checks a wall-clock deadline (`WRITE_BUDGET_MS`, 2ms total) before every syscall. The read half gets the symmetric shape (`read_bounded`, 3ms deadline between `read(2)` calls into a 4 KiB stack buffer) rather than documentation-only, with one deliberate divergence from the deferred-work sketch: no `SO_RCVTIMEO` re-arming, because a `read(2)` returns on *any* available data so the armed per-wait value already bounds each syscall, and re-arming would add `setsockopt` calls for no tighter bound. Both functions are generic over `Write`/`Read` so tests count syscalls through a counting adapter on a real `UnixStream` instead of reasoning about them. Worst case for each half is its budget plus one trailing syscall; neither budget value changed.

### Debug Log References

- `target/test-logs/20260731-213753-85850`: first green shim run with the bound in place (22 unit + 1 ignored, 23 contract).
- `target/test-logs/20260731-213820-86782`: the Task 4 verification run: both mechanism tests failing against the temporarily-restored unbounded code (write sweep: `Ok` completing 1 MiB; read dribble: late `BadResponse` after 4.92s instead of a ~3ms timeout).
- `target/test-logs/20260731-214530-98481`: final full-workspace regression run: 647 passed / 0 failed / 1 ignored.
- Local bench A/B readings in Completion Note 4 (summaries in `target/shim-bench-summary.json` per run, not retained across runs).

### Completion Notes List

1. **The write bound is real this time, and the mechanism test proves it the way the two retired tests did not.** `bounded_write_gives_up_within_budget_across_drain_rates` drives a 1 MiB payload against peers draining 8 KiB every 200/500/1000µs, the exact rates where the shipped code returned `Ok` after 40/97/189ms, and asserts `Err(Timeout)` (at these rates the unbounded code's failure mode is *success*, so `is_err` IS the mechanism assertion), the AC #3 elapsed consistency (`elapsed_ms` within 1ms of the budget or above, and at most measured wall time; the 1ms slack covers kernel-timer/truncation artifacts on the classify path, a review fix), and the syscall count (an abandoned chunk loop: `1 <= calls < 128`). Post-review the sweep also varies the drain QUANTUM (a 1024-byte row), asserts the drainer actually drained (`drained > 0`, so a dead peer thread cannot silently degenerate the sweep into the useless slower-than-budget shape), and the in-tree bounded measurement records the shipped function at 2.21/2.28/3.03ms for the three classic rates. Verified per Task 4 by swapping `write_bounded`'s body back to `write_all`: every rate completed 1 MiB and returned `Ok`, tripping the assertion (log `20260731-213820-86782`). Passed a 10/10 repeat loop after the swap-back.

2. **Task 1: the measurement table is now reproducible from the repo on both platforms.** The `#[ignore]`d `manual_repro_unbounded_write_all_measurement_table` runs the story's harness shape against std's `write_all` (which remains available forever, so the repro cannot rot out from under the fix). This machine, macOS arm64: 38.77ms / 96.40ms / 191.02ms at 200/500/1000µs, each `Ok` from ONE `write(2)`, matching the story's 40/97/189ms within noise. The 1400/2000µs rows returned slow `Ok`s through `write_all`'s compounding loop (8 and 115 syscalls) rather than the story's `Err` rows; scheduling-sensitive, as the test doc warns, and not load-bearing. Linux (`rust:1.94-bookworm` container, glibc): 200/500µs drain gave `Ok` after ~99ms with 11 syscalls (~95 KB accepted per 2ms-armed syscall, the story's "~31x more permissive" behavior), 1000µs and slower gave `Err` after ~6ms, 3x past the arm. The full `socket::tests` module also passes in that container, so the sweep is confirmed on Linux, not assumed. To be precise about what CI re-runs (review correction): the non-ignored drain-rate sweep test runs on ubuntu CI every PR; the two `#[ignore]`d measurement-table tests never run in CI by design and are manual-only.

3. **The read half is bounded, not documented around (AC #5).** Same deadline shape at 3ms; `bounded_read_gives_up_on_a_dribbling_peer` drives the byte-per-800µs peer that kept `read_line` alive for 12-24ms and asserts a ~3ms `Timeout` with elapsed consistency. Review correction on the count claim: a tight syscall-count assertion is impossible there, because a starved runner legitimately produces exactly one read (a single `read(2)` parked until `SO_RCVTIMEO` expires), so `calls >= 1` is a ran-at-all tripwire and the mechanism assertion is the `Timeout` match itself (the unbounded shape produces a late `BadResponse` instead). Verified to fail against the unbounded shape in the same Task 4 run (deadline disabled: the dribble fed the loop for ~4.9s until buffer-full surfaced a late `BadResponse`). Replacing `BufReader`/`String` with a stack buffer also removed the success path's only two heap allocations, which the shim discipline wanted anyway.

4. **AC #2, success path: syscalls counted, and the bench A/B'd with full disclosure.** `success_path_is_one_syscall` (400 bytes → exactly 1 `write(2)`, payload delivered intact), `read_success_is_one_syscall` (buffered `200\n` → exactly 1 `read(2)`), and `chunk_boundary_arithmetic` (8192 → 1 call, 8193 → 2 calls) count real syscalls through the adapter. Hot-path bench A/B against a stash, two pairs: the first HEAD reading was polluted (p99 70.86ms with p50 1.27ms, a machine hiccup mid-run, the third time this bench has misled on machine state, consistent with the recorded 5.16 lesson) and was discarded; the clean pair reads with-change p99 1.536ms vs HEAD 1.410ms with p50s 1.253 vs 1.239ms, inside the documented ±12% same-build spread and at ~20% of the committed 7.725ms macOS baseline. The authoritative AC #2 verdict is the CI per-platform gate (best-of-2 wrapper, Story 5.18); as of this record it has NOT yet run for this change (work is pre-PR), so AC #2 rests on the syscall counts plus the neutral local A/B until the PR's gate goes green.

5. **AC #3: the timeout now reports what actually happened.** `Error::Timeout` gained `elapsed_ms` (measured wall time) alongside `budget_ms` (the configured aggregate budget): `socket write timed out after 48ms (budget 2ms); event not sent`. Both figures are logged because they legitimately differ by up to one trailing syscall; the message-shape tests pin that the elapsed figure is the "after" number. The configured-value-only wording that under-reported 48ms as "after 2ms" is gone.

6. **AC #4 holds without changes.** Chunking does not alter the truncation consequence: a write that gives up mid-payload still leaves the daemon parsing a truncated line (`ingest: invalid JSON`, reply `400`), and the one-byte boundary case (only the trailing `\n` unsent → event IS recorded) is unchanged. `TimeoutOp::Write`'s doc needed no edit; verified against the new write path rather than assumed.

7. **One deliberate behavior change on an unreachable path, disclosed:** a non-UTF-8 daemon reply now reports as `BadResponse` (lossily rendered) instead of `SocketIo(InvalidData)`. The old classification was an implementation artifact of `read_line`; the bytes arrived fine, the daemon spoke garbage, and both variants sit in the same exit-0/WARN class, so the Story 5.10 partition and all three canaries are untouched. `parse_reply_covers_the_wire_grammar` pins the full reply grammar including this case, CRLF tolerance, EOF-without-newline parity with `read_line`, and the first-line-only framing.

8. **Drop-not-retry preserved, and the loss surface is now measured and stated in full (review correction).** No retry was added anywhere; a payload the daemon drains too slowly is now dropped at ~2ms instead of delivered after a 40-189ms stall inside Claude's hook. The widened loss surface is for multi-chunk payloads (over 8 KiB) and its trigger is wall-clock, so it includes a descheduled SHIM as well as a slow daemon, not "a starved daemon" only as this note originally said. Measured (in-tree bounded table, macOS arm64): healthy-idle delivers 16 KiB / 100 KiB / 1 MiB in 30µs / 117µs / 909µs; a drain paced at 8 KiB per 200µs (the 5.16 p50 daemon-wakeup figure) drops 100 KiB at 2.21ms. Maintainer decision at review: accept as the Axiom 3 trade, measured and documented (socket.rs WRITE_BUDGET_MS doc, project-context.md Performance bars, deferred-work item 2).

9. **Test discipline respected, with the AC #1 tension resolved as the story asked.** No synchronization sleeps: the paced drains are semantic timings (the mechanism under test), peer threads carry 30s hang guards and are joined, and every wall-clock ceiling is a wide-margin hang guard (`GENEROUS_CEILING`, 5s vs the 2.17-3.03ms in-tree measured values) with the comment saying exactly that; the regression-catching assertions are Err-versus-Ok and syscall counts, which starvation cannot flip. Tests that pin counts rather than timing arm their sockets generously so a starved runner cannot flake them (two such traps were caught and fixed during implementation). 10/10 green on the repeat loop; the two timing-sensitive tests take ~10ms combined.

10. **Verification.** `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings`, full `scripts/test.sh`: 647 passed / 0 failed / 1 ignored (638 baseline + 9 new: 8 in `socket.rs`, 1 in `error.rs`; the ignored one is the manual measurement). Linux: `socket::tests` green in a glibc container. Emdash sweep of the diff clean. No protocol/src change, so the changelog gate is correctly untriggered; no daemon change, no wire change, no migration. (Superseded counts after the review pass: see Note 11.)

11. **Code review pass (2026-07-31, three Opus layers) resolved same-session: 1 decision + 16 patches applied, 5 dismissed, 0 deferred.** The mechanism survived; the record and the residual claims did not, and the biggest correction is honesty about the write's trailing chunk: the deadline cannot interrupt a syscall, so the residual is one chunk's DRAIN TIME (quantum-dependent), not "one wait". Measured in-tree on the shipped `write_bounded` (new `#[ignore]`d bounded table): classic rates err at 2.21/2.28/3.03ms; a 256-byte/500µs drain quantum stretches the trailing chunk to 23.37ms; every claim site now states "budget + one trailing chunk's drain time" (socket.rs x3, project-context.md). The sweep gained a sub-chunk-quantum row and a `drained > 0` assertion so a dead drainer cannot silently turn it into the 5.16 useless shape. MAINTAINER DECISION (pickles): the multi-chunk payload delivery profile is accepted as the Axiom 3 trade, measured not asserted: healthy-idle takes the full 1 MiB cap in 909µs; a busy-machine drain pace (8 KiB/200µs, the 5.16 p50 wakeup) drops 100 KiB at 2.21ms; the trigger includes a descheduled shim. Also fixed: elapsed assertions gained 1ms slack for the kernel-timer/truncation artifact on the classify path; `read_eof` test moved off the production 3ms budget (two-syscall starvation flake, third instance of the same trap); overflow `BadResponse` truncates to a 64-byte prefix instead of shipping 4 KiB into one log line; `read_bounded` hardened against over-reporting `Read` impls and its doc no longer claims bytes past the newline are left unread; empty daemon reply now logs a named cause instead of a bare colon; drainer retries EINTR and reads before honoring stop; deferred-work strikes fixed; Notes 2/3/4/8 corrected where they over-claimed (CI does not run ignored tests; the read count assertion is a tripwire, not a count; the CI bench gate has not yet run pre-PR; the loss trigger is not only a starved daemon). Dismissed after verification: the reviewer claim that the overflow arm breaks one-line log framing is FALSE (that arm is newline-free by construction), the elapsed<=wall assertion being tautological is accepted as documentation, fractional-budget truncation is unreachable at 2/3ms, the EINTR byte-accounting premise is the same contract std's write_all relies on, and the deadline-before-buffer-full ordering is correct (the budget governs). Full suite after all fixes: 647 passed / 0 failed / 2 ignored (the review pass added only ignored measurement rows, no new counted tests); sweep + read mechanism tests re-verified 10/10; Linux container re-run green.

### File List

- crates/shim/src/socket.rs (modified: bounded write/read, reply parsing, tests)
- crates/shim/src/error.rs (modified: `Error::Timeout` gains `elapsed_ms`, doc + tests)
- docs/bmad/project-context.md (modified: §Performance bars aggregate-bound paragraph)
- docs/bmad/implementation-artifacts/deferred-work.md (modified: two entries marked resolved)
- docs/bmad/implementation-artifacts/sprint-status.yaml (modified: status tracking)
- docs/bmad/implementation-artifacts/5-17-shim-write-budget-is-not-a-bound.md (modified: this file)

## Change Log

- 2026-07-29: Story created as the Story 5.16 pass-2 escalation. Pass 1 found that `socket.rs`'s "write + read <= 5ms" claim was false; the fix attempted in 5.16 was measured by pass 2 and did not work (1 syscall, 0 re-arms, 189ms returning `Ok`), so it was backed out and the behavior filed here with the measurements attached. Story 5.16 keeps the diagnosability work it was scoped for. Deliberately carries the full measurement table, the reason the first attempt failed, and a verified candidate fix, so this does not start from scratch. Status -> ready-for-dev.
- 2026-07-31 (review): three-layer Opus code review returned 1 decision-needed + 16 patch + 5 dismissed + 0 deferred; all resolved same-session. Decision (pickles): multi-chunk payloads are best-effort under the 2ms budget, accepted as the measured Axiom 3 trade. Headline corrections: the write residual is one trailing chunk's drain time (measured 23.37ms at a pathological 256-byte quantum), not "one wait"; the shipped bound is now measured in-tree (2.21-3.03ms classic rates) instead of borrowing the 5.16 harness figures; sweep gained a sub-chunk-quantum row + dead-drainer detection; three test flake risks fixed; record over-claims corrected. Note 11 has the full accounting. Status -> done.
- 2026-07-31: dev-story complete, all 5 tasks and all 6 ACs. Shipped the verified candidate fix: each `write(2)` capped at the 8 KiB send buffer plus a 2ms aggregate deadline (`write_bounded`); read half symmetrically bounded at 3ms (`read_bounded`, stack buffer, no re-arm on measured grounds); `Error::Timeout` reports measured `elapsed_ms` beside the configured budget. Mechanism tests drive drain rates where the unbounded code returns *success* and were verified to fail against it before keeping (Task 4 swap runs logged); the measurement table is reproducible in-tree via an `#[ignore]`d harness test, confirmed on macOS and Linux (container + CI). 647 tests green, clippy/fmt clean, bench A/B neutral within same-build spread (one polluted reading discarded and disclosed). Claim sites reconciled: socket.rs, error.rs, project-context.md §Performance bars, deferred-work items 5.16-p1#2 and 5.16-p2#1 marked resolved. Status -> review.
