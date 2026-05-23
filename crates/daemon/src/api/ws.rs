//! `GET /ws` — WebSocket upgrade and per-connection task.
//!
//! Auth resolution: bearer token from `Authorization: Bearer ...` header
//! (preferred) or `?token=...` query param (fallback for clients that cannot
//! set headers — e.g. browser `new WebSocket()`). If both are present, the
//! header wins. Verification goes through [`crate::api::token::BearerToken`]
//! exactly like [`crate::api::auth::require_bearer`] — same 401 body, same
//! `tracing::instrument(skip_all)` discipline, no token byte ever logged.
//!
//! After auth, the handler acquires a per-connection permit from
//! `state.ws_semaphore` (the WS concurrency cap from Story 2.1 AC #6).
//! `try_acquire_owned` is intentionally non-blocking: the 257th client must
//! be *rejected* synchronously, not queued.
//!
//! The Hello frame is constructed BEFORE the upgrade response so a
//! same-startup HTTP `/status` snapshot and this WS Hello see consistent
//! values. Subscription to the broadcast hub happens pre-upgrade too — that
//! discipline matters for Story 2.2, when `projection::session::write` starts
//! publishing into the hub.
//!
//! See `crates/protocol/src/ws.rs` for the wire shapes. `ClientMessage` is
//! strict (`deny_unknown_fields`): any malformed inbound frame routes to
//! [`close_with_bad_message`] which closes with WS code 1008 (Policy
//! Violation) and a `bad message: <detail>` reason.

use std::time::Duration;

use axum::extract::ws::{
    CloseFrame as WsCloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade,
};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::OwnedSemaphorePermit;

use protocol::{
    ClientMessage, DroppedFrame, EventFrame, EventId, HelloFrame, ServerMessage, StateFrame,
};

use crate::broadcast::{BroadcastEnvelope, Topic};
use crate::db::queries::SELECT_HELLO_DB_FIELDS;
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "1.0";

/// WS close reasons are bounded by RFC 6455 §5.5.1 at 123 bytes. Includes
/// the `"bad message: "` prefix.
const WS_CLOSE_REASON_LIMIT: usize = 123;

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    token: Option<String>,
}

/// `GET /ws` handler. Authenticates the bearer before the upgrade; on auth
/// failure returns `401` without upgrading. If the connection semaphore is
/// exhausted, returns `503` without upgrading.
#[tracing::instrument(skip_all)]
pub async fn handle_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
) -> Response {
    // Header-vs-query precedence: when an `Authorization` header is present
    // AT ALL — even malformed (`Basic ...`, empty bearer, non-UTF-8) — it
    // wins and the query token is NOT consulted. Otherwise an attacker who
    // supplies `Authorization: Basic garbage` plus a known-good `?token=...`
    // would authenticate via the fallback, violating AC #5.
    let header_present = headers.contains_key(header::AUTHORIZATION);
    let header_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(str::trim))
        .filter(|s| !s.is_empty());

    let candidate = if header_present {
        header_token
    } else {
        query.token.as_deref().filter(|s| !s.is_empty())
    };

    let authorized = match candidate {
        Some(c) => state.bearer.verify(c),
        None => false,
    };
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }

    let permit = match state.ws_semaphore.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!("ws connection cap reached; rejecting upgrade");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "too many ws clients" })),
            )
                .into_response();
        }
    };

    let (oldest, history_clean) = compute_hello_db_fields(&state).await;
    let hello = HelloFrame {
        protocol_version: PROTOCOL_VERSION.to_string(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        oldest_available_event_id: oldest,
        daemon_started_at: state.started_at_ms,
        history_begins_cleanly: history_clean,
    };

    // Subscribe BEFORE the upgrade completes so events committed between
    // subscribe-time and Hello-send cannot be lost. Story 2.2 wires the
    // first publisher; for 2.1 the subscription is silent but the
    // discipline is set here.
    let rx = state.broadcaster.subscribe();

    ws.on_upgrade(move |socket| connection_task(socket, state, rx, hello, permit))
}

