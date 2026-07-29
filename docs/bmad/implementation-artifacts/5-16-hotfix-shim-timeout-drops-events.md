# Story 5.16: Hotfix — shim socket timeouts drop events and are indistinguishable from real I/O errors

Status: ready-for-dev

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

- [ ] **Task 1: Reproduce and explain the timeout before changing anything (AC: 3)**
  - [ ] Re-run the rc1 install-and-dogfood shape and confirm the drop rate. Capture `~/.bowerbird/shim.log` alongside daemon `tracing` output at `debug` (the ingest path logs `ingest: 200 accepted` at debug, [handler.rs:126](../../crates/daemon/src/ingest/handler.rs)).
  - [ ] Instrument or measure where the 3ms goes: connect, write, or the read of the reply. Note that connect is currently unbounded and outside the "5ms total" claim (AC #4).
  - [ ] Test hypothesis (a) specifically: reproduce with N concurrent WS presenters attached vs zero, since the daemon is `current_thread` and WS fanout shares that thread with ingest.
  - [ ] Write the finding into the Dev Agent Record **before** proposing a budget change. If the cause turns out to be a daemon-side stall, say so — the fix may belong in the daemon, not the shim.

- [ ] **Task 2: Add the `Timeout` error variant (AC: 1, 2)**
  - [ ] Add a timeout variant to `crates/shim/src/error.rs`. Follow the existing forward-compat pattern (see the reserved `Backpressure(String)` variant with `#[allow(dead_code)]`, [error.rs:38-43](../../crates/shim/src/error.rs)).
  - [ ] Wire it into `exit_code()` (→ 0), `level()` (→ WARN), `stderr_hint()` (→ `None`), and add it to `sample_variants()`. The partition tests are the gate; do not modify them to accommodate the variant.
  - [ ] Classify at the call site in `socket.rs`: map `ErrorKind::WouldBlock` **and** `ErrorKind::TimedOut` from the write/read operations to the new variant (`WouldBlock` is the macOS spelling, `TimedOut` the Linux one — handle both, this is a cross-platform classification, not a macOS special case).
  - [ ] Ensure the error message names the expired budget, so the log line says which timeout blew rather than restating the errno.

- [ ] **Task 3: Resolve the unbounded connect (AC: 4)**
  - [ ] Either bound the connect or correct the [socket.rs:24-25](../../crates/shim/src/socket.rs) comment. Prefer whichever keeps the hot path simpler (Axiom: prefer one code path over a branch).
  - [ ] If bounding: confirm it does not add a syscall to the success path.

- [ ] **Task 4: Contract test for the classification (AC: 6)**
  - [ ] Unit-test the io::Error → `Error` classification directly: `WouldBlock` → timeout variant, `TimedOut` → timeout variant, other kinds → `SocketIo`.
  - [ ] Assert the emitted log line distinguishes the two. Reuse the shim log-assertion patterns already in `tests/contract_shim.rs`.
  - [ ] Do **not** attempt to race a real socket timeout in the suite — it is nondeterministic and the parallel-test discipline forbids sleep-based synchronization.

- [ ] **Task 5: Budget decision, only if Task 1 justifies it (AC: 3, 5)**
  - [ ] If and only if Task 1 shows the budget is genuinely too tight, propose the new numbers with the measurement backing them. Record the decision inline as "Maintainer decision (pickles, DATE)".
  - [ ] Re-run the shim hot-path bench; confirm no p99 regression beyond the +15% CI gate.
  - [ ] If Task 1 shows the cause is daemon-side, **stop here** and file the daemon work separately rather than widening this story.

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

### Debug Log References

### Completion Notes List

### File List

## Change Log

- 2026-07-29: Story created via bmad-create-story as the Story 5.12 AC #5 escalation of an rc1 dogfood finding (two dropped events in ~5 min / ~33 events on the first fresh-machine `v0.1.0-rc1` install). Root cause identified during triage: macOS reports expired `SO_SNDTIMEO`/`SO_RCVTIMEO` as `EAGAIN`/`WouldBlock` rather than `TimedOut`, and the shim has no `Timeout` variant, so timeouts are indistinguishable from genuine socket errors. Deliberately scoped as a **diagnosability** story rather than a correctness one: dropping the event is likely correct per Axiom 3, and Task 1 gates any budget change on measurement. Two secondary findings folded in: the connect is unbounded and outside the code's own "5ms total" claim (AC #4), and the daemon's reply path is a non-blocking `try_send` with no durable write, which makes a 3ms overrun an anomaly to explain rather than a tight budget to relax (AC #3). Status → ready-for-dev.
