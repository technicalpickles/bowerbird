# Story 2.2: Real-time event and state broadcast to multiple tools

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want to receive live Claude Code event and state frames over my WebSocket connection simultaneously with other connected tools,
So that multiple tools can observe the same agent activity independently without affecting each other.

## Acceptance Criteria

1. **Given** three tools connected and subscribed to `events.*`
   **When** a new event is ingested by the daemon
   **Then** all three tools receive an `event` `ServerMessage` whose `event` payload is byte-identical (same `event_id`, `source`, `session_id`, `kind`, `reaction`, `payload`, `created_at`) and arrives in the same order on every subscriber, with end-to-end latency targeting hook→presenter p99 ≤100ms (NFR target; not a hard contract-test gate in this story).

2. **Given** a tool subscribed to `state.session.<id>` or `state.session.<id>.current_state`
   **When** an event causes the projection for session `<id>` to change
   **Then** the tool receives a `state` `ServerMessage` containing the updated `SessionState { current_state, last_event_kind, last_event_at_ms }` carrying that session's `source` and `session_id`, AND no `state` frame is delivered for events belonging to a different `session_id`.

3. **Given** a tool subscribed to `events.claude.*`
   **When** an event from `source = "claude"` is ingested AND (in a forward-compatible test) a synthetic event from a different source is published into the hub
   **Then** the tool receives the `claude` event and does NOT receive the other-source event — source-scoped topic filtering holds.

4. **Given** a tool subscribed to `state.session.*` (wildcard)
   **When** events arrive for three different concurrent `session_id`s
   **Then** the tool receives a `state` frame for each session, each frame correctly carrying the originating `session_id` — no cross-session field smearing.

5. **Given** two tools with identical topic subscriptions
   **When** one tool's WebSocket is closed (graceful or abrupt)
   **Then** the surviving tool continues to receive subsequent `event` / `state` frames in order with no interruption, no duplicate frames, and no permit underflow on the WS semaphore (consumer independence).

6. **Given** the projection write transaction for an event fails (e.g. `interact` error, `tx.commit` returns `Err`)
   **When** publishing would otherwise occur
   **Then** NO `BroadcastEnvelope` is published for that event — publishing is gated on transaction success. The existing error log is unchanged.

7. **Given** the daemon writes a `RecordingStarted` or `RecordingEnded` sentinel event (source `__daemon__`)
   **When** the transaction commits
   **Then** NO `BroadcastEnvelope` is published for the sentinel — sentinels are excluded from the user-facing event/state surface (matches the Story 1.7 exclusion of `__daemon__` from `GET /sessions`).

## Tasks / Subtasks