/// Probe the reader pool for the two DB-derived Hello fields. Returns
/// conservative defaults `(EventId(i64::MAX), false)` on any pool/DB error
/// rather than refusing the upgrade — the alternative (drop the connection
/// for a transient DB issue) would be worse for presenters.
async fn compute_hello_db_fields(state: &AppState) -> (EventId, bool) {
    let conn = match state.db.reader.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "ws hello: reader pool checkout failed; using conservative defaults");
            return (EventId(i64::MAX), false);
        }
    };
    // One SELECT statement so the two values are a consistent snapshot
    // (single SQLite query plan = single read txn from the planner's POV;
    // a concurrent commit between the two reads cannot interleave).
    let interact = conn
        .interact(|c| -> rusqlite::Result<(Option<i64>, bool)> {
            c.query_row(SELECT_HELLO_DB_FIELDS, [], |r| {
                let min: Option<i64> = r.get(0)?;
                let clean: i64 = r.get(1)?;
                Ok((min, clean != 0))
            })
        })
        .await;
    match interact {
        Ok(Ok((min, clean))) => (EventId(min.unwrap_or(i64::MAX)), clean),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "ws hello: DB probe failed; using conservative defaults");
            (EventId(i64::MAX), false)
        }
        Err(e) => {
            tracing::warn!(error = %e, "ws hello: interact failed; using conservative defaults");
            (EventId(i64::MAX), false)
        }
    }
}

