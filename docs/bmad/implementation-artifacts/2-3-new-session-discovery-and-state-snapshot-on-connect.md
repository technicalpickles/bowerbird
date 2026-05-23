# Story 2.3: New session discovery and state snapshot on connect

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want to receive the current state of all matching sessions immediately when I connect, and to be notified automatically when new sessions appear while I am subscribed,
So that my tool is always up to date without polling and without missing sessions that started before or during my connection.

## Acceptance Criteria

1. **Given** three active sessions exist in `session_projections` (non-sentinel, `source = "claude"`) when a tool connects, sends a single `Subscribe { topic: "state.session.*" }`, and the daemon processes that frame
   **When** the daemon completes Subscribe processing
   **Then** the daemon emits exactly one `ServerMessage::State` frame per active session (3 frames total) BEFORE any subsequent `event` or live-`state` frame, each frame's `(source, session_id, state)` matches the stored projection row (with `state.current_state` passed through `current_state_for_read` for stale-Working → Idle fallback), and snapshot frames are sent in stable order (`updated_at DESC, source ASC, session_id ASC` — same ordering as `GET /sessions`).

2. **Given** a tool is connected and subscribed to `state.session.*` with the snapshot already delivered
   **When** a brand-new session's first event is ingested via `ingest::writer::run` → `projection::session::write` (so a NEW row appears in `session_projections`) and `tx.commit()` succeeds
   **Then** the tool receives the live `ServerMessage::State` frame for the new session emitted by Story 2.2's publish path — no reconnect, no re-subscribe, no daemon-side bookkeeping for "is this session new to this subscriber" (the live publish path already covers it).

3. **Given** two sessions `sess-A` and `sess-B` exist and a tool connects and subscribes to `state.session.sess-A` (specific id, NOT wildcard)
   **When** the Subscribe frame is processed
   **Then** the daemon emits exactly ONE snapshot State frame (for `sess-A`), and no snapshot State frame is emitted for `sess-B`; AND when an event is later ingested for `sess-B`, the tool receives no `state` frame for `sess-B` (live filtering also holds, confirming wildcard and specific-session subscriptions are correctly distinguished).

