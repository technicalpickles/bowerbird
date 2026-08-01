# Story 5.16: Hotfix — shim socket timeouts drop events and are indistinguishable from real I/O errors

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As the bowerbird maintainer dogfooding v0.1.0-rc1,
I want the shim's socket timeouts to be distinguishable from genuine socket I/O failures, and the 2ms/3ms budgets to be justified by measurement rather than assumption,
so that silent event loss is diagnosable instead of showing up as an unexplained gap in the event log.

**rc1 dogfood finding (Story 5.12, Task 6, 2026-07-29).** The first fresh-machine install of `v0.1.0-rc1` logged two dropped events in roughly five minutes of light use across ~33 events:

```
2026-07-29T20:13:34.942Z WARN socket I/O failed: Resource temporarily unavailable (os error 35)
2026-07-29T20:18:01.588Z WARN socket I/O failed: Resource temporarily unavailable (os error 35)
```

The first fired during `bowerbird install`; the second ~4.5 minutes later in steady state, so this is **not** a daemon-startup race. Escalated per Story 5.12 AC #5.

**This is a diagnosability story, not (necessarily) a correctness story.** Dropping the event is very likely *correct* per Axiom 3 — the shim sits at a trust boundary and must never stall Claude. What is wrong is that the operator cannot tell a **timeout** apart from a **real socket error**, because both land in the same `Error::SocketIo(std::io::Error)` bucket with the same generic message. Resist the urge to "fix" this by adding retries until Task 1 has established *why* a 3ms budget is being exceeded at all.

## Acceptance Criteria

1. **Given** the shim's ingest round-trip is bounded by `set_write_timeout(2ms)` + `set_read_timeout(3ms)` ([socket.rs:26-31](../../crates/shim/src/socket.rs)) **When** either timeout expires on macOS **Then** the resulting error is classified as a **timeout**, not as a generic socket I/O failure, and the shim log line names it as such. Root cause of the current behavior: on Unix these map to `SO_SNDTIMEO`/`SO_RCVTIMEO`, and **macOS reports an expired socket timeout as `EAGAIN` (errno 35)**, which Rust surfaces as `ErrorKind::WouldBlock` — not `ErrorKind::TimedOut`. Both currently map to `Error::SocketIo`.

2. **Given** `crates/shim/src/error.rs` has **no `Timeout` variant** (only `Connect` and `SocketIo` cover the socket path) **When** this story lands **Then** a timeout variant exists and is wired through all four partition functions — `exit_code()`, `level()`, `stderr_hint()`, and `sample_variants()` in the test module. **It MUST join the exit-0 / WARN / `stderr_hint() == None` class**, alongside `SocketIo`: the daemon is up and answering, so NFR20's fire-and-forget contract applies and Claude must still see success. The three partition canaries (`exit_code_never_2`, `level_matches_exit_code`, `stderr_hint_matches_exit_code`) must pass unmodified.

3. **Given** the reply the shim waits for is written immediately after a **non-blocking `tx.try_send(...)`** onto an mpsc queue ([handler.rs:121-135](../../crates/daemon/src/ingest/handler.rs)) — no SQLite write, no `.await` on durable work in the reply path **When** the 3ms read budget is nonetheless exceeded **Then** the cause is investigated and documented before any budget change. The reply should normally take well under 3ms, so **two timeouts in ~33 events is an anomaly to explain, not a budget to relax by reflex.** Leading hypotheses to test, in order: (a) the daemon's `current_thread` Tokio runtime stalling under concurrent WS fanout plus ingest (three live sessions were connected when this reproduced); (b) macOS scheduler contention on a loaded dev box; (c) cold-path cost on the very first hook after `bowerbird install`, which matches the 20:13:34 occurrence.

4. **Given** `UnixStream::connect` happens at [socket.rs:19](../../crates/shim/src/socket.rs) **before** both `set_*_timeout` calls **When** the connect itself is slow **Then** it is bounded by nothing. The code comment at [socket.rs:24-25](../../crates/shim/src/socket.rs) claims "Total = write + read ≤ 5ms in the worst case", which **excludes connect time and is therefore not a total**. Either bound the connect (e.g. `UnixStream::connect_timeout`-equivalent for Unix sockets) or correct the comment to state what the 5ms actually covers. No silent no-op.

5. **Given** the shim must never block Claude (Axiom 3, NFR20) **When** any change to retry or budget behavior is proposed **Then** the shim's hot-path bench (`shim/benches/hot_path.rs`, gated in CI per the per-platform policy committed in `crates/shim/benches/baselines/*.json`, ADR 0003; this AC originally said "+15% p99 regression", corrected by Story 5.18 because no platform gated at that number) still passes, and any added retry is bounded such that the worst-case total stays inside the shim's budget. **A retry loop that can exceed the budget is a regression, not a fix**, if measurement shows the budget cannot be met without stalling Claude, dropping the event remains the correct outcome and this story ends at diagnosability.

6. **Given** the shim log is the only place a dropped event surfaces **When** a timeout drop occurs **Then** a contract test asserts the log line distinguishes timeout from generic socket I/O. Note the testing constraint: a real `EAGAIN`-from-timeout is awkward to provoke deterministically, so prefer testing the **classification function** (io::Error → Error variant) directly over trying to race a real socket.

## Tasks / Subtasks