async fn connection_task(
    mut socket: WebSocket,
    state: AppState,
    mut rx: tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
    hello: HelloFrame,
    _permit: OwnedSemaphorePermit,
) {
    // Hello goes out as the FIRST text frame, ahead of any other server
    // message.
    let hello_json = match serde_json::to_string(&ServerMessage::Hello(hello)) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = ?e, "ws: failed to serialize Hello frame");
            return;
        }
    };
    if let Err(e) = socket.send(Message::Text(hello_json.into())).await {
        tracing::debug!(error = ?e, "ws: failed to send Hello frame; closing");
        return;
    }

    let mut subscriptions: HashSet<Topic> = HashSet::new();
    let ping_interval = state.ws_config.ping_interval;
    let pong_timeout = state.ws_config.pong_timeout;
    let mut ping_timer =
        tokio::time::interval_at(tokio::time::Instant::now() + ping_interval, ping_interval);
    ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Pong-deadline state. When `awaiting_pong` is true, `pong_sleep` is
    // armed to fire `pong_timeout` after the Ping was sent. When false, the
    // sleep is parked far in the future and the `if awaiting_pong` guard
    // ensures the branch does not match the select. This gives us
    // deadline-granularity (close exactly `pong_timeout` after the Ping)
    // instead of only checking on the next ping tick, addressing the
    // review finding for AC #8.
    let mut awaiting_pong = false;
    let pong_park: Duration = Duration::from_secs(86_400);
    let pong_sleep = tokio::time::sleep(pong_park);
    tokio::pin!(pong_sleep);

    // Story 2.4 — per-connection lag-recovery state.
    //
    //   `last_delivered_event_id`: the latest `EventId` this socket
    //   actually dispatched via `dispatch_envelope` for an Event envelope.
    //   State envelopes do NOT advance the cursor (they carry no
    //   `event_id`; see `BroadcastEnvelope::State`).
    //
    //   `last_dropped_at`: wall-clock (tokio virtual clock in tests) of
    //   the most recent `DroppedFrame` emission. `None` until the first
    //   lag burst.
    //
    //   `pending_drop_count`: lag-burst envelopes that were SUPPRESSED
    //   inside an active coalescing window. Folded into the count of the
    //   next emission, or never emitted at all if the consumer catches
    //   up before the window expires (intentional — a fully-recovered
    //   presenter doesn't need a trailing recap).
    let mut last_delivered_event_id: Option<EventId> = None;
    let mut last_dropped_at: Option<tokio::time::Instant> = None;
    let mut pending_drop_count: u64 = 0;

    loop {
        tokio::select! {
            biased;

            _ = state.shutdown.cancelled() => {
                tracing::debug!("ws: shutdown signaled; exiting connection task");
                return;
            }

            _ = &mut pong_sleep, if awaiting_pong => {
                tracing::debug!("ws: pong timeout exceeded; closing connection");
                return;
            }

            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_text_frame(
                            &mut socket,
                            &mut subscriptions,
                            &mut rx,
                            &state,
                            text.as_str(),
                            &mut last_delivered_event_id,
                            &mut last_dropped_at,
                            &mut pending_drop_count,
                        )
                        .await
                        {
                            return;
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        close_with_bad_message(&mut socket, "binary frames are not accepted").await;
                        return;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // Pong cleared; park the sleep far in the future so
                        // the deadline branch will not fire until the next
                        // Ping arms it again.
                        awaiting_pong = false;
                        let park_deadline = tokio::time::Instant::now() + pong_park;
                        pong_sleep.as_mut().reset(park_deadline);
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // axum/tokio-tungstenite responds automatically; nothing to do.
                    }
                    Some(Ok(Message::Close(_))) => {
                        tracing::debug!("ws: client closed connection");
                        return;
                    }
                    Some(Err(e)) => {
                        tracing::debug!(error = ?e, "ws: recv error; closing");
                        return;
                    }
                    None => {
                        tracing::debug!("ws: socket ended");
                        return;
                    }
                }
            }

            recv = rx.recv() => {
                match recv {
                    Ok(env) => {
                        // Cursor advances ONLY after a matching Event was
                        // actually written to the wire — see DispatchOutcome
                        // doc for why this gate matters for state-only and
                        // narrow-topic subscribers.
                        match dispatch_envelope(&mut socket, &subscriptions, &env).await {
                            DispatchOutcome::Sent => {
                                if let BroadcastEnvelope::Event(ref ev) = env {
                                    last_delivered_event_id = Some(ev.event_id);
                                }
                            }
                            DispatchOutcome::Filtered => {}
                            DispatchOutcome::Closed => return,
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Story 2.4: project lag into a `DroppedFrame` via
                        // the coalescing helper. Socket stays open per AC #1.
                        if !emit_dropped_or_coalesce(
                            &mut socket,
                            &mut last_dropped_at,
                            &mut pending_drop_count,
                            last_delivered_event_id,
                            n,
                            state.ws_config.coalesce_window,
                        )
                        .await
                        {
                            return;
                        }
                    }
                    Err(RecvError::Closed) => {
                        tracing::debug!("ws: broadcast channel closed; exiting");
                        return;
                    }
                }
            }

            _ = ping_timer.tick() => {
                // If a Pong is already outstanding, do NOT send a fresh Ping
                // — that would reset our deadline tracking and let a dead
                // connection survive forever. The pong-deadline branch above
                // will fire when the existing deadline expires.
                if awaiting_pong {
                    continue;
                }
                if let Err(e) = socket.send(Message::Ping(Default::default())).await {
                    tracing::debug!(error = ?e, "ws: failed to send Ping; closing");
                    return;
                }
                awaiting_pong = true;
                let deadline = tokio::time::Instant::now() + pong_timeout;
                pong_sleep.as_mut().reset(deadline);
            }
        }
    }
}