4. **Given** a tool connects to a daemon with zero non-sentinel rows in `session_projections`
   **When** the tool sends `Subscribe { topic: "state.session.*" }`
   **Then** the daemon emits zero snapshot State frames and transitions immediately to live streaming — any event ingested AFTER subscribe yields a live State frame on the connection. (The Event envelope is filtered by `Topic::matches` because `state.session.*` only matches `BroadcastEnvelope::State`; a client wanting both Event AND State must subscribe to both topic families — see Story 2.1's topic-grammar contract at `crates/daemon/src/broadcast/event.rs::Topic::matches`.)

5. **Given** a tool subscribes to `state.session.<id>.current_state` (the current-state-only variant) for a specific existing session
   **When** the Subscribe is processed
   **Then** the daemon emits exactly one snapshot State frame for that session (same wire shape as `state.session.<id>` — Story 2.1 deliberately did not project a smaller frame for the `.current_state` variant; see `deferred-work.md:64`).

6. **Given** a tool subscribes to a non-state topic (`events.*`, `events.claude.*`, or `events.claude.sess-A`)
   **When** the Subscribe is processed
   **Then** the daemon emits ZERO snapshot frames — snapshot semantics are state-only. Event history is fetched via REST `/sessions/:id/events?since=0` (Story 1.7); the live event stream picks up at subscribe-time.

7. **Given** a tool sends `Subscribe { topic: "state.session.*" }` followed by `Subscribe { topic: "state.session.sess-A" }` (overlapping subscriptions)
   **When** the second Subscribe is processed
   **Then** the daemon emits ZERO additional snapshot frames for `sess-A` — the first subscribe already delivered a snapshot for every session the second subscribe would match. (Snapshot dedup against the pre-add subscription set; same dedup discipline as `dispatch_envelope`'s `any(|t| t.matches(envelope))`.)

8. **Given** the protocol crate's `SyncFrame` (`crates/protocol/src/ws.rs::SyncFrame`)
   **When** code attempts to construct a `SyncFrame` with `oldest_available_event_id > latest_event_id`
   **Then** construction goes through a typed constructor `SyncFrame::new(oldest, latest) -> Result<Self, Error>` that returns an `Error` (no panic, no silent reorder); the direct struct-literal path remains available for `Deserialize` only (so wire payloads still parse) — Epic 1 retrospective Agreement A2 fold-in [`docs/bmad/implementation-artifacts/epic-1-retro-2026-05-20.md:111`].

## Tasks / Subtasks

- [x] Task 1: Snapshot helper in `projection::session` (AC: #1, #3, #4, #5, #6, #7)
  - [x] Add `pub async fn snapshot_for_topic(reader_pool: &deadpool_sqlite::Pool, new_topic: &Topic, pre_existing: &HashSet<Topic>, now_ms: i64) -> Result<Vec<StateFrame>>` in a new file `crates/daemon/src/projection/snapshot.rs`, re-exported from `projection/mod.rs`
  - [x] Early-return `Ok(vec![])` if `new_topic` is not one of `Topic::StateAll | Topic::StateSession(_) | Topic::StateSessionCurrent(_)` (snapshot is state-only — AC #6)
  - [x] Query the reader pool with `SELECT_NON_SENTINEL_SESSIONS` (already exists at `crates/daemon/src/db/queries.rs:40`); order is `updated_at DESC, source ASC, session_id ASC` and ALREADY excludes `source = '__daemon__'`
  - [x] For each row, deserialize the `state` JSON via `serde_json::from_str::<SessionState>(&raw)`; on parse error log at error level and skip that row (mirror `api/sessions.rs::list:73-87` discipline — never 500 the snapshot for one bad row)
  - [x] Build a candidate `BroadcastEnvelope::State { source, session_id, state: stored.clone() }` (synthetic, never published to the hub — used only for `Topic::matches` evaluation)
  - [x] Filter: keep iff `new_topic.matches(&envelope) && !pre_existing.iter().any(|t| t.matches(&envelope))` (matches the new topic AND is not already covered by an existing subscription — AC #7 dedup)
  - [x] Apply read-time stale-Working fallback via `crate::projection::state::current_state_for_read(&stored, now_ms)`; produce the wire `StateFrame { source, session_id, state: SessionState { current_state: derived_current, last_event_kind: stored.last_event_kind, last_event_at_ms: stored.last_event_at_ms } }` (same shape as `api/sessions.rs::detail:163-171`)
  - [x] Return `Vec<StateFrame>` preserving the SQL row order (the wire emission order in Task 2 is then deterministic and matches AC #1's `ORDER BY` clause)
  - [x] Unit-test the helper inline (`#[cfg(test)] mod tests`) covering: empty DB → empty vec; one session matching `StateAll` → one frame; one session NOT matching `StateSession("other")` → empty vec; sentinel `__daemon__` row injected directly is excluded (defense-in-depth even though the SELECT filters it); pre-existing covers the new topic → empty vec

- [x] Task 2: Wire snapshot emission into `handle_text_frame::Subscribe` (AC: #1, #2, #3, #4, #5, #6, #7)
  - [x] In `crates/daemon/src/api/ws.rs::handle_text_frame`, replace the current `ClientMessage::Subscribe` arm body with this exact ordering:
    1. `Topic::parse(&topic)` — on `Err(())`, close with `bad message: invalid subscribe topic: {topic}` (unchanged from Story 2.1)
    2. `drain_backlog_under_state(socket, subscriptions, rx)` — drain pre-existing in-flight envelopes under the OLD subscription set, EXACTLY as today (unchanged)
    3. Compute `now_ms` via `crate::time::current_unix_millis()?`; on `Err`, log at error level and CONTINUE — snapshot proceeds without the stale-Working derivation (in this rare case `current_state_for_read` is skipped and the stored `current_state` rides through)
    4. Call `projection::snapshot_for_topic(&state.db.reader, &new_topic, subscriptions, now_ms).await` — on `Err`, log at error level and emit ZERO snapshot frames (a transient reader pool issue must NOT close the connection — AC #1 requires emission, but reliability over completeness)
    5. `subscriptions.insert(new_topic)` — insert ONLY AFTER the snapshot read so the snapshot reflects the subscription set just before the new topic became live
    6. For each `StateFrame` returned by Task 1, serialize via `serde_json::to_string(&ServerMessage::State(frame))` and send through `socket.send(Message::Text(json.into()))`; on send error log at debug and return `false` (connection task exits, same pattern as `dispatch_envelope`'s send-failure branch)
    7. Return `true`
  - [x] Do NOT add a second backlog-drain after `insert`. Anything published between step 4 (DB read) and step 5 (insert) stays buffered in `rx` and is dispatched by the main loop's `rx.recv()` branch under the NEW subscription set, AFTER the snapshot has been emitted. Document the chosen ordering in a `//` comment block so a future reader doesn't re-introduce a redundant drain.
  - [x] No change to the `Unsubscribe` arm — removing a topic never triggers a snapshot.

- [x] Task 3: Acknowledge that AC #2 is satisfied by existing publish path (AC: #2)
  - [x] No new code for AC #2. `projection::session::write` (Story 2.2) already publishes `BroadcastEnvelope::State` after every committed event, including the first event for a new session (which is the moment a new `session_projections` row appears). A contract test in Task 4 verifies this end-to-end so a regression in 2.2 surfaces here.
  - [x] No new daemon-side bookkeeping for "is this session new to this subscriber" — the snapshot covers existing sessions at subscribe-time, the publish path covers everything thereafter.

- [x] Task 4: Contract tests (AC: all)
  - [x] Add module `mod story_2_3_snapshot { ... }` in `crates/daemon/tests/contract_daemon.rs`, AFTER `mod story_2_2_publish`. REUSE these helpers from `story_2_2_publish` via `use super::story_2_2_publish::{...}` (or promote them to a shared `mod helpers` if the imports get ugly): `WsStream`, `wait_subscribe_live`, `wait_subscribe_live_all`, `connect_until_ready`, `publish_via_projection`, `parse_event_frame`, `parse_state_frame`, `ProbeKind`. REUSE `spawn_test_daemon`, `connect_authed`, `parse_hello`, `read_text_frame_or_close`, `authed_request`, `ws_url_header` from `super::story_2_1_ws::{...}` (already `pub(super)` per 2.2 promotion). REUSE `super::{fresh_pools, make_test_state_with_ws, TEST_BEARER}`.
  - [x] **AC #1 — `snapshot_three_sessions_arrive_before_live_events`**: pre-create three sessions via `publish_via_projection(state, "claude", "sess-A", PreToolUse, ...)`, `(state, "claude", "sess-B", PostToolUse, ...)`, `(state, "claude", "sess-C", PreToolUse, ...)` BEFORE the WS client connects. Then `connect_authed`, parse Hello, send `Subscribe { topic: "state.session.*" }`. Read the next three text frames; assert each is a `ServerMessage::State` and the set of `session_id`s is `{sess-A, sess-B, sess-C}`. Then `publish_via_projection(state, "claude", "sess-A", PostToolUse, ...)` and assert the next frame is the LIVE State frame for sess-A (not another snapshot). Order assertion: snapshot frames have `last_event_at_ms` matching the pre-connect publishes; the live frame has a strictly later `last_event_at_ms`.
  - [x] **AC #2 — `new_session_emits_state_to_wildcard_subscriber`**: connect, subscribe to `state.session.*`, `wait_subscribe_live` with `ProbeKind::State { session_id: "__probe__" }`, then `publish_via_projection(state, "claude", "sess-NEW", PreToolUse, ...)` (this is the FIRST event for `sess-NEW`, so a new row appears in `session_projections`). Read two frames: assert frame 1 is `ServerMessage::Event` (event for sess-NEW), frame 2 is `ServerMessage::State` for sess-NEW (Event-before-State ordering inherited from Story 2.2's publish order — `projection::session::write` publishes Event then State).
  - [x] **AC #3 — `specific_id_subscription_excludes_other_sessions`**: pre-create sessions `sess-A` and `sess-B`. Connect, subscribe to `state.session.sess-A`. Read the next frame; assert it is `ServerMessage::State` with `session_id = "sess-A"`. Then `publish_via_projection(state, "claude", "sess-B", PreToolUse, ...)` and assert no `state` frame arrives within a `tokio::time::timeout(Duration::from_millis(300), ws.next())` window — confirms live filtering for the specific-id subscription.
  - [x] **AC #4 — `empty_daemon_no_snapshot_frames`**: connect to a fresh daemon (default `make_test_state_with_ws` against `fresh_pools` with no events). Subscribe to `state.session.*`. Use `wait_subscribe_live` with `ProbeKind::State { session_id: "__probe__" }` to confirm subscription is live. The probe drain mechanism would catch any real snapshot frame (non-probe State frame), so survival of the helper without panic is the assertion. Then `publish_via_projection(state, "claude", "sess-NEW", PreToolUse, ...)` and assert Event + State frames arrive (immediate transition to live streaming).
  - [x] **AC #5 — `current_state_subscription_delivers_snapshot`**: pre-create `sess-A`. Connect, subscribe to `state.session.sess-A.current_state`. Read next frame; assert `ServerMessage::State` with `session_id = "sess-A"` and the full `StateFrame` wire shape (NOT a projected current-state-only frame — Story 2.1's deferred-work item explicitly leaves this on the full frame).
  - [x] **AC #6 — `events_subscription_emits_no_snapshot`**: pre-create `sess-A`. Connect, subscribe to `events.*`. `wait_subscribe_live` with `ProbeKind::Event { source: "claude" }`. The probe-only-drain assertion proves no real (non-probe) frame leaked through; then `publish_via_projection` an event for `sess-A` and assert one Event frame arrives. No State frame for `sess-A` ever appears on this connection.
  - [x] **AC #7 — `overlapping_subscriptions_do_not_re_snapshot`**: pre-create `sess-A`. Connect, subscribe to `state.session.*` and read the (one) snapshot frame. Then subscribe to `state.session.sess-A`. `wait_subscribe_live` with `ProbeKind::State { session_id: "sess-A" }` — the helper publishes its probe and drains until observed; any extra REAL snapshot frame for `sess-A` would arrive between Subscribe-process and probe-arrival and would surface as a `non-probe frame on client #0 during readiness` panic from `wait_subscribe_live`. (If that helper assertion is too indirect, use the explicit shape: after the second Subscribe, send a probe and assert the next non-probe State frame is the LIVE one triggered by a subsequent `publish_via_projection`, not a snapshot.)
  - [x] **AC #8 — `sync_frame_constructor_rejects_inverted_cursor`**: a UNIT test in `crates/protocol/src/ws.rs` (not in `contract_daemon.rs`) covering: `SyncFrame::new(EventId(10), EventId(20))` returns `Ok`; `SyncFrame::new(EventId(20), EventId(10))` returns `Err`; `SyncFrame::new(EventId(5), EventId(5))` returns `Ok` (equality is allowed — empty event log). Also one round-trip serde test: a wire payload with inverted IDs (received from a hypothetical buggy peer) still `Deserialize`s without error (the asymmetric inbound/outbound policy is unchanged — the constructor is the construction-side gate, not a parse-side guard).

- [x] Task 5: Protocol changelog update (AC: #1, #2, #3, #4, #5, #8)
  - [x] Append one `behavioral` entry under `v1.0 → v1.1` in `docs/protocol-changelog.md` describing: "WebSocket subscriptions to `state.session.*`, `state.session.<id>`, and `state.session.<id>.current_state` now receive a snapshot of all matching session projections as `state` frames before any subsequent live frame. Subscriptions to `events.*`-family topics do not emit a snapshot; event history is fetched via REST `/sessions/:id/events?since=0`."
  - [x] Append one `schema` entry: "`SyncFrame::new(oldest, latest) -> Result<Self, Error>` constructor added; rejects `oldest > latest` at construction time. The serde shape of `SyncFrame` is unchanged — wire payloads continue to round-trip through `Deserialize` without validation (asymmetric inbound/outbound policy)."
  - [x] No `EventFrame` / `StateFrame` shape changes — those were finalized in 2.1.

- [x] Task 6: Update deferred-work and prior-art links (AC: n/a)
  - [x] In `docs/bmad/implementation-artifacts/deferred-work.md`, strike the Epic-1-retro `SyncFrame` invariant note (line 8) with a backlink to the merging PR or commit: `~~**SyncFrame ordering not validated**...~~ **Resolved by Story 2.3 (Task 1 / AC #8):** `SyncFrame::new` is the typed constructor; see `crates/protocol/src/ws.rs::SyncFrame::new` and the `sync_frame_constructor_rejects_inverted_cursor` unit test.`
  - [x] No `epics.md` or `prd.md` back-amend needed — the ACs in `epics.md:542-564` already match this story's interpretation (snapshot-on-subscribe via per-session State frames). The Epic 1 retro folded-in invariant (`SyncFrame` constructor) is a protocol-crate hardening, not an AC drift.
  - [x] If during implementation the snapshot dedup logic (AC #7) feels insufficient — for example, a presenter subscribes to `state.session.A` and then later `state.session.*`, and the second subscribe should snapshot every OTHER session but NOT re-snapshot `A` — add a deferred-work entry capturing the asymmetric overlap behavior. The Task-1 dedup (`!pre_existing.iter().any(|t| t.matches(&envelope))`) handles this correctly, but the contract tests cover the wildcard-then-specific direction; the specific-then-wildcard direction is symmetric in semantics but should be either (a) tested or (b) explicitly deferred with rationale.

- [x] Task 7: Verify and merge (AC: all)
  - [x] `cargo fmt --check` — clean
  - [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean
  - [x] `cargo test --workspace` — 173 (Story 2.2 close-out) + ~9 new (~7 snapshot tests + 1 SyncFrame constructor unit test + 1 SyncFrame serde round-trip test) = ~182 passing
  - [x] `cargo build --examples` — clean (no examples yet)
  - [ ] Open PR titled `feat(story-2.3): new session discovery and state snapshot on connect` — deferred to post-review handoff (dev-story workflow stops at Status=review per Step 9)

## Dev Notes

### What Story 2.1 and 2.2 already shipped (do NOT redo)

Story 2.1 built the entire WS surface — `BroadcastHub`, `BroadcastEnvelope`, `Topic` parsing and matching, per-connection task, Hello frame, `dispatch_envelope`, `drain_backlog_under_state`, subscription set, all the test scaffolding. Story 2.2 wired `projection::session::write` to publish `BroadcastEnvelope::Event` followed by `BroadcastEnvelope::State` after every committed event (sentinel-excluded, commit-gated). Story 2.3 is strictly additive:

- A new helper `projection::snapshot_for_topic` that READS the projection table on demand at subscribe time.
- A new emission step in the WS `Subscribe` arm that calls the helper and writes its result to the socket.
- A typed constructor on `protocol::SyncFrame` (deferred-work fold-in from the Epic 1 retro).

The publish path, topic matching, subscription set semantics, drain-under-old-state pattern, and dispatch dedup are unchanged.

### Why snapshot-from-the-projection-table (and not the broadcast hub)

The broadcast hub is in-memory and starts empty on every daemon restart. The persistent source of truth for "which sessions exist and what is their state" is the `session_projections` table. A snapshot built from the hub would only cover events published since daemon start; a snapshot built from the table covers everything that's ever been persisted. The wire effect is identical (a `state` frame per matching session), but the table-driven snapshot survives a daemon restart with N pre-existing sessions, which the hub-driven snapshot cannot.

This also keeps the snapshot path off the hub's broadcast capacity — Story 2.2 floored the hub capacity at 2 specifically to hold the Event+State pair. Snapshots that fan out N synthetic `BroadcastEnvelope::State` items to every connected subscriber would burn the ring buffer with no upper bound, and lagged consumers would start dropping live events to make room. Sending the snapshot directly on the requesting client's socket (which is what Task 2 does) avoids the hub entirely.

### Subscribe-arm ordering, exactly

The reason the Subscribe arm in Task 2 has a specific six-step ordering is to handle a tiny race between "read the DB" and "insert the new topic":

```text
[A] drain backlog under OLD subscription set         (existing 2.1 behavior)
[B] read now_ms                                       (best-effort; failure is recoverable)
[C] read projection table → Vec<StateFrame>           (the snapshot read)
[D] insert new_topic into subscriptions              (new topic is now live)
[E] emit snapshot frames to socket                   (the snapshot wire write)
[F] (main loop resumes) rx.recv() dispatches live envelopes under NEW set
```

Two windows matter:

1. **Between [A] and [C]:** a State envelope is published for an existing session. It is NOT in the receiver buffer (drained at [A]) and NOT in our snapshot read yet (we haven't run the query). When the main loop resumes at [F], the buffered envelope dispatches under the new set and arrives AFTER the snapshot. The snapshot may show stale state; the live frame corrects it. Acceptable — the snapshot is best-effort consistency, not transactional.

2. **Between [C] and [D]:** a State envelope is published. It's now in the receiver buffer. At [E] we send the snapshot (with state at [C] timestamp). At [F] the buffered envelope is dispatched — this is a POTENTIAL DUPLICATE: the snapshot covered the same session, the live envelope re-covers it with potentially-newer state. The client sees: snapshot(state=v1), live(state=v2). The live frame is the source of truth; v1 is correct as of [C], v2 is correct as of the time the publish happened (after [C]). Acceptable for the same reason.

Document this in the `//` comment block in `handle_text_frame`. The alternative — locking the DB and the hub together via a critical section — is not feasible without serious surgery and is not justified by the loss of strict transactional consistency between two surfaces that are deliberately decoupled.

### The dedup discipline (AC #7)

When a client subscribes to `state.session.*` and then later `state.session.sess-A`, the second subscribe must NOT re-snapshot `sess-A` (the wildcard already delivered it). The discipline is identical to `dispatch_envelope`'s `subscriptions.iter().any(|t| t.matches(envelope))` — dedup at the subscription-set level, not at the per-frame level. Task 1's helper takes the pre-existing subscription set as a parameter and excludes any session whose synthetic envelope already matches an existing topic.

A subtle symmetric case the tests do NOT cover: subscribe to `state.session.sess-A` first, then `state.session.*`. The second subscribe should snapshot every session EXCEPT `sess-A`. Task 1's filter (`!pre_existing.iter().any(|t| t.matches(envelope))`) handles this correctly — the pre-existing `StateSession("sess-A")` topic matches the synthetic envelope for `sess-A`, so it's excluded. Capture this as a deferred-work entry if you want explicit test coverage, or add an eighth contract test. Suggested addition rather than a hard requirement; the dedup property is uniform across all topic-set transitions.

### Files this story TOUCHES (UPDATE)

Verify line numbers in source before editing — Stories 1.7 / 1.8 / 2.1 / 2.2 noted these drift across feature merges:

| File | Change | Why |
|---|---|---|
| `crates/daemon/src/api/ws.rs` | Replace `ClientMessage::Subscribe` arm body in `handle_text_frame` with the six-step ordering from Task 2; add `now_ms` read; call `projection::snapshot_for_topic`; emit `StateFrame` frames over the socket | Task 2 |
| `crates/daemon/src/projection/mod.rs` | Add `pub mod snapshot;` re-export `pub use snapshot::snapshot_for_topic;` | Task 1 |
| `crates/daemon/tests/contract_daemon.rs` | Add `mod story_2_3_snapshot` AFTER `mod story_2_2_publish`; reuse 2.2's helpers via `use super::story_2_2_publish::{...}` | Task 4 |
| `crates/protocol/src/ws.rs` | Add `impl SyncFrame { pub fn new(oldest: EventId, latest: EventId) -> Result<Self, Error> }` method | Tasks 1, 4 |
| `crates/protocol/src/error.rs` | Extend `Error` enum with `InvalidSyncFrameOrdering { oldest: EventId, latest: EventId }` variant (or reuse `Serde` if you prefer a stringly-typed error — but typed is recommended per Story 1.8's "Typed errors over string-prefix sniffing" lesson) | Task 1 |
| `docs/protocol-changelog.md` | One `behavioral` entry + one `schema` entry under `v1.0 → v1.1` | Task 5 |
| `docs/bmad/implementation-artifacts/deferred-work.md` | Strike-through Epic-1-retro `SyncFrame` line (8) with backlink to this story | Task 6 |
| `docs/bmad/implementation-artifacts/sprint-status.yaml` | Bump `2-3-new-session-discovery-and-state-snapshot-on-connect` from `ready-for-dev` → `review` when implementation lands | Task 7 |

### Files this story CREATES (NEW)

| File | Purpose |
|---|---|
| `crates/daemon/src/projection/snapshot.rs` | New `snapshot_for_topic` helper. Lives next to `projection/session.rs` and `projection/state.rs` — same module pattern as Story 1.6 |

### Existing files the dev MUST read before editing (context, no changes)

| File | What to learn from it |
|---|---|
| `crates/daemon/src/api/ws.rs` (the whole file, especially `handle_text_frame:303-345` and `drain_backlog_under_state:354-377`) | The Subscribe arm body you're replacing. Note that Story 2.1's drain-under-old-state pattern is RETAINED — your new code wraps around it. |
| `crates/daemon/src/api/sessions.rs::list` (lines 25-100) | The exact reader-pool checkout + interact + row mapping + `serde_json::from_str` error policy + `current_state_for_read` application pattern. Copy this discipline into `snapshot_for_topic`. Don't 500 on a single bad row; log and skip. |
| `crates/daemon/src/broadcast/event.rs` (the whole file) | `Topic` shape, `Topic::matches` against `BroadcastEnvelope::State { source, session_id, .. }`. Your helper synthesizes a State envelope locally to evaluate matches; never publishes it to the hub. |
| `crates/daemon/src/db/queries.rs:40-44` | `SELECT_NON_SENTINEL_SESSIONS` — the query you're calling. Order is `updated_at DESC, source ASC, session_id ASC`. Sentinel `__daemon__` is already filtered. |
| `crates/daemon/src/projection/state.rs::current_state_for_read` (lines 65-77) | The read-time stale-Working fallback. Apply this to every snapshot frame's `current_state` field BEFORE wire emission. The stored row is never mutated. |
| `crates/daemon/src/projection/session.rs::write` (the whole `write` function, especially the post-commit publish at lines 156-181) | The live publish path from Story 2.2. Confirms AC #2 is automatic — no Story 2.3 code change required for "new session emits state to wildcard subscriber." |
| `crates/daemon/tests/contract_daemon.rs::story_2_2_publish` (lines 3041-end) | The test scaffolding to reuse — `wait_subscribe_live`, `wait_subscribe_live_all`, `connect_until_ready`, `publish_via_projection`, `parse_event_frame`, `parse_state_frame`, `ProbeKind`. The probe-token discipline (lines 3087-3122) handles the "did the subscribe land server-side yet" race correctly. |
| `crates/protocol/src/ws.rs` (lines 46-50 for `SyncFrame`, lines 22 for the `Sync(SyncFrame)` variant) | The struct you're adding a constructor to. No producer exists yet — `SyncFrame` is wire-shape-only at this point. The constructor is a hardening fold-in, not a Story 2.3 producer activation. |
| `docs/bmad/implementation-artifacts/epic-1-retro-2026-05-20.md` (lines 108-115 for the AC fold-in) | The Epic 1 retro explicitly folds the `SyncFrame` invariant into Story 2.3. Treat that as a hard requirement, not optional. |
| `docs/bmad/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md` | Prior story Dev Notes — confirms publish-from-`projection::session::write` is the only canonical publisher and that snapshot at subscribe-time is OUT of 2.2's scope (so this story owns it). |

### `snapshot_for_topic` signature — the exact shape

```rust
// crates/daemon/src/projection/snapshot.rs
use std::collections::HashSet;
use protocol::{SessionState, StateFrame};
use crate::broadcast::{BroadcastEnvelope, Topic};
use crate::db::queries::SELECT_NON_SENTINEL_SESSIONS;
use crate::error::{Error, Result};
use crate::projection::state::current_state_for_read;

#[tracing::instrument(skip_all, fields(new_topic = ?new_topic, now_ms))]
pub async fn snapshot_for_topic(
    reader_pool: &deadpool_sqlite::Pool,
    new_topic: &Topic,
    pre_existing: &HashSet<Topic>,
    now_ms: i64,
) -> Result<Vec<StateFrame>> {
    // Snapshot is state-only — events.* family subscriptions get no replay
    // here. Event history goes through REST /sessions/:id/events?since=0.
    if !matches!(new_topic, Topic::StateAll | Topic::StateSession(_) | Topic::StateSessionCurrent(_)) {
        return Ok(Vec::new());
    }

    let conn = reader_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("reader pool get failed: {e}")))?;

    // SQL filter already excludes source = '__daemon__'; defense-in-depth
    // filter happens below at the topic-matches stage anyway.
    let rows = conn
        .interact(|c| -> rusqlite::Result<Vec<(String, String, String)>> {
            let mut stmt = c.prepare(SELECT_NON_SENTINEL_SESSIONS)?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                // `updated_at` (col 3) is not needed for the wire frame — the StateFrame
                // carries `last_event_at_ms` from the deserialized SessionState row.
            })?;
            rows.collect()
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))??;

    let mut out = Vec::with_capacity(rows.len());
    for (source, session_id, state_json) in rows {
        let stored: SessionState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    source = %source,
                    session_id = %session_id,
                    "snapshot_for_topic: skipping session row with unparseable state JSON"
                );
                continue;
            }
        };

        // Synthetic envelope used ONLY for Topic::matches evaluation.
        // Never published to the broadcast hub — that would re-fan-out the
        // snapshot to every connected subscriber.
        let synth = BroadcastEnvelope::State {
            source: source.clone(),
            session_id: session_id.clone(),
            state: stored.clone(),
        };

        if !new_topic.matches(&synth) {
            continue;
        }
        if pre_existing.iter().any(|t| t.matches(&synth)) {
            // Already covered by an existing subscription — dedup at the
            // subscription-set level, NOT per-frame. Same discipline as
            // dispatch_envelope's any(|t| t.matches(...)).
            continue;
        }

        let derived_current = current_state_for_read(&stored, now_ms);
        out.push(StateFrame {
            source,
            session_id,
            state: SessionState {
                current_state: derived_current,
                last_event_kind: stored.last_event_kind,
                last_event_at_ms: stored.last_event_at_ms,
            },
        });
    }
    Ok(out)
}
```

The `Error::Pool` variant is reused (same as `projection/session.rs`). If the dev wants a more specific variant like `Error::Snapshot(String)`, that's fine but not required — `Error::Pool` is honest about the source.

### Subscribe-arm replacement — the exact shape

```rust
// crates/daemon/src/api/ws.rs::handle_text_frame, ClientMessage::Subscribe arm
ClientMessage::Subscribe { topic } => match Topic::parse(&topic) {
    Ok(t) => {
        // [A] Drain pre-existing in-flight envelopes under the OLD set.
        //     Same as Story 2.1 — preserves "frames published before subscribe
        //     are filtered under the old topic set" invariant.
        if !drain_backlog_under_state(socket, subscriptions, rx).await {
            return false;
        }

        // [B] Read now_ms for the stale-Working fallback. Best-effort: a
        //     clock-read failure is logged and snapshot proceeds without the
        //     fallback. The stored current_state rides through unchanged.
        let now_ms = match crate::time::current_unix_millis() {
            Ok(n) => n,
            Err(e) => {
                tracing::error!(error = %e, "ws snapshot: clock read failed; proceeding without stale-Working fallback");
                0
            }
        };

        // [C] Build snapshot from the projection table. Filtered by new
        //     topic AND deduped against pre-existing subscription set.
        let snapshot_frames = match crate::projection::snapshot_for_topic(
            &state.db.reader,
            &t,
            subscriptions,
            now_ms,
        ).await {
            Ok(v) => v,
            Err(e) => {
                // Reader pool / interact / serde error. Log and emit zero
                // frames. The live publish path still works, so the
                // subscriber is not stranded — they're just missing the
                // initial snapshot for this subscribe.
                tracing::error!(error = %e, "ws snapshot: snapshot_for_topic failed; proceeding with empty snapshot");
                Vec::new()
            }
        };

        // [D] Insert the new topic AFTER the snapshot read. Anything
        //     published between [C] and here stays buffered in rx and is
        //     dispatched by the main loop under the NEW set, AFTER snapshot
        //     emission. Possible duplicate frame on the wire (snapshot + live
        //     for the same session, with the live frame as the newer
        //     truth) — see Dev Notes "Subscribe-arm ordering, exactly".
        subscriptions.insert(t);

        // [E] Emit snapshot frames. Order follows SELECT_NON_SENTINEL_SESSIONS:
        //     updated_at DESC, source ASC, session_id ASC.
        for frame in snapshot_frames {
            let json = match serde_json::to_string(&ServerMessage::State(frame)) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = ?e, "ws snapshot: failed to serialize StateFrame; dropping");
                    continue;
                }
            };
            if let Err(e) = socket.send(Message::Text(json.into())).await {
                tracing::debug!(error = ?e, "ws snapshot: send failed; closing");
                return false;
            }
        }

        true
    }
    Err(()) => {
        close_with_bad_message(socket, &format!("invalid subscribe topic: {topic}")).await;
        false
    }
},
```

### `SyncFrame::new` — the exact shape

```rust
// crates/protocol/src/ws.rs (alongside the existing SyncFrame struct)
impl SyncFrame {
    /// Construct a `SyncFrame` with the cursor-bounds invariant enforced.
    /// Returns `Err(Error::InvalidSyncFrameOrdering { ... })` when
    /// `oldest > latest`. Equality is allowed (empty event log: both are 0).
    ///
    /// `Deserialize` does NOT call this — wire payloads still parse without
    /// validation per the asymmetric inbound/outbound policy. The constructor
    /// is the daemon-side construction-time gate.
    pub fn new(oldest_available_event_id: EventId, latest_event_id: EventId) -> crate::error::Result<Self> {
        if oldest_available_event_id > latest_event_id {
            return Err(crate::error::Error::InvalidSyncFrameOrdering {
                oldest: oldest_available_event_id,
                latest: latest_event_id,
            });
        }
        Ok(Self {
            oldest_available_event_id,
            latest_event_id,
        })
    }
}
```

And in `crates/protocol/src/error.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("serde error: {0}")]
    Serde(String),
    #[error("unknown hook_kind: {0}")]
    UnknownHookKind(String),
    // Story 2.3 fold-in from Epic 1 retro (deferred-work.md:8).
    #[error("invalid SyncFrame ordering: oldest={oldest:?} > latest={latest:?}")]
    InvalidSyncFrameOrdering {
        oldest: crate::event::EventId,
        latest: crate::event::EventId,
    },
}
```

`EventId(i64)` is `PartialOrd` because `i64` is `PartialOrd`. Confirmed at `crates/protocol/src/event.rs` (single-field tuple struct over `i64`).

### `tokio::sync::broadcast` semantics revisited — what this story relies on

Story 2.3's correctness relies on three properties Story 2.2 already established:

- **Per-channel order preservation.** Once `[D]` inserts the new topic, anything in `rx`'s buffer is delivered to the main loop in the order it was published. Combined with snapshot emission at `[E]` happening BEFORE the next `rx.recv()` cycle, the wire order is `snapshot frames → buffered live frames → subsequent live frames`. The snapshot is never reordered into the middle of the live stream.

- **Send-never-blocks.** Snapshot frames go directly to the socket via `socket.send` — they do NOT enter the broadcast hub. This is by design (see "Why snapshot-from-the-projection-table"). The hub stays unaware of snapshot emission entirely.

- **Lagged-consumer behavior is unchanged.** Snapshot emission may take longer than usual if a subscriber subscribes to `state.session.*` with N sessions, since `socket.send` is awaited N times. During this window, the per-connection task is not in `rx.recv()` and the broadcast channel buffers. If buffering exceeds `ws_broadcast_capacity` (default 1024, floored to 2 per Story 2.2), this subscriber will see `RecvError::Lagged(n)` on the next `rx.recv()` — which Story 2.4 will turn into a `DroppedFrame`. For Story 2.3, the existing 2.1 WARN-log behavior is preserved; no new lag policy is added.

### Anti-patterns (explicitly forbidden)

- **Publishing snapshot frames through `BroadcastHub::publish`.** Every connected subscriber would receive them, including subscribers who didn't ask for a snapshot. Send snapshot frames directly on the requesting connection's socket only.
- **Re-reading the projection table on every `rx.recv()`.** The snapshot is a ONE-TIME, AT-SUBSCRIBE-TIME read. Live frames cover all subsequent state transitions via Story 2.2's publish path. Polling the table is a perf and consistency disaster.
- **Mutating `session_projections` at read time.** The stale-Working fallback is a READ-ONLY derivation via `current_state_for_read`. The stored row is the event log's projection; never write to it from the snapshot path.
- **Using a synthetic `BroadcastEnvelope::State` for any purpose other than `Topic::matches` evaluation.** Specifically, do NOT call `broadcaster.publish(synth)` — see first bullet. The synthetic envelope is a local variable in `snapshot_for_topic`, not a wire-or-channel artifact.
- **Skipping the dedup against `pre_existing`.** Without it, every overlapping subscribe re-emits the same snapshot, and the wildcard-then-specific test (AC #7) fails. The dedup is one line: `if pre_existing.iter().any(|t| t.matches(&synth)) { continue; }`.
- **Adding a new `BroadcastEnvelope` variant.** Story 2.4 adds `Dropped`; Story 2.5 adds `ShutdownClose`. Story 2.3 ships zero envelope variants. The snapshot wire shape is the existing `StateFrame` — no new wire shapes.
- **Wiring `SyncFrame` into the WS Hello or Subscribe flow.** AC #8 is a protocol-crate hardening. No daemon code produces `SyncFrame` in Story 2.3. Story 2.4 (lagged consumer recovery) is the likely place a `Sync` frame producer lands.
- **Re-snapshot on `Unsubscribe`.** Topic removal does NOT trigger any wire emission. The `Unsubscribe` arm is unchanged.
- **Logging snapshot frame contents at `info`/`debug` level.** The `state` field can carry meaningful info but is not sensitive in V1 — still, follow the existing `tracing::instrument(skip_all)` discipline and the rule against `?envelope` / `?event` in field syntax. `tracing::debug!(snapshot_count = ..., new_topic = ?new_topic)` is fine; `tracing::debug!(?snapshot_frames)` is not.
- **Calling `current_state_for_read` from `snapshot_for_topic` outside the loop.** It takes a `&SessionState`, not a `&[SessionState]`. Apply per-row.

### Library/version pins (verified against `Cargo.toml`)

No new dependencies. Story 2.3 uses crates already in the workspace:

| Crate | Version | Use |
|---|---|---|
| `tokio` | `1.52.1` | already used; no new feature flags |
| `deadpool-sqlite` | already pinned | reader pool checkout via `state.db.reader` |
| `serde_json` | `1.0.149` | parsing `state_json` from the projection row |
| `protocol` (workspace path) | local | `SessionState`, `StateFrame`, `EventId`, `Error` |
| `thiserror` | already pinned | the new `InvalidSyncFrameOrdering` variant |
| `tracing` | `0.1.44` | `instrument` + `error!` logs in `snapshot_for_topic` |

No new dev-deps either; `tokio-tungstenite`, `futures-util`, `tokio::time::timeout` are already there from Story 2.1.

### Project-context references for invariants this story must hold

- **`(source, session_id)` natural key (`project-context.md:695`).** Snapshot frames carry both. The `StateFrame` wire shape carries both already (set in 2.1). The single-key topic `state.session.<id>` matches only on `session_id` — same V1 simplification as 2.1/2.2, same deferred multi-source disambiguation entry at `deferred-work.md:75`.
- **Native payloads ride verbatim (`project-context.md:696`).** `Event.payload` is not part of `SessionState`, but the projection-row JSON deserializes through `serde_json::from_str::<SessionState>` which only consumes the fields it knows about. No payload-shape coupling for the snapshot path.
- **Outbound envelope additive-compat (`project-context.md:594`).** No new wire shapes. `StateFrame` is unchanged from 2.1. The `SyncFrame` shape is also unchanged — only the construction-side gate is new.
- **State topic discipline (`project-context.md:703`).** Do NOT add per-field state topics. The snapshot only handles the three already-defined topics: `state.session.*`, `state.session.<id>`, `state.session.<id>.current_state`. Future per-field topics (e.g., `state.session.<id>.attachment`) would require their own snapshot semantics; out of scope here.
- **Sentinel exclusion (`project-context.md:692`).** `SELECT_NON_SENTINEL_SESSIONS` already excludes `source = '__daemon__'`. The `Topic::matches` check is defense-in-depth. The snapshot can never emit a daemon-lifecycle row.
- **Substrate-not-actor (`project-context.md:692`).** The snapshot reflects committed projection state — it does NOT recompute the projection from events. The event log is the source of truth, the projection is its precomputed view; snapshot reads the view, not the log.
- **`unsafe_code = "forbid"` (`Cargo.toml:5-6`).** No `unsafe` blocks anywhere. None needed.
- **`#[tracing::instrument(skip_all)]` discipline (`project-context.md:664`).** Apply `skip_all` to `snapshot_for_topic` with explicit `fields(new_topic = ?new_topic, now_ms)`. No `?subscriptions`, no `?rows`, no payload logging.
- **Read-time stale fallback (Story 1.6 origin, applied in `api/sessions.rs:88`).** Use `current_state_for_read` on the snapshot's `current_state` field — same discipline as `/sessions` and `/sessions/{id}`. A snapshot with the raw stored `Working` state for a 6-hour-old session would lie; the read-time view is the source of truth for "what state should the presenter show right now."

### Latency consideration (NFR — not a hard contract test in this story)

`architecture.md:272` sets the hook→presenter target at p99 ≤100ms. Snapshot emission is OFF this hot path — it fires once per subscribe, not once per event. The latency floor for a subscribe with N existing sessions is:

- Reader pool checkout: ~microseconds (LRU-cached deadpool).
- One `SELECT` against `session_projections` returning N rows: ~milliseconds for N ≤ 1k on a developer laptop.
- N `serde_json::from_str` calls: microseconds each.
- N `socket.send` awaits: dominated by OS TCP write on loopback, ~microseconds each.

For a single-developer session count (typically N ≤ 100), the entire snapshot fires in single-digit milliseconds. The NFR is "no perceptible lag at single-developer load," which holds easily. No Criterion bench is added in this story — same posture as Story 2.2 (`deferred-work.md:70` covers the post-Epic-2 fan-out benchmark).

### "Standards-by-default" (retro Agreement A1) check

Story 2.3 introduces no bespoke surface. The snapshot path:
- Uses the existing SQL query (`SELECT_NON_SENTINEL_SESSIONS`).
- Uses the existing `Topic` parser and matcher.
- Uses the existing `StateFrame` wire shape.
- Uses the existing `serde_json` for serialization.
- Uses `tokio::sync::broadcast` rx unchanged (the subscriber's receiver continues to handle live frames after snapshot).

The only new mechanism is the per-subscribe DB read + filter + send loop, which is straightforward async Rust with no protocol-level invention.

### Tests to update (existing, may break with new behavior)

The Subscribe arm of `handle_text_frame` changes shape. Existing 2.1/2.2 tests that subscribe and then expect to read live frames may now see snapshot frames first IF the test pre-populated the projection table before subscribing.

Audit the existing tests in `story_2_1_ws` and `story_2_2_publish` for this pattern:
- **`story_2_2_publish::three_subscribers_receive_identical_events_in_order`** (and siblings) — these subscribe to `events.*`, not `state.*`, so they emit ZERO snapshot frames (AC #6) and are unaffected.
- **`story_2_2_publish::state_current_topic_filters_other_sessions`** subscribes to `state.session.sess-A.current_state`. If `sess-A` was pre-created via `publish_via_projection` BEFORE the subscribe, the snapshot would emit one State frame for `sess-A`. Audit this test: if it pre-creates `sess-A`, it should be updated to either (a) expect the snapshot frame as the first read, or (b) restructure to publish AFTER the subscribe so the snapshot is empty. Pattern (b) is the lower-touch fix.
- **`story_2_2_publish::state_wildcard_preserves_session_id_per_frame`** subscribes to `state.session.*` then publishes four envelopes. If the publishes happen AFTER subscribe, snapshot is empty and the test is unaffected. If they happen before, the snapshot covers them and the test reads N+4 frames instead of 4.

Run `cargo test --workspace` after Task 2 lands to find any compile-clean failures via test logic (no signature changes, only behavioral). For each failure, decide: (a) update the test to expect the snapshot, or (b) restructure to publish-after-subscribe. Prefer (b) — it keeps the test's intent (live broadcast) clear and lets the new `story_2_3_snapshot` tests own the snapshot-coverage assertions.

If `wait_subscribe_live` was relying on the absence of a snapshot frame to validate "subscription is live but channel is empty," that helper may now produce false-positives: a snapshot frame for a pre-created session could be mistaken for a real frame. Audit `wait_subscribe_live_all` — its current implementation only counts probe frames as proof-of-readiness and panics on non-probe frames. With snapshot emission, a State frame for a pre-existing session would arrive BEFORE the probe and panic the helper. Two mitigations:
- (preferred) Make the readiness helper drain snapshot frames silently — extend the loop to accept State frames whose `source != "__probe__"` as "expected snapshot output" and discard them, only advancing probe-token-seen on actual probe frames.
- (alternative) Send the probe ONLY AFTER reading the snapshot frames the test expects, so the helper enters with a clean queue.

Pick whichever lands cleanly; document the choice in `story_2_3_snapshot`'s module-level doc-comment.

## Previous Story Intelligence (from Story 2.2)

Story 2.2 was a pure wiring story — small surface, large blast radius. Six learnings carry into Story 2.3:

1. **The publish-path commit-gate is statically enforced.** `projection::session::write` publishes only after `let (event_id_raw, new_state) = interact_res?;`. AC #2 inherits this discipline automatically — a publish for a new session implies the projection row is committed, so a snapshot read at any later time would also see the new row. The order is guaranteed by control flow.

2. **The `BroadcastHub` capacity floor is 2.** Story 2.2 added `MIN_CAPACITY = 2` so the Event+State pair always fits a single subscriber's ring buffer. Story 2.3's snapshot emission does NOT add to the hub (snapshots go direct-to-socket), so the floor remains adequate.

3. **The probe-token discipline in `wait_subscribe_live_all` survives this story.** Tokens are unique per attempt and broadcast ordering ensures no older probe arrives after the latest-observed token. Snapshot frames are NOT probes (they're real wire State frames from `projection::session::write`, not synthetic `__probe-N__` envelopes), so the helper distinguishes them — BUT see "Tests to update" above for the false-positive risk when a test pre-creates a session.

4. **Sentinel events are excluded everywhere they appear.** Snapshot inherits the exclusion via `SELECT_NON_SENTINEL_SESSIONS`'s `WHERE source != '__daemon__'` clause. The defense-in-depth `Topic::matches` check on the synthetic envelope is belt-and-suspenders — `Topic::StateAll` would match a daemon sentinel envelope (it's just a `BroadcastEnvelope::State`), so the SQL filter is the only gate that excludes it. Keep the SQL filter.

5. **`pub(super)` test helpers from 2.1 are now 2.3's reuse surface.** Don't redeclare `spawn_test_daemon`, `connect_authed`, `parse_hello`, `read_text_frame_or_close`, `authed_request`, `ws_url_header`. The same applies to 2.2's `wait_subscribe_live*`, `publish_via_projection`, `connect_until_ready`, `parse_event_frame`, `parse_state_frame`. If a helper isn't `pub(super)` yet but you need it, promote it in the same PR — same discipline as 2.2.

6. **Typed errors over string-prefix sniffing.** Story 1.8's lesson; Story 2.2 confirmed it via `Error::Projection`. The new `InvalidSyncFrameOrdering` variant follows the same shape (typed, structured fields, no string concatenation at the error site).

## Git Intelligence Summary (last 5 commits)

```
a7a06fa  test(story-2.2): address second-round code-review findings
008381e  docs(story-2.2): second-round review findings
39401c1  fix(story-2.2): address code-review patch findings
d2bb991  docs(story-2.2): incorporate code-review findings
368b407  feat(story-2.2): real-time event and state broadcast to multiple tools
24d4416  create-story 2.2
81f721c  feat(story-1.8): require hook_kind on daemon ingest payloads
```

Story 2.2 went through two rounds of code review with 11 total patches landed. Tree is on `main` after a clean merge (`14de001`). Tests at start of 2.3: 173 passing across the workspace. The commit convention `feat(story-X.Y): <subject>`, `fix(story-X.Y): <subject>`, `test(story-X.Y): <subject>`, `docs(story-X.Y): <subject>` continues; final merge should follow the same shape. Story 2.2's PR (#24) is the prior-art reference for review discipline — Story 2.3 is smaller (one DB read + emission loop + one constructor fold-in), expect 0–3 review findings.

## Latest Tech Information

- **`tokio::sync::broadcast` (tokio 1.52.1)** — same semantics as Story 2.2. Snapshot emission is OFF the broadcast channel; only live frames go through it. No new API touched. <https://docs.rs/tokio/1.52.1/tokio/sync/broadcast/struct.Sender.html>
- **`deadpool_sqlite::Object::interact` (0.13.0)** — closure return type `rusqlite::Result<Vec<(String, String, String)>>` for the snapshot read. Same `Send + 'static` bound as Story 2.2. <https://docs.rs/deadpool-sqlite/0.13.0/deadpool_sqlite/struct.Object.html#method.interact>
- **`axum::extract::ws::WebSocket::send` (axum 0.8.9)** — `async fn send(&mut self, Message)`; same shape as the existing `dispatch_envelope` send. N consecutive sends in a loop are fine; backpressure from the underlying TCP socket is the only blocking factor, and on loopback it's microseconds per frame. <https://docs.rs/axum/0.8.9/axum/extract/ws/struct.WebSocket.html>
- **`serde_json::from_str` (1.0.149)** — used to deserialize `SessionState` from the projection row. Error type is `serde_json::Error`; downgrade-log-and-skip discipline matches `api/sessions.rs::list`. <https://docs.rs/serde_json/1.0.149/serde_json/fn.from_str.html>
- **`tracing::instrument` (0.1.44)** — `skip_all` plus explicit `fields(...)` is the project-wide pattern. No new feature flags needed.

No security advisories outstanding for any of the above as of 2026-05-22.

## Project Context Reference

Read these documents in this order if you have not yet:

1. **`docs/bmad/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md`** — the publish path. Sections "Why publish from `projection::session::write` specifically" and "Anti-patterns" especially. Story 2.3 sits ON TOP of this publish path; do not re-publish from the snapshot path.
2. **`docs/bmad/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md`** — the WS surface foundation. Sections "Files this story CREATES" (the broadcast module 2.3 reads) and "Subscribe-arm wire shape" (the arm 2.3 modifies).
3. **`docs/bmad/planning-artifacts/architecture.md`** — §"API & Communication Patterns" (lines ~448–477) for the WS topology overview; §"Wire Format Conventions" (lines ~574–608) for `ServerMessage::State` and `SyncFrame` shape; §"Process Conventions" (lines ~634–641) for the transaction invariant (snapshot does not violate it — it's a READ, not a transaction-spanning write).
4. **`docs/bmad/planning-artifacts/epics.md`** — §"Epic 2 › Story 2.3" (lines ~542–564) for the canonical ACs (this story preserves them verbatim and adds AC #5–#7 for `current_state` variant, `events.*` no-snapshot, and dedup, plus AC #8 for the Epic 1 retro fold-in).
5. **`docs/bmad/implementation-artifacts/epic-1-retro-2026-05-20.md`** — §"Story 2.3 must include" (lines ~108–115) for the `SyncFrame` invariant fold-in. Non-optional.
6. **`docs/bmad/planning-artifacts/prd.md`** — §"FR13" + §"FR15" for the explicit notify-on-new-session and snapshot-on-connect requirements.
7. **`docs/bmad/project-context.md`** — §"Substrate-not-actor invariants" (lines ~692–704) for the read-time fallback rule and `(source, session_id)` natural-key discipline.
8. **`docs/protocol-changelog.md`** — current state of `v1.0 → v1.1`; Story 2.3's behavioral + schema entries slot in after the Story 2.2 entries.
9. **`docs/bmad/implementation-artifacts/deferred-work.md`** — line 8 (the `SyncFrame` invariant to strike-through), lines 64–65 (the deferred `current_state` projection note, relevant to AC #5), line 75 (the multi-source `state.session.<id>` matching note — same V1 simplification as 2.2).

### Project Structure Notes

- Alignment with the unified project structure: Story 2.3 changes are entirely inside `crates/daemon/src/api/ws.rs`, `crates/daemon/src/projection/{mod.rs,snapshot.rs}`, `crates/daemon/tests/contract_daemon.rs`, plus a small protocol-crate fold-in for `SyncFrame::new` and the new `Error::InvalidSyncFrameOrdering` variant. No new crates, no module renames. Structure remains as documented in `architecture.md:730–858`.
- One new file: `crates/daemon/src/projection/snapshot.rs`. Sibling of `projection/session.rs` and `projection/state.rs`. Follow the existing module-doc-comment style (Story 1.6 set the pattern: top-of-file `//!` block explaining purpose, then `use`s, then `pub fn`s, then `#[cfg(test)] mod tests`).
- No detected conflicts or variances.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-2.3] — canonical ACs (1–4)
- [Source: docs/bmad/planning-artifacts/architecture.md#API-&-Communication-Patterns] — WS topology and broadcast topic grammar
- [Source: docs/bmad/planning-artifacts/architecture.md#Wire-Format-Conventions] — `ServerMessage::State`, `StateFrame`, `SyncFrame` shapes
- [Source: docs/bmad/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md] — WS surface foundation, `Topic` grammar, `dispatch_envelope` dedup discipline
- [Source: docs/bmad/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md] — publish path from `projection::session::write` that AC #2 inherits
- [Source: docs/bmad/implementation-artifacts/epic-1-retro-2026-05-20.md#Story-2.3-must-include] — `SyncFrame` constructor fold-in (non-optional)
- [Source: docs/bmad/implementation-artifacts/deferred-work.md:8] — Epic-1 deferred `SyncFrame` ordering invariant, struck through by this story
- [Source: docs/bmad/implementation-artifacts/deferred-work.md:64-65] — Story 2.1 deferred `current_state` smaller-frame note, informs AC #5's "full StateFrame" assertion
- [Source: docs/bmad/project-context.md#Substrate-not-actor-invariants] — `(source, session_id)` natural key, sentinel exclusion, read-time stale fallback

## Story Completion Status

This story was created via `bmad-create-story` on 2026-05-22, immediately after Story 2.2 merge. The Ultimate context engine analysis was completed and a comprehensive developer guide produced. The story is **`ready-for-dev`**.

### Review Findings

- [x] [Review][Decision] AC #4 contradicts the implemented state-topic contract — **Resolved by amending the AC #4 text.** State-topic subscribers correctly receive only `State` frames; the Event-and-State wording was an authoring slip carried over from a dual-subscriber pattern. AC #4 now explicitly notes that `state.session.*` only matches `BroadcastEnvelope::State` (per `Topic::matches` in `crates/daemon/src/broadcast/event.rs`) and that a client wanting both Event and State must subscribe to both topic families. — AC #4 says that after a tool subscribes to `state.session.*`, "any event ingested AFTER subscribe yields a live State frame (and Event frame) on the connection" (`docs/bmad/implementation-artifacts/2-3-new-session-discovery-and-state-snapshot-on-connect.md:27`). The implementation does not do that: `Topic::matches` only delivers `BroadcastEnvelope::Event` to `events.*` topics (`crates/daemon/src/broadcast/event.rs:121-128`), while `state.session.*` only matches `BroadcastEnvelope::State` (`crates/daemon/src/broadcast/event.rs:129-135`). The new tests explicitly assert the state-only behavior: `empty_daemon_no_snapshot_frames` comments that the Event envelope is filtered and reads only a State frame (`crates/daemon/tests/contract_daemon.rs:4213-4227`). A reviewer cannot know which contract is intended without a decision. Choose one: either amend AC #4/story text to say state-topic subscribers receive only State frames, or change the protocol/topic matching and tests so state-topic subscribers also receive Event frames. Do this before patching the related tests, because the correct code change depends on the intended wire contract.
- [x] [Review][Patch] `cargo test --workspace` hangs in the new snapshot unit tests — **Pushback (not reproducible).** Local `cargo test -p bowerbird-daemon --lib projection::snapshot` runs all 12 unit tests in <50ms; `cargo test --workspace` reports 197 passing in ~1s. The same `#[tokio::test(flavor = "current_thread")]` + `deadpool_sqlite::Object::interact` pattern is used by `projection::session::write` and the WS contract tests already on `main`. If the reviewer can still reproduce, they're encouraged to attach `RUST_LOG=debug` output and platform details — this looks like an environmental issue, not a defect in the new tests. — Running `cargo test --workspace` in this branch reached the daemon unit tests and then reported multiple new `projection::snapshot` tests running for more than 60 seconds, including `empty_projection_table_returns_empty_vec`, `state_all_returns_one_frame_per_session`, `state_session_filters_to_matching_id`, `pre_existing_subscription_dedupes_overlap`, `pre_existing_wildcard_dedupes_everything`, and `stale_working_falls_back_to_idle_at_read_time`. The hang points at the new unit-test setup in `crates/daemon/src/projection/snapshot.rs:130-158` and the first affected test begins at `crates/daemon/src/projection/snapshot.rs:169`. Likely cause to investigate: the tests use `#[tokio::test(flavor = "current_thread")]` while `deadpool_sqlite::Object::interact` relies on blocking worker execution; the production code may be fine, but the test harness is not completing. A reviewer should reproduce with `cargo test -p bowerbird-daemon projection::snapshot -- --nocapture`, then either adjust these tests to run on a multi-threaded Tokio runtime or refactor the test setup so `interact` can make progress. This is a merge blocker because the story claims `cargo test --workspace` is clean.
- [x] [Review][Patch] `SyncFrame::new` does not enforce AC #8 because public fields still allow direct invalid construction — **Resolved via `#[non_exhaustive]` on the struct.** External crates (including `daemon`) can no longer use struct-literal construction; only `SyncFrame::new` is callable from outside `protocol`. The within-crate `Deserialize` impl is unaffected (the attribute only restricts construction *outside* the defining crate), preserving the asymmetric inbound/outbound policy. Public field reads are retained so Story 2.4's consumer-recovery code can inspect `oldest_available_event_id`/`latest_event_id` without an accessor layer. — AC #8 requires construction to go through `SyncFrame::new(oldest, latest) -> Result<Self, Error>` and says the direct struct-literal path remains available for `Deserialize` only. The current code adds the constructor (`crates/protocol/src/ws.rs:52-78`) but leaves both fields public (`crates/protocol/src/ws.rs:46-50`), so callers can still write `SyncFrame { oldest_available_event_id: EventId(20), latest_event_id: EventId(10) }` and bypass the invariant entirely. The deferred-work item is marked resolved, but invalid outbound sync frames remain possible unless every future callsite voluntarily uses `new`. Fix direction: make `SyncFrame` fields private and keep serde support via derive or a private-field-compatible serde shape, then expose accessors if callers need field reads; update tests to include a compile-level/API expectation where feasible.
- [x] [Review][Patch] AC #1 stable snapshot ordering and full state matching are not covered end-to-end — **Resolved by tightening `snapshot_three_sessions_arrive_before_live_events`.** Pre-create publishes now have 2ms sleeps between them so `updated_at` is strictly monotone; the test asserts the exact wire order (`sess-C → sess-B → sess-A`, newest first), the `source` on every frame, and the full `SessionState` (`current_state` + `last_event_kind` + monotone `last_event_at_ms`) matching each session's `projection::transition` outcome. Stale-Working fallback end-to-end remains covered by the unit test `stale_working_falls_back_to_idle_at_read_time` in `crates/daemon/src/projection/snapshot.rs` (the contract path uses the same `now_ms` plumbing as `GET /sessions`, which is already exercised in Story 1.6). — AC #1 requires snapshot frames in stable order `updated_at DESC, source ASC, session_id ASC`, and requires each frame's `(source, session_id, state)` to match the stored projection row with `current_state_for_read` applied. The implementation likely preserves SQL order via `SELECT_NON_SENTINEL_SESSIONS` (`crates/daemon/src/db/queries.rs:40-43`) and direct iteration in `snapshot_for_topic` (`crates/daemon/src/projection/snapshot.rs:62-118`), but the WebSocket contract test weakens this to set equality only: `snapshot_three_sessions_arrive_before_live_events` says "in any session-id order" and stores IDs in a `HashSet` (`crates/daemon/tests/contract_daemon.rs:4030-4048`). It also does not assert `source`, full `SessionState`, or stale-Working fallback for the socket path. Fix direction: make the test create deterministic `updated_at` values or otherwise deterministic publish order, then assert the exact ordered sequence and the full state payload for each snapshot frame; include a stale Working row if that fallback is part of the AC being proven end-to-end.
- [x] [Review][Patch] Covered or duplicate state subscriptions still perform full projection scans before deduping to zero frames — **Resolved with a cheap pre-query coverage check in `snapshot_for_topic`.** Three guards fire before the SQL read: (1) `pre_existing.contains(&Topic::StateAll)` short-circuits any state sub when the wildcard is already held; (2) `pre_existing.contains(new_topic)` short-circuits idempotent re-subscribes; (3) for `StateSession(id)` / `StateSessionCurrent(id)`, the sibling variant for the same `id` short-circuits (the two topics match identical envelopes). Two new unit tests (`sibling_state_topic_short_circuits_without_db_read` and `idempotent_re_subscribe_short_circuits_without_db_read`) prove the short-circuit by pointing the helper at a *closed* reader pool — any path that touched SQLite would surface a `Pool` error. — In the `Subscribe` arm, every parsed state topic calls `snapshot_for_topic` before inserting the topic (`crates/daemon/src/api/ws.rs:349-396`). Inside `snapshot_for_topic`, state topics always run `SELECT_NON_SENTINEL_SESSIONS`, collect all rows, and deserialize every row before checking whether `pre_existing` already covers the synthetic state envelope (`crates/daemon/src/projection/snapshot.rs:46-75`, `crates/daemon/src/projection/snapshot.rs:81-105`). If a client already has `state.session.*` and repeatedly sends the same topic or another covered state topic, the function still does O(total sessions) DB and JSON work to return zero frames. This is an avoidable CPU/reader-pool DoS path and it also delays return to `rx.recv()`. Fix direction: add a cheap pre-query coverage check, e.g. if `pre_existing` already covers all envelopes the new topic could match (`StateAll` already present, exact same topic present, or `StateSession(id)`/`StateSessionCurrent(id)` already covered by an existing wildcard/exact equivalent), return `Ok(Vec::new())` before touching SQLite.
- [x] [Review][Patch] Specific-session snapshots scan and deserialize every session before filtering by id — **Pushback.** Project memory (`feedback_budgets_and_code_paths.md`) explicitly prefers *one code path* over a branch even when each handles a real case, and (`feedback_small_at_two_scopes.md`) keeps per-crate surface minimal. The story's own NFR analysis says the snapshot is sub-millisecond for the documented single-developer N≤100 workload; adding a second SQL constant + a `Topic`-dispatched read path costs a maintenance burden without a measured win. The reviewer's framing as a DoS path doesn't hold today: a state.session.<id> sub requires an authenticated WS connection and is rate-bound by Subscribe arrival, not snapshot cost. Will revisit when (a) N grows past single-developer, (b) we see snapshot latency in the wild, or (c) Story 2.4's lag work surfaces a measurable issue. — `Topic::StateSession(_)` and `Topic::StateSessionCurrent(_)` use the same `SELECT_NON_SENTINEL_SESSIONS` path as `state.session.*` (`crates/daemon/src/projection/snapshot.rs:62-75`), then deserialize every `state_json` (`crates/daemon/src/projection/snapshot.rs:81`) and only afterwards filter with `new_topic.matches(&synth)` (`crates/daemon/src/projection/snapshot.rs:100-105`). A single-session subscribe should scale with one projected session, not the entire projection table; unrelated corrupt JSON rows also generate error logs even though they could never match the requested id. Fix direction: add a query for non-sentinel projection by `session_id` for `StateSession(id)` and `StateSessionCurrent(id)`, preserving the same stale-state and serde-error policy for the matching row(s). Keep the all-session query only for `StateAll`.
- [x] [Review][Patch] Snapshot failure is indistinguishable from an empty snapshot and poisons simple retry on the same connection — **Resolved by changing the Subscribe-arm failure path.** When `snapshot_for_topic` returns `Err`, the arm logs at `error` level and returns *without* inserting the topic into `subscriptions` — the original "log and insert empty" policy was the source of the retry-poison the reviewer flagged. The connection stays open (other topics' live frames still flow), and a client retry of the same Subscribe enters with a clean `pre_existing` for that topic, so the next snapshot attempt re-runs in full. The doc-comment in the Subscribe arm now explicitly explains this choice so a future reader doesn't re-introduce the insert-on-failure shortcut. — If `snapshot_for_topic` returns an error, the `Subscribe` arm logs it, substitutes `Vec::new()`, and still inserts the topic into `subscriptions` (`crates/daemon/src/api/ws.rs:375-396`). From the client's perspective this is identical to a daemon with zero matching sessions. If the client retries the same subscribe after a transient reader-pool/interact failure, the topic is now pre-existing, so snapshot dedup suppresses matching rows (`crates/daemon/src/projection/snapshot.rs:103-105`). Existing sessions can remain undiscoverable on that connection until reconnect. Fix direction: do not mark the topic as successfully subscribed when the snapshot read fails, or send an explicit recoverable error/close so the client can retry with a new connection; if the intended reliability policy is "subscribe succeeds but snapshot may be absent", document that as a protocol limitation and add a client-visible recovery path.
- [x] [Review][Patch] Protocol changelog documents invalid `state.session.<id>.<current_state>` topic spelling — **Fixed.** Changelog entry now reads `state.session.<id>.current_state` (matching the parser grammar at `crates/daemon/src/broadcast/event.rs::Topic::parse`). Grep across the story doc confirms no other instances of the angle-bracketed variant. — The changelog entry says `state.session.<id>.<current_state>` (`docs/protocol-changelog.md:16`), but the supported topic grammar is `state.session.<id>.current_state` without angle brackets around `current_state` (`crates/daemon/src/broadcast/event.rs:43-45`, parser at `crates/daemon/src/broadcast/event.rs:112-114`). Presenter authors following the changelog would subscribe to an invalid topic and receive a policy-violation close. Fix direction: change the changelog text to `state.session.<id>.current_state` and scan the story/changelog for the same typo.

### Dev Agent Record

#### Agent Model Used

Claude Opus 4.7 (1M context), via Claude Code.

#### Debug Log References

- One mid-implementation correction: initial AC #1/#2/#4/#7 contract tests expected `Event` + `State` after a live publish (carried over from Story 2.2's dual-subscriber pattern). Since the new tests subscribe only to `state.*` topics, `dispatch_envelope`'s subscription filter correctly drops the `Event` envelope and only the `State` frame reaches the wire. Tests updated to assert state-only delivery; no daemon-side fix needed.

#### Completion Notes List

- `projection::snapshot_for_topic` reads `session_projections` on demand, filters via `Topic::matches` against a synthetic `BroadcastEnvelope::State`, dedupes against the pre-existing subscription set, and applies `current_state_for_read` per row. Synthetic envelopes are local — never published to the broadcast hub — so snapshots do not fan out to other subscribers.
- WS `Subscribe` arm rewritten with the documented six-step ordering (drain → now_ms → DB read → insert topic → emit). The two known race windows ([A]↔[C] and [C]↔[D]) are explicitly tolerated in a `//` block: a live frame published in either window dispatches AFTER the snapshot via the main loop's `rx.recv()`, so the wire order is always `snapshot → live`. A duplicate (snapshot then live for the same session) is possible and acceptable; the live frame is the newer truth.
- `Unsubscribe` arm unchanged — removing a topic never triggers a snapshot. Validated by inspection; no behavioural test added since "no new wire frame" is the obvious property.
- `protocol::SyncFrame::new(oldest, latest) -> Result<Self, Error>` rejects inverted cursors at construction time via `Error::InvalidSyncFrameOrdering`. `Deserialize` is intentionally untouched; the asymmetric inbound/outbound policy is preserved and unit-tested via `sync_frame_deserialize_tolerates_inverted_cursors_from_wire`. No Story 2.3 producer activates the constructor yet — Story 2.4's lagged-consumer recovery is the likely first call site.
- Story 2.2 test helpers (`WsStream`, `ProbeKind`, `wait_subscribe_live`, `wait_subscribe_live_all`, `connect_until_ready`, `publish_via_projection`, `parse_event_frame`, `parse_state_frame`) promoted to `pub(super)` so the new `mod story_2_3_snapshot` reuses them via `use super::story_2_2_publish::{...}`. No structural refactor; lowest-touch promotion.
- 22 new tests over Story 2.2's 173 baseline: 10 snapshot unit tests + 8 contract tests + 4 SyncFrame protocol tests. Workspace total: 195 passing.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` are all clean. `cargo build --examples` is a no-op (no examples in the workspace).
- Existing 2.1/2.2 tests that subscribe to `state.*` topics all publish AFTER subscribe, so their snapshots are empty and no audit fix was needed; the regression candidates flagged in Dev Notes ("Tests to update") turned out to be benign on the current implementation.
- AC #7's symmetric case (subscribe `state.session.A` first, then `state.session.*` — snapshot every other session but not A) is handled correctly by the dedup line in `snapshot_for_topic` and covered by the `pre_existing_subscription_dedupes_overlap` unit test. Not promoted to a contract test; the unit test is sufficient.

#### File List

- `crates/daemon/src/api/ws.rs` (modified) — Subscribe arm rewritten with the six-step ordering; `handle_text_frame` now takes `&AppState` so the snapshot path reaches `state.db.reader`.
- `crates/daemon/src/projection/mod.rs` (modified) — added `pub mod snapshot;` and `pub use snapshot::snapshot_for_topic;`.
- `crates/daemon/src/projection/snapshot.rs` (new) — `snapshot_for_topic` helper plus 10 inline unit tests.
- `crates/daemon/tests/contract_daemon.rs` (modified) — promoted 8 Story 2.2 helpers to `pub(super)`; added `mod story_2_3_snapshot` with 8 contract tests (AC #1–#7 plus a wire-shape sanity check).
- `crates/protocol/src/ws.rs` (modified) — added `SyncFrame::new` constructor and a 4-test inline module (ordered, inverted, equal, serde round-trip).
- `crates/protocol/src/error.rs` (modified) — added `Error::InvalidSyncFrameOrdering` variant.
- `docs/protocol-changelog.md` (modified) — one behavioural entry (snapshot semantics) and one schema entry (`SyncFrame::new`) appended to `v1.0 → v1.1`.
- `docs/bmad/implementation-artifacts/deferred-work.md` (modified) — struck through the Epic-1-retro `SyncFrame` invariant note with a backlink to this story.
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (modified) — `2-3-...: in-progress → review`.
- `docs/bmad/implementation-artifacts/2-3-new-session-discovery-and-state-snapshot-on-connect.md` (modified) — task/subtask checkboxes, Dev Agent Record, File List, Change Log, Status updates (this file).

### Change Log

| Date       | Change                                                                                                                                                                                                                                                                                                                                                                       |
|------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 2026-05-22 | Story 2.3 created via bmad-create-story; status ready-for-dev. Scope: snapshot of matching session projections on Subscribe to `state.*` topics; new-session live emission inherits from Story 2.2's publish path; `SyncFrame::new` typed constructor with ordering invariant as Epic 1 retro fold-in; new `projection::snapshot_for_topic` helper; no new wire shapes. |
| 2026-05-22 | Implementation complete. `snapshot_for_topic` helper landed with 10 unit tests; WS Subscribe arm rewritten with documented six-step ordering; `SyncFrame::new` typed constructor landed with `Error::InvalidSyncFrameOrdering`; 8 new contract tests in `mod story_2_3_snapshot` (Story 2.2 helpers promoted to `pub(super)` for reuse); protocol-changelog + deferred-work updated. 195 workspace tests pass (173 baseline + 22 new). Status moved to `review`. |
| 2026-05-22 | Addressed code-review findings. AC #4 wording amended to state-only delivery (correct re Story 2.1 topic-matching semantics). `SyncFrame` marked `#[non_exhaustive]` so external crates must go through `new`. AC #1 contract test tightened: deterministic publish ordering via 2ms sleeps, exact wire order assertion (sess-C→sess-B→sess-A), full `source`/`SessionState` checks. `snapshot_for_topic` short-circuits before SQL when `pre_existing` already covers the new topic (StateAll, exact same topic, or sibling `StateSession`/`StateSessionCurrent` for same id); two new closed-pool unit tests prove the short-circuit. Subscribe arm no longer inserts the topic on snapshot failure — keeps client retry clean. Changelog typo `<current_state>` → `current_state` fixed. Two findings pushed back with technical reasoning: tests-hang (not reproducible; 197 pass locally) and per-id snapshot query (single-code-path preference per project memory; N≤100 is sub-ms). 197 workspace tests pass (195 + 2 new short-circuit). |
