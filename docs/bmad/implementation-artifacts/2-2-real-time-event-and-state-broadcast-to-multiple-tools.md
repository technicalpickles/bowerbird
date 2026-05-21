# Story 2.2: Real-time event and state broadcast to multiple tools

Status: ready-for-dev

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

- [ ] Task 1: Thread `Arc<BroadcastHub>` into the ingest writer task (AC: #1, #2, #3, #4, #5)
  - [ ] Add `broadcaster: Arc<BroadcastHub>` parameter to `crates/daemon/src/ingest/writer.rs::run`
  - [ ] Update the spawn in `crates/daemon/src/main.rs::run` to pass `broadcaster.clone()` (the `Arc<BroadcastHub>` already constructed at line 164 — clone the `Arc`, do not share the same handle that gets moved into `AppState`)
  - [ ] No new module; no new file

- [ ] Task 2: Publish from `projection::session::write` after transaction commit (AC: #1, #2, #6)
  - [ ] Change the `write` signature from `(writer_pool, envelope)` to `(writer_pool, broadcaster: &BroadcastHub, envelope)` — pass by `&BroadcastHub` reference, not `Arc<BroadcastHub>`, because the caller already holds the `Arc` and `publish` only needs a `&self`
  - [ ] Inside the `interact` closure, capture `now_ms`, the assigned `event_id` from `tx.last_insert_rowid()`, AND the freshly computed `new_state: SessionState` so they can be returned to the outer scope alongside the existing `i64` event_id
  - [ ] Change the closure return type from `rusqlite::Result<i64>` to `rusqlite::Result<(i64, SessionState)>` — return `Ok((id, new_state))` only after `tx.commit()?` succeeds
  - [ ] After `let event_id = interact_res?;`, construct the `protocol::Event` (filling `event_id: EventId(event_id)`, `source`, `session_id`, `kind`, `reaction`, `payload`, `created_at: now_ms` — clone strings/payload from the variables retained in the outer scope before they moved into the closure) and call `broadcaster.publish(BroadcastEnvelope::Event(event.clone()))` then `broadcaster.publish(BroadcastEnvelope::State { source: event.source.clone(), session_id: event.session_id.clone(), state: new_state })`
  - [ ] Strict ordering: publish `Event` BEFORE `State` so a presenter consuming both topics sees the event that caused the transition before (or at worst interleaved with — broadcast preserves per-channel order, not cross-publish) the resulting projection update. Document the choice in a doc-comment on `write`
  - [ ] Do NOT add `broadcaster` parameters to `write_recording_started` / `write_recording_ended` — sentinel writes do NOT publish (per AC #7 and architecture's `source != '__daemon__'` exclusion already enforced in `SELECT_DISTINCT_SESSIONS_FROM_EVENTS` and `SELECT_HELLO_DB_FIELDS`)
  - [ ] Update the `tracing::instrument` annotation — no field change needed, but add a one-line `tracing::debug!(event_id, "ws: published event + state envelopes")` after the second publish so `-vv` debugging can confirm fan-out fired

- [ ] Task 3: Pass `broadcaster` from `writer::run` into `projection::session::write` (AC: #1)
  - [ ] In `crates/daemon/src/ingest/writer.rs::run`, change both `projection::session::write(&writer_pool, envelope)` callsites (the steady-state `recv` arm and the shutdown-drain arm) to `projection::session::write(&writer_pool, &broadcaster, envelope)`
  - [ ] Verify the shutdown-drain path still publishes — events accepted before shutdown that drain after `cancelled()` MUST publish so any WS clients still attached during the drain receive them, consistent with the Story 2.5 graceful shutdown design that hasn't shipped yet

- [ ] Task 4: Contract tests for the fan-out path (AC: #1, #2, #3, #4, #5, #6, #7)
  - [ ] Add a new module `story_2_2_publish` in `crates/daemon/tests/contract_daemon.rs` (next to `story_2_1_ws`) that REUSES `spawn_test_daemon`, `make_test_state_with_ws`, `connect_authed`, `parse_hello`, and `read_text_frame_or_close` — do NOT re-declare these
  - [ ] Helper `parse_event_frame(&Message) -> protocol::Event` and `parse_state_frame(&Message) -> protocol::StateFrame` mirroring `parse_hello`'s shape (match `ServerMessage::Event(f) => f.event`, `ServerMessage::State(f) => f`, panic on any other variant)
  - [ ] Helper `publish_via_projection(state: &AppState, source: &str, session_id: &str, kind: EventKind, reaction: Option<Reaction>, payload: &str) -> EventId` — calls `bowerbird_daemon::projection::session::write(&state.db.writer, &state.broadcaster, EventEnvelope { ... })` so the test exercises the REAL publish path, not a synthetic `broadcaster.publish(...)` shortcut
  - [ ] **AC #1 — three subscribers see identical event in identical order**: spawn the test daemon; connect THREE WS clients (different bearer-token strings? — no, same TEST_BEARER is fine; the bearer is the daemon-wide auth, not per-client identity); on each, send `{"op":"subscribe","topic":"events.*"}` and read the implicit subscribe-ack-by-silence; then publish two events via `publish_via_projection` for the same `(source, session_id)`; on each client read the next two frames AFTER Hello and assert they are `Event` frames with the same `event_id` sequence; assert the JSON serialization is byte-identical across clients (`serde_json::to_string` the parsed frame and compare)
  - [ ] **AC #2 — state.session.<id>.current_state only delivers for matching session**: subscribe to `state.session.sess-A.current_state`; publish_via_projection for `sess-A` (expect State frame), then publish_via_projection for `sess-B` (expect NO frame within a 300ms timeout — use `tokio::time::timeout(Duration::from_millis(300), ws.next())` and assert `Err(_)`); finally publish_via_projection for `sess-A` again (expect State frame). Tests both positive and negative match
  - [ ] **AC #3 — source-scoped filtering**: subscribe to `events.claude.*`; publish_via_projection for `("claude", "sess-1", ...)` (expect Event frame); inject a synthetic `BroadcastEnvelope::Event(Event { source: "codex", ... })` via `state.broadcaster.publish(...)` (the ONLY allowed synthetic-publish in this test module — used to simulate a future second-source adapter without changing production code); assert NO frame received for the codex event within 300ms
  - [ ] **AC #4 — state.session.* wildcard across concurrent sessions**: subscribe to `state.session.*`; publish_via_projection for THREE different `session_id`s interleaved (`sess-A`, `sess-B`, `sess-A`, `sess-C`); read four State frames and assert the `session_id` field on each matches the publication order — no cross-session smearing
  - [ ] **AC #5 — consumer independence**: connect TWO clients with identical subscription `events.*`; close client A via `ws_a.close(None).await`; publish_via_projection one event; assert client B receives the event Event frame within 1s; assert the WS semaphore has exactly ONE permit outstanding via a connect-cap probe (connect with `ws_max_conns = 2` — after A closes, a third connect attempt should succeed, indicating A's permit was released)
  - [ ] **AC #6 — publish gated on commit success**: the cleanest way to drive a commit failure deterministically is to drop the writer pool mid-test. As a pragmatic alternative, assert by code-reading + a unit test on the structure of `write`: confirm that the `broadcaster.publish` calls happen AFTER `interact_res?` has succeeded. A focused integration test for commit-fail is OPTIONAL — capture as deferred-work if it's awkward to drive
  - [ ] **AC #7 — sentinel events not published**: subscribe one client to `events.*`; call `projection::session::write_recording_started(&state.db.writer).await` directly; within 300ms timeout assert NO frame arrives. Then publish_via_projection a non-sentinel event and assert it DOES arrive (sanity that the channel still works)

- [ ] Task 5: Protocol changelog update (AC: #1, #2)
  - [ ] Append one `behavioral` entry to `docs/protocol-changelog.md` under the `v1.0 → v1.1` heading: "Story 2.2 — `event` and `state` `ServerMessage` frames are now published. Every successful `projection::session::write` (ingest path, non-sentinel events) publishes one `BroadcastEnvelope::Event` followed by one `BroadcastEnvelope::State` after the transaction commits. WS subscribers to `events.*`, `events.<source>.*`, `events.<source>.<session_id>`, `state.session.*`, `state.session.<id>`, and `state.session.<id>.current_state` (all topics already shipped in 2.1) now receive live frames. Sentinel events with `source = '__daemon__'` (`RecordingStarted`, `RecordingEnded`) are NOT published. Subscribers are fully independent — closing one connection does not affect others. End-to-end latency targets hook→presenter p99 ≤100ms (NFR; benchmarked separately)."
  - [ ] No `schema` entry needed — Story 2.1 already added `StateFrame` and the `ServerMessage::Event(EventFrame)` variant.

- [ ] Task 6: Update deferred-work and CHANGELOG (AC: n/a)
  - [ ] In `docs/bmad/implementation-artifacts/deferred-work.md`, append a "Deferred from: Story 2.2" section if any (likely empty — capture only items genuinely punted, e.g. a deterministic commit-fail test fixture if Task 4 left that as optional)
  - [ ] No `epics.md` or `prd.md` back-amends expected — the existing Story 2.2 ACs in `epics.md` already match this story's interpretation

- [ ] Task 7: Verify and merge (AC: all)
  - [ ] `cargo fmt --check`
  - [ ] `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] `cargo test --workspace` — confirm the existing 163 tests still pass and the new 2.2 tests pass (target ~170+)
  - [ ] `cargo build --examples` (no example changes expected — sanity check)
  - [ ] Open PR titled `feat(story-2.2): real-time event and state broadcast to multiple tools`

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

This story was created via `bmad-create-story` on 2026-05-21, immediately after Story 2.1 merge. The Ultimate context engine analysis was completed and a comprehensive developer guide produced. The story is **`ready-for-dev`**.

### Dev Agent Record

#### Agent Model Used

Claude Opus 4.7 (1M context), via Claude Code.

#### Debug Log References

_(Filled by the dev agent during implementation.)_

#### Completion Notes List

_(Filled by the dev agent during implementation.)_

#### File List

_(Filled by the dev agent during implementation.)_

### Change Log

| Date       | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
|------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 2026-05-21 | Story 2.2 created via bmad-create-story; status ready-for-dev. Scope: wire `projection::session::write` to publish `BroadcastEnvelope::Event` + `BroadcastEnvelope::State` after transaction commit; sentinel writes excluded; new contract tests covering five Epic 2 ACs plus commit-gating and sentinel-exclusion. |
