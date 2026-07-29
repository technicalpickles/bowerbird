# Story 5.16: Hotfix — shim socket timeouts drop events and are indistinguishable from real I/O errors

Status: review

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

5. **Given** the shim must never block Claude (Axiom 3, NFR20) **When** any change to retry or budget behavior is proposed **Then** the shim's p95 hot-path bench (`shim/benches/hot_path.rs`, gated in CI at +15% p99 regression) still passes, and any added retry is bounded such that the worst-case total stays inside the shim's budget. **A retry loop that can exceed the budget is a regression, not a fix** — if measurement shows the budget cannot be met without stalling Claude, dropping the event remains the correct outcome and this story ends at diagnosability.

6. **Given** the shim log is the only place a dropped event surfaces **When** a timeout drop occurs **Then** a contract test asserts the log line distinguishes timeout from generic socket I/O. Note the testing constraint: a real `EAGAIN`-from-timeout is awkward to provoke deterministically, so prefer testing the **classification function** (io::Error → Error variant) directly over trying to race a real socket.

## Tasks / Subtasks

- [x] **Task 1: Reproduce and explain the timeout before changing anything (AC: 3)**
  - [x] Re-run the rc1 install-and-dogfood shape and confirm the drop rate. Capture `~/.bowerbird/shim.log` alongside daemon `tracing` output at `debug` (the ingest path logs `ingest: 200 accepted` at debug, [handler.rs:126](../../crates/daemon/src/ingest/handler.rs)).
  - [x] Instrument or measure where the 3ms goes: connect, write, or the read of the reply. Note that connect is currently unbounded and outside the "5ms total" claim (AC #4).
  - [x] Test hypothesis (a) specifically: reproduce with N concurrent WS presenters attached vs zero, since the daemon is `current_thread` and WS fanout shares that thread with ingest.
  - [x] Write the finding into the Dev Agent Record **before** proposing a budget change. If the cause turns out to be a daemon-side stall, say so — the fix may belong in the daemon, not the shim.

- [x] **Task 2: Add the `Timeout` error variant (AC: 1, 2)**
  - [x] Add a timeout variant to `crates/shim/src/error.rs`. Follow the existing forward-compat pattern (see the reserved `Backpressure(String)` variant with `#[allow(dead_code)]`, [error.rs:38-43](../../crates/shim/src/error.rs)).
  - [x] Wire it into `exit_code()` (→ 0), `level()` (→ WARN), `stderr_hint()` (→ `None`), and add it to `sample_variants()`. The partition tests are the gate; do not modify them to accommodate the variant.
  - [x] Classify at the call site in `socket.rs`: map `ErrorKind::WouldBlock` **and** `ErrorKind::TimedOut` from the write/read operations to the new variant (`WouldBlock` is the macOS spelling, `TimedOut` the Linux one — handle both, this is a cross-platform classification, not a macOS special case).
  - [x] Ensure the error message names the expired budget, so the log line says which timeout blew rather than restating the errno.

- [x] **Task 3: Resolve the unbounded connect (AC: 4)**
  - [x] Either bound the connect or correct the [socket.rs:24-25](../../crates/shim/src/socket.rs) comment. Prefer whichever keeps the hot path simpler (Axiom: prefer one code path over a branch).
  - [x] If bounding: confirm it does not add a syscall to the success path. **(N/A — comment corrected instead of bounding; see Completion Note 3 for the measurement behind that call.)**

- [x] **Task 4: Contract test for the classification (AC: 6)**
  - [x] Unit-test the io::Error → `Error` classification directly: `WouldBlock` → timeout variant, `TimedOut` → timeout variant, other kinds → `SocketIo`.
  - [x] Assert the emitted log line distinguishes the two. Reuse the shim log-assertion patterns already in `tests/contract_shim.rs`.
  - [x] Do **not** attempt to race a real socket timeout in the suite — it is nondeterministic and the parallel-test discipline forbids sleep-based synchronization. **(Honored — no race. See Completion Note 4: the added end-to-end test uses a peer that never replies, which makes the expiry deterministic rather than raced.)**

- [x] **Task 5: Budget decision, only if Task 1 justifies it (AC: 3, 5)**
  - [x] If and only if Task 1 shows the budget is genuinely too tight, propose the new numbers with the measurement backing them. Record the decision inline as "Maintainer decision (pickles, DATE)". **(Task 1 showed the budget is NOT too tight — no change proposed, so no maintainer decision was required. See Completion Note 1.)**
  - [x] Re-run the shim hot-path bench; confirm no p99 regression beyond the +15% CI gate.
  - [x] If Task 1 shows the cause is daemon-side, **stop here** and file the daemon work separately rather than widening this story. **(Filed as taskwarrior `719e7027`; no daemon code touched.)**

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
- [Source: docs/bmad/project-context.md#Performance bars] — shim hot-path bench gates p99 at +15%.

## Dev Agent Record

### Agent Model Used

claude-opus-5 (Claude Code, `bmad-dev-story`), 2026-07-29.

### Debug Log References

Task 1 measurement harnesses were throwaway, std-only programs run against a **real release daemon** (`target/release/bowerbird-daemon`, the profile rc1 shipped) on an isolated `BOWERBIRD_DATA_DIR` with `BOWERBIRD_INGEST_SOCK=/tmp/bb516.sock`. They are not committed — they exist to produce the numbers below, and every one is reproducible from the descriptions here.

- `probe.rs` — replicates `socket.rs::send` syscall-for-syscall (same op order, same `BufReader::with_capacity(64)`) but times connect / write / read separately and reports `io::ErrorKind` + `raw_os_error` on failure. Timeouts are parameterized so the tail can be measured **untruncated** — with the real 2ms/3ms budgets in place the tail is invisible by construction, which is why the original finding had no numbers attached.
- `wsload.rs` — minimal std-only WS presenter (HTTP upgrade, bearer auth, masked client frames, Ping→Pong) used to put real fanout load on the daemon's `current_thread` runtime for hypothesis (a).
- `drive_shim.sh` — drives the **real** `target/release-shim/bowerbird-shim` binary, one fresh process per event, and counts `WARN` lines in `BOWERBIRD_SHIM_LOG`. This is the faithful dogfood shape (the in-process probe cannot see process-cold effects).
- `coldstart.sh` — hypothesis (c): fresh data dir + fresh daemon per round, spin until the ingest socket exists, fire exactly one request with the real budgets.
- `errno_proof.rs` — deterministic proof of the errno→`ErrorKind` mapping via a server that accepts and never replies. No load, no racing.

One environment note for anyone re-running this: the session scratchpad path is too long for `SUN_LEN` (104 bytes on macOS), so the ingest socket must be overridden to a short path. The daemon fails with a clear `path must be shorter than SUN_LEN` error, not a confusing one.

### Completion Notes List

1. **[Task 1 — AC #3 gate satisfied. The 3ms budget is NOT miscalibrated; the daemon's reply-path tail is starved by the OS scheduler under heavy system load.]**

   **The mechanism is confirmed, and reproduced byte-for-byte.** `errno_proof` provokes an expired `SO_RCVTIMEO` deterministically (a server that accepts and never replies) and gets:

   ```
   READ timeout expired after 3.760542ms
     ErrorKind    = WouldBlock          raw_os_error = Some(35)
     Display      = Resource temporarily unavailable (os error 35)
     kind == WouldBlock = true   kind == TimedOut = false
   ```

   Folded through the shim's current `Display`, that is `socket I/O failed: Resource temporarily unavailable (os error 35)` — **character-for-character the rc1 dogfood WARN line**. So the two dropped events were expired socket timeouts, not genuine I/O failures, and AC #1's root-cause claim is now proven rather than asserted. An expired `SO_SNDTIMEO` was provoked the same way and yields the identical `WouldBlock` / errno 35. On Linux the same expiry surfaces as `ErrorKind::TimedOut`, which is why Task 2 classifies **both** kinds.

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

   The load row is the finding. Note what does *not* move: p50 199µs and p90 291µs are indistinguishable from the idle rows. Only the tail explodes — p99 to 2.4ms and max to 5.7ms, straight through the 3ms budget. A reply path that got *slower* would shift the whole distribution; a distribution whose body is pinned and whose tail detonates is **scheduler starvation** — the daemon's `current_thread` runtime thread loses its slice on a heavily oversubscribed machine and does not get rescheduled for milliseconds. The work in the reply path is not the problem; being allowed to run is.

   That matches the dogfood conditions exactly: three live Claude Code sessions plus an install plus a `cargo` build is the compile-storm shape, and the two drops were 4.5 minutes apart with ~31 successes in between — a tail event, not a systemic failure.

   **Hypotheses (a) and (c) are refuted, not merely untested:**
   - **(a) WS fanout sharing the `current_thread` runtime — refuted.** Eight concurrent WS presenters subscribed to `state.session.*` + `events.*`, with **14,472 data frames actually delivered** during the run, moved read max to 385µs (paced) / 283µs (burst). Fanout does not starve ingest. This was the story's leading hypothesis; it is wrong.
   - **(c) cold path on the first hook after install — refuted.** Fifteen rounds of fresh-data-dir + fresh-daemon + spin-until-socket-exists + exactly one request, all with the real 2ms/3ms budgets: 15/15 succeeded, read 118–230µs with a single 919µs outlier. The first hook is not special. (Drop #1 landing during `bowerbird install` is coincidence — install is *when the machine is busiest*, which is hypothesis (b) again.)
   - **(b) macOS scheduler contention on a loaded dev box — confirmed**, and it is the whole explanation. But the *kind* of load decides everything, which is the trap worth recording: naive CPU spinners **reduced** latency (read p50 226µs vs 322µs idle) because they keep cores awake and clocked up, cancelling the idle-wakeup penalty below. Only realistic mixed load — parallel `rustc`, i.e. CPU **and** memory **and** I/O **and** process churn — reproduces the tail. Anyone re-testing this with a busy-loop will measure an *improvement* and wrongly conclude the budget is fine.

   **Methodology correction, disclosed rather than quietly fixed.** The 12 CPU spinners from the hypothesis-(b) spinner run did not die when I killed them, and kept running through the WS-fanout and cold-start experiments. That is why the table's machine-state column is explicit per row instead of a blanket "quiet machine". It does not weaken any conclusion here — every affected run is one where the daemon *stayed fast* (WS fanout, cold start, 300 real-shim invocations with zero drops), so the true quiet-machine result can only be faster, and the two rows the argument actually leans on (idle baseline, compile storm) were measured with the machine in the state their labels claim. It did, however, contaminate the first `hot_path` bench reading — see Completion Note 5.

   **Unrelated-but-real secondary finding: the *idle* path is ~10x slower than the burst path.** Read p50 is 322µs paced versus 27µs bursting, because a request arriving at a parked daemon pays thread wakeup on a cold core. Real Claude hooks are sparse, so real hooks *always* pay this. It saturates around 500–600µs (a 2s gap is no worse than a 100ms gap), so it is not the cause of any drop — but it means the honest headroom under 3ms is ~6x, not the ~100x you would infer from a burst benchmark. Worth knowing before anyone tightens the budget on the strength of `hot_path.rs` numbers.

   **Conclusion, which is Task 5's answer: do not change the budgets, and do not add a retry.** Three independent reasons, in increasing order of severity:
   1. The budget has ~6x headroom over the realistic (idle-path) p50 and ~10x over p90. It is not tight. Relaxing 3ms to, say, 10ms would convert *some* starvation events into successes while making the shim's worst case 3x longer on the trust-boundary side — trading Claude's responsiveness for event completeness, which Axiom 3 forbids.
   2. Under starvation, a retry is likely to hit the same stall — the daemon is not slow, it is *not running*. A bounded retry that fits inside the budget is too short to outlast a multi-millisecond deschedule; one that outlasts it has blown the budget by definition. This is exactly the "retry loop that can exceed the budget is a regression, not a fix" case AC #5 names.
   3. **A retry would duplicate events.** Verified against the schema: `events` is `event_id INTEGER PRIMARY KEY AUTOINCREMENT` ([db/migrations.rs:7](../../crates/daemon/src/db/migrations.rs)) with no natural-key uniqueness and no shim-supplied idempotency key — the `ON CONFLICT(source, session_id)` in [db/queries.rs:10](../../crates/daemon/src/db/queries.rs) is the *projection* upsert, not the event log. When the read times out, the daemon has almost always already `try_send`-ed the event ([handler.rs:121](../../crates/daemon/src/ingest/handler.rs)); the shim just never heard `200\n`. Re-sending would append a second row for one hook, corrupting the append-only log that Story 5.11 and `/replay` both treat as the source of truth. Retry is not merely ineffective here, it is unsafe without an idempotency key the protocol does not have.

   So this story delivers **diagnosability only**, which the story text itself names as a complete outcome. The daemon-side question (should ingest be insulated from scheduler starvation — a dedicated thread, or `rt-multi-thread` with `worker_threads=2`) is explicitly out of scope per §Scope boundary and is filed separately rather than widening this story.

2. **[Task 2 — AC #1, #2 — `Error::Timeout` added and wired; the before/after is the whole story.]** Same provoked failure, old shim then new shim, real `release-shim` binary against a peer that never replies:

   ```
   WARN socket I/O failed: Resource temporarily unavailable (os error 35)   <- rc1
   WARN socket read timed out after 3ms; event dropped                     <- now
   ```

   Exit 0 and empty stderr in both cases, as required.

   The variant is `Timeout { op: &'static str, budget_ms: u64 }` — a struct variant rather than a bare `Timeout` so the line names *which* operation blew *which* budget. Both fields are `Copy`/`&'static`, so nothing allocates (shim hot-path discipline). It joins the exit-0 / WARN / `stderr_hint() == None` class alongside `SocketIo`, and all three partition canaries (`exit_code_never_2`, `level_matches_exit_code`, `stderr_hint_matches_exit_code`) pass **unmodified** — the only test-module edit was adding two `Timeout` values to `sample_variants()`, which AC #2 requires. Both `op` spellings are sampled (`write`/`read`) so the canaries cover each.

   Two deliberate choices worth flagging for review:
   - **The budgets are now single-sourced** as `WRITE_BUDGET_MS`/`READ_BUDGET_MS` consts in `socket.rs`, feeding both the `set_*_timeout` calls and the message. Previously the numbers were inline literals; a message that hardcoded "3ms" could have drifted from a changed socket option and lied in the log. `budgets_match_the_documented_contract` pins them.
   - **The two `set_*_timeout` calls keep mapping to `SocketIo`, not `Timeout`.** They are `setsockopt` — they cannot time out, so classifying them as timeouts would be wrong. Only the actual `write_all` and `read_line` go through `classify`.

3. **[Task 3 — AC #4 — the "5ms total" comment was corrected, and the connect deliberately left unbounded. No silent no-op.]** The old comment claimed "Total = write + read ≤ 5ms in the worst case", which excluded the unbounded `connect` above it and was therefore not a total. It now states what the 5ms actually covers and why the asymmetry is safe rather than accidental.

   The measurement is what settles it: a Unix-socket `connect` to a listening socket completes **in the kernel** as soon as the connection lands in the accept backlog — it does not wait for the daemon to call `accept`, so unlike the reply read it does not depend on the daemon's thread being scheduled. The data shows exactly that split. In the compile-storm run that drove the read tail to 5686µs, connect's own worst case was **337µs** (p50 39µs). The phase that needs the daemon to run is the phase that blows up; connect is not that phase.

   Bounding it anyway was rejected on cost, not on effort: `std` has no `UnixStream::connect_timeout`, so it would take either a new shim dependency (`socket2`) or a hand-rolled non-blocking connect + poll + restore-to-blocking. That adds syscalls to the **success** path — the path whose entire job is to be invisible — to guard a phase with no measured tail, and it would also invalidate `classify`'s "this socket is never non-blocking, so `WouldBlock` can only mean timeout" premise. Correcting the claim is the honest fix; the real exposure (daemon starvation) is filed as `719e7027`.

4. **[Task 4 — AC #6 — classification covered at both levels; 6 tests added, 636 passing.]** Five unit tests in `socket.rs` cover the classification function directly, as the story asked: both kinds → `Timeout` with `op`/`budget_ms` preserved; a **raw** `from_raw_os_error(35)` (how macOS actually delivers it, rather than a synthesized `ErrorKind`) → `Timeout`; five genuine failure kinds (`BrokenPipe`, `ConnectionReset`, `ConnectionRefused`, `PermissionDenied`, `UnexpectedEof`) → still `SocketIo`, which is the guard against over-eager classification hiding real I/O errors; and the message asserting it names the op and budget and contains neither `socket I/O failed` nor `os error`.

   One added test goes beyond the unit level, and the reasoning matters because the story warned against it. `shim_names_socket_timeout_in_log_and_stays_silent` drives a **real** expired timeout through the real shim binary via a new `start_mock_ingest_silent` helper — a mock that accepts, reads the request, and never replies. This is **not** the race the story forbids: no reply can ever arrive, so the read budget must expire; there is no sleep used for synchronization and no ordering that can be lost. It closes a gap the unit tests structurally cannot reach — that the classified error actually arrives *in the log*, at WARN, with stderr still empty. Holding the accepted stream open is load-bearing (dropping it would give EOF → `BadResponse`, silently testing the wrong path), and the assertion is on `"timed out"` rather than on which operation expired, so a starved CI runner that blows the write budget first still passes for the right reason.

   The `contract_test_inventory.rs` whitelist was deliberately **not** touched: it pins the 10 architecture-required contract surfaces, and this is a story-specific test, not a new required surface.

5. **[Task 5 — AC #5 — bench gate re-verified, and a bad first reading corrected rather than accepted.]** The first `hot_path` run reported p99 5.691ms, **+113.65%** over the committed macOS baseline. It passed the gate only because `regression_max_ratio` is `null` per ADR 0003 — which is exactly the situation where a passing gate should not end the inquiry. It turned out the 12 CPU spinners from the hypothesis-(b) experiment were still running (load average 52); the reading was an artifact of my own harness, not of this change.

   Re-measured on a settled machine, and A/B'd against `HEAD` by stashing the change so both sides ran on the same machine state:

   | build | mean | p99 | vs baseline |
   | --- | --- | --- | --- |
   | `HEAD` (no change) | 1.249ms | 1.397ms | −47.6% |
   | with this change | 1.319ms | 1.429ms | −46.3% |
   | with this change (2nd sample) | 1.403ms | 1.598ms | −40.0% |

   Same-build run-to-run spread is ±12% (1.429 → 1.598), so the +2.3% `HEAD`-vs-change p99 delta is inside noise — as expected, since `classify` executes only on the error path and the success path gained nothing but two `const` reads. Comfortably inside the +15% gate and well under the 15ms absolute budget.

   Full verification, all green on macOS arm64: `cargo fmt --check`, `cargo clippy --all-targets --workspace -D warnings`, and `scripts/test.sh` at **636 passed / 0 failed** (630 was the Story 5.12 baseline; +6 is exactly this story's additions), log `target/test-logs/20260729-183346-51669`.

6. **Correction to a story-spec claim: `TimedOut` is not "the Linux spelling".** AC #1 and Task 2 both describe `ErrorKind::TimedOut` as the Linux counterpart to macOS's `WouldBlock`. That is not right. POSIX specifies `EAGAIN` for `SO_RCVTIMEO`/`SO_SNDTIMEO` expiry on **Linux and macOS alike**, so Unix generally lands on `WouldBlock`; `TimedOut` is where **Windows** (`WSAETIMEDOUT`) would land, and bowerbird scope-cuts Windows.

   The code is unchanged by this — both kinds are still matched — but the *reason* in the comment is now the correct one, because a comment asserting a false platform fact is exactly the kind of thing that made the original `socket.rs` "5ms total" claim mislead for two stories. The honest justification: **`std` does not pin which kind an expired socket timeout produces** (it documents either for a timed-out read/write), so matching both is coding to the documented contract rather than to one platform's observed errno. The `TimedOut` arm costs one pattern and removes the chance of a silent misclassification that would take another dogfood cycle to rediscover.

   What is actually verified versus inferred, stated plainly: macOS is **empirically proven** (`WouldBlock` / errno 35, reproduced deterministically). Linux is **not** empirically proven here — a container run was not available this session — but the Linux CI row of the `ci` job ran the new end-to-end timeout test and passed ([run 30497303501](https://github.com/technicalpickles/bowerbird/actions/runs/30497303501)), which proves Linux lands in one of the two matched arms without proving which. Given POSIX it is `WouldBlock`. If someone wants that pinned exactly, a container run of `socket::tests` is the cheap way.

7. **Story spec path correction, for the next reader.** The story's §"Files this story touches" table and Task 4 both say `tests/contract_shim.rs`; the file is actually at **`crates/shim/tests/contract_shim.rs`**. Related and load-bearing: `bowerbird-shim` is a **binary** crate, so its `error`/`socket` modules cannot be imported from an integration test — which is why AC #6's classification unit tests live in-crate under `#[cfg(test)] mod tests` in `socket.rs` rather than in `contract_shim.rs`. `crates/shim/src/log.rs` was listed as "UPDATE (maybe)" and needed **no change**: the existing `Display`-based log append already carries the new message verbatim.

### File List

- `crates/shim/src/error.rs` — UPDATE. Added `Error::Timeout { op, budget_ms }` and wired it through `exit_code()` (→ 0), `stderr_hint()` (→ `None`), and `sample_variants()` (two entries, one per `op`). `level()` derives from `exit_code()` and needed no edit. The three partition canaries are unmodified.
- `crates/shim/src/socket.rs` — UPDATE. Added `WRITE_BUDGET_MS`/`READ_BUDGET_MS` consts, the `classify()` helper (`WouldBlock`|`TimedOut` → `Timeout`, else `SocketIo`), routed `write_all`/`read_line` through it, corrected the "5ms total" comment per AC #4, and added a `#[cfg(test)] mod tests` with 5 classification tests.
- `crates/shim/tests/contract_shim.rs` — UPDATE. Added the `start_mock_ingest_silent` helper (accepts, reads, never replies, holds the stream open) and the `shim_names_socket_timeout_in_log_and_stays_silent` end-to-end contract test.
- `docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md` — UPDATE. Task checkboxes, Dev Agent Record, File List, Change Log, Status.
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — UPDATE. `5-16` ready-for-dev → in-progress → review, plus `last_updated`.

No `crates/protocol/src` change, so the changelog gate is correctly not triggered (and none was manufactured). No SQLite migration, no wire-format change, no daemon code touched.

## Change Log

- 2026-07-29: Story implemented via bmad-dev-story; all 5 tasks complete, all 6 ACs satisfied, status → review. **Task 1's reproduce-and-explain gate was honored: no code changed until the cause was measured.** Outcome is diagnosability-only, which is the story's own stated complete outcome — **no budget change and no retry**. The mechanism is now proven rather than asserted: a deterministically-provoked expired `SO_RCVTIMEO` yields `ErrorKind::WouldBlock` / `raw_os_error == Some(35)`, whose `Display` reproduces the rc1 dogfood WARN line character-for-character. The overrun itself is **OS scheduler starvation of the daemon's `current_thread` runtime**, not slow work: under a real compile storm the reply-path body is unmoved (p50 199µs, p90 291µs, identical to idle) while the tail detonates (p99 2436µs, max 5686µs) past the 3ms budget. The story's leading hypothesis (a) — WS fanout — was **refuted** with 8 presenters and 14,472 delivered frames (read max 385µs), and (c) cold-start was refuted across 15 fresh-daemon rounds. Three reasons not to retry, the third newly discovered: the budget already has ~6x headroom on the realistic idle path; a retry short enough to fit the budget cannot outlast a multi-millisecond deschedule; and a retry would **duplicate events**, because `events` has no natural-key uniqueness or idempotency key and the daemon has usually already `try_send`-ed the event before the shim gives up. Shipped: `Error::Timeout { op, budget_ms }` in the exit-0/WARN/no-stderr-hint class with the three partition canaries unmodified, `WouldBlock`+`TimedOut` classified cross-platform, budgets single-sourced so the message cannot drift from the socket option, the false "5ms total" comment corrected (connect left unbounded on measured grounds — 337µs worst case under the load that drove the read to 5.7ms, because a Unix-socket connect completes in-kernel and does not need the daemon scheduled), and 6 new tests. Verification green on macOS arm64: fmt, clippy `-D warnings`, `scripts/test.sh` 636 passed / 0 failed, and the shim hot-path bench A/B'd against `HEAD` (p99 1.397ms → 1.429ms, +2.3%, inside the ±12% same-build spread) at −46% vs baseline. One disclosure: the first bench reading was +113.65% and was traced to leftover CPU spinners from my own hypothesis-(b) experiment, not to this change — corrected rather than accepted on the strength of a disabled regression gate. Two follow-ups filed instead of scope-creeping: taskwarrior `719e7027` (insulate daemon ingest from scheduler starvation — dedicated thread or `worker_threads=2`) and `dfe88917` (the idle path is ~10x slower than burst, so `hot_path.rs` burst numbers overstate real headroom).
- 2026-07-29: Story created via bmad-create-story as the Story 5.12 AC #5 escalation of an rc1 dogfood finding (two dropped events in ~5 min / ~33 events on the first fresh-machine `v0.1.0-rc1` install). Root cause identified during triage: macOS reports expired `SO_SNDTIMEO`/`SO_RCVTIMEO` as `EAGAIN`/`WouldBlock` rather than `TimedOut`, and the shim has no `Timeout` variant, so timeouts are indistinguishable from genuine socket errors. Deliberately scoped as a **diagnosability** story rather than a correctness one: dropping the event is likely correct per Axiom 3, and Task 1 gates any budget change on measurement. Two secondary findings folded in: the connect is unbounded and outside the code's own "5ms total" claim (AC #4), and the daemon's reply path is a non-blocking `try_send` with no durable write, which makes a 3ms overrun an anomaly to explain rather than a tight budget to relax (AC #3). Status → ready-for-dev.