/// Process a single inbound text frame. Returns `false` if the connection
/// has been closed and the caller should exit.
///
/// Before applying a Subscribe/Unsubscribe, [`drain_backlog_under_state`]
/// flushes any envelopes currently in the broadcast receiver under the
/// CURRENT (pre-change) subscription state. This prevents a frame queued
/// before the client's Subscribe from being delivered after the new topic
/// is added (per the review finding for AC #2: "subsequent server frames"
/// is subsequent to the subscription, not subsequent to processing).
///
/// Story 2.3 — `Subscribe` for a `state.*` topic also emits a snapshot
/// of matching `session_projections` rows BEFORE the connection task's
/// main loop resumes. See the six-step ordering documented in the
/// `Subscribe` arm below.
#[allow(clippy::too_many_arguments)]
async fn handle_text_frame(
    socket: &mut WebSocket,
    subscriptions: &mut HashSet<Topic>,
    rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
    state: &AppState,
    text: &str,
    last_delivered_event_id: &mut Option<EventId>,
    last_dropped_at: &mut Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
) -> bool {
    let msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            close_with_bad_message(socket, &e.to_string()).await;
            return false;
        }
    };
    match msg {
        // Six-step ordering (Story 2.3):
        //
        //   [A] drain pre-existing in-flight envelopes under the OLD
        //       subscription set (unchanged from Story 2.1)
        //   [B] read now_ms for the read-time stale-Working fallback
        //   [C] read the projection table to build the snapshot, filtered
        //       by the NEW topic AND deduped against the pre-existing
        //       subscription set
        //   [D] insert the new topic into the subscription set so the
        //       main loop dispatches subsequent envelopes under the NEW
        //       set
        //   [E] emit each snapshot StateFrame on this connection's socket
        //   [F] (main loop resumes) `rx.recv()` dispatches buffered live
        //       envelopes under the NEW set, AFTER snapshot emission
        //
        // Window between [C] and [D]: a State envelope published in this
        // window is buffered in `rx` and dispatched at [F] under the new
        // set. The client may observe `snapshot(state=v1)` followed by
        // `live(state=v2)` for the same session — the live frame is the
        // newer truth. The snapshot is best-effort consistency, not
        // transactional, because locking the DB and the hub together is
        // not justified by the loss of strict consistency between two
        // surfaces that are deliberately decoupled. Do NOT add a second
        // drain after [D] — that would dispatch buffered envelopes under
        // the new set BEFORE the snapshot is emitted, reversing the
        // documented snapshot-first ordering.
        ClientMessage::Subscribe { topic } => match Topic::parse(&topic) {
            Ok(t) => {
                // [A] Drain pre-existing in-flight envelopes under the
                //     OLD subscription set.
                if !drain_backlog_under_state(
                    socket,
                    subscriptions,
                    rx,
                    last_delivered_event_id,
                    last_dropped_at,
                    pending_drop_count,
                    state.ws_config.coalesce_window,
                )
                .await
                {
                    return false;
                }

                // [B] Best-effort clock read. Failure logs and falls back
                //     to 0 — `current_state_for_read` then never triggers
                //     the stale-Working derivation in this rare window;
                //     the stored `current_state` rides through.
                let now_ms = match crate::time::current_unix_millis() {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "ws snapshot: clock read failed; proceeding without stale-Working fallback",
                        );
                        0
                    }
                };

                // [C] Build the snapshot. On reader pool/interact/serde
                //     error, log and emit an EMPTY snapshot — but still
                //     insert the topic at [D] so live frames flow. The
                //     trade-off is documented in the Subscribe-arm
                //     header above: snapshot is best-effort
                //     initialisation aid; the live stream is the
                //     primary contract. If the snapshot is essential,
                //     the client reconnects (which is the canonical WS
                //     retry mechanism and the only path that gets a
                //     fresh `pre_existing` set). Returning early
                //     without inserting the topic would leave the
                //     client silently unsubscribed for this topic
                //     because the protocol has no Subscribe ack/error
                //     frame in V1 (see protocol-changelog.md).
                let snapshot_frames = match crate::projection::snapshot_for_topic(
                    &state.db.reader,
                    &t,
                    subscriptions,
                    now_ms,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "ws snapshot: snapshot_for_topic failed; emitting empty snapshot, subscription remains live",
                        );
                        Vec::new()
                    }
                };

                // [D] Insert the new topic. Envelopes published between
                //     [C] and here stay buffered in rx and dispatch under
                //     the new set at [F], after snapshot emission.
                subscriptions.insert(t);

                // [E] Emit snapshot frames. Order is the SQL row order:
                //     `updated_at DESC, source ASC, session_id ASC`.
                for frame in snapshot_frames {
                    let json = match serde_json::to_string(&ServerMessage::State(frame)) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!(
                                error = ?e,
                                "ws snapshot: failed to serialize StateFrame; dropping",
                            );
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
        ClientMessage::Unsubscribe { topic } => match Topic::parse(&topic) {
            Ok(t) => {
                if !drain_backlog_under_state(
                    socket,
                    subscriptions,
                    rx,
                    last_delivered_event_id,
                    last_dropped_at,
                    pending_drop_count,
                    state.ws_config.coalesce_window,
                )
                .await
                {
                    return false;
                }
                subscriptions.remove(&t);
                true
            }
            Err(()) => {
                close_with_bad_message(socket, &format!("invalid unsubscribe topic: {topic}"))
                    .await;
                false
            }
        },
    }
}

