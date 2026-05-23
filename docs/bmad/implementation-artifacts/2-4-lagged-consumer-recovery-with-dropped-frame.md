# Story 2.4: Lagged consumer recovery with dropped frame

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want bowerbird to notify me with a single `dropped` frame when my tool falls behind the event stream, rather than silently losing events or closing my connection,
So that my tool can detect the gap and re-fetch state via REST to recover gracefully.

## Acceptance Criteria

**Given** a broadcast channel with capacity 1024 and a tool whose WebSocket read loop is blocked
**When** 1025 envelopes are published before the tool reads any
**Then** the tool receives exactly one `dropped` frame containing the lag count in envelopes (not bytes), and the next frame after the dropped frame is the next legitimate event — the socket remains open

**Given** a tool receives a `dropped` frame
**When** the tool calls `GET /sessions/:id/events?since=<last_delivered_event_id>` via REST
**Then** it can re-fetch the missed events using `oldest_available_event_id` in the response to detect whether the gap is recoverable (gap recoverable when `oldest_available_event_id <= last_delivered_event_id + 1`)

**Given** a tool is lagging continuously for 30 seconds past the drop threshold
**When** the backpressure policy is applied
**Then** the daemon does not emit 50,000 individual `dropped` frames — it coalesces them into a bounded number of `dropped` frames per policy period (backpressure escalation contract test from `project-context.md:596`); the bounded count is `ceil(30s / coalesce_window) ≤ 31`

**Given** a tool that has received a `dropped` frame
**When** it resumes consuming envelopes normally
**Then** subsequent envelopes arrive in order with no further interruption — the channel is not permanently degraded, and `last_delivered_event_id` continues to advance from the post-drop envelope

## Tasks / Subtasks