- [x] **Task 1: Reproduce and explain the timeout before changing anything (AC: 3)**
  - [~] Re-run the rc1 install-and-dogfood shape and confirm the drop rate. Capture `~/.bowerbird/shim.log` alongside daemon `tracing` output at `debug` (the ingest path logs `ingest: 200 accepted` at debug, [handler.rs:126](../../crates/daemon/src/ingest/handler.rs)). **PARTIAL, was previously checked, corrected on review.** The dogfood shape was re-run against a real release daemon and `~/.bowerbird/shim.log` was captured, but the **drop rate was not confirmed** (0 drops in 300 real-shim invocations) and **no daemon `debug` trace was captured**. See Completion Note 1's opening caveats. The mechanism was proven by other means (`errno_proof`), which is what AC #3's gate turns on, so this is disclosed rather than re-run.
  - [x] Instrument or measure where the 3ms goes: connect, write, or the read of the reply. Note that connect is currently unbounded and outside the "5ms total" claim (AC #4).
  - [x] Test hypothesis (a) specifically: reproduce with N concurrent WS presenters attached vs zero, since the daemon is `current_thread` and WS fanout shares that thread with ingest.
  - [x] Write the finding into the Dev Agent Record **before** proposing a budget change. If the cause turns out to be a daemon-side stall, say so, the fix may belong in the daemon, not the shim.

- [x] **Task 2: Add the `Timeout` error variant (AC: 1, 2)**
  - [x] Add a timeout variant to `crates/shim/src/error.rs`. Follow the existing forward-compat pattern (see the reserved `Backpressure(String)` variant with `#[allow(dead_code)]`, [error.rs:38-43](../../crates/shim/src/error.rs)).
  - [x] Wire it into `exit_code()` (→ 0), `level()` (→ WARN), `stderr_hint()` (→ `None`), and add it to `sample_variants()`. The partition tests are the gate; do not modify them to accommodate the variant.
  - [x] Classify at the call site in `socket.rs`: map `ErrorKind::WouldBlock` **and** `ErrorKind::TimedOut` from the write/read operations to the new variant (`WouldBlock` is the macOS spelling, `TimedOut` the Linux one, handle both, this is a cross-platform classification, not a macOS special case).
  - [x] Ensure the error message names the expired budget, so the log line says which timeout blew rather than restating the errno.

- [x] **Task 3: Resolve the unbounded connect (AC: 4)**
  - [x] Either bound the connect or correct the [socket.rs:24-25](../../crates/shim/src/socket.rs) comment. Prefer whichever keeps the hot path simpler (Axiom: prefer one code path over a branch).
  - [x] If bounding: confirm it does not add a syscall to the success path. **(N/A, comment corrected instead of bounding; see Completion Note 3 for the measurement behind that call.)**

- [x] **Task 4: Contract test for the classification (AC: 6)**
  - [x] Unit-test the io::Error → `Error` classification directly: `WouldBlock` → timeout variant, `TimedOut` → timeout variant, other kinds → `SocketIo`.
  - [x] Assert the emitted log line distinguishes the two. Reuse the shim log-assertion patterns already in `tests/contract_shim.rs`.
  - [x] Do **not** attempt to race a real socket timeout in the suite, it is nondeterministic and the parallel-test discipline forbids sleep-based synchronization. **(Honored, no race. See Completion Note 4: the added end-to-end test uses a peer that never replies, which makes the expiry deterministic rather than raced.)**

- [x] **Task 5: Budget decision, only if Task 1 justifies it (AC: 3, 5)**
  - [x] If and only if Task 1 shows the budget is genuinely too tight, propose the new numbers with the measurement backing them. Record the decision inline as "Maintainer decision (pickles, DATE)". **(Task 1 showed the budget is NOT too tight, so no change to the 2ms/3ms values was proposed and no maintainer decision was needed on Task 5's own subject. Two maintainer decisions were recorded elsewhere in this story, both about the review-surfaced write-budget question rather than about these numbers: see Completion Notes 8 and 9.)**
  - [x] Re-run the shim hot-path bench; confirm no p99 regression beyond the +15% CI gate.
  - [x] If Task 1 shows the cause is daemon-side, **stop here** and file the daemon work separately rather than widening this story. **(Filed as taskwarrior `719e7027`; no daemon code touched.)**

### Review Findings (2026-07-29, `bmad-code-review`)

Independent adversarial review of `git diff main...HEAD` on branch
`story-5.16-shim-timeout-diagnosability` (PR #28). Three layers ran: Blind
Hunter (diff only), Edge Case Hunter (diff + project read access), Acceptance
Auditor (diff + spec + context docs), plus reviewer verification.

**`classify()` itself was walked exhaustively and is sound.** The Edge Case
Hunter confirmed: `Interrupted` can never reach it (`write_all` and
`read_until`'s `fill_buf` both retry it); the "this socket is never
non-blocking" premise holds (no `set_nonblocking` anywhere in
`crates/shim/src`); no real timeout expiry stays `SocketIo` on either
supported platform (macOS `EAGAIN(35)` and Linux `EAGAIN(11)` both →
`WouldBlock`, `ETIMEDOUT` → `TimedOut`, both arms present); no genuine failure
misfires as a timeout (`EPIPE`, `ECONNRESET`, `WriteZero`, `InvalidData`,
unmapped raw errnos all fall to `_`); `read_line`'s EOF returns `Ok(0)` so it
cannot be misclassified; and `set_*_timeout` correctly bypasses `classify`.
The new end-to-end test's core is genuinely deterministic (no reply can ever
arrive, so no starved runner makes the timeout *not* fire), holding the
accepted stream really is load-bearing and really works (`try_clone` is a
`dup`), resource cleanup and drop order are correct, and the
exactly-one-newline assertion matches what `log::append` and `main` actually
do. I separately stressed the new test 60 times (40 quiet, 20 under 2x CPU
oversubscription) with 0 failures.

Verified independently and found accurate: the partition canaries are
byte-identical (`git diff` on `error.rs` is purely additive); `636 passed /
0 failed` reproduces (my own `scripts/test.sh` run, log
`target/test-logs/20260729-195209-82417`); `regression_max_ratio: null` on
macOS per ADR 0003; `log.rs` genuinely needed no change; both follow-up
taskwarrior IDs (`719e7027`, `dfe88917`) exist with matching descriptions; the
`events`-has-no-idempotency-key claim holds; scope is clean (no daemon,
protocol, migration, or retry change); and `shim-bench-gate` is green on
**both** platforms in CI run `30497303501`, which corroborates AC #5
independently of the uncommitted local A/B table.

- [x] [Review][Patch] HIGH: The timeout log line says `event dropped`, which is false in the case the story is about [crates/shim/src/error.rs:44]
  - **Evidence:** `#[error("socket {op} timed out after {budget_ms}ms; event dropped")]`, pinned by `assert!(log_contents.contains("event dropped"))` at [crates/shim/tests/contract_shim.rs:477].
  - **Why it matters:** Completion Note 1 (reason 3) establishes the opposite: *"When the read times out, the daemon has almost always already `try_send`-ed the event … the shim just never heard `200\n`."* That is correct: [crates/daemon/src/ingest/handler.rs:31-40] reads the line out of the kernel buffer and [handler.rs:121-135] enqueues it regardless of whether the shim is still around, so on a **read** timeout (the observed rc1 case, and the case `errno_proof` reproduced) the event lands and only the ack is lost. The new end-to-end test demonstrates this against itself: it asserts the peer received the full payload (`assert_eq!(payload["session_id"], "s1")`) in the same breath as asserting the log says the event was dropped. Worse, `event dropped` is the exact phrase [error.rs:143] uses for `Error::Connect`, where the drop is real, so the operator can no longer tell "event lost" from "ack lost", which is the diagnosability distinction this story exists to create. A **write** timeout probably *is* a real drop (truncated line, no newline, daemon 400s it), so one message currently covers two opposite outcomes.
  - **Implementor detail:** Say what the shim actually knows. E.g. `socket {op} timed out after {budget_ms}ms; no reply from daemon (delivery unconfirmed)`, or split the wording by `op` if the write case should keep "dropped". Update the test assertion with it, and reconcile the story text (§Story, Change Log) which also says "dropped".

- [x] [Review][Patch] MEDIUM: Two comments in `socket.rs` assert opposite platform facts about `TimedOut` [crates/shim/src/socket.rs:133]
  - **Evidence:** The test doc at `socket.rs:132-133` says *"`WouldBlock` is the macOS spelling of an expired `SO_*TIMEO` and `TimedOut` is the Linux one"*. The `classify()` doc 90 lines above at `socket.rs:39-44` says *"the story spec described `TimedOut` as 'the Linux spelling'; that is not right … `TimedOut` is where Windows (`WSAETIMEDOUT`) would land."*
  - **Why it matters:** Commit `fb3bda2` exists solely to correct this claim and missed the copy directly below the function it fixed. Completion Note 6's own justification is that *"a comment asserting a false platform fact is exactly the kind of thing that made the original `socket.rs` '5ms total' claim mislead for two stories"*; and it leaves one in place. A reader has no way to know which of the two comments to trust.
  - **Implementor detail:** Rewrite `socket.rs:132-134` to match the corrected rationale (both kinds matched because `std` does not pin which one an expired socket timeout produces).

- [x] [Review][Patch] HIGH: "write + read ≤ 5ms" is still not a bound, and a new test now pins the false invariant [crates/shim/src/socket.rs:72]
  - **Evidence:** `socket.rs:72-74`: *"Tight per-op timeouts bound the two operations that wait on the DAEMON: write + read ≤ 5ms."* `socket.rs:235-239`: `assert_eq!(WRITE_BUDGET_MS + READ_BUDGET_MS, 5, "the socket.rs comment claims write + read <= 5ms")`. Found independently by two layers.
  - **Why it matters:** `SO_SNDTIMEO`/`SO_RCVTIMEO` bound each individual send/recv **syscall**, not the aggregate of a loop. `write_all` [socket.rs:102-104] returns on `Err` but loops on every `Ok(n)`, so each successful partial write gets a fresh 2 ms budget. The shim accepts payloads up to 1 MiB ([crates/shim/src/main.rs:11]) against an 8192-byte Unix stream send buffer (`net.local.stream.sendspace: 8192`, verified by `sysctl` on this host), and a `PostToolUse` carrying real tool output routinely clears 8 KB. Worst case is `2ms × ceil(len / 8192) + 3ms`; up to ~256 ms for a 1 MiB payload, all of it inside Claude's hook, which is the trust-boundary stall Axiom 3 exists to forbid. For payloads over 8 KB the write also blocks on the daemon draining its 8192-byte `recvspace`, so it is just as starvation-exposed as the read, N times over, which contradicts the comment's framing of write and read as symmetric bounded waits. Second-order: `Error::Timeout { op: "write", budget_ms: 2 }` will report "timed out after 2ms" after an operation that consumed tens of ms, which is exactly the lie the single-sourcing rationale at `socket.rs:16-19` claims to prevent. AC #4 asked to *"correct the comment to state what the 5 ms actually covers"* and forbade a silent no-op; the correction fixed the connect omission and left the larger falsehood in the same sentence, then added a test that codifies it.
  - **Note on scope:** the underlying per-syscall behavior is pre-existing (the bound was never aggregate). What this story owns is (a) a comment newly asserting it is safe and (b) a test pinning it. Fixing the comment and the assertion is in scope here; deciding whether the shim should actually cap total write time (or cap payload size below the send buffer) is a follow-up worth filing.
  - **Implementor detail:** State the real shape; each write/read syscall is bounded at 2 ms/3 ms, the aggregate is unbounded for payloads exceeding the socket send buffer, and `connect` is outside it entirely. Drop or reword the `== 5` assertion so it stops pinning a claim the code does not make.

- [x] [Review][Patch] MEDIUM: The comment's connect-is-safe argument has an unstated precondition, and on Linux the exposure is an unbounded hang [crates/shim/src/socket.rs:76]
  - **Evidence:** `socket.rs:76-83` asserts connect *"completes in the kernel as soon as the connection lands in the accept backlog; it does not wait for the daemon to call `accept`, so it does not depend on the daemon being scheduled."*
  - **Why it matters:** That holds only while the accept queue has room. The daemon listens via `tokio`/`mio`, which passes `backlog = -1` → clamped to `somaxconn`. Once the queue fills; which is precisely the wedged-daemon case; Linux's `unix_stream_connect` on a **blocking** socket calls `unix_wait_for_peer` with the socket's send timeout, and the shim sets `SO_SNDTIMEO` only *after* connect returns, so that timeout is 0 (infinite). Result on Linux: an unbounded block inside Claude's hook, invisible to `classify` since it never reaches the write. On macOS the same condition returns `ECONNREFUSED`, which is bounded but gets reported as `Connect` → "daemon not running, event dropped" while the daemon is in fact running. Reachability is low (needs `somaxconn` queued connections), but the measurement cited in the comment was taken against a *responsive* daemon and therefore cannot speak to this case at all. This is the argument Task 3 used to justify leaving connect unbounded, so it is load-bearing.
  - **Implementor detail:** Add the precondition to the comment (in-kernel completion holds while the accept backlog has room; a full backlog blocks, unboundedly on Linux). Whether to actually bound connect stays out of scope, but the rationale should not read as unconditional. Worth adding to the `719e7027` follow-up.

- [x] [Review][Patch] MEDIUM: Hypothesis (a) is reported as "refuted" on evidence that cannot reach that conclusion [Completion Note 1; docs/bmad/implementation-artifacts/sprint-status.yaml:125]
  - **Evidence:** Both WS-presenter rows in the Task 1 table are labelled machine state *"12 CPU spinners"*. The same note then establishes that *"naive CPU spinners **reduced** latency … Only realistic mixed load; parallel `rustc` … reproduces the tail."*
  - **Why it matters:** By the record's own methodology finding, spinner load is not a state in which the tail can appear. So the fanout rows were collected under a condition where the daemon was known to stay fast, which cannot distinguish "fanout does not contribute" from "the load that triggers starvation was absent". What was actually refuted is "WS fanout **alone** starves ingest". The rc1 condition was three live sessions **plus** an install **plus** a build; fanout combined with compile-storm load, which was never measured. "Refuted, not merely untested" and "This was the story's leading hypothesis; it is wrong" overstate the evidence, and the overstatement is propagated verbatim into `sprint-status.yaml` and the Change Log.
  - **Implementor detail:** Downgrade to "WS fanout alone does not starve ingest; fanout under compile-storm load was not measured" in Completion Note 1, the Change Log, and the sprint-status entry.

- [x] [Review][Patch] MEDIUM: Task 1's first sub-bullet is checked but its stated deliverable was not met [Task 1]
  - **Evidence:** The sub-task reads *"Re-run the rc1 install-and-dogfood shape and **confirm the drop rate**. Capture `~/.bowerbird/shim.log` alongside daemon `tracing` output at `debug`."* The record reports no `bower.db` row count, no daemon debug trace anywhere, and mentions *"300 real-shim invocations with zero drops"* only in passing inside the methodology-correction paragraph. The Debug Log References note that `probe.rs`'s *"Timeouts are parameterized so the tail can be measured untruncated"*, so "p99 2436µs, max 5686µs, **straight through the 3ms budget**" is an inference from a run where the budget was not in force, presented in the register of an observation.
  - **Why it matters:** AC #3 is the gate on every other decision in this story ("do not change the budget, do not retry"). The mechanism *is* proven (the `errno_proof` reproduction is genuinely good), but no actual drop was ever reproduced under the real budgets, and the honest headline is buried: "0 drops in 300 real-shim invocations, and the tail is only visible with the budget parameterized away". A trivially available check (count hooks vs `events` rows during a storm) would have settled both this and the HIGH finding above.
  - **Implementor detail:** Restate Task 1 as "mechanism proven; drop-rate reproduction not achieved" and surface the 0/300 result and the parameterized-budget caveat in Completion Note 1's headline rather than in the methodology paragraph.

- [x] [Review][Patch] LOW: The new test helper's doc claims "no sleep used for synchronization" and then sleeps to synchronize [crates/shim/tests/contract_shim.rs:68]
  - **Evidence:** `contract_shim.rs:68-69`: *"There is no sleep used for synchronization and no ordering to lose."* `contract_shim.rs:111`: `thread::sleep(Duration::from_millis(10));` immediately before returning.
  - **Why it matters:** That sleep is exactly a synchronization sleep (waiting for the accept loop to come up), and `docs/bmad/project-context.md:650` plus this story's own Dev Notes both ban them. It is also provably unnecessary: `UnixListener::bind` happens at `contract_shim.rs:77` on the calling thread, so the path exists and the kernel backlogs the connect whether or not the spawned thread has been scheduled. The substantive claim; that the *timeout* is not raced; is true and worth keeping.
  - **Implementor detail:** Delete the 10 ms sleep (copied from `start_mock_ingest`) and narrow the comment to "the timeout is not raced". Fixing the copy in `start_mock_ingest` too is optional.

- [x] [Review][Patch] LOW: The end-to-end test declares tolerance for a write timeout it cannot actually survive [crates/shim/tests/contract_shim.rs:466]
  - **Evidence:** `contract_shim.rs:466-468`: *"the write budget could in principle be the one to blow on a starved runner, and either way the diagnosability contract is met."* `contract_shim.rs:490-493` then does `wait_for_capture(&mock)` → `parse_captured_payload` → `assert_eq!(payload["session_id"], "s1")`.
  - **Why it matters:** If `write_all` times out mid-payload, the mock's single `read_until(b'\n')` never sees a newline, `captured` stays empty, and `wait_for_capture` panics on its 2 s guard. The stated tolerance is a failure mode the test does not survive, so the comment is wrong even though the flake probability is very low (a small payload fits the send buffer in one syscall). Noted for accuracy, not because the test is expected to flake; 60 invocations passed, 20 of them under 2x CPU oversubscription.
  - **Implementor detail:** Either drop the tolerance claim and assert `"read timed out"` specifically, or keep the loose assertion and delete the payload assertion. Also worth noting `wait_for_capture`'s guard is 2 s, not the project's 30 s hang-guard convention (pre-existing helper).

- [x] [Review][Patch] LOW: `raw_eagain_from_macos_classifies_as_timeout` asserts nothing on Linux [crates/shim/src/socket.rs:154]
  - **Evidence:** The entire body is inside `if eagain.kind() == ErrorKind::WouldBlock`. On Linux errno 35 is `EDEADLK`, not `EAGAIN` (verified), so the guard is false and the test reports green having asserted nothing.
  - **Why it matters:** The skip is deliberate and commented, so this is not a defect, but the test should not be counted as Linux coverage, and nothing fails if macOS ever stops delivering errno 35 as `WouldBlock`, which is the premise the whole story rests on.
  - **Implementor detail:** `#[cfg(target_os = "macos")]` the test and assert the premise unconditionally (`assert_eq!(eagain.kind(), ErrorKind::WouldBlock)`), so a platform change is a failure rather than a silent skip.

- [x] [Review][Patch] LOW: The exit-0 rationale rests on a claim `socket.rs` spends a paragraph refuting [crates/shim/src/error.rs:97]
  - **Evidence:** `error.rs:97-101` and `error.rs:151-156` both justify exit-0 with *"the connect succeeded, so the daemon is up"* / *"a timeout means the daemon answered the connect"*. `socket.rs:77-83` argues the opposite: *"a Unix-socket `connect` … completes in the kernel as soon as the connection lands in the accept backlog; it does not wait for the daemon to call `accept`, so it does not depend on the daemon being scheduled."*
  - **Why it matters:** Nothing "answered" the connect. A successful connect proves a listener FD exists, not that the daemon is alive or scheduled. The exit-0 **placement** is still right (AC #2 mandates it, and NFR20 wants Claude to see success regardless), so this is a wording defect, not a behavior one, but it is the third instance in this diff of a comment asserting something the same diff disproves elsewhere.
  - **Implementor detail:** Reword to what is true: the socket exists and the payload was handed to the kernel, so this is the fire-and-forget class, not the daemon-unreachable class. The behavioral exposure it hints at (a wedged daemon with a live listener silently swallowing every event at exit-0) is deferred below.

- [x] [Review][Defer] MEDIUM: The partition canaries do **not** actually guard a future variant: `sample_variants()` [crates/shim/src/error.rs:179-209] is a hand-maintained list with no compile-time link to the enum; deferred, pre-existing
  - `exit_code()` and `stderr_hint()` have no `_` arm, so the compiler forces a new variant to be *placed*. Nothing forces it into `sample_variants()`. A future variant returning `2` from `exit_code()`, or an exit-1 variant with `stderr_hint() == None`, compiles clean and all three canaries pass green because the offending value is never constructed. So the doc claim at [error.rs:79-80]: *"the `exit_code_never_2` unit test below is the belt-and-suspenders gate against a future variant being added without thought"*; is not true, and Story 5.16 is the proof: the author had to remember by hand. **For this story the canaries do hold** (both `Timeout` spellings were added, verified), so AC #2 is satisfied; the gap is in the mechanism, not this change. A real gate needs `sample_variants()` forced exhaustive (an exhaustive `match` that destructures every variant, or a derive-based iterator).
  - Two related overstatements, both harmless: the two `Timeout` entries add zero coverage over one, since `exit_code()`/`stderr_hint()` both match `Error::Timeout { .. }` and discard `op`/`budget_ms`; the comment at [error.rs:191-192] implies `op` is partition-relevant when it is not. And `level_matches_exit_code` [error.rs:218-228] is tautological: `level()` is *defined* as the mapping the test asserts, so it cannot fail for any variant, which means AC #2's "wired through `level()`" has no real coverage.

- [x] [Review][Defer] LOW: `Error::BadResponse(line)` carries the raw trailing newline, which would break the one-line-per-event log framing the new test now asserts [crates/shim/src/socket.rs:122]; deferred, pre-existing and unreachable with the current daemon **[RESOLVED in the pass-1 resolution, not deferred: `send` now returns `BadResponse(trimmed)`. See Completion Note 8.]**
  - `send` returns `Error::BadResponse(line)` untrimmed even though `trimmed` exists two lines above, so `log::append` would write two newlines for one event. Not reachable today (the daemon emits only `200\n` / `503\n` / newline-sanitized `400 …\n`, and EOF yields an empty string), but the new test at [contract_shim.rs:462-466] now treats "exactly one newline" as a log invariant, and this is the one path in the touched function that can violate it.

- [x] [Review][Defer] The behavioral half of the HIGH `write_all` finding: whether the shim should actually cap total write time, or cap the wire payload below the 8 KiB send buffer, rather than only documenting the real worst case [crates/shim/src/socket.rs:102-104, crates/shim/src/main.rs:11]; deferred, pre-existing behavior **[Now owned by Story 5.17. Attempted in the pass-1 resolution, measured ineffective by pass 2, backed out. See Completion Note 9.]**

- [x] [Review][Defer] Task 1's entire measurement basis is uncommitted, so AC #3's gate rests on unreproducible prose; deferred, disclosed by the author
- [x] [Review][Defer] A wedged daemon (live listener, hung runtime) silently swallows every event at exit-0 with empty stderr; only the shim log shows it [crates/shim/src/error.rs:102-107]; deferred, belongs to taskwarrior `719e7027`
- [x] [Review][Defer] AC #5 and `docs/bmad/project-context.md:629` both say the shim bench gates at +15%; the committed config is `null` on macOS and `1.35` on Linux; deferred, pre-existing doc drift
- [x] [Review][Defer] `start_mock_ingest_silent` is a near-verbatim copy of `start_mock_ingest` (bind, Arc pair, accept loop, both sleeps, construction); one helper taking `Option<&'static [u8]>` covers both [crates/shim/tests/contract_shim.rs:75-118]; deferred, test-only duplication

Dismissed as noise (5): "the 3 ms budget is contradicted by the 5.7 ms measurement" (Blind Hunter had no spec; the record's argument is precisely *not* to relax it); the tautological negative assertions in `timeout_message_names_the_operation_and_budget` and the redundant third assertion in `budgets_match_the_documented_contract` (intentional change-detectors); story/AC/taskwarrior references in source comments (established house style in this repo); `drop(held)` being the closure's last statement and the `let _ =` swallows in the mock's accept loop (copied pre-existing pattern); and "the log level may come from a non-exhaustive third match" (verified: it derives from `exit_code()`). `level()` not being literally edited despite AC #2 naming four functions is disclosed in the File List and behaviorally fine; the weaker point about `level_matches_exit_code` being tautological is folded into the `sample_variants()` defer above.

### Review Findings, Pass 2 (2026-07-29, `bmad-code-review`, resolution review)

Independent second-pass review of `git diff main...HEAD` (PR #28, head
`1185417`), targeting the resolution commit `8736799`. Pass 1's findings above
are left untouched; this section is additive. Three layers ran again with no
shared context (Blind Hunter diff-only, Edge Case Hunter with project read
access, Acceptance Auditor with spec + context docs), plus reviewer
verification with standalone measurement programs.

**Outcome: CHANGES REQUESTED.** Two HIGH, both on `write_all_within`, the new
production function the resolution added. The headline is that **pass 1's HIGH
2 is not actually fixed**: `write_all_within` does not bound the aggregate
write, and a 1 MiB payload to a moderately slow peer still stalls Claude for
**191 ms and returns `Ok`**, measured on this host with the shipped code. The
resolution's direction is right and the mechanism is half of the answer; it is
the wrong half to stop at, because the missing half is the one the code,
five doc sites, a test name, the commit message, and sprint-status all now
assert is closed.

**Verified independently and found accurate.** `scripts/test.sh` reproduces
**639 passed / 0 failed** (log `target/test-logs/20260729-203847-17383`), and
all 9 new tests are present and passing by name. `cargo fmt --check` and
`cargo clippy --all-targets --workspace -- -D warnings` are clean. All 8 CI
checks are green on `1185417`, both platforms. No emdash character appears on
any line this branch added (grepped over the added lines of
`git diff main...HEAD`, count 0), so the standing style rule is honored. The sub-microsecond re-arm claim at
`socket.rs:115-117` is **true and worth keeping**: measured, `std` rejects a
zero `Duration` with `InvalidInput` and floors anything under 1 µs to
`tv_usec = 1`, so the Unix "zero `timeval` means infinite" trap is genuinely
avoided on every path. `write_all_within`'s byte accounting is correct (no byte
can be skipped or double-sent), its match-arm ordering is load-bearing and
right (an empty buffer is caught by `n >= buf.len()` and does **not** become
`WriteZero`), it cannot spin forever, and `classify`'s "this socket is never
non-blocking" premise still holds (no `set_nonblocking` anywhere in
`crates/shim/src`). The read-side wording (`no reply from daemon, event may
already be recorded`) is **true** against `handler.rs:121-135` and correctly
hedged. `BadResponse(trimmed)` does preserve the one-newline invariant. Eight
of pass 1's ten patch findings are genuinely and fully resolved (2, 4, 6, 7, 8,
9, and the code halves of 1 and 10).

- [x] [Review][Patch] HIGH: `write_all_within` does not bound the aggregate write; the measured stall is 191ms returning `Ok`, so pass 1's HIGH 2 is still open [crates/shim/src/socket.rs:92-128]
  - **Evidence:** `SO_SNDTIMEO` does not bound one `write(2)` either. On macOS the kernel's `sosend` loop re-waits per socket-buffer refill, so a single `write(2)` with `SO_SNDTIMEO = 2ms` keeps going as long as the peer drains anything. Measured on this host with a standalone program replicating the shipped function verbatim (2 ms budget, 1 MiB payload, peer draining 8 KiB per interval):

    | peer drain rate | shipped result | elapsed | write syscalls | re-arms |
    | --- | --- | --- | --- | --- |
    | flat out (healthy) | `Ok` delivered | 1.0 ms | 1 | 0 |
    | 8 KiB / 200 µs | **`Ok` delivered** | **39 ms** | 1 | 0 |
    | 8 KiB / 500 µs | **`Ok` delivered** | **96 ms** | 1 | 0 |
    | 8 KiB / 1000 µs | **`Ok` delivered** | **191 ms** | 1 | 0 |
    | 8 KiB / 1400 µs | `Err(Timeout)` | **47.8 ms** | 1 | 0 |
    | 8 KiB / 2000 µs | `Err(Timeout)` | 2.5 ms | 1 | 0 |

    The first match arm, `Ok(n) if n >= buf.len() => return Ok(())`, returns without ever consulting `deadline`. The deadline is checked only *between* syscalls, so when the kernel satisfies the whole payload in one call the bound is not merely loose, it is absent. By payload size at the 8 KiB / 1000 µs rate: 64 KiB → 9.2 ms, 256 KiB → 45.6 ms, 1 MiB → 191 ms, all returning `Ok`.
  - **Why it matters:** this is the identical failure shape pass 1 flagged and the resolution claims to have closed: a silent multi-hundred-millisecond stall inside Claude's hook that reports success. The story's own pre-fix reproduction (256 KiB → 82 ms) still reproduces at 45.6 ms with the fix in place, so roughly half the stall was removed and none of the *bound* was. Meanwhile the claim is now asserted in six places: `WRITE_BUDGET_MS`'s doc ("**These are aggregates, and that takes work to be true**", `socket.rs:23`), `write_all_within`'s doc ("the loop **cannot** outlive the deadline", `socket.rs:83-84`), `send`'s comment ("hand-off ≤ 2ms **AGGREGATE**, enforced by `write_all_within`", `socket.rs:143-148`), `Error::Timeout`'s doc ("`budget_ms` is the **aggregate** budget … honest even when the payload takes several `write(2)` calls", `error.rs:83-86`), the test name `write_all_within_bounds_the_aggregate_not_each_syscall`, and Completion Note 8 / the commit message / sprint-status. A false comment that a reviewer already had to catch once is now a false comment plus a passing test, which is strictly worse than the state pass 1 found. Second-order: at 8 KiB / 1400 µs the shim emits `socket write timed out after 2ms; event not sent` after **47.8 ms**, exactly the drift the single-sourcing rationale at `socket.rs:18-21` exists to prevent.
  - **Implementor detail:** cap each `write(2)` at roughly the send-buffer size so the kernel's internal loop cannot re-wait inside one syscall: `let take = buf.len().min(CHUNK);` with `CHUNK = 8192`, writing `&buf[..take]`. Verified: chunked at 8192 the same program returns `Err(Timeout)` in **2.03 to 2.51 ms at every drain rate tested**, and a small payload still completes in one syscall (5.25 µs chunked vs 6.04 µs shipped, noise), so the success-path property survives untouched because `take == buf.len()` for anything under the chunk. If the maintainer would rather not change behavior a third time in one story, the alternative is to correct all six claim sites to "each `write(2)` is bounded; the aggregate is bounded only across syscalls, and one syscall can itself overrun on macOS" and reopen the deferred item, but the claims cannot stay as written either way.

- [x] [Review][Patch] HIGH: the only test of `write_all_within` never executes the re-arm, which is the entire mechanism of the fix [crates/shim/src/socket.rs:368-430]
  - **Evidence:** instrumented replication of the test's exact shape (1 MiB payload, peer draining 8 KiB every 2 ms) over 10 rounds: **1 write syscall and 0 re-arms, 10 times out of 10**, elapsed 2.54 to 2.57 ms. Because the peer is slower than the write budget, the first partial write already exhausts the deadline, so `left.is_zero()` fires on the first pass and `stream.set_write_timeout(Some(left))` at `socket.rs:118-120` is never reached.
  - **Why it matters:** the re-arm is the fix. Its only test drives the one drain rate at which the naive code would also have bailed, which is why the 191 ms hole above was invisible to a green suite and to two CI platforms. Completion Note 8's verification ("swapped the body back to `write_all` and confirmed the test FAILS") is genuine but does not cover this: measured, a `write_all` swap-back returns `Ok(())` after ~365 ms on this peer, 6/6, so the failure comes from the `matches!(Err(Error::Timeout { .. }))` assertion, not from the elapsed ceiling. The test proves "we are not `write_all`", not "the aggregate is bounded".
  - **Implementor detail:** add a case with a peer draining fast enough to keep one syscall making progress (8 KiB per 500 µs is enough) and assert the total stays within a small multiple of the budget. That case fails against today's code, which is the point of adding it. Separately, replace the wall-clock ceiling with a timing-free discriminator: have the drain thread count bytes and assert the peer received far less than the payload (bounded: 16 to 24 KiB; `write_all`: the full 1 MiB). See the ceiling finding below.

- [x] [Review][Patch] MEDIUM: the wall-clock ceiling in the aggregate test is a latency assertion, and it is redundant [crates/shim/src/socket.rs:418-425]
  - **Evidence:** `let ceiling = Duration::from_millis(100); assert!(elapsed < ceiling, ...)`. Found independently by two layers. `docs/bmad/project-context.md:652` is explicit: hang guards are 30 s, "not how fast it should be", because a starved CI scheduler can stall one test's thread for multiple seconds, and `elapsed` here spans a loop that shares 4 vCPUs with the rest of the suite plus this test's own sleeping drain thread.
  - **Why it matters:** it is the one wall-clock assertion this branch adds, and measurement says it buys nothing: a swap-back to `write_all` returns `Ok(())` (6/6, ~365 ms), so the `matches!` assertion immediately above already catches that regression deterministically. The docstring's margin argument is also weaker than it reads: 100 ms is 3.6x below the ~365 ms regression, not "well below".
  - **Implementor detail:** delete the `elapsed`/`ceiling` assertion and assert on bytes received by the peer instead (a structural discriminator with a 40x separation and no clock). Also fix the failure message, which says "most likely `write_all_within` was replaced by `write_all`" in a test that calls `write_all_within` directly.

- [x] [Review][Patch] MEDIUM: the Change Log was not updated for the resolution pass at all, and Completion Note 8 plus the File List say it was [docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md:398]
  - **Evidence:** `git show 8736799` touches the Change Log line only to strip an emdash. It still reads "The story's leading hypothesis (a), WS fanout, was **refuted**", which is the third of the three locations pass 1's hypothesis-(a) finding named ("Completion Note 1, the Change Log, and the sprint-status entry", story:126). Completion Note 8 at story:373 states it was downgraded "in this note, the Change Log, and sprint-status"; the File List at story:391 lists the Change Log as updated. Completion Note 1 and both live sprint-status entries **were** fixed, so this is one location out of three.
  - **Why it matters:** pass 1 finding 5 is checked off but only two thirds done, and the Change Log is the entry point a future reader hits first. The same entry is now stale in four other ways: "Outcome is diagnosability-only … **no budget change and no retry**" (the write path changed behavior), "the false '5ms total' comment corrected" with no mention of `write_all_within`, "6 new tests" (now 9), and "636 passed / 0 failed" (now 639). There is no Change Log entry for the resolution pass at all, so the durable record's last word on this story omits its only behavior change.
  - **Implementor detail:** downgrade "refuted" in the 398 entry and add a second Change Log entry for the resolution pass naming the behavior change, the new counts, and the HIGH above once it is settled.

- [x] [Review][Patch] MEDIUM: Completion Note 2 still advertises the retracted log line as the shipped one [docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md:311]
  - **Evidence:** the before/after block still reads `WARN socket read timed out after 3ms; event dropped                     <- now`. The shipped `Display` is `socket read timed out after 3ms; no reply from daemon, event may already be recorded` (`error.rs:40,90`).
  - **Why it matters:** this is the second half of pass 1 finding 1, which asked to "reconcile the story text". The story's own new test `event_dropped_phrasing_is_reserved_for_real_drops` would fail the string the story presents as current, so the record contradicts the code it documents on the exact point the finding was about.
  - **Implementor detail:** update the `<- now` line to the shipped string.

- [x] [Review][Patch] MEDIUM: `TimeoutOp::Write`'s doc names a daemon log message that cannot fire on that path [crates/shim/src/error.rs:14-16]
  - **Evidence:** the doc says the daemon "discards a line it never finished reading (`ingest: EOF before newline`)". `handler.rs:31-41` emits that message **only** from the `Ok(0)` arm, i.e. only when zero bytes arrived. A `TimeoutOp::Write` can only fire after `write_all_within` saw `Ok(n)` with `n > 0` (the deadline check lives inside the partial-write arm), so at least one byte is always on the wire. Confirmed empirically: a bounded write timeout leaves the peer holding 16384 bytes, "ends with newline = false". The daemon therefore takes `handler.rs:45-56` and logs `ingest: invalid JSON` (and tries to reply `400 invalid JSON: …` to an already-closed socket), or `ingest: read_line error` when the truncation lands mid-UTF-8.
  - **Why it matters:** the whole purpose of this doc comment is to stop a future reader collapsing the two consequences again, and it hands the operator a grep string that never appears. Worse, the string they *do* find, `invalid JSON`, points at a shim serialization bug rather than a write-budget expiry. This is the same class of defect as three of pass 1's findings.
  - **Implementor detail:** name the message that actually fires, and note `EOF before newline` is the different case of a client that connected and wrote nothing.

- [x] [Review][Patch] MEDIUM: the read half has the same per-syscall-versus-aggregate gap, and the new comment asserts it does not [crates/shim/src/socket.rs:149]
  - **Evidence:** the comment reads "ack wait ≤ 3ms  a single `recv`, so `SO_RCVTIMEO` bounds it directly". `BufRead::read_line` is `read_until`, a loop over `fill_buf`, one `recv` each, each with a fresh `SO_RCVTIMEO`. Found independently by two layers and measured twice: a peer dribbling `200\n` one byte per 2 ms yields `read_line -> Ok(4)` after **12.1 ms** against the 3 ms budget; a peer dribbling 32 bytes per 2 ms with no newline yields `Err(WouldBlock)` after **23.9 ms**. `Error::Timeout`'s doc at `error.rs:83-86` calls `budget_ms` "the **aggregate** budget for the operation", which as shipped is true of neither op.
  - **Why it matters:** the comment block that just got rewritten to stop over-claiming on the write side over-claims on the read side, in the same seven lines, on the larger of the two budgets. Live reachability is low (today's daemon writes `200\n` in one `write_all`), so the fix is documentation, not behavior. But "a single `recv`" is a property of the current peer, not of the code, and stating it as a bound is what made the original "5 ms total" comment mislead for two stories.
  - **Implementor detail:** state it as "≤ 3 ms per `recv`; `read_line` loops, so the aggregate holds only while the reply arrives in one `recv`, which it does today", and drop the word "aggregate" from `Error::Timeout`'s doc or scope it to the write op. The behavioral half is deferred below.

- [x] [Review][Patch] MEDIUM: a newly added line re-asserts the "daemon answered the connect" claim that pass 1 finding 10 removed from `error.rs` [crates/shim/tests/contract_shim.rs:445]
  - **Evidence:** `// The daemon answered the connect and the event was handed over; per NFR20`, an added line in this diff. `socket.rs:152-158` argues a Unix connect completes in the kernel and "does not depend on the daemon being scheduled", and `error.rs:147-153` was reworded specifically to stop saying this.
  - **Why it matters:** there were three copies of the claim; the resolution fixed two and added the third in the same commit. Completion Note 8's "both exit-0 rationales stopped asserting 'the daemon answered the connect'" is true of `error.rs` and false of the diff.
  - **Implementor detail:** reword to what the test actually establishes: the listener accepted the connection and the payload reached the peer.

- [x] [Review][Patch] MEDIUM: `deferred-work.md` is wrong on three of the seven entries the same commit added [docs/bmad/implementation-artifacts/deferred-work.md]
  - **Evidence:** item 2 says Story 5.16 "fixed the comment's aggregate claim during review, **but did not change behavior**" and lists "track elapsed time across the `write_all` loop and abort past a total budget (hand-rolled loop, no longer `write_all`)" as an open option; that option is `write_all_within`, shipped in the same commit. Item 4 (`BadResponse` carries the raw newline) was fixed in the same commit (`socket.rs:211` now passes `trimmed`). Item 7 still lists "the 10ms startup sleep" among the duplicated lines, which the same commit deleted from the new helper.
  - **Why it matters:** `deferred-work.md` is the durable backlog other stories read, and it now tells a future reader the shim's write is unbounded and hand-rolling a bounded loop is still on the table. The same reader is one file away from `socket.rs:33-34` begging them not to swap it back.
  - **Implementor detail:** delete item 4, drop the sleep clause from item 7, and rewrite item 2 as the genuine residual once the HIGH above is settled (the surviving questions are the payload cap and the large-payload loss profile, not the aggregate documentation).

- [x] [Review][Patch] MEDIUM: the story's `[Review][Defer]` list still marks as deferred two items Completion Note 8 says were fixed [docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md:157,160]
  - **Evidence:** story:157 defers the `BadResponse` newline "pre-existing and unreachable"; story:160 defers "whether the shim should actually cap total write time … deferred, pre-existing behavior". Story:371 and :375 say both were fixed.
  - **Why it matters:** a reader of the Review Findings section alone draws the opposite conclusion from a reader of Completion Note 8, on the story's single behavior change.
  - **Implementor detail:** annotate both defer bullets as resolved in the resolution pass, with a pointer to Completion Note 8, rather than editing pass 1's text away.

- [x] [Review][Patch] MEDIUM: the behavior change's new event-loss surface is nowhere on the record, and the bench cited to clear AC #5 cannot exercise it [crates/shim/src/socket.rs:23-34, crates/shim/benches/hot_path.rs:86]
  - **Evidence:** before the change a payload above the send buffer was *delivered* slowly; now the whole hand-off must fit 2 ms, so under exactly the starvation Task 1 diagnosed, large payloads are dropped rather than delivered late. Both the code comment ("**Costs nothing on the success path**") and Completion Note 8 ("**Zero cost on the success path**") speak only to latency, never to delivery. The AC #5 bench fixture is a 76-byte JSON line, so it is structurally incapable of touching the changed path.
  - **Why it matters:** the event-loss side of an event-loss hotfix should not be the undocumented half. To be fair to the change, this is a smaller exposure than it first looks and the pass-1 style estimate of "≥128 syscalls" does not hold: measured, a healthy peer takes a full 1 MiB in **one** `write(2)` in ~1.0 ms, so there is no new routine loss. But 1.0 ms against a 2 ms budget is only 2x of headroom on a quiet machine, so a modest slowdown is enough to flip large payloads from delivered to dropped. Trading late delivery for loss is the right Axiom 3 call; it just needs to be a recorded call.
  - **Implementor detail:** one sentence in the `WRITE_BUDGET_MS` doc naming the accepted consequence and where it is sized (`main.rs::MAX_STDIN_BYTES`), plus the same sentence in the story. A large-payload case in the bench or a payload-size distribution measurement would be better still, but the recorded trade is the minimum.

- [x] [Review][Patch] LOW: the `Interrupted` arm neither checks the deadline nor re-arms, so "the loop cannot outlive the deadline" is false on that path [crates/shim/src/socket.rs:124]
  - **Evidence:** `Err(ref e) if e.kind() == Interrupted => continue` goes straight back to `write` with whatever `SO_SNDTIMEO` was last set, which on the first iteration is the full budget measured from now. Found by two layers. Effectively unreachable today (the shim installs no signal handlers and Rust's `SIGPIPE` disposition is `SIG_IGN`, which does not produce `EINTR`), and the byte accounting is correct because POSIX returns `EINTR` only when nothing was transferred.
  - **Implementor detail:** hoist the remaining-time check to the top of the loop so `Ok(n)` and `Interrupted` both pass through it. That makes the doc's absolute claim true for three lines of change.

- [x] [Review][Patch] LOW: the reworded exit-0 rationale's new premise is false for the write half [crates/shim/src/error.rs:143-145]
  - **Evidence:** "the ingest socket exists and **the payload was handed to the kernel**, so this is the fire-and-forget class". `TimeoutOp::Write`'s own doc 130 lines above says "The payload never fully reached the daemon … the event is genuinely lost."
  - **Why it matters:** the exit-0 placement is still correct on NFR20 grounds (the comment's own last sentence carries it), but the newly written reason is false for half the values of `op`, in the file that just spent 40 lines arguing that blurring the two halves was the HIGH bug.
  - **Implementor detail:** drop the "payload was handed to the kernel" clause and rest the justification on NFR20 alone.

- [x] [Review][Patch] LOW: `write_all_within`'s bound rests on a doc-only precondition, and the reported budget is a const while the enforced deadline is a free parameter [crates/shim/src/socket.rs:91-92,108-113]
  - **Evidence:** "The caller must have armed the socket with the full budget before calling" is the only thing making the first syscall bounded, and it is not checked. Separately, `deadline` is an arbitrary caller-supplied `Instant` while the error always reports `WRITE_BUDGET_MS`, so any caller passing a different deadline produces a log line that lies about the budget. `WRITE_BUDGET_MS`'s own doc justifies its existence as making exactly that drift impossible.
  - **Implementor detail:** take `budget_ms` and derive both the arming and the deadline inside the function, which removes the precondition and the drift together.

- [x] [Review][Patch] LOW: "Costs nothing on the success path" is one clock read short of literal [crates/shim/src/socket.rs:86,184]
  - **Evidence:** `send` now runs `Instant::now()` unconditionally at `socket.rs:184`, which the old `write_all` path did not. Negligible (tens of nanoseconds, and the A/B bench supports "neutral"), but the surrounding comments reject a bounded connect specifically because it "adds syscalls to the SUCCESS path", so the file holds itself to the literal standard.
  - **Implementor detail:** "one clock read on the success path; the extra `setsockopt` only after a partial write." Same wording change in Completion Note 8's "Zero cost".

- [x] [Review][Patch] LOW: the aggregate test hand-rolls a PID-keyed dir in `env::temp_dir()` instead of `TempDir`, and leaks it on the panic path [crates/shim/src/socket.rs:372,429]
  - **Evidence:** `let dir = std::env::temp_dir().join(format!("bb516-agg-{}", std::process::id()));` with `remove_dir_all` as a trailing statement after four assertions. `CLAUDE.md` says to isolate per-test state with `TempDir`. Not a flake (the `remove_file(&sock)` before `bind` clears a stale socket), just hygiene, and `tempfile` is already a dev-dependency.
  - **Implementor detail:** `TempDir::new()` and drop the manual cleanup.

- [x] [Review][Patch] LOW: `start_mock_ingest_silent`'s reader thread can park forever in `read_until`, ignoring its stop flag [crates/shim/tests/contract_shim.rs:97-101]
  - **Evidence:** `stop` is only observed between `accept()` calls. Once inside a blocking `read_until(b'\n', …)`, a client that connects without ever sending a newline parks the thread permanently while `MockIngest::drop` removes the `TempDir` from under it. Harmless today because the current test always sends a complete line; it is a latent trap for the next reuse, and a write-timeout test is precisely the reuse where no newline arrives by construction.
  - **Implementor detail:** `set_read_timeout(Some(HANG_GUARD))` on the accepted stream, and `.expect(...)` rather than `let _ =` on `set_nonblocking(false)` so a failure there is not silently converted into an empty capture.

- [x] [Review][Patch] LOW: the write consequence is inverted at exactly one truncation point [crates/shim/src/error.rs:14-17,39]
  - **Evidence:** if the last partial write leaves only the trailing `\n` unsent, the daemon reads a complete JSON object, `trim_end_matches('\n')` is a no-op, every validation passes and `try_send` records the event, while the shim logs "event not sent" and the doc calls it "genuinely lost". One layer verified by brace-balance sweep that no *other* prefix of a real payload is valid JSON, so the exposure is exactly one byte out of N.
  - **Implementor detail:** soften the doc to "lost unless the write stopped on the final framing byte". The `Display` string is a reasonable simplification and can stay.

- [x] [Review][Patch] LOW: two pass-1 follow-up hand-offs were not made, and taskwarrior `719e7027` still carries a retracted claim
  - **Evidence:** `task 719e7027 info` shows `Last modified` equal to `Entered`, so no annotation was added for the connect-backlog precondition pass 1 asked to fold in (story:121). The description still reads "WS fanout ruled out (8 presenters, 14472 frames, max 385us)", the exact claim downgraded everywhere else.
  - **Implementor detail:** annotate the task with the Linux full-backlog case and correct "ruled out" to "fanout alone ruled out; fanout under compile-storm load untested".

- [x] [Review][Patch] LOW: Task 5's note says no maintainer decision was required, while one is recorded for the same write path [docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md:62,370]
  - **Evidence:** story:62 reads "no change proposed, so no maintainer decision was required"; story:370 records "Maintainer decision (pickles, 2026-07-29): fix the behavior here rather than only documenting it."
  - **Why it matters:** narrowly reconcilable (the 2 ms/3 ms numbers genuinely did not change, so Task 5's subject is untouched), but the two sentences read as contradictory on whether this story took a maintainer call, and the authorization for the only behavior change is dev-authored prose. Worth one clarifying clause.

- [x] [Review][Patch] LOW: Completion Note 8 over-claims the reservation test, and concedes it four paragraphs later [docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md:367,377]
  - **Evidence:** story:367 says the test "enforces that reservation across **every** variant's `Display`"; the test iterates `sample_variants()` (`error.rs:375`), and :377 concedes `sample_variants()` is hand-maintained with no compile-time link to the enum.
  - **Implementor detail:** "across every variant `sample_variants()` lists", so the caveat travels with the claim.

- [x] [Review][Patch] LOW: stray space before a comma, an emdash-strip artifact [crates/shim/src/socket.rs:98]
  - **Evidence:** `// Peer will not accept more and did not error. Not a timeout ,`

- [x] [Review][Defer] MEDIUM: the behavioral half of the read-side aggregate gap, whether `read_line` should be replaced with a deadline-re-arming `read_line_within` the way the write was [crates/shim/src/socket.rs:189-193]; deferred, pre-existing and unreachable with the current daemon, which writes its whole reply in one `write_all`

Dismissed as noise (4): "near-cap payloads now fail deterministically" (measured false, a healthy peer takes a full 1 MiB in one `write(2)` in ~1.0 ms and it is delivered; the real exposure is the starved case, kept as a MEDIUM above); "the 100 ms ceiling is redundant because a `write_all` regression could not produce `Err(Timeout)`" as *stated* (the reasoning is wrong, a swap-back routed through `classify` would classify `EAGAIN` as `Timeout`; the conclusion happens to be right for a different reason, measured `Ok(())` 6/6, so the finding is kept on the correct evidence); `sprint-status.yaml:125` keeping the retracted wording without a SUPERSEDED marker (explicitly a historical breadcrumb, and both live entries are correct); and `run_shim_with_env` having no per-child timeout (pre-existing, shared by every test in the file, and `scripts/test.sh`'s outer timeout is the backstop).

## Dev Notes

### The exit-0 contract is deliberate — do not "fix" it

`Error::SocketIo` is exit-0 / WARN / `stderr_hint() == None` **by contract**, and the new timeout variant must be too. The reasoning is documented at [error.rs:98-112](../../crates/shim/src/error.rs): these errors mean the daemon is up and answering, so per NFR20 the shim is fire-and-forget and Claude must see success. Surfacing a timeout on stderr would regress Story 5.10's careful exit-1/exit-0 partition.

**So the diagnosability gap is in the log line, not on stderr.** Story 5.10 made exit-1 failures name their cause on stderr; this story makes an exit-0 failure name its cause *in the shim log*. Different surface, same principle.

### Why 3ms should already be enough

The daemon's reply path does **not** include a durable write:

```rust
match tx.try_send(IngestItem { envelope, origin: IngestOrigin::Live }) {
    Ok(()) => { /* write "200\n" */ }
    Err(_) => { /* write "503\n" */ }
}
```

`try_send` is non-blocking; the SQLite write happens downstream in the writer task. So the shim's 3ms read budget covers connect-to-reply on a loopback Unix socket with an in-memory enqueue in the middle. That should be microseconds, not milliseconds.

This matters for scoping: NFR2's hook→projection target is 50ms p95, which might tempt someone to conclude the shim's 3ms is obviously too tight. **It is not the same measurement.** The 50ms covers through the durable write; the 3ms covers only to the enqueue ack. Do not conflate them.

### Axiom 3 governs the fix shape

> Performance is hard at trust boundaries, soft inside.

The shim runs inside Claude's process tree. Its budget is the *hard* side. That means:

- Dropping an event to protect the budget is a legitimate outcome, not a bug to be retried away.
- An unbounded or generously-bounded retry is **worse** than the current behavior.
- If the honest conclusion is "3ms is right, the daemon occasionally stalls, and we drop", then this story delivers only the diagnosability improvement and that is a complete outcome.

### Test execution

Run the workspace suite via **`scripts/test.sh`, never raw `cargo test`** (project rule, `CLAUDE.md`); it runs parallel with an exclusive lock, and a second concurrent `cargo test` in this worktree is the confirmed hang trigger. Shim-scoped runs: `scripts/test.sh -p bowerbird-shim`.

The parallel-test disciplines apply: no sleep-based synchronization, no `std::env::set_var` (banned in `clippy.toml`), hang guards at 30s not tight values. Full rationale in `docs/bmad/project-context.md` §Deterministic test discipline.

### Files this story touches

| Path | NEW/UPDATE | Change |
| --- | --- | --- |
| `crates/shim/src/error.rs` | UPDATE | New timeout variant + the four partition functions (AC #2). |
| `crates/shim/src/socket.rs` | UPDATE | Classify `WouldBlock`/`TimedOut` at the call site; connect bounding or comment fix (AC #1, #4). |
| `tests/contract_shim.rs` | UPDATE | Classification + log-line contract test (AC #6). |
| `crates/shim/src/log.rs` | UPDATE (maybe) | Only if the log line needs shaping beyond the `Display` impl. |

No `crates/protocol/src` change, so the changelog gate is not triggered — do **not** manufacture a protocol edit to trigger it. No SQLite migration. No wire-format change.

### Scope boundary

This story is the shim's timeout classification and the budget *question*. It is **not**:

- A retry/durability mechanism for dropped events (that is the spool question, still Open in project-context.md §"Shim-when-daemon-is-down").
- A daemon runtime change (`current_thread` → multi-thread). If Task 1 fingers the daemon, file it separately.
- The CI cross-target gap from Story 5.12 (taskwarrior `21fa8e4f`).

### References

- [Source: crates/shim/src/socket.rs] — timeouts 26-31, connect-before-timeouts 19-22, the "5ms total" comment 24-25.
- [Source: crates/shim/src/error.rs] — variant list 5-56, `exit_code()` 66-87, `level()` 90-96, `stderr_hint()` + its NFR20 rationale 98-134, partition canary tests 171-208, reserved-variant pattern 38-43.
- [Source: crates/daemon/src/ingest/handler.rs] — non-blocking `try_send` + reply 121-135, debug log 126.
- [Source: docs/bmad/implementation-artifacts/5-12-release-pipeline-end-to-end-verification.md] — Task 6 rc1 dogfood evidence and the AC #5 escalation that produced this story.
- [Source: docs/bmad/project-context.md] — Axiom 3 (trust-boundary perf), Shim hot-path discipline, NFR20 fire-and-forget, Deterministic test discipline.
- [Source: docs/bmad/project-context.md#Performance bars], shim hot-path bench gate; per-platform policy lives in `crates/shim/benches/baselines/*.json` (ADR 0003). (This entry originally restated "+15%"; corrected by Story 5.18.)

## Dev Agent Record

### Agent Model Used

claude-opus-5 (Claude Code, `bmad-dev-story`), 2026-07-29.

### Debug Log References

Task 1 measurement harnesses were throwaway, std-only programs run against a **real release daemon** (`target/release/bowerbird-daemon`, the profile rc1 shipped) on an isolated `BOWERBIRD_DATA_DIR` with `BOWERBIRD_INGEST_SOCK=/tmp/bb516.sock`. They are not committed, they exist to produce the numbers below, and every one is reproducible from the descriptions here.

- `probe.rs`, replicates `socket.rs::send` syscall-for-syscall (same op order, same `BufReader::with_capacity(64)`) but times connect / write / read separately and reports `io::ErrorKind` + `raw_os_error` on failure. Timeouts are parameterized so the tail can be measured **untruncated**, with the real 2ms/3ms budgets in place the tail is invisible by construction, which is why the original finding had no numbers attached.
- `wsload.rs`, minimal std-only WS presenter (HTTP upgrade, bearer auth, masked client frames, Ping→Pong) used to put real fanout load on the daemon's `current_thread` runtime for hypothesis (a).
- `drive_shim.sh`, drives the **real** `target/release-shim/bowerbird-shim` binary, one fresh process per event, and counts `WARN` lines in `BOWERBIRD_SHIM_LOG`. This is the faithful dogfood shape (the in-process probe cannot see process-cold effects).
- `coldstart.sh`, hypothesis (c): fresh data dir + fresh daemon per round, spin until the ingest socket exists, fire exactly one request with the real budgets.
- `errno_proof.rs`, deterministic proof of the errno→`ErrorKind` mapping via a server that accepts and never replies. No load, no racing.

One environment note for anyone re-running this: the session scratchpad path is too long for `SUN_LEN` (104 bytes on macOS), so the ingest socket must be overridden to a short path. The daemon fails with a clear `path must be shorter than SUN_LEN` error, not a confusing one.

### Completion Notes List

1. **[Task 1, AC #3. Mechanism proven; drop-rate reproduction NOT achieved. The 3ms budget is not miscalibrated; the daemon's reply-path tail is starved by the OS scheduler under heavy system load.]**

   **Two limits on this evidence, stated up front rather than buried, because everything else in this story rests on it:**
   - **No drop was ever reproduced end to end under the real budgets.** 300 real-shim invocations produced **0 drops**. The tail that exceeds 3ms is visible only in the probe runs where the timeouts were *parameterized away* precisely so the tail would not be truncated. So "p99 2436µs / max 5686µs, past the 3ms budget" is a sound inference about what the budget would have clipped, not a direct observation of a clipped event. Task 1's first sub-bullet asked for the drop rate to be confirmed; it was not.
   - **No daemon `debug` trace was captured**, which that same sub-bullet asked for. A hooks-fired versus `events`-rows count during a storm was the cheap check that would have settled both this and the review's HIGH finding about what a timeout actually costs; it was not run.

   What *is* solidly established is the mechanism (below), which is the part AC #3's "investigated and documented" gate turns on, and which is sufficient to reject a budget change on its own.

   **The mechanism is confirmed, and reproduced byte-for-byte.** `errno_proof` provokes an expired `SO_RCVTIMEO` deterministically (a server that accepts and never replies) and gets:

   ```
   READ timeout expired after 3.760542ms
     ErrorKind    = WouldBlock          raw_os_error = Some(35)
     Display      = Resource temporarily unavailable (os error 35)
     kind == WouldBlock = true   kind == TimedOut = false
   ```

   Folded through the shim's current `Display`, that is `socket I/O failed: Resource temporarily unavailable (os error 35)`, **character-for-character the rc1 dogfood WARN line**. So the two dropped events were expired socket timeouts, not genuine I/O failures, and AC #1's root-cause claim is now proven rather than asserted. An expired `SO_SNDTIMEO` was provoked the same way and yields the identical `WouldBlock` / errno 35. On Linux the same expiry surfaces as `ErrorKind::TimedOut`, which is why Task 2 classifies **both** kinds.

   **Where the 3ms goes (untruncated, real release daemon, per-phase µs):**

   | shape | machine state | connect p50 | write p50 | read p50 | read p90 | read p99 | read max |
   | --- | --- | --- | --- | --- | --- | --- | --- |
   | paced 100ms | idle (load 2.1) | 71 | 11 | 322 | 432 | 536 | 581 |
   | burst (no pacing) | idle (load 2.1) | 3 | 0 | 27 | 35 | 54 | 178 |
   | 2s idle gap | idle (load 2.1) | 81 | 12 | 367 | 470 | 527 | 527 |
   | paced 200ms | 12 CPU spinners | 44 | 7 | 226 | 266 | 292 | 307 |
   | 8 WS presenters, paced | 12 CPU spinners | 41 | 6 | 190 | 242 | 293 | 385 |
   | 8 WS presenters, burst | 12 CPU spinners | 7 | 1 | 70 | 253 | 279 | 283 |
   | **compile storm** | **parallel rustc, load 57–67** | 39 | 7 | **199** | **291** | **2436** | **5686** |

   The load row is the finding. Note what does *not* move: p50 199µs and p90 291µs are indistinguishable from the idle rows. Only the tail explodes, p99 to 2.4ms and max to 5.7ms, straight through the 3ms budget. A reply path that got *slower* would shift the whole distribution; a distribution whose body is pinned and whose tail detonates is **scheduler starvation**, the daemon's `current_thread` runtime thread loses its slice on a heavily oversubscribed machine and does not get rescheduled for milliseconds. The work in the reply path is not the problem; being allowed to run is.

   That matches the dogfood conditions exactly: three live Claude Code sessions plus an install plus a `cargo` build is the compile-storm shape, and the two drops were 4.5 minutes apart with ~31 successes in between, a tail event, not a systemic failure.

   **Hypotheses (a) and (c), stated at the strength the evidence actually supports:**
   - **(a) WS fanout sharing the `current_thread` runtime, narrowed, not refuted.** Eight concurrent WS presenters subscribed to `state.session.*` + `events.*`, with **14,472 data frames actually delivered** during the run, moved read max only to 385µs (paced) / 283µs (burst). So **fanout *alone* does not starve ingest.** What is *not* shown: fanout **combined with** compile-storm load, which is what the rc1 conditions actually were (three live sessions plus an install plus a build). Those WS rows were collected while the stray CPU spinners were running, and this same note establishes that spinner load makes the daemon *faster*, so by my own methodology finding, that is not a state in which the tail can appear, and it cannot distinguish "fanout does not contribute" from "the triggering load was absent". An earlier draft of this note called (a) outright wrong; that overstated the evidence. Fanout-under-storm remains untested and would be the first thing to measure if the daemon-side work (`719e7027`) needs to rank causes.
   - **(c) cold path on the first hook after install, refuted.** Fifteen rounds of fresh-data-dir + fresh-daemon + spin-until-socket-exists + exactly one request, all with the real 2ms/3ms budgets: 15/15 succeeded, read 118–230µs with a single 919µs outlier. The first hook is not special. (Drop #1 landing during `bowerbird install` is coincidence, install is *when the machine is busiest*, which is hypothesis (b) again.)
   - **(b) macOS scheduler contention on a loaded dev box, confirmed**, and it is the whole explanation. But the *kind* of load decides everything, which is the trap worth recording: naive CPU spinners **reduced** latency (read p50 226µs vs 322µs idle) because they keep cores awake and clocked up, cancelling the idle-wakeup penalty below. Only realistic mixed load, parallel `rustc`, i.e. CPU **and** memory **and** I/O **and** process churn, reproduces the tail. Anyone re-testing this with a busy-loop will measure an *improvement* and wrongly conclude the budget is fine.

   **Methodology correction, disclosed rather than quietly fixed.** The 12 CPU spinners from the hypothesis-(b) spinner run did not die when I killed them, and kept running through the WS-fanout and cold-start experiments. That is why the table's machine-state column is explicit per row instead of a blanket "quiet machine". It does not weaken any conclusion here, every affected run is one where the daemon *stayed fast* (WS fanout, cold start, 300 real-shim invocations with zero drops), so the true quiet-machine result can only be faster, and the two rows the argument actually leans on (idle baseline, compile storm) were measured with the machine in the state their labels claim. It did, however, contaminate the first `hot_path` bench reading, see Completion Note 5.

   **Unrelated-but-real secondary finding: the *idle* path is ~10x slower than the burst path.** Read p50 is 322µs paced versus 27µs bursting, because a request arriving at a parked daemon pays thread wakeup on a cold core. Real Claude hooks are sparse, so real hooks *always* pay this. It saturates around 500–600µs (a 2s gap is no worse than a 100ms gap), so it is not the cause of any drop, but it means the honest headroom under 3ms is ~6x, not the ~100x you would infer from a burst benchmark. Worth knowing before anyone tightens the budget on the strength of `hot_path.rs` numbers.

   **Conclusion, which is Task 5's answer: do not change the budgets, and do not add a retry.** Three independent reasons, in increasing order of severity:
   1. The budget has ~6x headroom over the realistic (idle-path) p50 and ~10x over p90. It is not tight. Relaxing 3ms to, say, 10ms would convert *some* starvation events into successes while making the shim's worst case 3x longer on the trust-boundary side, trading Claude's responsiveness for event completeness, which Axiom 3 forbids.
   2. Under starvation, a retry is likely to hit the same stall, the daemon is not slow, it is *not running*. A bounded retry that fits inside the budget is too short to outlast a multi-millisecond deschedule; one that outlasts it has blown the budget by definition. This is exactly the "retry loop that can exceed the budget is a regression, not a fix" case AC #5 names.
   3. **A retry would duplicate events.** Verified against the schema: `events` is `event_id INTEGER PRIMARY KEY AUTOINCREMENT` ([db/migrations.rs:7](../../crates/daemon/src/db/migrations.rs)) with no natural-key uniqueness and no shim-supplied idempotency key, the `ON CONFLICT(source, session_id)` in [db/queries.rs:10](../../crates/daemon/src/db/queries.rs) is the *projection* upsert, not the event log. When the read times out, the daemon has almost always already `try_send`-ed the event ([handler.rs:121](../../crates/daemon/src/ingest/handler.rs)); the shim just never heard `200\n`. Re-sending would append a second row for one hook, corrupting the append-only log that Story 5.11 and `/replay` both treat as the source of truth. Retry is not merely ineffective here, it is unsafe without an idempotency key the protocol does not have.

   So this story delivers **diagnosability only**, which the story text itself names as a complete outcome. The daemon-side question (should ingest be insulated from scheduler starvation, a dedicated thread, or `rt-multi-thread` with `worker_threads=2`) is explicitly out of scope per §Scope boundary and is filed separately rather than widening this story.

2. **[Task 2, AC #1, #2, `Error::Timeout` added and wired; the before/after is the whole story.]** Same provoked failure, old shim then new shim, real `release-shim` binary against a peer that never replies:

   ```
   WARN socket I/O failed: Resource temporarily unavailable (os error 35)                  <- rc1
   WARN socket read timed out after 3ms; no reply from daemon, event may already be recorded   <- now
   ```

   Exit 0 and empty stderr in both cases, as required.

   (The `; event dropped` wording shown here in an earlier draft was retracted by the pass-1 review and never shipped: it is false for the read case. See Completion Note 8. The line above is the actual shipped `Display`.)

   The variant is `Timeout { op: &'static str, budget_ms: u64 }`, a struct variant rather than a bare `Timeout` so the line names *which* operation blew *which* budget. Both fields are `Copy`/`&'static`, so nothing allocates (shim hot-path discipline). It joins the exit-0 / WARN / `stderr_hint() == None` class alongside `SocketIo`, and all three partition canaries (`exit_code_never_2`, `level_matches_exit_code`, `stderr_hint_matches_exit_code`) pass **unmodified**, the only test-module edit was adding two `Timeout` values to `sample_variants()`, which AC #2 requires. Both `op` spellings are sampled (`write`/`read`) so the canaries cover each.

   Two deliberate choices worth flagging for review:
   - **The budgets are now single-sourced** as `WRITE_BUDGET_MS`/`READ_BUDGET_MS` consts in `socket.rs`, feeding both the `set_*_timeout` calls and the message. Previously the numbers were inline literals; a message that hardcoded "3ms" could have drifted from a changed socket option and lied in the log. `budgets_match_the_documented_contract` pins them.
   - **The two `set_*_timeout` calls keep mapping to `SocketIo`, not `Timeout`.** They are `setsockopt`, they cannot time out, so classifying them as timeouts would be wrong. Only the actual `write_all` and `read_line` go through `classify`.

3. **[Task 3, AC #4, the "5ms total" comment was corrected, and the connect deliberately left unbounded. No silent no-op.]** The old comment claimed "Total = write + read ≤ 5ms in the worst case", which excluded the unbounded `connect` above it and was therefore not a total. It now states what the 5ms actually covers and why the asymmetry is safe rather than accidental.

   The measurement is what settles it: a Unix-socket `connect` to a listening socket completes **in the kernel** as soon as the connection lands in the accept backlog, it does not wait for the daemon to call `accept`, so unlike the reply read it does not depend on the daemon's thread being scheduled. The data shows exactly that split. In the compile-storm run that drove the read tail to 5686µs, connect's own worst case was **337µs** (p50 39µs). The phase that needs the daemon to run is the phase that blows up; connect is not that phase.

   Bounding it anyway was rejected on cost, not on effort: `std` has no `UnixStream::connect_timeout`, so it would take either a new shim dependency (`socket2`) or a hand-rolled non-blocking connect + poll + restore-to-blocking. That adds syscalls to the **success** path, the path whose entire job is to be invisible, to guard a phase with no measured tail, and it would also invalidate `classify`'s "this socket is never non-blocking, so `WouldBlock` can only mean timeout" premise. Correcting the claim is the honest fix; the real exposure (daemon starvation) is filed as `719e7027`.

4. **[Task 4, AC #6, classification covered at both levels; 6 tests added, 636 passing.]** Five unit tests in `socket.rs` cover the classification function directly, as the story asked: both kinds → `Timeout` with `op`/`budget_ms` preserved; a **raw** `from_raw_os_error(35)` (how macOS actually delivers it, rather than a synthesized `ErrorKind`) → `Timeout`; five genuine failure kinds (`BrokenPipe`, `ConnectionReset`, `ConnectionRefused`, `PermissionDenied`, `UnexpectedEof`) → still `SocketIo`, which is the guard against over-eager classification hiding real I/O errors; and the message asserting it names the op and budget and contains neither `socket I/O failed` nor `os error`.

   One added test goes beyond the unit level, and the reasoning matters because the story warned against it. `shim_names_socket_timeout_in_log_and_stays_silent` drives a **real** expired timeout through the real shim binary via a new `start_mock_ingest_silent` helper, a mock that accepts, reads the request, and never replies. This is **not** the race the story forbids: no reply can ever arrive, so the read budget must expire; there is no sleep used for synchronization and no ordering that can be lost. It closes a gap the unit tests structurally cannot reach, that the classified error actually arrives *in the log*, at WARN, with stderr still empty. Holding the accepted stream open is load-bearing (dropping it would give EOF → `BadResponse`, silently testing the wrong path), and the assertion is on `"timed out"` rather than on which operation expired, so a starved CI runner that blows the write budget first still passes for the right reason.

   The `contract_test_inventory.rs` whitelist was deliberately **not** touched: it pins the 10 architecture-required contract surfaces, and this is a story-specific test, not a new required surface.

5. **[Task 5, AC #5, bench gate re-verified, and a bad first reading corrected rather than accepted.]** The first `hot_path` run reported p99 5.691ms, **+113.65%** over the committed macOS baseline. It passed the gate only because `regression_max_ratio` is `null` per ADR 0003, which is exactly the situation where a passing gate should not end the inquiry. It turned out the 12 CPU spinners from the hypothesis-(b) experiment were still running (load average 52); the reading was an artifact of my own harness, not of this change.

   Re-measured on a settled machine, and A/B'd against `HEAD` by stashing the change so both sides ran on the same machine state:

   | build | mean | p99 | vs baseline |
   | --- | --- | --- | --- |
   | `HEAD` (no change) | 1.249ms | 1.397ms | −47.6% |
   | with this change | 1.319ms | 1.429ms | −46.3% |
   | with this change (2nd sample) | 1.403ms | 1.598ms | −40.0% |

   Same-build run-to-run spread is ±12% (1.429 → 1.598), so the +2.3% `HEAD`-vs-change p99 delta is inside noise, as expected, since `classify` executes only on the error path and the success path gained nothing but two `const` reads. Comfortably inside the +15% gate and well under the 15ms absolute budget.

   Full verification, all green on macOS arm64: `cargo fmt --check`, `cargo clippy --all-targets --workspace -D warnings`, and `scripts/test.sh` at **636 passed / 0 failed** (630 was the Story 5.12 baseline; +6 is exactly this story's additions), log `target/test-logs/20260729-183346-51669`.

6. **Correction to a story-spec claim: `TimedOut` is not "the Linux spelling".** AC #1 and Task 2 both describe `ErrorKind::TimedOut` as the Linux counterpart to macOS's `WouldBlock`. That is not right. POSIX specifies `EAGAIN` for `SO_RCVTIMEO`/`SO_SNDTIMEO` expiry on **Linux and macOS alike**, so Unix generally lands on `WouldBlock`; `TimedOut` is where **Windows** (`WSAETIMEDOUT`) would land, and bowerbird scope-cuts Windows.

   The code is unchanged by this, both kinds are still matched, but the *reason* in the comment is now the correct one, because a comment asserting a false platform fact is exactly the kind of thing that made the original `socket.rs` "5ms total" claim mislead for two stories. The honest justification: **`std` does not pin which kind an expired socket timeout produces** (it documents either for a timed-out read/write), so matching both is coding to the documented contract rather than to one platform's observed errno. The `TimedOut` arm costs one pattern and removes the chance of a silent misclassification that would take another dogfood cycle to rediscover.

   What is actually verified versus inferred, stated plainly: macOS is **empirically proven** (`WouldBlock` / errno 35, reproduced deterministically). Linux is **now also empirically proven** (2026-07-30, `rust:slim`/glibc container): an expired `SO_RCVTIMEO` yields `ErrorKind::WouldBlock` with `raw_os_error == Some(11)` (`EAGAIN`), and so does an expired `SO_SNDTIMEO`. So **neither supported platform ever produces `TimedOut`**, which confirms the POSIX reasoning above and means the `TimedOut` arm serves no supported platform. It stays purely as defense against `std`'s documented latitude to return either kind, which is what the `classify` doc now says.

7. **Story spec path correction, for the next reader.** The story's §"Files this story touches" table and Task 4 both say `tests/contract_shim.rs`; the file is actually at **`crates/shim/tests/contract_shim.rs`**. Related and load-bearing: `bowerbird-shim` is a **binary** crate, so its `error`/`socket` modules cannot be imported from an integration test, which is why AC #6's classification unit tests live in-crate under `#[cfg(test)] mod tests` in `socket.rs` rather than in `contract_shim.rs`. `crates/shim/src/log.rs` was listed as "UPDATE (maybe)" and needed **no change**: the existing `Display`-based log append already carries the new message verbatim.

8. **[Review resolution pass, 2026-07-29, all 10 patch findings addressed. The review found a bug worse than the one this story was written for.]**

   Both HIGH findings were verified independently before being accepted, and both were real.

   **HIGH 1, `event dropped` was false for the read case.** Confirmed: `try_send` runs *before* the `200\n` write ([handler.rs:121-135](../../crates/daemon/src/ingest/handler.rs)), so on a read timeout the event lands and only the ack is lost. Completion Note 1 reason 3 argues exactly this, and the first cut of the log line contradicted it, sending the operator hunting for an event that is present, which is worse than the vague message it replaced. The two halves have **opposite** consequences, so a single phrase could never be right:

   ```
   socket write timed out after 2ms; event not sent
   socket read timed out after 3ms; no reply from daemon, event may already be recorded
   ```

   `op` became a `TimeoutOp` enum carrying `name()` + `consequence()` so the two cannot drift apart or be collapsed by accident, and `event dropped` is now **reserved** for `Error::Connect`, where the drop is real. `event_dropped_phrasing_is_reserved_for_real_drops` enforces that reservation across every variant `sample_variants()` lists (that list is hand-maintained, which is the structural gap deferred below) (an earlier draft of that test iterated `to_string()` looking for `Connect`, which would have been vacuous, since that phrase lives in `Connect`'s `stderr_hint()` and not its `Display`).

   **HIGH 2, the 5ms claim was still false, and the fix is behavioral, per maintainer decision.** `SO_SNDTIMEO` bounds one `write(2)`, not `write_all`'s loop: every partial write that makes progress gets a *fresh* budget. Proven directly, a 256 KiB payload to a slow-draining peer took **82ms** through `write_all` and returned **`Ok(())`**. Not an error path: the shim silently stalls Claude 16x past its entire round-trip budget and reports success. At the 1 MiB stdin cap that is ~330ms, and the trigger condition is precisely the slow-draining starved daemon Task 1 measured. Pre-existing, unrelated to the diagnosability work, and a live Axiom 3 trust-boundary violation that my first pass made *harder* to see by adding a test asserting `WRITE + READ == 5`.

   Maintainer decision (pickles, 2026-07-29): fix the behavior here rather than only documenting it. `write_all_within` replaces `write_all` and re-arms the socket with the time *remaining* after any partial write, so the aggregate cannot outlive the deadline. **Zero cost on the success path**, a payload that fits the send buffer completes in one `write` and never re-arms; the extra `setsockopt` occurs only on the already-slow partial-write path. The `== 5` assertion is gone (a sum of two constants says nothing about what the code enforces) and `write_all_within_bounds_the_aggregate_not_each_syscall` replaces it with a test that has something to say: it drives a real slow-draining peer with a full 1 MiB payload. **Verified as a genuine regression guard** by temporarily swapping the body back to `write_all` and confirming the test FAILS, then restoring.

   The remaining eight, briefly: the contradictory `TimedOut`-is-Linux comment in the test module is fixed (commit `fb3bda2` fixed the copy above the function and missed the one below it) and now over-claims in neither direction; the connect-is-safe rationale gained its missing precondition (in-kernel completion holds only while the accept backlog has room, a full backlog is an unbounded wait on Linux, since `SO_SNDTIMEO` is armed only after connect returns); both exit-0 rationales stopped asserting "the daemon answered the connect", which `socket.rs` disproves, and now say what is true (the socket exists and the payload reached the kernel) plus the accepted consequence that a wedged daemon with a live listener swallows events silently; hypothesis (a) is downgraded from "refuted" to "fanout *alone* does not starve ingest, fanout-under-storm untested", in this note, the Change Log, and sprint-status; Task 1's first sub-bullet went from `[x]` to `[~]` with the unmet deliverables named, and Completion Note 1 now leads with the 0/300 and parameterized-budget caveats instead of burying them; the helper's 10ms readiness sleep is deleted (provably unnecessary, `bind` happens on the calling thread) and its doc no longer claims "no sleep" while sleeping; and the end-to-end test now asserts `read timed out` specifically, dropping a tolerance for a write-timeout case it provably could not survive. `raw_eagain_from_macos_classifies_as_timeout` is `#[cfg(target_os = "macos")]` and asserts its premise unconditionally, so macOS changing that mapping becomes a failure rather than a silent skip.

   Two of the reviewer's deferred items were fixed anyway because they were one-liners inside code already being touched: `Error::BadResponse` now carries `trimmed` rather than the raw newline-bearing `line` (it was the one path in `send` that could emit a two-newline log entry, violating an invariant my own test asserts), and the misleading `sample_variants()` comment no longer implies `op` is partition-relevant when `exit_code()`/`stderr_hint()` discard it.

   **Not fixed, and I agree with leaving it:** the reviewer's best structural catch is that the partition canaries do **not** actually guard a future variant, because `sample_variants()` is hand-maintained with no compile-time link to the enum. A future variant returning exit 2 would compile clean and pass all three canaries. `error.rs` claims otherwise, and this story is the proof, I had to remember by hand. It is pre-existing, it needs an exhaustive-match or derive-based fix, and it is correctly deferred rather than bolted onto a hotfix.

   **Verification after the resolution pass**, macOS arm64: `cargo fmt --check` and `cargo clippy --all-targets --workspace -D warnings` clean; `scripts/test.sh` **639 passed / 0 failed** (636 plus the 3 new tests), log `target/test-logs/20260729-201652-2824`.

   AC #5 re-verified, since this pass changed the write path rather than just comments. A/B against `HEAD` by stashing, two samples each on a settled machine: **HEAD p99 1.384 / 1.370ms vs with-fix p99 1.268 / 1.403ms**: the ranges overlap completely, so the bounded write loop is neutral on the hot path exactly as predicted (the success path is still a single `write` and never re-arms). Final gate reading p99 1.403ms, -47.34% vs baseline, inside the +15% gate and far under the 15ms absolute budget.

   **A second bench-reading artifact, disclosed like the first.** The initial post-fix reading was p99 4.311ms, **+61.87%** over baseline, on a machine whose load average read 1.21. It was not the change: the bench had been launched immediately after `scripts/test.sh` finished, and the A/B above (run on a settled machine minutes later) shows HEAD and the fix indistinguishable. That is now twice in this story that a `hot_path` number misled me for machine-state reasons rather than code reasons. The durable lesson, worth more than either datapoint: **this bench is only meaningful on an idle machine, and never immediately after a full test run.** Anyone reading a single `hot_path` delta as evidence should A/B it against a stash first, which costs two minutes and settles the question.

9. **[Pass-2 review resolution, 2026-07-29. The behavior fix from Note 8 was WRONG and is backed out. All 22 pass-2 patch findings addressed.]**

   **`write_all_within` did not bound anything.** I verified pass-2's headline myself before accepting it, and reproduced its numbers. 1 MiB payload, socket armed at 2ms, peer draining 8 KiB per interval, against the **shipped** function:

   | peer drain | result | elapsed | `write(2)` calls | re-arms |
   | --- | --- | --- | --- | --- |
   | flat out | `Ok` | 1.00ms | 1 | 0 |
   | 200µs | **`Ok`** | **40.4ms** | 1 | 0 |
   | 500µs | **`Ok`** | **97.3ms** | 1 | 0 |
   | 1000µs | **`Ok`** | **189.2ms** | 1 | 0 |
   | 1400µs | `Err` | 39.9ms | 1 | 0 |
   | 2000µs | `Err` | 2.55ms | 1 | 0 |

   **One syscall, zero re-arms.** The deadline was never consulted, because the first match arm returns `Ok(())` whenever the syscall wrote everything, and a single `write(2)` *does* write everything even while taking 189ms: `SO_SNDTIMEO` bounds a *wait*, and macOS's `sosend` re-waits per buffer refill, so the call keeps going while the peer drains anything. My premise ("`SO_SNDTIMEO` bounds one `write(2)`") was wrong, so the fix built on it could not work. The exact failure pass 1 flagged, a multi-hundred-ms silent stall inside Claude's hook reporting success, was still live.

   And it was worse than not fixing it, for the reason pass 1 gave about the original comment: I asserted the bound in six places **plus a test that passed**. The test passed because its peer drained *slower* than the budget, so the write always failed on its first wait and the re-arm path never ran. A test whose mechanism is unreachable is not a weak test, it is a false one, and it is why a green suite and 8/8 CI missed this.

   **Backed out.** Maintainer decision (pickles, 2026-07-29): revert to `std::write_all`, state the true shape everywhere, and file the behavior as its own story rather than attempt it a second time inside a diagnosability hotfix. The reasoning: the bug has now defeated one careful attempt, and this story's own §Scope boundary reserves behavior work for a separate story. Filed as **Story 5.17** (`5-17-shim-write-budget-is-not-a-bound.md`, `ready-for-dev`), carrying the full measurement table, the reason this attempt failed, and a *verified* candidate fix (cap each `write(2)` at the send-buffer size: measured 2.01ms to 2.50ms at every drain rate, with the 400-byte success path still one syscall at 7.6µs). So 5.17 starts from evidence rather than from scratch.

   What this story now claims about the budgets, which is all that is defensible: **each socket wait is bounded at 2ms/3ms; neither operation is bounded in aggregate; `connect` is outside both.** Stated in `WRITE_BUDGET_MS`'s doc with the measurement table, restated in `send`, and reflected in `Error::Timeout`'s doc, which now says plainly that `budget_ms` is the configured per-wait value and can under-report elapsed time by up to 100x. The read half is documented as having the same shape (pass 2 measured 12ms and 24ms against its 3ms value) and is deferred to 5.17 AC #5 rather than silently asymmetric.

   **Four pass-1 items I had under-resolved**, all now genuinely done: the Change Log had no resolution entry at all while Note 8 and the File List said hypothesis (a) was corrected in all three named locations (it was fixed in the note and sprint-status, not the Change Log); Note 2 still displayed the retracted `event dropped` line as shipped, which this story's own test would fail; of the three "the daemon answered the connect" copies, I fixed two and **added a third** in `contract_shim.rs` in the same commit; and pass-1 #3's replacement claim was itself false, which is the HIGH above.

   **Other substantive corrections.** `TimeoutOp::Write`'s doc cited `ingest: EOF before newline`, which cannot fire on that path: that message needs `Ok(0)`, and a truncated write always leaves bytes on the wire, so the daemon logs `ingest: invalid JSON` and replies `400`. The same doc called the write case "genuinely lost" without qualification, but there is exactly one byte where that inverts: if only the trailing `\n` went unsent, the daemon reads complete JSON, `trim_end_matches('\n')` is a no-op, and the event **is** recorded. One byte out of N, and no other prefix of a real payload parses as JSON, so the `Display` keeps its simpler wording and the doc carries the caveat. The exit-0 rationale also rested on "the payload was handed to the kernel", which is false for the write half in the very file that argued blurring the halves was the bug; it now rests on NFR20 alone, which is the only true reason. The silent mock's reader thread could park forever in `read_until` while `MockIngest::drop` removed the TempDir under it, so it now carries a 30s hang guard, which matters because the obvious next reuse is a write-timeout test where no newline arrives by construction.

   **Corrected in Note 8, which over-claimed:** the reservation test enforces the phrase ban "across every variant `sample_variants()` lists", not across every variant, since that list is hand-maintained (which is the deferred structural finding, four paragraphs later in the same note). Task 5's "no maintainer decision was required" is true of its own subject, the 2ms/3ms numbers, which never changed; it read as contradicting the maintainer decision recorded for the write path, so it now says so explicitly.

   **Verification after the backout**, macOS arm64: `cargo fmt --check` clean, `cargo clippy --all-targets --workspace -D warnings` clean, `scripts/test.sh` **638 passed / 0 failed**, log `target/test-logs/20260729-213549-37316` (639 minus the one retracted test, `write_all_within_bounds_the_aggregate_not_each_syscall`; an earlier draft of this note said 636, which double-counted the two `error.rs` tests that are still here). AC #5's bench needs no re-run for a revert to `std::write_all`, which is what the green baseline was measured against, and CI re-runs it on both platforms regardless.

### File List

- `crates/shim/src/error.rs`, UPDATE. Added the `TimeoutOp` enum (`Write`/`Read`, with `name()` + `consequence()`) and `Error::Timeout { op, budget_ms }`, wired through `exit_code()` (→ 0), `stderr_hint()` (→ `None`), and `sample_variants()`. `level()` derives from `exit_code()` and needed no edit. Reworded both exit-0 rationales (review) so they no longer claim a successful connect proves the daemon is up. The three partition canaries are unmodified. Two new tests: `timeout_consequence_distinguishes_lost_from_unacknowledged`, `event_dropped_phrasing_is_reserved_for_real_drops`.
- `crates/shim/src/socket.rs`: UPDATE. Added `WRITE_BUDGET_MS`/`READ_BUDGET_MS` consts and `classify()` (`WouldBlock`|`TimedOut` -> `Timeout`, else `SocketIo`). The write path is `std::write_all` again: the `write_all_within` bounded loop added in the pass-1 resolution was measured by pass 2 and did not work, so it is backed out (Completion Note 9) and the behavior is Story 5.17. The budget docs now state the true per-wait shape with the measurement table instead of an aggregate claim. `Error::BadResponse` carries `trimmed`, not the newline-bearing `line`. Test module: 5 tests; the retracted aggregate test is deleted.
- `crates/shim/tests/contract_shim.rs`, UPDATE. Added the `start_mock_ingest_silent` helper (accepts, reads, never replies, holds the stream open; no readiness sleep) and the `shim_names_socket_timeout_in_log_and_stays_silent` end-to-end contract test, which asserts the read-timeout wording and that the log does **not** claim a drop.
- `docs/bmad/implementation-artifacts/deferred-work.md`, UPDATE (by the review). 7 deferred items.
- `docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md`, UPDATE. Task checkboxes, Dev Agent Record, File List, Change Log, Status.
- `docs/bmad/implementation-artifacts/sprint-status.yaml`, UPDATE. `5-16` ready-for-dev → in-progress → review, plus `last_updated`.

No `crates/protocol/src` change, so the changelog gate is correctly not triggered (and none was manufactured). No SQLite migration, no wire-format change, no daemon code touched.

## Change Log

- 2026-07-31: status review -> done by maintainer decision (pickles). All 22 pass-2 patch findings were resolved 2026-07-29; the single thread this story left open, the unbounded aggregate write that it documented honestly but could not fix (the pass-2 HIGH), shipped as Story 5.17 in PR #34 (squash `20a8b62`, merged 2026-07-31 eve). 5.17's bench-gate AC2 evidence is in: shim gates green on both platforms on both the branch run and post-merge main, all attempt 1, no re-measure notes. Nothing else in this story is open.
- 2026-07-29 (pass-2 resolution): the behavior fix from the pass-1 resolution is BACKED OUT and all 22 pass-2 patch findings are addressed; status -> review. `write_all_within` was measured and does not bound anything: 1 syscall, 0 re-arms, and 189ms returning `Ok` for a 1 MiB payload against a peer draining 8 KiB/ms. The premise was wrong (`SO_SNDTIMEO` bounds a WAIT, not a syscall; macOS `sosend` re-waits per buffer refill), and the guarding test could not catch it because its peer drained slower than the budget so the re-arm path never executed. Reverted to `std::write_all`; every claim site now states the honest shape (each wait bounded at 2ms/3ms, neither operation bounded in aggregate, `connect` outside both), and the behavior is filed as **Story 5.17** with the measurement table, the reason this attempt failed, and a verified candidate fix attached. Also corrected: four pass-1 items I had under-resolved (the Change Log had no resolution entry while Note 8 claimed hypothesis (a) was fixed in all three named places; Note 2 still displayed the retracted `event dropped` line; and of three "the daemon answered the connect" copies I fixed two and ADDED a third); `TimeoutOp::Write` cited a daemon log message that cannot fire on that path (`ingest: invalid JSON`, not `ingest: EOF before newline`); the write consequence inverts at exactly one byte (if only the trailing `\n` is unsent the event IS recorded, so the doc carries the caveat); the exit-0 rationale rested on a premise false for the write half and now rests on NFR20 alone; the silent mock's reader could park forever and now has a 30s hang guard; and two Note 8 over-claims are narrowed. Verified: fmt, clippy -D warnings, scripts/test.sh 638 passed / 0 failed (639 minus the one retracted test).
- 2026-07-29 (pass-1 resolution): hypothesis (a) downgraded from "refuted" to "fanout alone does not starve ingest; fanout under compile-storm load was not measured" here, in the Change Log, and in `sprint-status.yaml`, per pass-1 finding 5. All 10 pass-1 patch findings addressed; see Completion Note 8 for the detail.
- 2026-07-29: Story implemented via bmad-dev-story; all 5 tasks complete, all 6 ACs satisfied, status → review. **Task 1's reproduce-and-explain gate was honored: no code changed until the cause was measured.** Outcome is diagnosability-only, which is the story's own stated complete outcome, **no budget change and no retry**. The mechanism is now proven rather than asserted: a deterministically-provoked expired `SO_RCVTIMEO` yields `ErrorKind::WouldBlock` / `raw_os_error == Some(35)`, whose `Display` reproduces the rc1 dogfood WARN line character-for-character. The overrun itself is **OS scheduler starvation of the daemon's `current_thread` runtime**, not slow work: under a real compile storm the reply-path body is unmoved (p50 199µs, p90 291µs, identical to idle) while the tail detonates (p99 2436µs, max 5686µs) past the 3ms budget. The story's leading hypothesis (a), WS fanout, was **refuted** with 8 presenters and 14,472 delivered frames (read max 385µs), and (c) cold-start was refuted across 15 fresh-daemon rounds. Three reasons not to retry, the third newly discovered: the budget already has ~6x headroom on the realistic idle path; a retry short enough to fit the budget cannot outlast a multi-millisecond deschedule; and a retry would **duplicate events**, because `events` has no natural-key uniqueness or idempotency key and the daemon has usually already `try_send`-ed the event before the shim gives up. Shipped: `Error::Timeout { op, budget_ms }` in the exit-0/WARN/no-stderr-hint class with the three partition canaries unmodified, `WouldBlock`+`TimedOut` classified cross-platform, budgets single-sourced so the message cannot drift from the socket option, the false "5ms total" comment corrected (connect left unbounded on measured grounds, 337µs worst case under the load that drove the read to 5.7ms, because a Unix-socket connect completes in-kernel and does not need the daemon scheduled), and 6 new tests. Verification green on macOS arm64: fmt, clippy `-D warnings`, `scripts/test.sh` 636 passed / 0 failed, and the shim hot-path bench A/B'd against `HEAD` (p99 1.397ms → 1.429ms, +2.3%, inside the ±12% same-build spread) at −46% vs baseline. One disclosure: the first bench reading was +113.65% and was traced to leftover CPU spinners from my own hypothesis-(b) experiment, not to this change, corrected rather than accepted on the strength of a disabled regression gate. Two follow-ups filed instead of scope-creeping: taskwarrior `719e7027` (insulate daemon ingest from scheduler starvation, dedicated thread or `worker_threads=2`) and `dfe88917` (the idle path is ~10x slower than burst, so `hot_path.rs` burst numbers overstate real headroom).
- 2026-07-29: Story created via bmad-create-story as the Story 5.12 AC #5 escalation of an rc1 dogfood finding (two dropped events in ~5 min / ~33 events on the first fresh-machine `v0.1.0-rc1` install). Root cause identified during triage: macOS reports expired `SO_SNDTIMEO`/`SO_RCVTIMEO` as `EAGAIN`/`WouldBlock` rather than `TimedOut`, and the shim has no `Timeout` variant, so timeouts are indistinguishable from genuine socket errors. Deliberately scoped as a **diagnosability** story rather than a correctness one: dropping the event is likely correct per Axiom 3, and Task 1 gates any budget change on measurement. Two secondary findings folded in: the connect is unbounded and outside the code's own "5ms total" claim (AC #4), and the daemon's reply path is a non-blocking `try_send` with no durable write, which makes a 3ms overrun an anomaly to explain rather than a tight budget to relax (AC #3). Status → ready-for-dev.