/// Flush every envelope currently in the broadcast receiver, dispatching
/// each one under the current (pre-change) subscription state. Used by
/// [`handle_text_frame`] before applying a Subscribe/Unsubscribe so frames
/// queued before the change cannot leak across the topic-set update.
///
/// Returns `false` if a downstream send failed and the connection should
/// exit. Lag detected during drain goes through the same coalescing helper
/// as the main `rx.recv()` arm; the per-connection state is shared (Story
/// 2.4 AC #3 — both lag surfaces coalesce together).
#[allow(clippy::too_many_arguments)]
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
                // Same wire-side cursor discipline as the main rx.recv arm
                // — see DispatchOutcome doc and the code-review finding
                // for Story 2.4.
                match dispatch_envelope(socket, subscriptions, &env).await {
                    DispatchOutcome::Sent => {
                        if let BroadcastEnvelope::Event(ref ev) = env {
                            *last_delivered_event_id = Some(ev.event_id);
                        }
                    }
                    DispatchOutcome::Filtered => {}
                    DispatchOutcome::Closed => return false,
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
                )
                .await
                {
                    return false;
                }
            }
            Err(TryRecvError::Closed) => return true,
        }
    }
}

/// The decision the coalescing policy made for one lag arrival. Pure
/// data; no I/O, no `&mut WebSocket`. Lets the policy be unit-tested
/// deterministically with no `tokio::time::pause` / advance dance and
/// no TCP scaffolding — addresses Story 2.4 code-review finding #2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoalesceDecision {
    /// Window expired (or first lag ever); emit a `DroppedFrame` with
    /// these values. Caller resets `pending_drop_count` and records
    /// `last_dropped_at = now`.
    Emit {
        count: u64,
        first: EventId,
        last: EventId,
    },
    /// Within the coalesce window; the policy suppressed emission.
    /// `pending_drop_count` was incremented in-place by `n`.
    Suppress,
}

/// Pure coalescing decision. Splits the "should we emit?" policy from
/// the I/O-bound `emit_dropped_or_coalesce`, so the contract can be
/// covered by deterministic unit tests that construct fake `Instant`
/// values via `Instant::now() + Duration`.
///
/// Mutates `pending_drop_count` in BOTH branches (incremented by `n`
/// on Suppress; reset to 0 by the caller on Emit). The function is
/// `#[must_use]` because dropping the result on the Emit branch would
/// lose the wire-id range.
#[must_use]
fn coalesce_decision(
    now: tokio::time::Instant,
    last_dropped_at: Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    last_delivered_event_id: Option<EventId>,
    n: u64,
    coalesce_window: Duration,
) -> CoalesceDecision {
    let within_window = last_dropped_at
        .map(|t| now.duration_since(t) <= coalesce_window)
        .unwrap_or(false);

    if within_window {
        *pending_drop_count = pending_drop_count.saturating_add(n);
        return CoalesceDecision::Suppress;
    }

    let count = pending_drop_count.saturating_add(n);
    let first = match last_delivered_event_id {
        Some(EventId(id)) => EventId(id.saturating_add(1)),
        None => EventId(0),
    };
    // Best-estimate upper-bound. The broadcast channel doesn't expose
    // the post-lag cursor; the presenter recovers via REST with its OWN
    // last_delivered_event_id (which is the authoritative cursor it
    // tracked from prior `event` frames). The values here are
    // informational and satisfy the DroppedFrame::new(count > 0,
    // first <= last) invariant.
    let last_offset = count.saturating_sub(1).min(i64::MAX as u64) as i64;
    let last = EventId(first.0.saturating_add(last_offset));
    CoalesceDecision::Emit { count, first, last }
}