- [ ] **Task 1 — Add typed `DroppedFrame::new` constructor in `crates/protocol/src/ws.rs`** (AC: #1, #3; folds in `deferred-work.md:9`)
  - [ ] 1.1 Add `impl DroppedFrame { pub fn new(count: u64, first: EventId, last: EventId) -> Result<Self, Error> { ... } }` enforcing `count > 0`, `first <= last`. Reject `count == 0` (vacuous Dropped is a bug; coalescing should suppress it instead).
  - [ ] 1.2 Add `Error::InvalidDroppedFrame { count, first, last }` variant in `crates/protocol/src/error.rs` alongside the existing `InvalidSyncFrameOrdering` variant (same pattern from Story 2.3).
  - [ ] 1.3 Unit tests in `crates/protocol/src/ws.rs::tests`: `dropped_frame_new_accepts_valid`, `dropped_frame_new_rejects_zero_count`, `dropped_frame_new_rejects_inverted_ids`, `dropped_frame_new_allows_count_one_first_eq_last`, `dropped_frame_deserialize_tolerates_invalid_from_wire` (asymmetric inbound/outbound policy — `Deserialize` does NOT call `new`).
  - [ ] 1.4 Update `docs/bmad/implementation-artifacts/deferred-work.md` line 9 with strike-through + backlink: `~~**DroppedFrame invariants not validated**~~ **Resolved by Story 2.4 (Task 1):** ...`. Mirror the format of the SyncFrame entry resolved by Story 2.3 (line 8).

- [ ] **Task 2 — Add `ws_broadcast_coalesce_window` config knob in `crates/daemon/src/config.rs`** (AC: #3)
  - [ ] 2.1 Add field `pub ws_broadcast_coalesce_window: Duration` to `Config`.
  - [ ] 2.2 Default to `Duration::from_secs(1)` in `Config::with_bowerbird_dir`. At 30s sustained lag with a 1s window, frame count is bounded at ≤31, satisfying AC #3 with a healthy safety margin under 50,000.
  - [ ] 2.3 Add the same field to `WsConfig` in `crates/daemon/src/state.rs` (`coalesce_window: Duration`); wire it through `crates/daemon/src/main.rs` where `WsConfig` is constructed (look for the existing `ping_interval` / `pong_timeout` wiring — same pattern).

- [ ] **Task 3 — Per-connection cursor + coalescing state in `crates/daemon/src/api/ws.rs::connection_task`** (AC: #1, #2, #3, #4)
  - [ ] 3.1 Add three locals next to `awaiting_pong` / `pong_sleep`: `let mut last_delivered_event_id: Option<EventId> = None;`, `let mut last_dropped_at: Option<tokio::time::Instant> = None;`, `let mut pending_drop_count: u64 = 0;`.
  - [ ] 3.2 In `dispatch_envelope` (or its caller in the main loop), when an `Event` envelope dispatches successfully, set `last_delivered_event_id = Some(ev.event_id)`. State envelopes do NOT advance the cursor — they carry no `event_id`. (See Dev Notes "Cursor tracking only on Event dispatch" for the rationale.)
  - [ ] 3.3 Replace the body of the `Err(RecvError::Lagged(n))` arm at `crates/daemon/src/api/ws.rs:262-266` with the projection routine described in Dev Notes "Lagged → Dropped projection — the exact shape". The arm calls a new helper `emit_dropped_or_coalesce(socket, &mut last_dropped_at, &mut pending_drop_count, last_delivered_event_id, n, state.ws_config.coalesce_window).await`.
  - [ ] 3.4 Also replace the `Err(TryRecvError::Lagged(n))` arm in `drain_backlog_under_state` (lines 472-477) with the same helper. Both lag surfaces must coalesce together — they share the same per-connection state.
  - [ ] 3.5 After a `Dropped` frame emits, the next legitimate envelope (whether Event or State) is dispatched normally and updates `last_delivered_event_id` if it's an Event. The socket stays open. `pending_drop_count` resets to `0` on every successful emission.

- [ ] **Task 4 — `emit_dropped_or_coalesce` helper** (AC: #1, #3)
  - [ ] 4.1 New private async fn in `crates/daemon/src/api/ws.rs` — see Dev Notes "Coalescing helper — the exact shape" for the full body.
  - [ ] 4.2 Behaviour: on first call OR when `now - last_dropped_at > coalesce_window`, emit one `DroppedFrame` with `count = pending_drop_count + n` and computed `first/last` event ids; on subsequent calls within the window, only accumulate (`pending_drop_count += n`) without emitting. When lag eventually stops and a normal envelope arrives, any accumulated `pending_drop_count` from suppressed calls remains — it folds into the NEXT lag event (or is never emitted at all, which is acceptable per the design: a presenter that catches up entirely doesn't need a trailing recap).
  - [ ] 4.3 Wire-id computation:
    - `first_dropped_event_id = EventId(last_delivered.0 + 1)` when `last_delivered_event_id` is `Some`; otherwise `EventId(0)` (the empty-stream sentinel meaning "from the beginning").
    - `last_dropped_event_id` is the larger of `first_dropped_event_id` and `EventId(first.0 + count - 1)` — a best estimate, since the broadcast channel doesn't expose the post-lag cursor synchronously. Document in the function header that this is an upper-bound estimate, not a precise cursor; the presenter recovers via REST anyway and uses `last_delivered_event_id` (which it already tracked from prior Event frames it received) as the authoritative `?since=` cursor.
    - Construct via `DroppedFrame::new(...)` from Task 1; on the (impossible) `Err`, log at ERROR and skip emission. The error is unreachable by construction but the typed constructor enforces the invariant statically.
  - [ ] 4.4 Send the frame via `ServerMessage::Dropped(frame)` → `serde_json::to_string` → `socket.send(Message::Text(...))`. Return `false` on socket send failure to signal the caller to exit the connection task (same pattern as `dispatch_envelope`).

- [ ] **Task 5 — Contract tests in `crates/daemon/tests/contract_daemon.rs::story_2_4_dropped`** (AC: #1, #2, #3, #4)
  - [ ] 5.1 Create `mod story_2_4_dropped { ... }` AFTER `mod story_2_3_snapshot`. Reuse 2.1/2.2/2.3 helpers via `use super::story_2_1_ws::{...}; use super::story_2_2_publish::{...};`.
  - [ ] 5.2 Test `dropped_frame_after_1025_envelopes_with_blocked_reader` (AC #1) — spawn a test daemon with `ws_broadcast_capacity = 1024`, connect a client, subscribe to `events.*`, do NOT read further; publish 1025 envelopes via `publish_via_projection`; resume reading; assert FIRST received frame after subscribe is `dropped` with `count >= 1` (exact count depends on whether the client read the subscribe ack — `publish_via_projection` publishes both Event and State, so 1025 / 2 ≈ 513 events get a `Lagged` reading; assert `count >= 1` and `count <= 1025`, and the NEXT frame is a legitimate `event` or `state`. Document the exact expected count in a comment so future maintainers don't widen this assertion silently.
  - [ ] 5.3 Test `dropped_frame_keeps_socket_open` (AC #1, #4) — after the dropped frame, publish 3 more events, assert all 3 arrive in order as `event` frames; assert socket never closes.
  - [ ] 5.4 Test `dropped_frame_carries_count_in_envelopes` (AC #1) — assert `count` field is a positive integer (envelopes, not bytes); assert `first_dropped_event_id <= last_dropped_event_id`; do NOT assert exact `first/last` values (best-estimate semantics per Task 4.3 — document the looseness in the test comment).
  - [ ] 5.5 Test `dropped_frame_rest_refetch_recovers` (AC #2) — after dropped, the client does `GET /sessions/{id}/events?since=<last_delivered_event_id>`; assert response includes the dropped events (count > 0) AND the response's `oldest_available_event_id` confirms recoverability (`oldest_available_event_id <= last_delivered_event_id + 1`). Document that `last_delivered_event_id` is the cursor the client tracked from prior `event` frames (NOT taken from the `dropped` frame, whose ids are best-estimate).
  - [ ] 5.6 Test `sustained_lag_does_not_storm_dropped_frames` (AC #3) — config `ws_broadcast_capacity = 16`, `coalesce_window = Duration::from_millis(200)`, then in a tight loop for 30 seconds publish 100 envelopes per 100ms (300 publish cycles, 30,000 envelopes total) while the client never reads; resume reading; count the dropped frames received; assert `count <= 200` (margin over the theoretical `30s / 200ms = 150` ceiling) and definitely not >> 1000. Use `tokio::test(start_paused = true)` + `tokio::time::advance` to make this fast and deterministic — see Dev Notes "Deterministic test discipline" for the pattern.
  - [ ] 5.7 Test `lag_during_snapshot_emits_dropped_after_snapshot_completes` (Story 2.3 deferred-work line 79) — small `ws_broadcast_capacity = 4`, pre-populate `session_projections` with ~10 sessions, subscribe to `state.session.*` so the snapshot loop is busy on `socket.send`, concurrently publish > 4 envelopes via the writer pool, then resume reading; assert the snapshot frames arrive followed by a single `dropped` frame, and the socket stays open. Document the test as the natural extension of 2.3's deferred snapshot-lag scenario.
  - [ ] 5.8 Test `lag_in_drain_backlog_emits_dropped_through_same_helper` (AC #1, #3) — saturate `rx` with > capacity envelopes, then issue a `subscribe` (which triggers `drain_backlog_under_state`); assert the lag detected during drain produces a `dropped` frame via the same coalescing path — NOT silently discarded as today.
  - [ ] 5.9 Test `coalesce_window_resets_after_silence` (AC #3 lower bound, AC #4) — sustained lag → first dropped → silence for `coalesce_window + 100ms` → another lag burst → expect a SECOND dropped frame (not a third within the original window). Confirms the window is a sliding boundary, not a once-per-connection latch.

- [ ] **Task 6 — Update `docs/protocol-changelog.md`** (AC: #1)
  - [ ] 6.1 Add one `behavioral` entry under `v1.0 → v1.1`: "Lagged consumers receive `dropped` frame with `count`, `first_dropped_event_id`, `last_dropped_event_id`. Coalescing window default 1s; `count` is in envelopes (not bytes); `first/last` ids are best-estimate upper-bounds — presenters recover via REST `?since=last_delivered`."
  - [ ] 6.2 Add one `schema` entry: typed `DroppedFrame::new(...)` constructor with `count > 0` and `first <= last` invariants (asymmetric inbound/outbound policy preserved — `Deserialize` unchanged).

- [ ] **Task 7 — Update `docs/bmad/implementation-artifacts/sprint-status.yaml`** (story-completion bookkeeping)
  - [ ] 7.1 When implementation lands and code review passes, bump `2-4-lagged-consumer-recovery-with-dropped-frame` from `ready-for-dev` → `review`.
  - [ ] 7.2 Strike-through `deferred-work.md` lines 9 and 79 with backlinks to this story (same pattern as Story 2.3's resolution of line 8).

## Dev Notes

### What stories 2.1, 2.2, 2.3 already shipped (do NOT redo)

Story 2.4 is strictly additive over the existing WS surface:

- **Protocol crate (Story 2.1, 2.3):**
  - `ServerMessage::Dropped(DroppedFrame)` already exists at `crates/protocol/src/ws.rs:23`.
  - `DroppedFrame { count: u64, first_dropped_event_id: EventId, last_dropped_event_id: EventId }` already exists at `crates/protocol/src/ws.rs:108-113`.
  - `ServerMessage::Unknown` catch-all closes the additive-compat gap for any future variants.
  - `Error::InvalidSyncFrameOrdering { oldest, latest }` is the pattern to copy for `Error::InvalidDroppedFrame`.
  - `SyncFrame::new` typed constructor (Story 2.3) sets the precedent for `DroppedFrame::new`.

- **Daemon broadcast layer (Story 2.1, 2.2):**
  - `BroadcastHub::new(capacity)` floored at `MIN_CAPACITY = 2`. The default is `ws_broadcast_capacity = 1024` from `Config::with_bowerbird_dir`.
  - `BroadcastHub::subscribe()` returns `broadcast::Receiver<BroadcastEnvelope>`. `BroadcastHub::publish` swallows `SendError` on zero subscribers.
  - `BroadcastEnvelope::Event(Event)` and `BroadcastEnvelope::State { source, session_id, state }`. **Do NOT add a `BroadcastEnvelope::Dropped` variant** — the in-channel comment at `crates/daemon/src/broadcast/event.rs:30-31` predates Story 2.1's design and is stale. Lag is detected per-receiver in `rx.recv()`, so the dropped projection happens locally in the per-connection task, never on the hub.
  - `projection::session::write` publishes Event then State after every committed event (commit-gated, sentinel-excluded).

- **Daemon WS layer (Story 2.1, 2.3):**
  - `crates/daemon/src/api/ws.rs::connection_task` has the main `select!` loop with arms for shutdown, pong-deadline, `socket.recv()`, `rx.recv()`, and `ping_timer`. The `RecvError::Lagged(n)` branch at lines 262-266 currently logs WARN and continues — Story 2.4 replaces the body. The `RecvError::Closed` branch returns; do NOT change that.
  - `drain_backlog_under_state` at lines 459-482 has a parallel `TryRecvError::Lagged(n)` branch at lines 472-477. Same treatment.
  - `dispatch_envelope` at lines 489-522 projects `BroadcastEnvelope` to `ServerMessage::Event(EventFrame)` / `ServerMessage::State(StateFrame)`. Story 2.4 will read `event_id` off the `Event` variant to advance the per-connection cursor; do not change the dispatch logic itself.
  - `state.ws_config.ping_interval` / `pong_timeout` is the pattern for new config knobs: add to `Config`, add to `WsConfig`, thread through `main.rs`.

- **Test scaffolding (Story 2.1, 2.2, 2.3):**
  - `spawn_test_daemon(state)` constructs a server, returns `(SocketAddr, JoinHandle)`. Pass a pre-built `AppState`.
  - `connect_authed(addr, TEST_BEARER)` opens a WS with bearer auth.
  - `read_text_frame_or_close(&mut ws)` reads one frame, panics on close. Use this for normal reads.
  - `wait_subscribe_live_all(&mut [ws], &state, probe)` (Story 2.2) — handles the "did the subscribe land server-side yet" race using a unique per-attempt probe event. Reuse it. Note Story 2.3's caveat: snapshot frames for pre-populated sessions arrive before the probe — extend `wait_subscribe_live_all` if your test pre-populates the projection table.
  - `publish_via_projection(...)` (Story 2.2) — the canonical way to inject events into the broadcast hub through the production publish path. Use it; do NOT call `broadcaster.publish` directly from tests (it bypasses `projection::session::write`'s commit-gating).
  - `TEST_BEARER` constant.

### Why per-connection cursor tracking is necessary

`tokio::sync::broadcast::RecvError::Lagged(n)` tells the receiver "the channel advanced n positions past your cursor; you missed n values." It does NOT expose the post-lag broadcast position, and the broadcast channel offers no way to query "what is my current position" or "what is the next event id that would be delivered."

The DroppedFrame wire shape promises `first_dropped_event_id` and `last_dropped_event_id`. To produce those, the daemon must know the dropped range. Two possible sources:

1. **Read from the DB.** `SELECT MAX(event_id)` plus arithmetic. Adds DB load to a backpressure-stressed daemon; the very condition that caused the drop. Wrong shape.
2. **Track per-connection cursor.** The per-connection task already sees every successful `Event` dispatch through `dispatch_envelope`. Recording `last_delivered_event_id` is O(1) per Event with no cross-task coordination.

Option 2 is the standard pattern for any WebSocket pub/sub consumer (Kafka offsets, Postgres LSN, Kinesis sequence numbers — same shape). Story 2.4 adopts it.

### Cursor tracking only on Event dispatch (not State)

`BroadcastEnvelope::State` carries no `event_id` — see the variant definition at `crates/daemon/src/broadcast/event.rs:25-29`. The projection update that produces the State envelope is *triggered by* an event, but the wire frame doesn't expose the triggering event_id (deliberate decoupling — `StateFrame.state.last_event_at_ms` carries the timestamp, not the id).

Consequence: `last_delivered_event_id` only advances on Event dispatches. A pure-`state.*` subscriber that never receives an Event will have `last_delivered_event_id = None` at the time of its first lag. The `first_dropped_event_id` defaults to `EventId(0)` ("from the beginning") in that case; the presenter recovers by reading the REST `oldest_available_event_id` from any subsequent `GET /sessions/:id/events` response, then doing a full state-sync via `GET /sessions`. This is acceptable — a state-only subscriber that lags doesn't have precise event-history needs by definition.

### Lagged → Dropped projection — the exact shape

```rust
// crates/daemon/src/api/ws.rs::connection_task — replace the body of
// the `recv` arm's RecvError::Lagged branch (currently lines 262-266).

recv = rx.recv() => {
    match recv {
        Ok(env) => {
            if let BroadcastEnvelope::Event(ref ev) = env {
                last_delivered_event_id = Some(ev.event_id);
            }
            if !dispatch_envelope(&mut socket, &subscriptions, &env).await {
                return;
            }
        }
        Err(RecvError::Lagged(n)) => {
            if !emit_dropped_or_coalesce(
                &mut socket,
                &mut last_dropped_at,
                &mut pending_drop_count,
                last_delivered_event_id,
                n,
                state.ws_config.coalesce_window,
            ).await {
                return;
            }
        }
        Err(RecvError::Closed) => {
            tracing::debug!("ws: broadcast channel closed; exiting");
            return;
        }
    }
}
```

And the parallel update inside `drain_backlog_under_state`:

```rust
async fn drain_backlog_under_state(
    socket: &mut WebSocket,
    subscriptions: &HashSet<Topic>,
    rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
    last_delivered_event_id: &mut Option<EventId>,
    last_dropped_at: &mut Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    coalesce_window: Duration,
) -> bool {
    use tokio::sync::broadcast::error::TryRecvError;
    loop {
        match rx.try_recv() {
            Ok(env) => {
                if let BroadcastEnvelope::Event(ref ev) = env {
                    *last_delivered_event_id = Some(ev.event_id);
                }
                if !dispatch_envelope(socket, subscriptions, &env).await {
                    return false;
                }
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Lagged(n)) => {
                if !emit_dropped_or_coalesce(
                    socket,
                    last_dropped_at,
                    pending_drop_count,
                    *last_delivered_event_id,
                    n,
                    coalesce_window,
                ).await {
                    return false;
                }
            }
            Err(TryRecvError::Closed) => return true,
        }
    }
}
```

The helper signature carries enough state to be reusable from both call sites without sharing global mutable state. `connection_task` owns the lifetimes; the helper borrows them mutably for the duration of its async body.

### Coalescing helper — the exact shape

```rust
use protocol::{DroppedFrame, EventId, ServerMessage};

#[tracing::instrument(skip_all, fields(n, pending_drop_count = *pending_drop_count, has_cursor = last_delivered_event_id.is_some()))]
async fn emit_dropped_or_coalesce(
    socket: &mut WebSocket,
    last_dropped_at: &mut Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    last_delivered_event_id: Option<EventId>,
    n: u64,
    coalesce_window: Duration,
) -> bool {
    let now = tokio::time::Instant::now();
    let within_window = last_dropped_at
        .map(|t| now.duration_since(t) <= coalesce_window)
        .unwrap_or(false);

    if within_window {
        // Suppress; accumulate. The next emission folds this in.
        *pending_drop_count = pending_drop_count.saturating_add(n);
        tracing::debug!(coalesced_into_pending = *pending_drop_count, "ws: dropped frame coalesced");
        return true;
    }

    let count = pending_drop_count.saturating_add(n);
    let first = match last_delivered_event_id {
        Some(EventId(id)) => EventId(id.saturating_add(1)),
        None => EventId(0),
    };
    // Best-estimate upper-bound. The broadcast channel doesn't expose the
    // post-lag cursor; the presenter recovers via REST with its OWN
    // last_delivered_event_id (which is the authoritative cursor it tracked
    // from prior `event` frames). The values here are informational and
    // satisfy the DroppedFrame::new(count > 0, first <= last) invariant.
    let last = EventId(first.0.saturating_add(count.saturating_sub(1).min(i64::MAX as u64) as i64));

    let frame = match DroppedFrame::new(count, first, last) {
        Ok(f) => f,
        Err(e) => {
            // Statically unreachable by construction (count >= 1 from n >= 1,
            // and last >= first by the saturating_add above). Logged at ERROR
            // because if we reach here the contract assumptions changed.
            tracing::error!(error = %e, "ws: DroppedFrame::new rejected; skipping emission");
            return true;
        }
    };

    let json = match serde_json::to_string(&ServerMessage::Dropped(frame)) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = ?e, "ws: failed to serialize DroppedFrame; dropping");
            return true;
        }
    };
    if let Err(e) = socket.send(Message::Text(json.into())).await {
        tracing::debug!(error = ?e, "ws: send DroppedFrame failed; closing");
        return false;
    }

    *last_dropped_at = Some(now);
    *pending_drop_count = 0;
    tracing::warn!(dropped = count, "ws: dropped frame emitted");
    true
}
```

**Behaviour summary in three rules:**

1. First lag, or any lag after `coalesce_window` of silence → emit one `DroppedFrame` immediately, reset `pending_drop_count`, record `last_dropped_at = now`.
2. Lag within the window → silently accumulate into `pending_drop_count`. No wire emission.
3. Catch-up (no lag) → no action. Any `pending_drop_count` accumulated during a window of suppressed lag stays parked; it folds into the next lag burst's count, or is never emitted if the connection catches up fully. Documented behaviour: a fully-recovered presenter doesn't need a trailing recap.

### `DroppedFrame::new` — the exact shape

```rust
// crates/protocol/src/ws.rs (alongside the existing DroppedFrame struct)

impl DroppedFrame {
    /// Construct a `DroppedFrame` with field invariants enforced:
    /// - `count > 0` (vacuous Dropped is a bug; the daemon's coalescing
    ///   suppresses zero-count emissions before reaching this constructor).
    /// - `first_dropped_event_id <= last_dropped_event_id`.
    ///
    /// Returns `Err(Error::InvalidDroppedFrame { ... })` on violation.
    ///
    /// `Deserialize` does NOT call this — wire payloads (including ones
    /// from a hypothetical buggy peer) still parse without validation per
    /// the asymmetric inbound/outbound policy. The constructor is the
    /// daemon-side construction-time gate. Story 2.3's `SyncFrame::new`
    /// is the sibling pattern.
    pub fn new(
        count: u64,
        first_dropped_event_id: EventId,
        last_dropped_event_id: EventId,
    ) -> crate::error::Result<Self> {
        if count == 0 {
            return Err(crate::error::Error::InvalidDroppedFrame {
                count,
                first: first_dropped_event_id,
                last: last_dropped_event_id,
            });
        }
        if first_dropped_event_id > last_dropped_event_id {
            return Err(crate::error::Error::InvalidDroppedFrame {
                count,
                first: first_dropped_event_id,
                last: last_dropped_event_id,
            });
        }
        Ok(Self {
            count,
            first_dropped_event_id,
            last_dropped_event_id,
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
    #[error("invalid SyncFrame ordering: oldest={oldest:?} > latest={latest:?}")]
    InvalidSyncFrameOrdering {
        oldest: crate::event::EventId,
        latest: crate::event::EventId,
    },
    // Story 2.4 fold-in from deferred-work.md:9.
    #[error("invalid DroppedFrame: count={count} first={first:?} last={last:?}")]
    InvalidDroppedFrame {
        count: u64,
        first: crate::event::EventId,
        last: crate::event::EventId,
    },
}
```

### Files this story TOUCHES (UPDATE)

Verify line numbers in source before editing — stories 1.7 / 1.8 / 2.1 / 2.2 / 2.3 noted these drift:

| File | Change | Why |
|---|---|---|
| `crates/protocol/src/ws.rs` | Add `impl DroppedFrame { pub fn new(...) }` typed constructor alongside `DroppedFrame` struct (~line 113) | Task 1 |
| `crates/protocol/src/error.rs` | Add `Error::InvalidDroppedFrame { count, first, last }` variant alongside the existing `InvalidSyncFrameOrdering` | Task 1 |
| `crates/daemon/src/config.rs` | Add `ws_broadcast_coalesce_window: Duration` field to `Config`; default `Duration::from_secs(1)` | Task 2 |
| `crates/daemon/src/state.rs` | Add `coalesce_window: Duration` to `WsConfig` | Task 2 |
| `crates/daemon/src/main.rs` | Thread `ws_broadcast_coalesce_window` into `WsConfig` construction | Task 2 |
| `crates/daemon/src/api/ws.rs` | Add per-connection lag state (3 locals); replace `RecvError::Lagged` arm body (lines 262-266) and `TryRecvError::Lagged` arm body (lines 472-477); update `dispatch_envelope` callers to advance `last_delivered_event_id`; widen `drain_backlog_under_state` signature; add `emit_dropped_or_coalesce` helper | Tasks 3, 4 |
| `crates/daemon/tests/contract_daemon.rs` | Add `mod story_2_4_dropped { ... }` after `mod story_2_3_snapshot` with 8 contract tests | Task 5 |
| `docs/protocol-changelog.md` | One `behavioral` entry + one `schema` entry under `v1.0 → v1.1` | Task 6 |
| `docs/bmad/implementation-artifacts/deferred-work.md` | Strike-through lines 9 (DroppedFrame invariants) and 79 (lag-during-snapshot) with backlinks to this story | Tasks 1.4, 7.2 |
| `docs/bmad/implementation-artifacts/sprint-status.yaml` | Bump `2-4-...` from `ready-for-dev` → `review` when implementation lands | Task 7.1 |

### Files this story CREATES (NEW)

None. All work is in existing files. This is a wiring + protocol-hardening story, not a structural one.

### Existing files the dev MUST read before editing (context, no changes)

| File | What to learn from it |
|---|---|
| `crates/protocol/src/ws.rs` (entire file, esp. `DroppedFrame` at ~line 108 and `SyncFrame::new` at ~line 73) | The wire shape and the sibling typed-constructor pattern to copy. `SyncFrame::new` is the template — same `Result<Self, Error>` return, same `Deserialize` asymmetry, same `#[non_exhaustive]` consideration (apply to `DroppedFrame` too if it isn't already; Story 2.3 added it to `SyncFrame`). |
| `crates/protocol/src/error.rs` | The error enum lives here; copy the `InvalidSyncFrameOrdering` variant pattern. |
| `crates/protocol/src/event.rs` (just `EventId(i64)` definition) | `EventId` is `i64`-backed and `Ord`. `EventId(0)` is the "empty stream" sentinel used in `oldest_available_event_id`. |
| `crates/daemon/src/api/ws.rs` (entire file) | The connection task you're editing. Pay particular attention to: lines 199-205 (the awaiting_pong / pong_sleep pattern — copy this state-on-locals approach for `last_dropped_at`); lines 262-266 (the arm body you replace); lines 459-482 (`drain_backlog_under_state` — the parallel lag surface); lines 489-522 (`dispatch_envelope` — where `last_delivered_event_id` advances). |
| `crates/daemon/src/broadcast/event.rs` | `BroadcastEnvelope::Event(Event)` carries `event_id`; `BroadcastEnvelope::State { source, session_id, state }` does not. The "stale comment at line 30-31 about a `Dropped` envelope variant" — **do NOT act on that comment**; the design changed. Lag is per-receiver. |
| `crates/daemon/src/broadcast/hub.rs` | `MIN_CAPACITY = 2` floor. Tests with `BroadcastHub::new(16)` and similar are valid. |
| `crates/daemon/src/config.rs` and `crates/daemon/src/state.rs` | The pattern for adding a config knob: `Config` field → `Config::with_bowerbird_dir` default → `WsConfig` field → `main.rs` wiring. Three files, no surprises. |
| `crates/daemon/src/api/events.rs` (entire file) | The REST endpoint AC #2 depends on. `EventListResponse.oldest_available_event_id` is already returned. `?since=N` is exclusive (`event_id > N`), so the AC #2 cursor is `last_delivered_event_id` — NOT `first_dropped_event_id`. The Dropped frame's `first/last` ids are informational; the presenter's `last_delivered_event_id` is the authoritative recovery cursor. |
| `crates/daemon/tests/contract_daemon.rs::story_2_2_publish` (lines 3041–3940) | Test helpers — `wait_subscribe_live_all`, `publish_via_projection`, the probe-token discipline. Reuse them. The Story 2.2 commentary on probe-token uniqueness applies unchanged. |
| `crates/daemon/tests/contract_daemon.rs::story_2_3_snapshot` (lines 3945–end) | The most recent test module. The pattern for `mod story_2_4_dropped` sits next to it. Snapshot-handling test patterns transfer directly to the lag-during-snapshot test (Task 5.7). |
| `docs/bmad/implementation-artifacts/2-3-new-session-discovery-and-state-snapshot-on-connect.md` | Story 2.3 Dev Notes, especially the "Subscribe-arm ordering" section. The lag-during-snapshot scenario (Task 5.7) is rooted in this story's design — the snapshot loop holds the connection task off `rx.recv()`, accumulating broadcast backlog that may overflow capacity. |

### Anti-patterns (explicitly forbidden)

- **Adding a `BroadcastEnvelope::Dropped` variant.** Lag is per-receiver, not per-hub. The stale comment at `crates/daemon/src/broadcast/event.rs:30-31` predates the design. The Dropped projection happens in the per-connection task, never on the broadcast channel. Adding the variant would couple every subscriber to every other subscriber's lag.
- **Reading from the DB on `Lagged`.** A backpressure-stressed daemon should not add DB load to recover from backpressure. Per-connection cursor tracking is O(1) per Event dispatch and has zero cross-task cost.
- **Closing the socket on lag.** AC #1 requires the socket stays open. The presenter handles the gap; the daemon does not unilaterally disconnect. (Future hardening — disconnect on "lag persistent for N minutes" — is out of scope; this story coalesces, doesn't escalate to disconnect.)
- **Emitting `Sync` frames from this story.** `SyncFrame::new` exists (Story 2.3 fold-in) but no daemon code produces it yet. The architecture lists a Sync producer as future work; do NOT bolt it onto Story 2.4's Dropped path. Story 2.4's recovery contract is fully served by Dropped + existing REST.
- **Emitting Dropped when `n == 0`.** `RecvError::Lagged(n)` always has `n >= 1` (it's only constructed when the cursor advanced past `n` values), but `DroppedFrame::new` rejects `count == 0` defensively. The coalescing path also never emits with `pending_drop_count + n == 0`. Bug if you reach it.
- **Reusing `tracing::warn!` for the new emission.** Use `tracing::warn!` on EMISSION (good signal — a presenter actually got a Dropped) and `tracing::debug!` on COALESCING SUPPRESSION (high-cardinality noise in tight lag loops). The level distinction is what makes the lag-storm test actually useful as a CI diagnostic when it regresses.
- **Polling for "is the broadcast caught up".** There is no such API on `tokio::sync::broadcast`. The per-connection state machine handles catch-up implicitly: when normal `Ok(env)` returns from `rx.recv()`, you're caught up; that's the only signal. Do not invent a side channel.
- **Sleeping or yielding inside the lag arm.** The lag arm is in the per-connection `select!` loop. Any await that blocks the loop blocks future `rx.recv()`, ping handling, and shutdown observation. The helper's `socket.send(...).await` is acceptable because that's the same await every other arm uses; do not add additional awaits inside the lag-handling path.
- **Using `tokio::time::sleep` for the coalescing window.** The window is implemented as a passive `Instant`-comparison check on each call to the helper. No active timer, no scheduled wake-up. The next `Lagged` arrival is what triggers the window-expired check; this is intentional and avoids interfering with the `select!` loop's other arms.
- **Treating `dispatch_envelope` as the *only* cursor-advance site.** The same advance must happen in `drain_backlog_under_state`'s `Ok(env)` arm. Both paths dispatch Event envelopes; both must update the cursor. The cleanest pattern is to factor the cursor-advance into a tiny helper or do it inline before each `dispatch_envelope` call — pick whichever the dev finds cleaner. Test 5.8 covers the drain-arm path.
- **Logging the event payload, the envelope, or the Dropped frame's full JSON.** Same `#[tracing::instrument(skip_all)]` discipline as Story 2.2 / 2.3. The helper's instrument attributes (`fields(n, pending_drop_count, has_cursor)`) are sufficient and do not leak event content.

### Library/version pins (verified against `Cargo.toml`)

No new dependencies. Story 2.4 uses crates already in the workspace:

| Crate | Version | Use |
|---|---|---|
| `tokio` | `1.52.1` (sync + time features already on) | `tokio::sync::broadcast::error::{RecvError, TryRecvError}` (existing), `tokio::time::Instant` (existing), `tokio::time::Duration` (existing) |
| `protocol` (workspace path) | local | `DroppedFrame`, `EventId`, `ServerMessage`, `Error` |
| `serde_json` | `1.0.149` | serialize `ServerMessage::Dropped(frame)` to wire JSON |
| `thiserror` | already pinned | the new `InvalidDroppedFrame` variant |
| `tracing` | `0.1.44` | `instrument`, `warn!`, `debug!`, `error!` |
| `axum` | `0.8.x` | `Message::Text`, `WebSocket::send` (existing) |

No new dev-deps either.

### Project-context references for invariants this story must hold

- **Substrate observes, doesn't interpret (Axiom 1, `project-context.md:44`).** `DroppedFrame` carries mechanical facts: `count`, `first/last` (best-estimate). No `severity`, no `recovery_strategy`, no `should_reconnect` flag. The presenter decides what to do with the dropped frame; the daemon doesn't suggest. ([Reaction enum follows demand, not anticipation](project-context.md:699) is the canonical statement of this principle — applies equally to dropped-frame fields.)
- **Mechanical facts in the protocol; semantics in the presenter (Axiom 4, `project-context.md:57`).** AC #2's "gap recoverable when `oldest_available_event_id <= last_delivered_event_id + 1`" is a derivation the **presenter** does, not the daemon. The daemon emits `oldest_available_event_id` (already shipped in Story 1.7's `EventListResponse`); the presenter compares. No `gap_recoverable: bool` field anywhere.
- **Performance is hard at trust boundaries (Axiom 3, `project-context.md:52`).** The WS surface is on the daemon side of the trust boundary; perf is *soft* here. A 1s coalescing window is generous, not penny-pinching. We will not chase a 100ms window to reduce dropped-frame latency at the cost of frame storms.
- **`(source, session_id)` natural key (`project-context.md:695`).** `DroppedFrame` does NOT carry source/session_id — it's a per-connection signal, not a per-session one. A lagged connection lagged across all its subscriptions. Do not add session_id to the wire shape "for symmetry" with State frames; the wire shape is already fixed by the protocol crate.
- **Backpressure escalation contract test (`project-context.md:596`).** This is literally Task 5.6. The test asserts the policy, not just the mechanism.
- **Outbound envelope additive-compat (`project-context.md:594`).** The asymmetric `deny_unknown_fields` policy means the typed `DroppedFrame::new` constructor is daemon-side only. `Deserialize` continues to accept anything that matches the field shape; a future v1.x can add fields without breaking older presenters. Story 2.4 ships zero new wire fields — `count`, `first_dropped_event_id`, `last_dropped_event_id` were already in the protocol crate.
- **`unsafe_code = "forbid"` (`Cargo.toml:5-6`).** No `unsafe` blocks. None needed.
- **`#[tracing::instrument(skip_all)]` discipline (`project-context.md:664`).** Apply to `emit_dropped_or_coalesce` with explicit `fields(n, pending_drop_count, has_cursor)`. No `?envelope`, no `?frame`, no payload exposure.
- **State emission and event INSERT atomicity (`architecture.md:589`).** Story 2.4 does NOT touch the projection path. The publish remains commit-gated as established in Story 2.2. Lag detection happens downstream of publish; a lagged consumer never causes a rollback.
- **Single-threaded runtime — `select!` discipline (`project-context.md:97`).** The connection task is one of many tasks on the `current_thread` runtime. Any await inside the lag arm blocks the entire runtime for that duration. `socket.send` is unavoidable; everything else in the helper is sync. Do not add awaits.

### Latency consideration (NFR — no perf gate in this story)

`architecture.md:272` sets the hook→presenter target at p99 ≤100ms. Story 2.4's added work is in the lag-recovery path, which is OFF the steady-state hook→presenter flow. Steady-state cost: zero — `last_delivered_event_id` is updated on a single integer move per Event dispatch, no allocation.

Lag-recovery cost (one-shot per lag burst, per connection): one `Instant::now()`, one duration comparison, one `DroppedFrame::new` call, one `serde_json::to_string` of a fixed-size struct (~80 bytes), one `socket.send`. All under a millisecond on loopback. The lag-storm coalescing means a sustained-lag scenario emits ≤ 1 frame per `coalesce_window`, not per `Lagged()` recv, so the cost is bounded.

No Criterion bench added in this story. Same posture as Story 2.2 / 2.3. The hook→presenter p99 benchmark deferred at `deferred-work.md:70` will cover this path once it lands.

### Deterministic test discipline

`project-context.md:642-645` mandates: no real `sleep()` in tests. Use `tokio::test(start_paused = true)` + `tokio::time::advance` for time-dependent assertions.

Task 5.6 ("sustained lag does not storm dropped frames") is the test most exposed to flakiness. The design:

1. `#[tokio::test(start_paused = true)]` — virtual clock from the start.
2. Build an `AppState` with `ws_broadcast_capacity = 16`, `ws_config.coalesce_window = Duration::from_millis(200)`.
3. In a loop: `publish_via_projection(...)` × 100 envelopes; `tokio::time::advance(Duration::from_millis(100)).await;` 300 cycles total = 30 virtual seconds.
4. Resume reading; count `dropped` frames. Assert `<= 200` (≈ 30s / 200ms = 150 plus margin) and not `>> 1000`.

The virtual clock keeps the test under 1 real second of wall time. If the runtime ever sees a real `sleep()` for the coalescing window (which it does not — the helper is passive), the test would diverge from prod behaviour. The Instant comparison uses `tokio::time::Instant`, which respects `tokio::time::pause()`/`advance`.

Task 5.9 ("coalesce_window resets after silence") uses the same pattern: advance the virtual clock past `coalesce_window + 100ms` after the first dropped, then trigger another lag burst.

### "Standards-by-default" (retro Agreement A1) check

Story 2.4 introduces no bespoke surface. Every primitive is already in the workspace:

- Lag detection: `tokio::sync::broadcast::error::RecvError::Lagged` (existing; Story 2.1 already handles this enum at WARN).
- Wire shape: `protocol::DroppedFrame` (existing; Story 2.1 / 2.3 shipped the struct).
- Typed constructor pattern: `SyncFrame::new` (Story 2.3) is the template.
- Per-connection state on locals in `select!` loop: the `awaiting_pong` / `pong_sleep` pattern from Story 2.1 is the template.
- Coalescing via passive Instant comparison: no new dependency, no new abstraction. Standard `tokio::time::Instant`.

The only new mechanism is the helper function that ties these primitives together — twenty-odd lines of straightforward async Rust.

### Tests to update (existing, may break with new behavior)

The `RecvError::Lagged` arm is currently silent (logs only). After Story 2.4, it emits a wire frame. Audit existing tests for lag scenarios:

- **`story_2_2_publish::three_subscribers_receive_identical_events_in_order`** and siblings — these publish modest counts (< broadcast capacity) and do not provoke `Lagged`. Unaffected.
- **`story_2_2_publish::*`** any test that intentionally provokes lag was not written (Story 2.2 deferred lag tests to 2.4). Confirm by grepping for `Lagged` in the existing test module — should be zero matches outside of comments.
- **`story_2_3_snapshot::*`** — none of these tests provoke lag intentionally (the deferred-work entry at line 79 explicitly carries the lag-during-snapshot test to this story). Unaffected.
- **`story_2_1_ws::ws_pre_subscribe_backlog_does_not_leak_to_new_subscription`** — uses `drain_backlog_under_state` indirectly. Existing assertion is about which envelopes survive the drain. Does not provoke lag (small envelope count). Unaffected.

Run `cargo test --workspace` after Task 3 lands. Any test that now sees a `dropped` frame before its expected `event` / `state` frame needs to either drain the dropped frame and assert it's well-formed, or reduce the publish count to stay under capacity. Prefer the latter — the tests that were not designed to test lag should not encounter it.

The `drain_backlog_under_state` signature widens (Task 3.4). All callers are in `crates/daemon/src/api/ws.rs` — a search for `drain_backlog_under_state(` shows exactly two call sites in the Subscribe and Unsubscribe arms. Both need the new params threaded through. The compile error is the source of truth.

## Previous Story Intelligence (from Story 2.3)

Story 2.3 was the snapshot-on-subscribe story. It shipped four code paths Story 2.4 builds on:

1. **`SyncFrame::new` typed constructor** at `crates/protocol/src/ws.rs:73-88`. Story 2.4 copies this pattern exactly for `DroppedFrame::new`. The error variant (`InvalidSyncFrameOrdering`) is the sibling for `InvalidDroppedFrame`. The asymmetric inbound/outbound policy (Deserialize untouched) is the same.

2. **Subscribe-arm ordering** with `[A] drain backlog → [B] read now_ms → [C] read projection → [D] insert topic → [E] emit snapshot → [F] main loop resumes`. Story 2.4 does NOT change this ordering. The lag-during-snapshot test (Task 5.7) provokes lag during step [E] (when the per-connection task is busy on `socket.send`), and the test asserts the dropped frame arrives in step [F] after the snapshot completes — which is the natural ordering.

3. **`drain_backlog_under_state` and its `TryRecvError::Lagged` arm** at lines 472-477. Story 2.3 preserved Story 2.1's WARN-log behaviour here. Story 2.4 turns it into a coalesced emission — through the SAME helper used in the main `recv = rx.recv()` arm. Both surfaces share state.

4. **`current_state_for_read` and the stale-Working read-time fallback** at `crates/daemon/src/projection/state.rs:65-77`. Story 2.4 does NOT touch this. The snapshot path (Story 2.3) is the only consumer.

Story 2.3's review feedback was two rounds, primarily about the lag-during-snapshot scenario (resolved as deferred-work line 79, which this story closes). The pattern: when a story's design naturally exposes a follow-up issue, the issue is captured as deferred-work with a backlink to the next story that owns it. Story 2.4 closes two deferred-work entries (line 9 and line 79) the same way Story 2.3 closed Story 2.1's SyncFrame entry.

The lesson from Story 2.3's deferred-work entry at line 79 carries directly: **the snapshot loop holds the connection task off `rx.recv()` for the duration of N `socket.send` awaits**. A small `ws_broadcast_capacity` plus concurrent publishes can overflow during this window. Task 5.7 codifies this scenario as a contract test.

The "Standards-by-default" retro Agreement A1 was honored in Story 2.3 (no bespoke surface, every primitive already in workspace). Story 2.4 continues the discipline — see the "Standards-by-default check" section above.

## Git Intelligence Summary (last 5 commits)

```
23adef3 docs(story-2.3): mark story done
1e9f87b fix(story-2.3): address second-round code-review findings
e9f1832 docs(story-2.3): incorporate second-round code-review findings
b5cf29c fix(story-2.3): address code-review findings
ee042c9 docs(story-2.3): incorporate code-review findings
b128603 feat(story-2.3): new session discovery and state snapshot on connect
cfd7ecf create-story 2.3
```

Story 2.3 landed cleanly after two review rounds. The commit convention `feat(story-X.Y): <subject>` is stable; story commits land on a `story-X.Y` branch and merge into `main`. Story 2.4 should:

- Branch from `main` as `story-2.4`.
- One `feat(story-2.4): lagged consumer recovery with dropped frame` commit (or split into protocol-changes, config-changes, ws-changes, contract-tests — dev's choice).
- One or more `docs(story-2.4): incorporate code-review findings` / `fix(story-2.4): address code-review findings` rounds (Story 2.3 had two; Story 2.4 is similarly scoped — expect one or two).
- A `docs(story-2.4): mark story done` after review passes.
- Merge as `Merge pull request #N from technicalpickles/story-2.4`.

Tree state at story start: workspace tests passing, Story 2.3 merged, Story 2.4 backlog item promoted to ready-for-dev by this file. Existing test count is on the order of 200+ across the workspace.

Story 2.3's PR is the most recent prior art for review discipline. The "second-round findings" pattern suggests the reviewer found follow-ups after the first patch. Anticipate the same: write the implementation defensively, run `cargo test --workspace` and `cargo clippy --all-targets --workspace -- -D warnings` locally, but expect review to surface improvements you didn't see.

## Latest Tech Information

No new dependencies. Key existing crate behaviours Story 2.4 depends on (versions verified against the workspace `Cargo.toml`):

- **`tokio` 1.52.1** — `broadcast::error::RecvError::Lagged(u64)` is stable since 1.0; `broadcast::error::TryRecvError::Lagged(u64)` likewise. `tokio::time::Instant` respects `pause()`/`advance` in tests. `tokio::time::Duration` is a re-export of `std::time::Duration`.
- **`axum` 0.8.x** — `axum::extract::ws::Message::Text(Utf8Bytes)` is the wire shape; `WebSocket::send(Message)` returns `Result<(), axum::Error>`. Existing usage in `ws.rs` is the template.
- **`serde_json` 1.0.149** — `to_string(&ServerMessage)` is the existing serialize path. Tag is `"op"`, rename_all snake_case (`"dropped"` is the variant tag on the wire).
- **`tracing` 0.1.44** — `#[instrument(skip_all, fields(...))]` is the discipline. Level macros (`warn!`, `debug!`, `error!`) are field-list style only.

No latest-version research needed; the relevant APIs are stable across all 1.x tokio releases and don't need upgrading.

## Project Context Reference

This story leans on the following project-context invariants (full quotes available in `docs/bmad/project-context.md`):

- **Axiom 1 (substrate observes, doesn't interpret)** — DroppedFrame carries facts (count, ids), no semantics (no severity, no should_reconnect).
- **Axiom 3 (perf hard at trust boundaries, soft inside)** — coalesce window default 1s is generous; no need to chase sub-100ms emission latency.
- **Axiom 4 (mechanical facts in protocol, semantics in presenter)** — gap-recoverability inference happens in the presenter, not via a `gap_recoverable: bool` field.
- **(source, session_id) natural key** — DroppedFrame is per-connection, not per-session. No session_id field.
- **Outbound envelope additive-compat** — typed constructor is daemon-side; Deserialize untouched.
- **Sentinel exclusion** — daemon sentinel events (`source = '__daemon__'`) are filtered from broadcasts by Story 2.2; lag on a sentinel-only subscription is theoretically impossible because sentinels aren't published. Test does not need to cover this.
- **Single-threaded runtime, `select!` discipline** — no extra awaits in the lag arm.
- **No unsafe_code, no unwrap outside tests, `#[tracing::instrument(skip_all)]`** — uniform discipline from Story 1.x onward.
- **Backpressure escalation contract test** — `project-context.md:596` lists this verbatim; Task 5.6 is the implementation.

Architecture references:

- `architecture.md:42-44` — DroppedFrame on lag with `first_dropped_event_id` + `last_dropped_event_id` + count.
- `architecture.md:157-159` — "precision cursors; presenter passes `first_dropped_event_id` as `since` to REST re-fetch." Story 2.4's best-estimate semantics document the deviation from this ideal; the test (Task 5.5) uses `last_delivered_event_id` instead, which is the cursor the presenter authoritatively tracked.
- `architecture.md:172-178` — the eight-step reconnect flow. Story 2.4 implements step 7 ("DroppedFrame handling — REST re-fetch from `first_dropped_event_id`") with the caveat above.
- `architecture.md:464` — "slow consumer receives `DroppedFrame`; channel never blocks." Confirms the design: `tokio::sync::broadcast::Sender::send` never blocks; lag is detected receiver-side.

Epic 1 / Story 1.7 reference:

- `crates/daemon/src/api/events.rs::list` is the REST endpoint AC #2 depends on. It already returns `oldest_available_event_id` per the protocol; no daemon change needed for AC #2.

## Story Completion Status

Status at story authoring: **ready-for-dev** — comprehensive developer guide created.

This story file is the dev agent's complete reference for Story 2.4. The dev agent has:

- Acceptance criteria from `epics.md:566-588` translated into testable BDD form.
- Exact code shape for the `Lagged` arm replacement, the coalescing helper, the typed `DroppedFrame::new` constructor, and the per-connection state.
- The list of files to touch (10) and files to create (0).
- The list of files to READ before editing (10).
- Eight contract tests with explicit scope per AC.
- Anti-patterns (12 items) — the failure modes review would catch otherwise.
- Project-context references for every invariant the implementation must preserve.
- Two deferred-work entries this story closes (line 9 — DroppedFrame invariants; line 79 — lag-during-snapshot).
- The commit convention, branch shape, and review-round expectation set by Stories 2.1 / 2.2 / 2.3.

Expected scope: medium. The per-connection state machine is the genuinely-new piece; the protocol-crate typed constructor follows a proven template (Story 2.3's `SyncFrame::new`). Eight contract tests is on the higher end for an Epic 2 story but matches the test-density Story 2.2 / 2.3 set.

## Dev Agent Record

### Agent Model Used

_To be filled in by the dev agent at implementation time._

### Debug Log References

_To be filled in by the dev agent at implementation time._

### Completion Notes List

_To be filled in by the dev agent at implementation time._

### File List

_To be filled in by the dev agent at implementation time._