- [x] Task 1: Thread `Arc<BroadcastHub>` into the ingest writer task (AC: #1, #2, #3, #4, #5)
  - [x] Add `broadcaster: Arc<BroadcastHub>` parameter to `crates/daemon/src/ingest/writer.rs::run`
  - [x] Update the spawn in `crates/daemon/src/main.rs::run` to pass `broadcaster.clone()` (the `Arc<BroadcastHub>` already constructed at line 164 — clone the `Arc`, do not share the same handle that gets moved into `AppState`)
  - [x] No new module; no new file

- [x] Task 2: Publish from `projection::session::write` after transaction commit (AC: #1, #2, #6)
  - [x] Change the `write` signature from `(writer_pool, envelope)` to `(writer_pool, broadcaster: &BroadcastHub, envelope)` — pass by `&BroadcastHub` reference, not `Arc<BroadcastHub>`, because the caller already holds the `Arc` and `publish` only needs a `&self`
  - [x] Inside the `interact` closure, capture `now_ms`, the assigned `event_id` from `tx.last_insert_rowid()`, AND the freshly computed `new_state: SessionState` so they can be returned to the outer scope alongside the existing `i64` event_id
  - [x] Change the closure return type from `rusqlite::Result<i64>` to `rusqlite::Result<(i64, SessionState)>` — return `Ok((id, new_state))` only after `tx.commit()?` succeeds
  - [x] After `let event_id = interact_res?;`, construct the `protocol::Event` (filling `event_id: EventId(event_id)`, `source`, `session_id`, `kind`, `reaction`, `payload`, `created_at: now_ms` — clone strings/payload from the variables retained in the outer scope before they moved into the closure) and call `broadcaster.publish(BroadcastEnvelope::Event(event.clone()))` then `broadcaster.publish(BroadcastEnvelope::State { source: event.source.clone(), session_id: event.session_id.clone(), state: new_state })`
  - [x] Strict ordering: publish `Event` BEFORE `State` so a presenter consuming both topics sees the event that caused the transition before (or at worst interleaved with — broadcast preserves per-channel order, not cross-publish) the resulting projection update. Document the choice in a doc-comment on `write`
  - [x] Do NOT add `broadcaster` parameters to `write_recording_started` / `write_recording_ended` — sentinel writes do NOT publish (per AC #7 and architecture's `source != '__daemon__'` exclusion already enforced in `SELECT_DISTINCT_SESSIONS_FROM_EVENTS` and `SELECT_HELLO_DB_FIELDS`)
  - [x] Update the `tracing::instrument` annotation — no field change needed, but add a one-line `tracing::debug!(event_id, "ws: published event + state envelopes")` after the second publish so `-vv` debugging can confirm fan-out fired

- [x] Task 3: Pass `broadcaster` from `writer::run` into `projection::session::write` (AC: #1)
  - [x] In `crates/daemon/src/ingest/writer.rs::run`, change both `projection::session::write(&writer_pool, envelope)` callsites (the steady-state `recv` arm and the shutdown-drain arm) to `projection::session::write(&writer_pool, &broadcaster, envelope)`
  - [x] Verify the shutdown-drain path still publishes — events accepted before shutdown that drain after `cancelled()` MUST publish so any WS clients still attached during the drain receive them, consistent with the Story 2.5 graceful shutdown design that hasn't shipped yet

- [x] Task 4: Contract tests for the fan-out path (AC: #1, #2, #3, #4, #5, #6, #7)
  - [x] Add a new module `story_2_2_publish` in `crates/daemon/tests/contract_daemon.rs` (next to `story_2_1_ws`) that REUSES `spawn_test_daemon`, `make_test_state_with_ws`, `connect_authed`, `parse_hello`, and `read_text_frame_or_close` — do NOT re-declare these
  - [x] Helper `parse_event_frame(&Message) -> protocol::Event` and `parse_state_frame(&Message) -> protocol::StateFrame` mirroring `parse_hello`'s shape (match `ServerMessage::Event(f) => f.event`, `ServerMessage::State(f) => f`, panic on any other variant)
  - [x] Helper `publish_via_projection(state: &AppState, source: &str, session_id: &str, kind: EventKind, reaction: Option<Reaction>, payload: &str) -> EventId` — calls `bowerbird_daemon::projection::session::write(&state.db.writer, &state.broadcaster, EventEnvelope { ... })` so the test exercises the REAL publish path, not a synthetic `broadcaster.publish(...)` shortcut
  - [x] **AC #1 — three subscribers see identical event in identical order**: `three_subscribers_receive_identical_events_in_order` — connects three WS clients to `events.*`, publishes two events via `publish_via_projection`, asserts byte-identical wire text across all three subscribers AND event-id ordering matches publication order
  - [x] **AC #2 — state.session.<id>.current_state only delivers for matching session**: `state_current_topic_filters_other_sessions` — subscribes to `state.session.sess-A.current_state`; positive match (sess-A), negative match (sess-B, 300ms timeout asserts no frame), positive match (sess-A again)
  - [x] **AC #3 — source-scoped filtering**: `events_source_filter_excludes_other_source` — subscribes to `events.claude.*`, publishes claude event via the real path (frame received), then injects a synthetic codex event via `state.broadcaster.publish(...)` (the documented test-only synthetic publish), asserts no frame for codex within 300ms
  - [x] **AC #4 — state.session.* wildcard across concurrent sessions**: `state_wildcard_preserves_session_id_per_frame` — subscribes to `state.session.*`, publishes for `["sess-A", "sess-B", "sess-A", "sess-C"]`, asserts each State frame's session_id matches publication order
  - [x] **AC #5 — consumer independence**: `consumer_independence_and_semaphore_release` — two clients with `events.*`, close A, publish, assert B receives; then connect a third client with `ws_max_conns = 2` to prove A's semaphore permit was released
  - [x] **AC #6 — publish gated on commit success**: deferred per the story spec (optional integration test for deterministic commit-fail); captured in `deferred-work.md`. Verified by code-reading: `broadcaster.publish` is statically positioned after `interact_res?` in `crates/daemon/src/projection/session.rs::write`, making it unreachable on commit failure
  - [x] **AC #7 — sentinel events not published**: `sentinel_writes_are_not_published` — subscribes to `events.*`, calls `write_recording_started` and `write_recording_ended` directly, asserts no frame within 300ms for either, then publishes a non-sentinel event and asserts it DOES arrive

- [x] Task 5: Protocol changelog update (AC: #1, #2)
  - [x] Append one `behavioral` entry to `docs/protocol-changelog.md` under the `v1.0 → v1.1` heading
  - [x] No `schema` entry needed — Story 2.1 already added `StateFrame` and the `ServerMessage::Event(EventFrame)` variant

- [x] Task 6: Update deferred-work and CHANGELOG (AC: n/a)
  - [x] In `docs/bmad/implementation-artifacts/deferred-work.md`, appended "Deferred from: Story 2.2" section covering (a) the deterministic commit-failure integration test for AC #6, and (b) the hook→presenter p99 ≤100ms Criterion benchmark
  - [x] No `epics.md` or `prd.md` back-amends needed — the existing Story 2.2 ACs in `epics.md` already match this story's interpretation

- [x] Task 7: Verify and merge (AC: all)
  - [x] `cargo fmt --check` — clean
  - [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean
  - [x] `cargo test --workspace` — see Dev Agent Record → Completion Notes for the latest authoritative test count (initial landing: 169 = 163 pre-2.2 + 6 new; rounds 1-4 added more — Dev Agent Record tracks the current totals)
  - [x] `cargo build --examples` — no examples in workspace (no-op warning, expected)
  - [ ] Open PR titled `feat(story-2.2): real-time event and state broadcast to multiple tools`

### Review Findings

- [x] [Review][Patch] Replace fixed sleeps used as WS synchronization with observable readiness / retry helpers [crates/daemon/tests/contract_daemon.rs:3139, crates/daemon/tests/contract_daemon.rs:3226, crates/daemon/tests/contract_daemon.rs:3295, crates/daemon/tests/contract_daemon.rs:3349, crates/daemon/tests/contract_daemon.rs:3392, crates/daemon/tests/contract_daemon.rs:3401, crates/daemon/tests/contract_daemon.rs:3443]
  - **Problem:** The new Story 2.2 WS tests send a `Subscribe` frame, sleep for 20ms, then publish. `consumer_independence_and_semaphore_release` also sleeps 50ms after closing a client before probing semaphore release. Under slow CI or scheduler pressure, the daemon may not have processed the subscribe/close yet. A publish can then be drained under the old empty subscription set, or the reconnect probe can run before the closed task drops its permit, producing a false failure.
  - **Fix direction:** Add deterministic helpers in `story_2_2_publish` instead of fixed sleeps. For subscribe readiness, subscribe to the target topic and publish a harmless probe envelope for that same topic in a bounded retry loop until the client receives it, then proceed with the assertion event(s). Use a unique probe `session_id` per test so the probe cannot be mistaken for the asserted frame. For permit release, retry the third `connect_authed` attempt until it succeeds or a short deadline expires, matching the existing retry pattern in `story_2_1_ws::ws_257th_connection_rejected_503`.
  - **Verification:** Run `cargo test -p bowerbird-daemon --test contract_daemon story_2_2_publish`. The tests should pass without any `tokio::time::sleep(...)` calls in the new Story 2.2 module except sleeps used inside explicit bounded retry backoff.
- [x] [Review][Patch] Add a runtime sentinel guard in `projection::session::write` so release builds cannot publish sentinel events through the public write path [crates/daemon/src/projection/session.rs:52]
  - **Problem:** `projection::session::write` has a `debug_assert!` rejecting `RecordingStarted` / `RecordingEnded`, but debug assertions disappear in release builds. Because `write` is public and accepts any `EventEnvelope`, a future caller could accidentally pass a sentinel kind through this path. The function would commit it and then publish `BroadcastEnvelope::Event` + `BroadcastEnvelope::State`, violating AC #7's requirement that sentinel lifecycle events never reach the user-facing WS surface.
  - **Fix direction:** Convert the debug-only guard into a runtime check at the top of `write`. If `envelope.kind` is `RecordingStarted` or `RecordingEnded`, return an error before pool checkout or transaction work. Use an existing daemon error variant if acceptable, or add a narrow error variant such as `Error::Projection(String)` if that reads better than overloading `Error::Ingest`. Keep the `write_recording_started` / `write_recording_ended` paths unchanged and still without a broadcaster parameter.
  - **Verification:** Add a contract test that calls `projection::session::write(&state.db.writer, &state.broadcaster, EventEnvelope { kind: RecordingStarted, source: "__daemon__", session_id: "__daemon__", ... })`, asserts it returns `Err`, and asserts an `events.*` subscriber receives no frame. Existing `sentinel_writes_are_not_published` should still pass.
- [x] [Review][Patch] Remove contradictory "NOT YET wired" wording from the Story 2.1 changelog entry now that Story 2.2 wires publishing [docs/protocol-changelog.md:13]
  - **Problem:** The `v1.0 -> v1.1` changelog now contains both the old Story 2.1 statement that event publishing into the broadcast hub is "NOT YET wired" and the new Story 2.2 statement that `event` and `state` frames are broadcast. Because both entries live under the same release heading, readers can reasonably interpret the release behavior either way.
  - **Fix direction:** Edit only the Story 2.1 changelog bullet to make it historical and non-contradictory. For example, replace the final sentence with wording like: "Story 2.1 introduced the WS surface and topic grammar; Story 2.2 wires live event/state publishing." Keep the Story 2.2 behavioral entry as the authoritative current behavior.
  - **Verification:** Re-read `docs/protocol-changelog.md` under `v1.0 -> v1.1` and confirm there is no current-tense claim that WS event publishing is unwired.
- [x] [Review][Patch] Enforce a minimum broadcast capacity of at least two envelopes now that each successful write publishes Event then State [crates/daemon/src/broadcast/hub.rs:19]
  - **Problem:** `BroadcastHub::new(capacity)` passes the configured capacity directly to `tokio::sync::broadcast::channel`. Story 2.2 now publishes two envelopes for every committed event: Event first, then State. If capacity is configured to `1`, the State publish can evict the Event before a slow subscriber reads, so a subscriber to both `events.*` and `state.session.*` may observe only the State and miss the causal Event. Capacity `0` is also invalid for Tokio broadcast channels.
  - **Fix direction:** Clamp or validate capacity at hub construction. The smallest safe local fix is `let capacity = capacity.max(2);` inside `BroadcastHub::new`, with a doc comment explaining that Story 2.2 publishes Event + State as a pair and capacity must hold at least one pair. If config validation exists elsewhere later, it can reject invalid user input earlier, but the hub should remain defensive.
  - **Verification:** Add a unit test for `BroadcastHub::new(1)` or an integration-style test that subscribes, publishes one committed event, and confirms both Event and State are receivable. If asserting hub internals is awkward, use a small test in `broadcast::hub` that constructs capacity `1`, subscribes, publishes two envelopes, and expects both to be readable after the clamp.
- [x] [Review][Defer] Shutdown-drain publishes are not guaranteed to reach attached WS clients because WS tasks also exit on the shared shutdown token [crates/daemon/src/ingest/writer.rs:30, crates/daemon/src/api/ws.rs:208] — deferred, pre-existing
  - **Problem:** Story 2.2 correctly threads the broadcaster into the ingest writer's shutdown-drain loop, but production WS connection tasks also use the same shutdown token and a biased select that exits before reading more broadcast envelopes. During shutdown, events drained by the writer can be committed and published after WS tasks have already stopped dispatching, so the comment in `writer.rs` overstates what production currently guarantees.
  - **Why deferred:** This is a shutdown sequencing design issue, not a Story 2.2 publish-wiring bug. Fixing it needs a two-phase shutdown model, separate ingest/WS cancellation tokens, or a deliberate WS drain window, which belongs with Story 2.5's graceful-shutdown notification design.
  - **Future fix direction:** In Story 2.5, define the shutdown order explicitly: stop accepting new ingest, drain accepted ingest and publish, allow WS tasks to flush pending broadcast frames or send shutdown frames, then close sockets.
- [x] [Review][Defer] State topic matching remains `session_id`-only, so future multi-source session-id collisions need source-scoped state topics or an equivalent protocol shape [crates/daemon/src/broadcast/event.rs:130] — deferred, pre-existing
  - **Problem:** `BroadcastEnvelope::State` carries both `source` and `session_id`, but `Topic::StateSession` and `Topic::StateSessionCurrent` match only on `session_id`. If a future second adapter publishes `codex/sess-1` while a client intended to observe `claude/sess-1`, the current `state.session.sess-1` topic would deliver both.
  - **Why deferred:** V1 only has the `"claude"` production source, and this limitation already exists in the REST `/sessions/{id}` disambiguation story. Adding source-scoped state topics now would be a protocol expansion beyond Story 2.2's stated "no new wire shapes / no new topic grammar" scope.
  - **Future fix direction:** When a second adapter ships, add a source-qualified state subscription shape such as `state.<source>.session.<id>` or an equivalent protocol-level disambiguator, and update REST session lookup at the same time so `(source, session_id)` remains the natural key everywhere.
- [x] [Review][Patch] Make WS subscribe-readiness probes unique per attempt so delayed probe frames cannot leak into assertions [crates/daemon/tests/contract_daemon.rs:3087, crates/daemon/tests/contract_daemon.rs:3142, crates/daemon/tests/contract_daemon.rs:3190]
  - **Problem:** `wait_subscribe_live` and `wait_subscribe_live_all` publish identical `__probe__` envelopes and then rely on short 10ms/20ms "queue empty" drains before returning. If WS dispatch is delayed beyond that drain timeout, a prior probe can arrive after the helper returns and the next real assertion can read the stale probe instead of the intended event/state frame.
  - **Fix direction:** Make each probe attempt uniquely identifiable and return only after the latest probe has been observed. One concrete shape: add a monotonic `ProbeToken(u64)` generated inside the helper loop. For event probes, encode the token in `Event.event_id` and/or `Event.session_id = "__probe__-<token>"`. For state probes, keep the requested `session_id` unchanged when needed for `state.session.<id>` matching, and encode the token in `StateFrame.source = "__probe__-<token>"`. Replace `is_probe_frame(&Message) -> bool` with a parser that returns the observed token. In `wait_subscribe_live_all`, publish one token per iteration and exit only when every client has observed that same token; broadcast ordering then proves no older probe can arrive later on those clients.
  - **Validation:** Confirm the helpers no longer depend on "queue empty" drain timeouts as proof of readiness, and run `cargo test -p bowerbird-daemon --test contract_daemon story_2_2_publish` repeatedly enough to shake scheduler timing.
- [x] [Review][Patch] Add an ingest-socket -> writer -> WS subscriber contract test so Story 2.2 covers the actual daemon ingress wiring [crates/daemon/src/ingest/writer.rs:23, crates/daemon/src/ingest/writer.rs:36, crates/daemon/tests/contract_daemon.rs:3280]
  - **Problem:** The Story 2.2 fan-out tests drive `projection::session::write` directly with `state.broadcaster`, so they verify the canonical publisher but not the new `ingest::writer::run` wiring. A future mistake could persist real hook events through the ingest writer while using the wrong broadcaster, or no broadcaster, and these tests would still pass.
  - **Fix direction:** Add one contract test in `story_2_2_publish` that combines the existing WS harness with the ingest harness from `ingest_no_db_row_on_missing_hook_kind` (lines ~858-895). Create `state = default_state(pools, 4)`, spawn the WS server with `spawn_test_daemon(state.clone())`, and manually spawn `ingest::listener::run(sock_path, tx, ingest_shutdown.clone(), adapter)` plus `ingest::writer::run(rx, state.db.writer.clone(), state.broadcaster.clone(), ingest_shutdown.clone())`. Connect a WS client, subscribe to `events.*`, wait for readiness, then send a valid hook line through the Unix socket using the existing `send_line_recv_response` helper: `{"hook_kind":"PreToolUse","session_id":"sess-ingest-broadcast","tool_name":"Test"}\n`.
  - **Validation:** Assert the Unix socket response starts with `200`, then read the WS client and assert it receives `ServerMessage::Event` with `source == "claude"` and `session_id == "sess-ingest-broadcast"`. End the test by cancelling both the ingest shutdown token and `state.shutdown`. Run the new targeted test plus `cargo test -p bowerbird-daemon --test contract_daemon story_2_2_publish`.
- [x] [Review][Patch] Add a cross-topic ordering test for a client subscribed to both `events.*` and `state.session.*` [crates/daemon/src/projection/session.rs:172, crates/daemon/src/projection/session.rs:173, crates/daemon/tests/contract_daemon.rs:3305, crates/daemon/tests/contract_daemon.rs:3405]
  - **Problem:** `projection::session::write` intentionally publishes Event before State, but current tests split event-only and state-only subscribers. A regression that reverses the two publishes could still pass the current Story 2.2 test module while breaking the presenter model promised by the write doc-comment and AC #1/#2 combination.
  - **Fix direction:** Add a WS test with one client subscribed to both `events.*` and `state.session.*`. Send both subscribe frames, wait for event and state readiness using the fixed probe helper, then publish one event through `publish_via_projection(&state, "claude", "sess-order", EventKind::PreToolUse, None, "{}")`.
  - **Validation:** Read exactly the next two WS frames. Assert frame 1 is `ServerMessage::Event` with the returned `EventId` and `session_id == "sess-order"`. Assert frame 2 is `ServerMessage::State` with `source == "claude"` and `session_id == "sess-order"`. Run `cargo test -p bowerbird-daemon --test contract_daemon story_2_2_publish`.
- [x] [Review][Patch] Reject reserved `__daemon__` source through `projection::session::write` [crates/daemon/src/projection/session.rs:56]
  - **Problem:** The new runtime guard rejects only `RecordingStarted` / `RecordingEnded`. A future caller can still pass `source = "__daemon__"` with a non-sentinel kind such as `PreToolUse`; that row is excluded from REST/session queries by `source != '__daemon__'`, but Story 2.2 would publish it to `events.*` and `state.session.*`.
  - **Fix direction:** Treat `__daemon__` as a reserved source at the same boundary as the sentinel-kind guard. Return `Err(Error::Projection(...))` before pool checkout when `envelope.source == "__daemon__"` in `projection::session::write`; keep `write_recording_started` / `write_recording_ended` as the only writers allowed to create daemon-lifecycle rows.
  - **Validation:** Add a contract test that calls `projection::session::write` with `source = "__daemon__"` and a non-sentinel kind, asserts `Err`, verifies no DB row is inserted, and verifies an `events.*` subscriber receives no frame.
- [x] [Review][Patch] Add a daemon-side ingest line/payload cap before broadcast fan-out [crates/daemon/src/ingest/handler.rs:29]
  - **Problem:** `ingest::handler` uses `read_line` into an unbounded `String`. Before Story 2.2 this could already push oversized payloads into SQLite from a direct local socket client; now the same payload is also cloned into the broadcast ring, serialized as an `Event`, and sent to every matching WS subscriber. The shim has a 1 MiB stdin cap, but a direct socket writer can bypass it.
  - **Fix direction:** Add a daemon-side max ingest line cap, ideally matching the shim's 1 MiB payload cap plus newline slack. Reject oversized input before JSON parse, enqueue, DB write, or broadcast with a deterministic 400/413-style line response.
  - **Validation:** Add a contract test that writes an oversized line to the Unix socket and asserts the response is rejection, no DB row is inserted, and no WS frame is broadcast.
- [x] [Review][Patch] Tighten Story 2.2 WS coverage for state contents, exact topic forms, abrupt close, and duplicates [crates/daemon/tests/contract_daemon.rs:3438]
  - **Problem:** The current Story 2.2 tests cover the main fan-out path, but some AC language is only partially asserted: state tests mostly check `session_id` rather than full `source` + `SessionState`; AC #5 says graceful or abrupt close and no duplicate frames but the test uses a graceful close and reads only one surviving frame; live tests do not cover exact topic forms `events.<source>.<session_id>` and `state.session.<id>`.
  - **Fix direction:** Extend the existing `story_2_2_publish` module rather than adding new helpers: assert full `StateFrame` fields after `PreToolUse` / `PostToolUse`, add an abrupt disconnect/no-duplicate consumer-independence case, and add focused live-path tests for the exact event and state topic forms.
  - **Validation:** Run `cargo test -p bowerbird-daemon --test contract_daemon story_2_2_publish`.
- [x] [Review][Patch] Soften the shutdown-drain comment so it does not promise WS delivery after shared shutdown cancellation [crates/daemon/src/ingest/writer.rs:31]
  - **Problem:** The code comment says drained events "MUST still publish so any WS clients still attached during graceful shutdown receive them", but the already-deferred shutdown sequencing issue notes production WS tasks also exit on the shared shutdown token and may stop dispatching before the writer drains. The code does publish during drain; the comment over-promises delivery.
  - **Fix direction:** Reword the comment to say the writer still commits and publishes accepted events during drain, while actual WS flush semantics remain deferred to Story 2.5's graceful-shutdown design.
  - **Validation:** Comment-only change; no test needed beyond `cargo fmt --check`.
- [x] [Review][Patch] Update stale story completion notes and test counts after review patches [docs/bmad/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md:381]
  - **Problem:** The story artifact now has `Status: review`, but `Story Completion Status` still says the story is `ready-for-dev`. The Dev Agent Record also says 6 new tests / 169 total even though later review patches brought Story 2.2 to 9 targeted tests and 173 workspace tests.
  - **Fix direction:** Normalize the completion status and notes to the current state before merge: `review`, 9 Story 2.2 tests, 173 workspace tests, and the updated file-list/changelog wording.
  - **Validation:** Re-read the story artifact and confirm no current-state section contradicts the top-level `Status: review`.
- [x] [Review][Defer] Define escaping or validation for topic-reserved `source` / `session_id` characters [crates/daemon/src/broadcast/event.rs:86] — deferred, pre-existing
  - **Problem:** The topic grammar from Story 2.1 splits on `.` and reserves `*`, while the ingest path accepts arbitrary non-empty `session_id` strings. A session id such as `abc.def` or `*` can be stored and broadcast, but clients cannot subscribe exactly to `events.claude.<session_id>`, `state.session.<id>`, or `state.session.<id>.current_state` without an escaping rule or identifier validation.
  - **Why deferred:** This is a protocol/topic grammar design issue introduced with the Story 2.1 subscription syntax. Story 2.2 makes live delivery observable but does not define a new topic grammar. The correct fix requires choosing between rejecting topic-reserved identifier characters at ingest/write boundaries or adding an escaped/structured subscription form.
  - **Future fix direction:** Before arbitrary adapter/session identifiers are treated as supported protocol input, decide and document the identifier grammar or escaping scheme, then add contract tests for dotted and `*` session IDs.
- [x] [Review][Patch] Raise the daemon ingest wire cap so shim-valid near-1 MiB payloads are not rejected [crates/daemon/src/ingest/handler.rs:15]
  - **Problem:** `MAX_INGEST_LINE_BYTES` is set to exactly `1 << 20`, and `handle` reads `MAX + 1` bytes before rejecting a line that has no newline. The shim's cap applies to raw stdin before it injects `"hook_kind"` and appends the newline. A stdin JSON object that is valid at the shim cap can therefore become a wire line larger than `1 << 20` after `serde_json::to_vec`, `hook_kind` insertion, and newline framing, causing the daemon to return `400 line exceeds ...` for an event the shim accepted.
  - **Fix direction:** Separate the daemon's wire-line cap from the shim's raw stdin cap. Use a daemon cap that allows shim overhead, for example `MAX_INGEST_LINE_BYTES = SHIM_STDIN_CAP + 4096` or a named `MAX_INGEST_WIRE_LINE_BYTES`, while still rejecting clearly oversized direct-socket clients before parse/enqueue/broadcast. Keep the response deterministic. Add a contract test that drives the real shim-to-daemon path with a valid JSON object near the shim cap and asserts the daemon accepts it, plus keep the direct oversized socket rejection test.
  - **Verification:** Run `cargo test -p bowerbird-daemon --test contract_daemon story_2_2_publish::ingest_oversized_line_rejected_before_broadcast` and the shim round-trip contract that covers the near-cap payload.
- [x] [Review][Patch] Normalize broadcast capacity to Event+State pair boundaries, not just a minimum of two [crates/daemon/src/broadcast/hub.rs:32]
  - **Problem:** `BroadcastHub::new` floors `0` and `1` to capacity `2`, but odd capacities still allow orphaned state frames after lag. With `ws_broadcast_capacity = 3`, two committed writes publish `Event1, State1, Event2, State2`; a lagged receiver can lose `Event1` while the ring still retains `State1, Event2, State2`. The WS task logs `RecvError::Lagged` and continues, so a dual-topic subscriber may then receive a `State` frame without the causal `Event`.
  - **Fix direction:** Normalize the effective capacity to an even number after applying the minimum, or reject odd configured capacities during config validation. The local defensive fix is inside `BroadcastHub::new`: `let capacity = capacity.max(Self::MIN_CAPACITY); let capacity = capacity + (capacity % 2);`. Update the doc comment to say the capacity is rounded to whole Event+State pairs.
  - **Verification:** Extend `capacity_floor_holds_event_state_pair` or add a sibling unit test for raw capacity `3`: subscribe, publish two Event+State pairs without reading, observe `Lagged`, then assert the next deliverable envelope starts on an `Event` boundary rather than `State1`.
- [x] [Review][Patch] Update stale `BroadcastHub` comments that still describe Story 2.1 pre-publish behavior [crates/daemon/src/broadcast/hub.rs:6]
  - **Problem:** The module comment still says Story 2.1 exposes `subscribe` / `publish` and "no daemon code publishes yet"; the `publish` doc comment still says Story 2.1 does not publish from daemon code. Story 2.2 now publishes from `projection::session::write`, so the comments contradict the implementation and protocol changelog.
  - **Fix direction:** Rewrite the module and method comments to say Story 2.1 introduced the hub and Story 2.2 wires production publishing through `projection::session::write`. Keep the note that tests may use `publish` as a synthetic publisher for topic-filter coverage.
  - **Verification:** Re-read `crates/daemon/src/broadcast/hub.rs` and confirm there is no current-tense claim that daemon publishing is unwired.
- [x] [Review][Patch] Refresh the Task 7 verification block after post-review test additions [docs/bmad/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md:86]
  - **Problem:** Task 7 still says `cargo test --workspace` was `169 passed (163 pre-2.2 + 6 new 2.2 tests)`, while the Dev Agent Record later says Story 2.2 now has 14 module tests plus one hub unit test and a workspace total of 178. Both statements are in the same story artifact, so a dev cannot tell which verification result is authoritative.
  - **Fix direction:** Update Task 7 to the current verification result from the latest review pass, or replace the exact count with a pointer to the Dev Agent Record's latest test-count line. Keep the command list itself intact.
  - **Verification:** Re-read the story artifact and confirm no current-state verification block contradicts the Dev Agent Record.
- [x] [Review][Patch] Add positive sentinel checks after terminal negative WS timeout assertions [crates/daemon/tests/contract_daemon.rs:3601]
  - **Problem:** Several Story 2.2 negative tests end immediately after "no frame within 300ms" (`events_source_filter_excludes_other_source`, `projection_write_rejects_daemon_source_at_runtime`, `exact_event_and_state_topic_forms_match_live`, and `ingest_oversized_line_rejected_before_broadcast`). If a forbidden frame is delayed beyond the timeout, the test can pass before the bad frame becomes visible.
  - **Fix direction:** After each terminal negative timeout, publish a positive matching sentinel event/frame and assert the next frame is that sentinel. Broadcast ordering makes any delayed forbidden frame arrive first and fail deterministically. For example, after the synthetic `codex` event timeout in `events_source_filter_excludes_other_source`, publish a real `claude` event via `publish_via_projection` and assert the next `EventFrame` carries that returned `EventId` and sentinel `session_id`.
  - **Verification:** Run `cargo test -p bowerbird-daemon --test contract_daemon story_2_2_publish`; the tests should still pass and should no longer end on a pure silence assertion when a later positive match can prove queue ordering.

## Dev Notes

### What Story 2.1 already shipped (do NOT redo)

Story 2.1 was the watershed story for Epic 2. It established the entire WS surface, including the BroadcastHub plumbing and a `publish` method designed for Story 2.2 to call. Specifically, ALL of the following already exist and work:

- `crates/daemon/src/broadcast/hub.rs::BroadcastHub` with `subscribe()` returning a `broadcast::Receiver<BroadcastEnvelope>` and a `publish(envelope)` method that swallows `SendError` on zero-subscribers (the normal idle daemon state). Channel capacity defaults to `config.ws_broadcast_capacity` (1024).
- `crates/daemon/src/broadcast/event.rs::BroadcastEnvelope` with `Event(Event)` and `State { source, session_id, state }` variants — the wire-projection in `dispatch_envelope` (in `crates/daemon/src/api/ws.rs`) already maps these to `ServerMessage::Event(EventFrame { event })` and `ServerMessage::State(StateFrame { source, session_id, state })`.
- `crates/daemon/src/broadcast/event.rs::Topic` parsing + `matches` for all six grammar arms (`events.*`, `events.<source>.*`, `events.<source>.<session_id>`, `state.session.*`, `state.session.<id>`, `state.session.<id>.current_state`).
- `crates/daemon/src/api/ws.rs` per-connection task — subscribes to the hub PRE-upgrade (so events committed between subscribe-time and Hello-send are not lost), dispatches via `dispatch_envelope` (dedups overlapping topic matches so one envelope = one wire frame), drains pre-Subscribe backlog under the prior subscription state, handles Lagged at WARN.
- `protocol::ServerMessage::Event(EventFrame)` / `protocol::ServerMessage::State(StateFrame)` — both wire shapes are stable, both deserialize permissively, the `Unknown` catch-all covers future variants.
- `make_test_state_with_ws` + `spawn_test_daemon` test helpers in `crates/daemon/tests/contract_daemon.rs` — Story 2.2 tests reuse them as-is.

**The single missing piece** is the call to `broadcaster.publish(...)` from `projection::session::write`. Story 2.1's `BroadcastHub::publish` doc-comment literally says "Story 2.2 wires this into `projection::session::write`." This story closes that loop.

### Why publish from `projection::session::write` specifically

The publish must happen exactly where the load-bearing correctness invariant from architecture.md:880 lives: the projection UPSERT + event INSERT in a single transaction. Two reasons:

1. **AC #6 — commit-gating.** Publishing before `tx.commit()?` returns `Ok` means a presenter could see an event that the DB later rolls back. The publish call must be unreachable on commit failure.
2. **Single canonical publisher (mirrors the single-writer transaction invariant).** Any other publisher would be a new responsibility splitting the substrate's contract. Story 2.1's anti-pattern list explicitly forbids "adding a publish call to `BroadcastHub` anywhere outside `crates/daemon/src/broadcast/`" during 2.1; 2.2 narrows that to "outside `crates/daemon/src/projection/session.rs::write`."

The `write_recording_started` and `write_recording_ended` functions also commit transactions and could publish — but their events carry `source = "__daemon__"` and `session_id = "__daemon__"`, which are deliberately excluded from the user-facing session surface (see Story 1.7's exclusion clause in `SELECT_DISTINCT_SESSIONS_FROM_EVENTS` and `SELECT_HELLO_DB_FIELDS`). Publishing them would surface daemon lifecycle as if it were agent activity. Hence Task 2 explicitly excludes sentinel writers.

### Files this story TOUCHES (UPDATE)

Verify line numbers in source before editing (Story 1.7 / 1.8 / 2.1 noted these drift):

| File | Change | Why |
|---|---|---|
| `crates/daemon/src/projection/session.rs` | Extend `write` signature with `&BroadcastHub`; widen the interact closure return to `(i64, SessionState)`; publish Event + State after `interact_res?` | Task 2 |
| `crates/daemon/src/ingest/writer.rs` | Accept `Arc<BroadcastHub>` parameter; pass `&broadcaster` to both `projection::session::write` callsites | Tasks 1, 3 |
| `crates/daemon/src/main.rs::run` | Pass `broadcaster.clone()` (the `Arc<BroadcastHub>`) into the `ingest::writer::run` spawn at line ~151–155 | Task 1 |
| `crates/daemon/tests/contract_daemon.rs` | Add `story_2_2_publish` module with five+ contract tests | Task 4 |
| `docs/protocol-changelog.md` | One `behavioral` entry under `v1.0 → v1.1` | Task 5 |
| `docs/bmad/implementation-artifacts/deferred-work.md` | Append Story 2.2 section if needed (probably none) | Task 6 |

### Files this story CREATES (NEW)

None. All work is in existing files. This is a wiring story, not a structural one.

### Existing files the dev MUST read before editing (context, no changes)

| File | What to learn from it |
|---|---|
| `crates/daemon/src/broadcast/hub.rs` | The `publish` method already exists; signature is `pub fn publish(&self, envelope: BroadcastEnvelope)`. Pass by `&BroadcastHub`, not `Arc<BroadcastHub>` — the `Arc` is for sharing ownership across tasks; the publish method itself only needs `&self`. |
| `crates/daemon/src/api/ws.rs::dispatch_envelope` | The wire-projection from `BroadcastEnvelope` to `ServerMessage` is already done here. Story 2.2 adds NO wire-projection logic — it only feeds the upstream side of the channel. |
| `crates/daemon/src/projection/session.rs::write` | The current shape: pool checkout → interact closure with one transaction containing UPSERT + INSERT → return `EventId`. The closure-return widening must preserve the existing error mapping (`Error::Pool` for pool/interact errors, `?` propagation for rusqlite errors). |
| `crates/daemon/src/projection/state.rs::transition` | The pure state-transition function `transition(prev, kind, now_ms) -> SessionState`. `write` already calls this inside the closure to produce `new_state`; Story 2.2 just needs to return this `new_state` from the closure (don't recompute it outside the transaction — that would race with concurrent writes, even though the single-writer pool means only one writer exists in practice). |
| `crates/daemon/src/ingest/writer.rs` | The current `run(rx, writer_pool, shutdown)` signature; both call paths (`recv` arm and shutdown-drain arm) invoke `projection::session::write`. Both need to pass `&broadcaster` after Task 1. |
| `crates/daemon/src/main.rs::run` (lines 143–180) | Where `broadcaster: Arc<BroadcastHub>` is constructed and moved into `AppState`. Story 2.2 needs to `.clone()` the `Arc` BEFORE the `AppState` literal so it's available for the `ingest::writer::run` spawn. The pattern is the same one already used for `pools.writer.clone()` and `shutdown.clone()`. |
| `crates/daemon/tests/contract_daemon.rs::story_2_1_ws` (lines 2210–end) | The test scaffolding to reuse — `spawn_test_daemon`, `connect_authed`, `read_text_frame_or_close`, `parse_hello`. The new `story_2_2_publish` module sits next to this one. |
| `docs/bmad/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md` | Previous story's Dev Notes — confirms the "publish from `projection::session::write` in Story 2.2" wiring decision. Section "Anti-patterns" lists this. |

### `write` signature change — the exact shape

Current (`crates/daemon/src/projection/session.rs:33`):

```rust
pub async fn write(
    writer_pool: &deadpool_sqlite::Pool,
    envelope: EventEnvelope,
) -> Result<EventId> { ... }
```

After Story 2.2:

```rust
pub async fn write(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
) -> Result<EventId> { ... }
```

Inside the function, the existing closure already computes `new_state` via `transition`. Widen the closure return type and return both values, then publish in the outer scope. Sketch:

```rust
// 1. Capture cloned fields for the post-commit Event construction.
//    The originals move into the closure; clones live in the outer scope.
let source = envelope.source.clone();
let session_id = envelope.session_id.clone();
let kind = envelope.kind.clone();
let reaction = envelope.reaction.clone();
let payload_for_event = envelope.payload.clone();
// (existing variables `source`, `session_id`, etc. used by the closure already
//  shadow these; the explicit pre-clone is to make ownership obvious. Adjust
//  to whichever variable form the existing code uses — minimize duplication.)

let interact_res = conn
    .interact(move |c| -> rusqlite::Result<(i64, SessionState)> {
        let tx = c.transaction()?;
        // ... existing prev_state SELECT and transition() call ...
        let new_state = transition(prev_state.as_ref(), kind_for_transition, now_ms);
        // ... existing UPSERT + INSERT ...
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok((id, new_state))
    })
    .await
    .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

let (event_id_raw, new_state) = interact_res?;
let event_id = EventId(event_id_raw);

// Post-commit publish — Event first, then State (see ordering note below).
let event = Event {
    event_id,
    source: source.clone(),
    session_id: session_id.clone(),
    kind,
    reaction,
    payload: payload_for_event,
    created_at: now_ms,
};
broadcaster.publish(BroadcastEnvelope::Event(event));
broadcaster.publish(BroadcastEnvelope::State {
    source,
    session_id,
    state: new_state,
});
tracing::debug!(event_id = event_id.0, "ws: published event + state envelopes");

Ok(event_id)
```

Ordering: `Event` first, `State` second. Rationale: a presenter that consumes both `events.*` AND `state.session.*` expects "this event happened" → "and here is the resulting state." `tokio::sync::broadcast` is single-channel for `BroadcastEnvelope` (Story 2.1's design — one envelope type, not separate channels per topic class), so per-channel ordering across the two `publish` calls is preserved for every subscriber. Different subscribers may see them interleaved with subsequent publishes if they're lagging, but never in reverse order within their own stream.

### `tokio::sync::broadcast` semantics — what's guaranteed

Story 2.1 chose `tokio::sync::broadcast` deliberately. The guarantees that Story 2.2's ACs depend on:

- **Identical content across receivers (AC #1).** `Sender::send(value)` clones `value` into each `Receiver`'s ring buffer (the broadcast channel internally uses `Arc<T>` semantics; receivers see `Clone`d values from the same source). Two receivers reading the same position see byte-identical data — they cannot observe different versions of the same envelope.
- **Same order across receivers (AC #1).** Each receiver maintains its own cursor into the channel's ring buffer; cursors only advance, never rewind. Two receivers reading from position 5 to position 10 see envelopes in positions 5, 6, 7, 8, 9, 10 in that order. There is no per-receiver reordering.
- **Per-subscriber lag detection (Story 2.4 territory, not 2.2).** When a slow receiver falls behind the broadcast capacity, its next `recv` returns `Err(RecvError::Lagged(n))`. Story 2.1 already handles this at WARN level in the WS per-connection task; Story 2.4 will project it to a `DroppedFrame`. Story 2.2 doesn't change this path.
- **Send never blocks (AC #1 latency).** `broadcast::Sender::send` returns immediately. Slow receivers do not back-pressure the publisher; they accumulate lag and eventually see `Lagged`. This is what makes the hook→presenter latency budget achievable: `projection::session::write` is on the hot path and its publish step must never await receivers.

### Anti-patterns (explicitly forbidden)

- **Publishing before `tx.commit()?` returns `Ok`.** The publish is unreachable on commit failure (AC #6). Any publish inside the `interact` closure violates this.
- **Publishing from anywhere other than `projection::session::write`.** Mirrors the single-writer transaction invariant. If you find yourself calling `broadcaster.publish` from `ingest/handler.rs` or `api/sessions.rs` or anywhere else outside `projection/session.rs`, stop — you're outside Story 2.2's scope. (Test-only synthetic publishes in `contract_daemon.rs::story_2_2_publish` are the documented exception.)
- **Publishing from `write_recording_started` / `write_recording_ended`.** Sentinel events are excluded from the user-facing surface (AC #7). Adding a publish call here would surface daemon lifecycle as session activity.
- **Cloning the entire `Arc<BroadcastHub>` per-call inside `write`.** Pass `broadcaster: &BroadcastHub` — `publish` only needs `&self`. Cloning the `Arc` is for moving across task/thread boundaries, which `write` does not do.
- **Reordering Event after State.** AC #1's "same order" is observable: a `state` frame for an event the presenter hasn't seen yet violates the substrate's mental model. Always Event → State.
- **Calling `transition` twice.** The closure already computes `new_state` for the UPSERT. Return it from the closure; do not recompute outside the transaction. Re-running `transition` against the post-commit DB would also race against subsequent writes.
- **Using `try_send` / `send_replace` / `try_broadcast`.** `tokio::sync::broadcast::Sender::send` is the right call — it never blocks and returns `Ok` even with zero subscribers. The `BroadcastHub::publish` wrapper already does the right thing (`let _ = self.tx.send(envelope)`); just call it.
- **Adding a new `BroadcastEnvelope` variant.** Story 2.4 adds `Dropped`; Story 2.5 adds `ShutdownClose` (per the existing TODO in `event.rs`). Story 2.2 ships zero envelope variants. If you think you need one, you're over-scoping.
- **Logging the event payload.** `tracing::debug!(event_id = ..., ...)` is fine. `tracing::debug!(?event)` or `tracing::debug!(payload = %payload)` is not — the payload can contain user data including tool-output bytes.

### Library/version pins (verified against `Cargo.toml`)

No new dependencies. Story 2.2 uses crates already in the workspace:

| Crate | Version | Use |
|---|---|---|
| `tokio` | `1.52.1` (sync feature already on) | `tokio::sync::broadcast` via the existing `BroadcastHub` |
| `protocol` | workspace path | `Event`, `EventId`, `EventEnvelope`, `SessionState`, `EventKind`, `Reaction` |
| `serde_json` | `1.0.149` | Only for the existing `state_json` serialize call inside the closure |
| `tracing` | `0.1.44` | New `tracing::debug!` line |
| `deadpool-sqlite` | already pinned | The `interact` closure return type widens — no API change |

No new dev-deps either; `tokio-tungstenite` and `futures-util` are already there from Story 2.1.

### Project-context references for invariants this story must hold

- **Transaction invariant (`architecture.md:634-641`, `project-context.md:700`).** The UPSERT + INSERT are the only operations in the SQLite transaction. Story 2.2 does NOT add a third write to the transaction — the publish is OUTSIDE the closure, AFTER `interact_res?` returns. The closure still has exactly two writes.
- **State emission and event INSERT atomicity (`architecture.md:589`).** SIGKILL during a load must leave projection rows and event-log rows in agreement. The publish happens after `tx.commit()?`, so a SIGKILL between commit and publish leaves the DB consistent but the WS subscriber missing one event — recoverable via the cursor-gap mechanism (`oldest_available_event_id` already shipped in Story 1.7 / 2.1 Hello). This is the deliberate trade: WAL durability over best-effort fan-out.
- **`(source, session_id)` natural key (`project-context.md:695`).** `BroadcastEnvelope::State` carries both; the `Topic::StateSession(id)` topic is single-key on `session_id` only (deliberate V1 simplification — see Story 2.1's deferred-work cross-reference at `deferred-work.md:51` for multi-source disambiguation). Story 2.2 does not change this.
- **Outbound envelope additive-compat (`project-context.md:594`).** No new wire shapes shipped — `EventFrame` and `StateFrame` are unchanged. The asymmetric `deny_unknown_fields` policy continues to hold without modification.
- **Native payloads ride verbatim (`project-context.md:696`).** The `Event.payload` field is the raw JSON string from the adapter — Story 2.2 does NOT touch it on the publish path. The presenter sees what the adapter produced.
- **`unsafe_code = "forbid"` (`Cargo.toml:5-6`).** No `unsafe` blocks anywhere. No `unsafe` is needed for this work.
- **`#[tracing::instrument(skip_all)]` discipline (`project-context.md:664`).** The existing `write` annotation already uses `skip_all` with fields. The new `tracing::debug!` line at the bottom adds only `event_id` — no payload, no envelope, no broadcaster handle.

### Latency consideration (NFR — not a hard contract test in this story)

`architecture.md:272` sets the hook→presenter target at p99 ≤100ms. The latency budget for Story 2.2's added work is roughly:
- `broadcast::Sender::send` for two envelopes: ~microseconds (in-process channel push; no IO, no allocation beyond the envelope itself which is already cloned for each receiver internally).
- Per-receiver dispatch from `dispatch_envelope` through `socket.send`: dominated by the OS TCP write, well under 1ms on loopback.

There is no perf gate added in this story. A future benchmark story (post-Epic 2) may add a Criterion harness for hook→presenter p99; for now the design relies on standard-tier broadcast semantics being fast enough at single-developer load. If a presenter author reports lag, the deferred backpressure-counters work (`ws_broadcast_lag`, `ws_client_queue_depth` — see `project-context.md:499`) is the diagnosis path.

### "Standards-by-default" (retro Agreement A1) check

Story 2.2 introduces no bespoke surface. The publish path is `tokio::sync::broadcast`, which is the same standard pub/sub primitive already chosen in Story 2.1. The wire shapes (`EventFrame`, `StateFrame`) are already shipped. No new framing, no custom dispatch, no hand-rolled fan-out — only the function-call wiring that 2.1 deferred to 2.2.

### Tests to update (existing, may break with new signature)

`projection::session::write` is called from production code only via `ingest::writer::run`. Test code in `contract_daemon.rs` calls it in a few places — search for `projection::session::write(` and update each callsite to pass `&state.broadcaster` (or a test-local `BroadcastHub::new(16)` for isolated unit tests). Specifically:

- `crates/daemon/tests/contract_daemon.rs` — every test that calls `write` directly. Run `cargo test --workspace` after the signature change to find them via compile errors.

If a test only cares about the DB side and doesn't need to assert on the broadcast, it can pass a throwaway `&BroadcastHub::new(16)` whose receivers are never created — `publish` is a no-op on a hub with zero subscribers (the `let _ = self.tx.send(envelope)` swallow). This is fine and explicitly intended.

## Previous Story Intelligence (from Story 2.1)

Story 2.1 was the WS surface watershed and shipped 38 + 4 = 42 new tests on top of the 121 pre-Epic-2 tests, bringing the workspace total to 163. Five learnings carry directly into Story 2.2:

1. **The hub was designed with `publish` already in mind.** Story 2.1's `BroadcastHub::publish` doc-comment names this story explicitly: "Story 2.2 wires this into `projection::session::write`." There is no design ambiguity — the wiring point is decided.
2. **Pre-Subscribe backlog drains under the OLD subscription state.** This matters for Story 2.2 because the publisher (now `projection::session::write`) doesn't know which subscribers exist. If a client connects, subscribes, and immediately receives a State frame, that frame was either published after the subscribe OR pre-Subscribe and drained under the empty topic set (which filters it out). Either way is correct.
3. **`ServerMessage::Unknown` catch-all closes the additive-compat gap.** Story 2.2 ships no new variants, but if a future story adds one, older clients deserialize as `Unknown` and survive. This is why no `schema` entry is needed in the changelog for Story 2.2.
4. **TraceLayer span redaction (RedactedSpan in `api/mod.rs`).** Story 2.2 introduces no new URI surface and inherits this discipline automatically — but the new `tracing::debug!` line on the publish path must still avoid logging tokens or payloads. Use field-list syntax (`event_id = event_id.0`), not `?event` / `?envelope`.
5. **Sentinel events are excluded everywhere they appear.** `SELECT_DISTINCT_SESSIONS_FROM_EVENTS` filters `source != '__daemon__'`; `SELECT_HELLO_DB_FIELDS` does the same. AC #7 extends this exclusion to the publish path. The lesson: when a new presenter-facing surface ships, the `__daemon__` exclusion is part of the contract.

The Story 1.8 lesson "Typed errors over string-prefix sniffing" continues to apply: any new error case in `write` (there shouldn't be one in 2.2, but if `broadcaster.publish` ever grows a `Result`, prefer a `protocol::Error` variant over a string match).

## Git Intelligence Summary (last 5 commits)

```
aebb6c6  Merge pull request #23 from technicalpickles/story-2.1
81f721c  feat(story-2.1): WebSocket connection and topic subscription
c44cbf5  create-story 2.1
1f62549  Merge pull request #22 from technicalpickles/epic-1-retro
bc657f7  docs(epic-1): retrospective and epic completion status
```

Story 2.1 merged cleanly. Tree is on `story-2.2` branch from `main`. Tests at start of 2.2: 163 passing across the workspace. The commit convention `feat(story-X.Y): <subject>` continues; final merge should be `Merge pull request #N from technicalpickles/story-2.2`.

Story 2.1's PR (#23) is the prior-art reference for review discipline — it landed with one review round covering nine findings. Expect Story 2.2 to be smaller (it's a wiring story) — likely 0–3 findings.

## Latest Tech Information

- **`tokio::sync::broadcast` (tokio 1.52.1)** — `Sender::send` returns `Result<usize, SendError<T>>`; `usize` is the count of active subscribers, `SendError` only when there are zero. `BroadcastHub::publish` already swallows the error. Reference: <https://docs.rs/tokio/1.52.1/tokio/sync/broadcast/struct.Sender.html#method.send>. No API changes between 1.51 and 1.52 in this area.
- **`deadpool_sqlite::Object::interact`** — the closure return type can be any `Send + 'static`. Widening from `rusqlite::Result<i64>` to `rusqlite::Result<(i64, SessionState)>` is fine; `SessionState` is `Send + Clone` (its fields are `Copy` and `i64`). Reference: <https://docs.rs/deadpool-sqlite/0.13.0/deadpool_sqlite/struct.Object.html#method.interact>.
- **`protocol::Event` construction** — all six fields are owned (`String`, `i64`, enum, `Option<Reaction>`). Constructing one in the outer scope of `write` requires cloning `source`, `session_id`, and `payload` from the captured variables (or restructuring to compute them once and clone once into the Event). Either is fine; clarity over micro-optimization here — the publish is not the hot path (the SQLite write dominates).

No security advisories outstanding for any of the above as of 2026-05-21.

## Project Context Reference

Read these documents in this order if you have not yet:

1. **`docs/bmad/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md`** — especially "Files this story CREATES" (the broadcast module Story 2.2 builds on) and "Anti-patterns" (the publish-from-`projection::session::write` boundary).
2. **`docs/bmad/planning-artifacts/architecture.md`** — §"Architectural Decisions › API & Communication Patterns" (lines ~448–477) for the WS topology overview; §"Process Conventions" (lines ~634–641) for the transaction invariant; §"Wire Format Conventions" (lines ~574–608) for the `ServerMessage` enum that 2.2 must produce via `BroadcastEnvelope`.
3. **`docs/bmad/planning-artifacts/epics.md`** — §"Epic 2 › Story 2.2" (lines ~514–540) for the canonical ACs (this story preserves them verbatim, with one added AC each for publish-gating and sentinel exclusion).
4. **`docs/bmad/planning-artifacts/prd.md`** — §"NFR" especially NFR2 (no perceptible lag at single-developer load) — informs the "no perf gate in this story" choice.
5. **`docs/bmad/project-context.md`** — §"Substrate-not-actor invariants" (lines ~692–704) for the `(source, session_id)` natural-key + sentinel-exclusion + native-payloads-verbatim rules.
6. **`docs/protocol-changelog.md`** — current state of v1.0 → v1.1; Story 2.2's behavioral entry slots in after the Story 2.1 entries.

### Project Structure Notes

- Alignment with the unified project structure: Story 2.2 changes are entirely inside `crates/daemon/src/{projection,ingest,main.rs}` plus `crates/daemon/tests/contract_daemon.rs` and the protocol changelog. No new modules, no new crates, no rename of files. The structure remains as documented in `architecture.md:730–858`.
- No detected conflicts or variances.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-2.2] — canonical ACs
- [Source: docs/bmad/planning-artifacts/architecture.md#API-&-Communication-Patterns] — WS topology
- [Source: docs/bmad/planning-artifacts/architecture.md#Process-Conventions] — transaction invariant (commit-gating origin)
- [Source: docs/bmad/planning-artifacts/architecture.md#Wire-Format-Conventions] — `ServerMessage`, `StateFrame` shape
- [Source: docs/bmad/project-context.md#Substrate-not-actor-invariants] — sentinel exclusion + (source, session_id) natural key
- [Source: docs/bmad/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md] — prior story's design + anti-pattern list

## Story Completion Status

Story 2.2 was created via `bmad-create-story` on 2026-05-21, immediately after Story 2.1 merge, and shipped through three rounds of code review over 2026-05-21 and 2026-05-22. A fourth review pass on 2026-05-22 found unresolved patch findings, so the story is currently back in **`in-progress`** until those action items are addressed.

### Dev Agent Record

#### Agent Model Used

Claude Opus 4.7 (1M context), via Claude Code.

#### Debug Log References

No debug-loop incidents. The initial `write` signature change cascaded cleanly through 17 existing test callsites + 1 `writer::run` callsite, all surfaced via `cargo check` and fixed mechanically. The three code-review rounds also went without debug-loop incidents — each patch landed against a clean `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` baseline.

#### Completion Notes List

- **`projection::session::write` is now the sole publisher of user-facing `BroadcastEnvelope::Event` / `BroadcastEnvelope::State` frames.** Sentinel writers (`write_recording_started`, `write_recording_ended`) deliberately have no `broadcaster` parameter — daemon lifecycle stays off the user-facing surface (AC #7).
- **Publishing is statically commit-gated.** The two `broadcaster.publish(...)` calls live after `let (event_id_raw, new_state) = interact_res?;` in `crates/daemon/src/projection/session.rs`, making them unreachable on any pool/interact/rusqlite/commit error. AC #6 is enforced by control flow.
- **Two runtime guards reject misuse before pool checkout.** Sentinel `EventKind` (`RecordingStarted`/`RecordingEnded`) and the reserved `source = "__daemon__"` both return `Err(Error::Projection(...))` at the top of `write`, defending AC #7 in release builds.
- **`BroadcastHub::new` clamps capacity at `MIN_CAPACITY = 2`** so each subscriber's ring buffer always holds at least the Event+State pair a single committed write produces.
- **`ingest::handler` caps the read at 1 MiB** (matches the shim's `MAX_STDIN_BYTES`) by wrapping the read half in `read_half.take(MAX + 1)` and rejecting `buf.len() > MAX && !buf.ends_with('\n')` with a deterministic 400 line response — before JSON parse, enqueue, DB write, or broadcast.
- **WS readiness uses token-based probes, not fixed sleeps.** `wait_subscribe_live` / `wait_subscribe_live_all` publish uniquely-tokened probe envelopes and return only once every client has observed the latest token. The `ProbeKind::EventForSession` variant added in the third round carries the token in `payload` so probes can match the exact `events.<source>.<session_id>` form.
- **Closure-return widened to `(i64, SessionState)`.** Returning `new_state` out of the `interact` closure avoids recomputing `transition` outside the transaction (which would race in any future multi-writer world even though today's single-writer pool serializes writes).
- **Existing tests use throwaway `&BroadcastHub::new(16)` for `projection::session::write` calls that don't care about the broadcast.** This is the pattern the story Dev Notes documented; `publish` is a no-op on a hub with zero subscribers (the `let _ = self.tx.send(envelope)` swallow handles it). The temporary's lifetime is extended to the surrounding `.await` statement per Rust 2021 lifetime-extension.
- **`story_2_1_ws`'s test helpers (`spawn_test_daemon`, `connect_authed`, `parse_hello`, `read_text_frame_or_close`) promoted to `pub(super)`** so the sibling `story_2_2_publish` module can reuse them without redeclaration (per the story's "do NOT re-declare these" instruction).
- **`broadcaster` construction in `main.rs::run` moved before the ingest-writer spawn** so the `Arc<BroadcastHub>` can be cloned into the writer task. The same `Arc` lands in `AppState` directly afterward; no second hub instance exists.
- **AC #6 deferred-as-described.** The structural property (publish unreachable on commit failure) is enforced by code position; a deterministic commit-fail integration fixture is captured in `deferred-work.md` for a future chaos-test story.
- **Story 2.2 targeted tests:** 15 in `story_2_2_publish` module + 2 `broadcast::hub` unit tests (`capacity_floor_holds_event_state_pair`, `odd_capacity_rounds_up_to_hold_two_pairs`) = 17 targeted tests.
- **Workspace test count: 163 → 180.** Initial Story 2.2 landing added 6 tests (163 → 169). First review round added 2 (169 → 171). Second review round added 2 (171 → 173). Third review round added 5 (173 → 178): `projection_write_rejects_daemon_source_at_runtime`, `ingest_oversized_line_rejected_before_broadcast`, `state_frame_carries_full_session_state`, `exact_event_and_state_topic_forms_match_live`, `consumer_independence_abrupt_close_no_duplicates`. Fourth review round added 2 (178 → 180): `ingest_near_shim_cap_line_accepted`, `odd_capacity_rounds_up_to_hold_two_pairs`. The fourth round also extended four existing negative-timeout tests with positive sentinels — no new top-level tests, but each end-of-test assertion now proves queue ordering rather than relying on silence alone.

#### File List

- `crates/daemon/src/projection/session.rs` — added `broadcaster: &BroadcastHub` parameter to `write`; widened `interact` closure return to `(i64, SessionState)`; added post-commit Event + State publish with `tracing::debug!` line; converted sentinel-kind `debug_assert!` to a runtime `Err(Error::Projection)` guard (review round 1); added matching runtime guard rejecting `source == "__daemon__"` (review round 3); updated function-level doc comment
- `crates/daemon/src/ingest/writer.rs` — added `broadcaster: Arc<BroadcastHub>` parameter to `run`; pass `&broadcaster` into both `projection::session::write` callsites (steady-state recv arm and shutdown-drain arm); reworded shutdown-drain comment so it describes writer behaviour and defers end-to-end WS flush semantics to story 2.5 (review round 3)
- `crates/daemon/src/ingest/handler.rs` — added `MAX_INGEST_LINE_BYTES`, wrapped the read half in `read_half.take(MAX + 1)`, and rejects `buf.len() > MAX && !buf.ends_with('\n')` with a deterministic 400 line response before JSON parse, enqueue, DB write, or broadcast (review round 3); raised the cap to `SHIM_STDIN_CAP + 4096` so shim-valid near-1 MiB payloads survive `hook_kind` injection and newline framing (review round 4)
- `crates/daemon/src/broadcast/hub.rs` — floored `BroadcastHub::new` capacity at `MIN_CAPACITY = 2` so the Event+State pair always fits a single subscriber's ring buffer (review round 1); rounded capacity up to the next even number after the floor and refreshed stale module-level + `publish` doc comments that still said Story 2.1 was pre-publish (review round 4)
- `crates/daemon/src/main.rs` — moved `BroadcastHub` construction above the ingest-writer spawn; pass `broadcaster.clone()` into the spawn
- `crates/daemon/tests/contract_daemon.rs` — added `story_2_2_publish` module (15 contract tests across four review rounds — see Completion Notes for the per-round breakdown); promoted `spawn_test_daemon`/`connect_authed`/`parse_hello`/`read_text_frame_or_close`/`authed_request`/`ws_url_header` to `pub(super)` in `story_2_1_ws`; replaced fixed sleeps with token-based `wait_subscribe_live` / `wait_subscribe_live_all` / `connect_until_ready` helpers (review round 1); added `ProbeKind::EventForSession` variant so probes can match exact-form `events.<source>.<session_id>` subscriptions (review round 3); updated 17 existing `projection::session::write` callsites + 1 `writer::run` callsite to the new signature; extended four negative-timeout tests with positive sentinels so broadcast ordering proves no forbidden frame arrives even after the 300ms window (review round 4)
- `docs/protocol-changelog.md` — added behavioral entry under `v1.0 → v1.1` describing the new publish behavior; rewrote the Story 2.1 entry to drop the contradictory "NOT YET wired" wording (review round 1)
- `docs/bmad/implementation-artifacts/deferred-work.md` — added "Deferred from: Story 2.2" section with three entries (commit-failure integration test for AC #6, hook→presenter latency bench, shutdown-drain WS delivery semantics → story 2.5)
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — bumped `2-2-real-time-event-and-state-broadcast-to-multiple-tools` from `ready-for-dev` → `review` during implementation, then back to `in-progress` after the fourth review pass found unresolved patch findings; updated `last_updated` header
- `docs/bmad/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md` — status `ready-for-dev` → `review` during implementation, then back to `in-progress` after fourth-round review findings; tasks/subtasks marked complete; four rounds of review findings added under `### Review Findings`; Dev Agent Record + File List + Change Log kept in sync across rounds

### Change Log

| Date       | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
|------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 2026-05-21 | Story 2.2 created via bmad-create-story; status ready-for-dev. Scope: wire `projection::session::write` to publish `BroadcastEnvelope::Event` + `BroadcastEnvelope::State` after transaction commit; sentinel writes excluded; new contract tests covering five Epic 2 ACs plus commit-gating and sentinel-exclusion. |
| 2026-05-21 | Story 2.2 implemented and marked review. `projection::session::write` extended to publish Event + State after `tx.commit()` (commit-gated, sentinel-excluded). `writer::run` and `main.rs::run` threaded with `Arc<BroadcastHub>`. New `story_2_2_publish` test module added with 6 contract tests covering ACs #1–#5 and #7; AC #6 deferred-as-documented. Existing 163 tests updated to new `write` signature, all still passing (workspace total 169). `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, `cargo build --examples` all clean. Protocol changelog and deferred-work doc updated. |
| 2026-05-21 | Addressed 4 code-review [Patch] findings (2 [Defer] items recorded in deferred-work.md). (1) Replaced fixed `sleep(20ms)` WS synchronization with `wait_subscribe_live` / `wait_subscribe_live_all` (synthetic-probe + drain) and a `connect_until_ready` retry helper; only sleep remaining lives inside the bounded retry. (2) Converted the sentinel `debug_assert!` in `projection::session::write` to a runtime `Err(Error::Projection(...))` return so release builds also reject sentinel kinds before pool checkout. (3) Rewrote the Story 2.1 changelog entry to drop the contradictory "NOT YET wired" wording. (4) Floored `BroadcastHub::new` capacity at `MIN_CAPACITY = 2` so the Event+State pair always fits a single subscriber's ring buffer. Added two new tests: `broadcast::hub::capacity_floor_holds_event_state_pair` (covers (4)) and `projection_write_rejects_sentinel_kinds_at_runtime` (covers (2)). Workspace tests: 169 → 171 passing. |
| 2026-05-21 | Addressed 3 second-round code-review [Patch] findings. (5) Made the WS subscribe-readiness probes unique per attempt by encoding a monotonic `u64` token into the probe envelope (`session_id` for Event probes, `source` for State probes) and exiting only when every client has observed a probe with `token >= latest_published_token`; broadcast ordering guarantees no older probe can arrive later, closing the stale-probe race. (6) Added end-to-end contract test `ingest_socket_event_reaches_ws_subscriber` that drives the full production path (`ingest::listener::run` + `ingest::writer::run` + `projection::session::write`) so a regression that swapped or dropped the writer task's broadcaster handle would be observable. (7) Added cross-topic ordering test `event_published_before_state_to_dual_subscriber` that subscribes one client to both `events.*` and `state.session.*` and asserts exactly two frames in `Event`-then-`State` order, guarding the doc-commented invariant. Workspace tests: 171 → 173 passing. |
| 2026-05-22 | Addressed 5 third-round code-review [Patch] findings (1 [Defer] item acknowledged in-place; the shutdown-drain WS delivery [Defer] is tracked in `deferred-work.md`). (8) Added a runtime guard rejecting `envelope.source == "__daemon__"` at the same boundary as the sentinel-kind guard in `projection::session::write`, with the new `projection_write_rejects_daemon_source_at_runtime` contract test. (9) Added `MAX_INGEST_LINE_BYTES = 1 << 20` to `ingest::handler` and wrap the read half in `read_half.take(MAX + 1)` so oversized payloads (which could bypass the shim's matching cap from a direct local-socket client) are rejected with a deterministic 400 line response before JSON parse, enqueue, DB write, or broadcast; covered by `ingest_oversized_line_rejected_before_broadcast`. (10) Tightened Story 2.2 WS coverage: `state_frame_carries_full_session_state` asserts the complete `source` + `SessionState` after Pre/PostToolUse, `exact_event_and_state_topic_forms_match_live` covers the non-wildcard `events.<source>.<session_id>` and `state.session.<id>` forms (via a new `ProbeKind::EventForSession` readiness variant that carries the token in `payload`), and `consumer_independence_abrupt_close_no_duplicates` extends AC #5 to TCP-drop / no-graceful-close while asserting no duplicate or surplus frames on the surviving subscriber. (11) Reworded the `writer::run` shutdown-drain comment so it describes writer behaviour (still commits and publishes accepted events during drain) and defers end-to-end WS flush semantics to story 2.5. (12) Refreshed `Story Completion Status`, Dev Agent Record, File List, and Change Log to match current state (`review`, 14 `story_2_2_publish` tests + 1 `broadcast::hub` test, workspace 178). Workspace tests: 173 → 178 passing. |
| 2026-05-22 | Fourth code-review pass recorded 5 unresolved [Patch] findings and moved story/sprint status back to `in-progress`: daemon wire cap rejects shim-valid near-cap payloads, odd broadcast capacity can orphan State frames after lag, `BroadcastHub` comments still describe pre-publish behavior, Task 7 verification count is stale, and terminal negative WS tests need follow-up positive sentinel checks. Existing defer items from prior reviews remain tracked in `deferred-work.md`. |
| 2026-05-22 | Addressed all 5 fourth-round code-review [Patch] findings; story/sprint status back to `review`. (13) Split the daemon's wire-line cap from the shim's stdin cap: `MAX_INGEST_LINE_BYTES = SHIM_STDIN_CAP + 4096` so shim-valid near-1 MiB payloads survive `hook_kind` injection and newline framing on the wire; new `ingest_near_shim_cap_line_accepted` contract test exercises a >1 MiB shim-shaped wire line and the existing oversized-line rejection test now targets the new daemon cap. (14) Rounded `BroadcastHub::new` capacity up to the next even number after the `MIN_CAPACITY` floor so small odd raw capacities (`3`, …) hold both Event+State pairs without `Lagged`; new `odd_capacity_rounds_up_to_hold_two_pairs` unit test covers the raw-`3` case. (15) Rewrote the stale `BroadcastHub` module and `publish` doc comments so they describe Story 2.2's wired publish path rather than the pre-publish Story 2.1 state. (16) Replaced Task 7's hard-coded `169 passed` count with a pointer to the Dev Agent Record's latest test-count line so the verification block can't go stale again. (17) Added positive sentinels after the four Story 2.2 negative-timeout assertions (`events_source_filter_excludes_other_source`, `projection_write_rejects_daemon_source_at_runtime`, `exact_event_and_state_topic_forms_match_live`, `ingest_oversized_line_rejected_before_broadcast`) so broadcast ordering proves no forbidden frame slips past the 300ms window. Workspace tests: 178 → 180 passing. Story 2.2 targeted tests: 15 → 17 (15 in `story_2_2_publish` module + 2 `broadcast::hub` unit tests). |