/// Project a `RecvError::Lagged(n)` / `TryRecvError::Lagged(n)` into the
/// wire-level `DroppedFrame` shape, with per-connection coalescing.
///
/// Behaviour (three rules):
///
/// 1. **First lag, or any lag after `coalesce_window` of silence** → emit
///    one `DroppedFrame` immediately, reset `pending_drop_count`, record
///    `last_dropped_at = now`.
/// 2. **Lag within the window** → silently accumulate into
///    `pending_drop_count`. No wire emission.
/// 3. **Catch-up (no lag arriving)** → no action. Accumulated
///    `pending_drop_count` stays parked; it folds into the next lag burst's
///    count, or is never emitted at all if the connection catches up fully.
///    Documented behaviour: a fully-recovered presenter doesn't need a
///    trailing recap.
///
/// The window is a passive `Instant`-comparison check on each call; there
/// is no active timer, no scheduled wake-up. The next `Lagged` arrival is
/// what triggers the window-expired check. This is deliberate — adding a
/// `tokio::time::sleep` arm to the connection's `select!` loop would
/// complicate the loop without any benefit to the AC.
///
/// `first_dropped_event_id` is a best-estimate from
/// `last_delivered_event_id + 1`; `last_dropped_event_id` is
/// `first + count - 1` (saturating). The broadcast channel doesn't expose
/// the post-lag cursor, so these are upper-bound informational values.
/// The presenter recovers via REST with its OWN `last_delivered_event_id`
/// (the authoritative cursor it tracked from prior `event` frames).
///
/// Returns `false` if the socket send failed; the caller should exit.
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
    let decision = coalesce_decision(
        now,
        *last_dropped_at,
        pending_drop_count,
        last_delivered_event_id,
        n,
        coalesce_window,
    );

    let (count, first, last) = match decision {
        CoalesceDecision::Suppress => {
            tracing::debug!(
                coalesced_into_pending = *pending_drop_count,
                "ws: dropped frame coalesced"
            );
            return true;
        }
        CoalesceDecision::Emit { count, first, last } => (count, first, last),
    };

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

/// Outcome of one `dispatch_envelope` call, used by the per-connection
/// task to decide whether to keep looping AND whether to advance the
/// `last_delivered_event_id` recovery cursor.
///
/// Story 2.4 split this off from a bare `bool` after the first code-review
/// round caught a cursor-corruption bug: a state-only subscriber received
/// every Event envelope through `rx.recv()` (broadcast topics are
/// receiver-side filters, not channel-side), so advancing the cursor on
/// `rx.recv()` Ok arms — instead of after a successful matching wire-send —
/// meant the next `DroppedFrame.first_dropped_event_id` was computed from
/// an Event the presenter never received.
enum DispatchOutcome {
    /// Topic-match hit AND wire send succeeded. The caller should advance
    /// `last_delivered_event_id` if the envelope was an `Event`.
    Sent,
    /// No topic-match, or matched but serialization failed (logged at
    /// ERROR). The caller should NOT advance the cursor — nothing reached
    /// the wire.
    Filtered,
    /// Wire send failed; the caller should exit the connection task.
    Closed,
}

/// Dispatch a broadcast envelope to the client iff any topic in the
/// subscription set matches. Sends at most one wire frame even when
/// multiple subscription entries match (Topic-match invariant: dedup at
/// dispatch). Returns `DispatchOutcome::Closed` if the socket failed to
/// send and the task should exit; `Sent` if a matching frame reached the
/// wire; `Filtered` otherwise (no match or serialization failure).
async fn dispatch_envelope(
    socket: &mut WebSocket,
    subscriptions: &HashSet<Topic>,
    envelope: &BroadcastEnvelope,
) -> DispatchOutcome {
    let any_match = subscriptions.iter().any(|t| t.matches(envelope));
    if !any_match {
        return DispatchOutcome::Filtered;
    }
    let frame = match envelope {
        BroadcastEnvelope::Event(ev) => ServerMessage::Event(EventFrame { event: ev.clone() }),
        BroadcastEnvelope::State {
            source,
            session_id,
            state,
        } => ServerMessage::State(StateFrame {
            source: source.clone(),
            session_id: session_id.clone(),
            state: state.clone(),
        }),
    };
    let json = match serde_json::to_string(&frame) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = ?e, "ws: failed to serialize ServerMessage; dropping");
            return DispatchOutcome::Filtered;
        }
    };
    if let Err(e) = socket.send(Message::Text(json.into())).await {
        tracing::debug!(error = ?e, "ws: send failed; closing");
        return DispatchOutcome::Closed;
    }
    DispatchOutcome::Sent
}

/// Sanitize a string for inclusion in a WS Close frame reason. Strips `\n`
/// and `\r` (which would corrupt frame framing) and truncates on a char
/// boundary to keep the total reason within the RFC 6455 §5.5.1 123-byte
/// limit (after the `"bad message: "` prefix).
fn sanitize_for_wire_ws(prefix: &str, detail: &str) -> String {
    let mut out = String::with_capacity(WS_CLOSE_REASON_LIMIT);
    out.push_str(prefix);
    for ch in detail.chars() {
        if ch == '\n' || ch == '\r' {
            out.push(' ');
        } else {
            // Pre-check char would push us past the byte limit; stop on the
            // last char boundary that fits.
            if out.len() + ch.len_utf8() > WS_CLOSE_REASON_LIMIT {
                break;
            }
            out.push(ch);
        }
    }
    if out.len() > WS_CLOSE_REASON_LIMIT {
        // Defensive: should be unreachable because we check above.
        out.truncate(WS_CLOSE_REASON_LIMIT);
    }
    out
}

async fn close_with_bad_message(socket: &mut WebSocket, detail: &str) {
    let reason = sanitize_for_wire_ws("bad message: ", detail);
    tracing::debug!(detail = %reason, "ws: bad message; closing");
    let frame = WsCloseFrame {
        code: 1008,
        reason: Utf8Bytes::from(reason),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_newlines() {
        let r = sanitize_for_wire_ws("bad message: ", "line1\nline2\rline3");
        assert!(!r.contains('\n'));
        assert!(!r.contains('\r'));
        assert!(r.starts_with("bad message: "));
    }

    #[test]
    fn sanitize_caps_at_123_bytes() {
        let big = "x".repeat(500);
        let r = sanitize_for_wire_ws("bad message: ", &big);
        assert!(r.len() <= WS_CLOSE_REASON_LIMIT);
        assert!(r.starts_with("bad message: "));
    }

    #[test]
    fn sanitize_respects_char_boundaries() {
        // Build a string whose final char crosses the limit when included.
        let detail: String = "a".repeat(WS_CLOSE_REASON_LIMIT - 14) + "héllo";
        let r = sanitize_for_wire_ws("bad message: ", &detail);
        assert!(r.is_char_boundary(r.len()));
        assert!(r.len() <= WS_CLOSE_REASON_LIMIT);
    }

    // ----- Story 2.4 coalesce_decision unit tests -----
    //
    // The coalescing policy is the timing-sensitive contract Story 2.4
    // depends on. The async `emit_dropped_or_coalesce` and the e2e
    // contract tests (`sustained_lag_does_not_storm_dropped_frames`,
    // `coalesce_window_resets_after_silence`) exercise the wire end-to-
    // end but are wall-clock sensitive. These unit tests cover the
    // policy contract deterministically — Instant values are
    // constructed by arithmetic, no scheduler, no TCP. Addresses
    // Story 2.4 code-review finding #2.

    #[test]
    fn coalesce_first_lag_ever_emits() {
        let now = tokio::time::Instant::now();
        let mut pending = 0u64;
        let d = coalesce_decision(
            now,
            None,
            &mut pending,
            None,
            7,
            Duration::from_secs(1),
        );
        assert_eq!(
            d,
            CoalesceDecision::Emit {
                count: 7,
                first: EventId(0),
                last: EventId(6),
            }
        );
        // pending_drop_count is NOT zeroed by the pure decision — the
        // async caller does that after a successful wire send.
        assert_eq!(pending, 0);
    }

    #[test]
    fn coalesce_first_lag_uses_cursor_plus_one() {
        let now = tokio::time::Instant::now();
        let mut pending = 0u64;
        let d = coalesce_decision(
            now,
            None,
            &mut pending,
            Some(EventId(100)),
            3,
            Duration::from_secs(1),
        );
        assert_eq!(
            d,
            CoalesceDecision::Emit {
                count: 3,
                first: EventId(101),
                last: EventId(103),
            }
        );
    }

    #[test]
    fn coalesce_within_window_suppresses_and_accumulates() {
        let base = tokio::time::Instant::now();
        let last_dropped_at = Some(base);
        let mut pending = 5u64;
        let d = coalesce_decision(
            base + Duration::from_millis(50),
            last_dropped_at,
            &mut pending,
            Some(EventId(10)),
            4,
            Duration::from_millis(100),
        );
        assert_eq!(d, CoalesceDecision::Suppress);
        // pending accumulates by n
        assert_eq!(pending, 9);
    }

    #[test]
    fn coalesce_window_expired_emits_pending_plus_n() {
        // Reproduces the AC #4 "window resets after silence" contract:
        // last_dropped_at was 200ms ago; window is 150ms; the next lag
        // emits a fresh Dropped frame whose count includes any
        // accumulated pending.
        let base = tokio::time::Instant::now();
        let last_dropped_at = Some(base);
        let mut pending = 3u64;
        let d = coalesce_decision(
            base + Duration::from_millis(200),
            last_dropped_at,
            &mut pending,
            Some(EventId(50)),
            10,
            Duration::from_millis(150),
        );
        // pending stays untouched here — the caller resets it on
        // successful wire emission.
        assert_eq!(pending, 3);
        assert_eq!(
            d,
            CoalesceDecision::Emit {
                count: 13, // 3 pending + 10 new
                first: EventId(51),
                last: EventId(63),
            }
        );
    }

    #[test]
    fn coalesce_boundary_equal_to_window_suppresses() {
        // The implementation uses `<=` for the within-window check; this
        // documents that an exact-boundary call is suppression, not
        // emission. A regression to strict `<` would flip this test.
        let base = tokio::time::Instant::now();
        let mut pending = 0u64;
        let d = coalesce_decision(
            base + Duration::from_millis(100),
            Some(base),
            &mut pending,
            Some(EventId(1)),
            1,
            Duration::from_millis(100),
        );
        assert_eq!(d, CoalesceDecision::Suppress);
        assert_eq!(pending, 1);
    }

    #[test]
    fn coalesce_pure_repeated_calls_within_window_accumulate() {
        // Models a tight lag-storm sequence: first lag emits, then
        // three more lags arrive within the window — all three are
        // suppressed and folded into `pending`. The single Emit is the
        // bound the AC #3 "≤ 31 frames over 30s" contract relies on.
        let base = tokio::time::Instant::now();
        let mut pending = 0u64;

        // First lag — Emit.
        let d1 = coalesce_decision(
            base,
            None,
            &mut pending,
            Some(EventId(0)),
            5,
            Duration::from_millis(100),
        );
        assert!(matches!(d1, CoalesceDecision::Emit { .. }));
        // Simulate the caller's post-emit bookkeeping.
        pending = 0;
        let last_dropped_at = Some(base);

        // Three more lags within the window — all suppressed.
        for offset_ms in [10, 30, 60] {
            let d = coalesce_decision(
                base + Duration::from_millis(offset_ms),
                last_dropped_at,
                &mut pending,
                Some(EventId(0)),
                2,
                Duration::from_millis(100),
            );
            assert_eq!(d, CoalesceDecision::Suppress);
        }
        assert_eq!(pending, 6); // 2 + 2 + 2 accumulated

        // After window expiry, the next lag emits with count = pending + n.
        let d_final = coalesce_decision(
            base + Duration::from_millis(150),
            last_dropped_at,
            &mut pending,
            Some(EventId(0)),
            1,
            Duration::from_millis(100),
        );
        assert_eq!(
            d_final,
            CoalesceDecision::Emit {
                count: 7, // 6 pending + 1 new
                first: EventId(1),
                last: EventId(7),
            }
        );
    }

    #[test]
    fn coalesce_count_one_first_eq_last() {
        // A single-envelope lag with no prior cursor must produce
        // first == last == EventId(0). Matches the
        // dropped_frame_new_allows_count_one_first_eq_last invariant
        // in `crates/protocol/src/ws.rs`.
        let now = tokio::time::Instant::now();
        let mut pending = 0u64;
        let d = coalesce_decision(
            now,
            None,
            &mut pending,
            None,
            1,
            Duration::from_secs(1),
        );
        assert_eq!(
            d,
            CoalesceDecision::Emit {
                count: 1,
                first: EventId(0),
                last: EventId(0),
            }
        );
    }
}
