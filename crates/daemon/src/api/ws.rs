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
    ClientMessage, CloseFrame as ProtocolCloseFrame, DroppedFrame, EventFrame, EventId, HelloFrame,
    ServerMessage, StateFrame,
};

use crate::api::filter::parse_states_array;
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
    if state.shutdown_requested.is_cancelled() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "daemon shutting down" })),
        )
            .into_response();
    }

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
    // Story 5.8 (ADR 0008) — the `(source, session_id)` natural keys this
    // connection has already emitted a snapshot frame for. The snapshot dedup
    // keys on delivered rows, NOT on subscribed topics: a topic set cannot
    // express a filtered subscribe's partial coverage. Recording the actual
    // delivered keys makes every snapshot contract fall out of one model —
    // identical re-subscribes are idempotent, a wider re-subscribe re-sends
    // only the keys a narrower one never covered, and (via the prune on
    // `Unsubscribe` below) coverage lapses when no active subscription tracks
    // the session, so a re-subscribe re-snapshots the drift.
    let mut snapshotted_keys: HashSet<(String, String)> = HashSet::new();
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
    // Second-round review fix: gap cursor captured at first suppression
    // of a coalescing window so the eventual Dropped emission reports
    // the true gap start rather than a later cursor-advanced value.
    let mut pending_dropped_first: Option<EventId> = None;

    loop {
        tokio::select! {
            biased;

            _ = state.ws_close_requested.cancelled() => {
                tracing::debug!("ws: shutdown close requested; draining and closing connection");
                close_for_daemon_shutdown(
                    &mut socket,
                    &subscriptions,
                    &mut snapshotted_keys,
                    &mut rx,
                    &mut last_delivered_event_id,
                    &mut last_dropped_at,
                    &mut pending_drop_count,
                    &mut pending_dropped_first,
                    state.ws_config.coalesce_window,
                )
                .await;
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
                            &mut snapshotted_keys,
                            &mut rx,
                            &state,
                            text.as_str(),
                            &mut last_delivered_event_id,
                            &mut last_dropped_at,
                            &mut pending_drop_count,
                            &mut pending_dropped_first,
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
                            DispatchOutcome::Sent => match &env {
                                BroadcastEnvelope::Event(ev) => {
                                    last_delivered_event_id = Some(ev.event_id);
                                }
                                // Story 5.8 pass-3: a live-delivered State frame
                                // keeps the row current on this connection, so
                                // record its `(source, session_id)` key in the
                                // snapshot-coverage set. An identical re-subscribe
                                // must not re-snapshot a session the live stream
                                // already carried (no-double-delivery, protocol.md).
                                BroadcastEnvelope::State {
                                    source,
                                    session_id,
                                    ..
                                } => {
                                    snapshotted_keys.insert((source.clone(), session_id.clone()));
                                }
                            },
                            DispatchOutcome::Filtered => {}
                            DispatchOutcome::Closed => return,
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Story 2.4: project lag into a `DroppedFrame` via
                        // the coalescing helper. Socket stays open per AC #1.
                        //
                        // Second-round review fix: when no subscription is
                        // active, lag is a transport-layer event with no
                        // wire-contract impact (the client never asked for
                        // any stream). Silent fast-forward instead of
                        // emitting a Dropped frame the presenter didn't
                        // earn through a subscription.
                        if subscriptions.is_empty() {
                            tracing::debug!(
                                dropped = n,
                                "ws: lag observed pre-subscription; silent fast-forward"
                            );
                            continue;
                        }
                        // Story 5.8 pass-4: broadcast `Lagged(n)` reports only a
                        // count of evicted envelopes, never their identities, so
                        // we cannot know whether a `State` frame for a covered
                        // session was in the gap. Any key in `snapshotted_keys`
                        // may now be stale on this connection, and the
                        // no-double-delivery dedup would otherwise make a
                        // re-subscribe SKIP it — leaving a state-only subscriber
                        // permanently stale (it can't replay missed state via
                        // `/sessions/{id}/events?since=`). Conservatively
                        // invalidate ALL snapshot coverage so the next
                        // re-subscribe re-snapshots the drift (the snapshot is
                        // the catch-up mechanism). An extra re-send of an
                        // unaffected row is idempotent; a missed update is not.
                        snapshotted_keys.clear();
                        if !emit_dropped_or_coalesce(
                            &mut socket,
                            &mut last_dropped_at,
                            &mut pending_drop_count,
                            &mut pending_dropped_first,
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
    snapshotted_keys: &mut HashSet<(String, String)>,
    rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
    state: &AppState,
    text: &str,
    last_delivered_event_id: &mut Option<EventId>,
    last_dropped_at: &mut Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    pending_dropped_first: &mut Option<EventId>,
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
        //       by the NEW topic AND deduped against the `(source,
        //       session_id)` keys already snapshot-delivered on this
        //       connection (Story 5.8: per-key, NOT per-topic — a topic set
        //       can't express a filtered subscribe's partial coverage)
        //   [D] insert the new topic into the subscription set so the
        //       main loop dispatches subsequent envelopes under the NEW
        //       set; record each delivered key at [E]
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
        ClientMessage::Subscribe { topic, states } => {
            // Story 5.8 (ADR 0008): parse the optional snapshot-scoping
            // `states` filter before anything else. An invalid token closes
            // the connection with `bad message` (same loud-reject as an
            // invalid topic — strict-inbound policy). Each array element is
            // validated as a SINGLE token (not CSV-joined), so a comma-bearing
            // element is rejected rather than silently split — one wire value,
            // one grammar input, the same discipline `topic` follows. An
            // empty/absent `states` yields an empty filter (unfiltered, the
            // v1.0 default). The token table is shared with the REST `?state=`
            // parser via `crate::api::filter`.
            let state_filter = match parse_states_array(&states) {
                Ok(f) => f,
                Err(msg) => {
                    close_with_bad_message(socket, &msg).await;
                    return false;
                }
            };
            match Topic::parse(&topic) {
                Ok(t) => {
                    // Story 5.8 pass-3 (ADR 0008): `states` scopes the
                    // snapshot-on-subscribe burst, and only `state.session.*`
                    // family topics have one. A non-empty `states` on an event
                    // topic has nothing to scope, so accepting it would silently
                    // discard the presenter's intent — the strict-inbound axiom
                    // (project-context §"Wire format") fails loud instead, the
                    // same loud-reject an invalid token or topic gets. An empty
                    // `states` stays valid on any topic (an ordinary subscribe).
                    if !state_filter.is_empty() && !t.is_state_session_family() {
                        close_with_bad_message(
                            socket,
                            &format!(
                                "states filter is only valid on state.session.* topics, not {topic:?}"
                            ),
                        )
                        .await;
                        return false;
                    }

                    // [A] Drain pre-existing in-flight envelopes under the
                    //     OLD subscription set.
                    if !drain_backlog_under_state(
                        socket,
                        subscriptions,
                        snapshotted_keys,
                        rx,
                        last_delivered_event_id,
                        last_dropped_at,
                        pending_drop_count,
                        pending_dropped_first,
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
                    //     fresh, empty `snapshotted_keys` set). Returning early
                    //     without inserting the topic would leave the
                    //     client silently unsubscribed for this topic
                    //     because the protocol has no Subscribe ack/error
                    //     frame in V1 (see protocol-changelog.md).
                    //     Dedup keys on `snapshotted_keys` (the `(source,
                    //     session_id)` rows this connection already sent a
                    //     snapshot frame for), NOT on subscribed topics: a
                    //     prior filtered subscribe only covered a subset of
                    //     its topic, so a topic-level dedup would either
                    //     suppress rows the client never got or re-send rows
                    //     it already has (Story 5.8 finding).
                    let snapshot_frames = match crate::projection::snapshot_for_topic(
                        &state.db.reader,
                        &t,
                        snapshotted_keys,
                        now_ms,
                        &state_filter,
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

                    // [E] Emit snapshot frames and record each delivered
                    //     `(source, session_id)` key so a later subscribe on
                    //     this connection won't re-send it (the dedup model
                    //     above). Order is the SQL row order:
                    //     `updated_at DESC, source ASC, session_id ASC`.
                    for frame in snapshot_frames {
                        snapshotted_keys.insert((frame.source.clone(), frame.session_id.clone()));
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
                    close_with_bad_message(socket, &format!("invalid subscribe topic: {topic}"))
                        .await;
                    false
                }
            }
        }
        ClientMessage::Unsubscribe { topic } => match Topic::parse(&topic) {
            Ok(t) => {
                if !drain_backlog_under_state(
                    socket,
                    subscriptions,
                    snapshotted_keys,
                    rx,
                    last_delivered_event_id,
                    last_dropped_at,
                    pending_drop_count,
                    pending_dropped_first,
                    state.ws_config.coalesce_window,
                )
                .await
                {
                    return false;
                }
                subscriptions.remove(&t);
                // Story 5.8: snapshot coverage lapses when no active
                // subscription still tracks the session. Drop keys no longer
                // covered so a later re-subscribe re-snapshots the drift that
                // accumulated while unsubscribed — the snapshot is a catch-up,
                // and unsubscribing stops the live updates that kept it
                // current (the `state.session.<id>.current_state` sibling is
                // handled by `covers_state_session`, which keys on the id).
                snapshotted_keys.retain(|(_source, sid)| {
                    subscriptions.iter().any(|s| s.covers_state_session(sid))
                });
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
    snapshotted_keys: &mut HashSet<(String, String)>,
    rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
    last_delivered_event_id: &mut Option<EventId>,
    last_dropped_at: &mut Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    pending_dropped_first: &mut Option<EventId>,
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
                    DispatchOutcome::Sent => match &env {
                        BroadcastEnvelope::Event(ev) => {
                            *last_delivered_event_id = Some(ev.event_id);
                        }
                        // Story 5.8 pass-3: same per-key coverage recording as
                        // the main `rx.recv()` arm — a State frame drained under
                        // the OLD subscription set (before a Subscribe inserts
                        // the new topic) keeps that row current, so the snapshot
                        // built next must dedup against it.
                        BroadcastEnvelope::State {
                            source, session_id, ..
                        } => {
                            snapshotted_keys.insert((source.clone(), session_id.clone()));
                        }
                    },
                    DispatchOutcome::Filtered => {}
                    DispatchOutcome::Closed => return false,
                }
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Lagged(n)) => {
                // Same pre-subscription guard as the main rx.recv arm
                // (second-round review fix). `drain_backlog_under_state`
                // runs under the OLD subscription set (before the new
                // Subscribe is inserted); if that set was empty, the lag
                // happened entirely outside any wire contract — silent
                // fast-forward.
                if subscriptions.is_empty() {
                    tracing::debug!(
                        dropped = n,
                        "ws: drain lag observed pre-subscription; silent fast-forward"
                    );
                    continue;
                }
                // Story 5.8 pass-4: same opaque-gap invalidation as the main
                // `rx.recv()` arm — a dropped batch may have carried a `State`
                // frame for a covered session, so clear snapshot coverage. This
                // drain runs immediately before a Subscribe/Unsubscribe applies,
                // so the snapshot that subscribe builds next re-snapshots the
                // drift instead of skipping it as already-covered.
                snapshotted_keys.clear();
                if !emit_dropped_or_coalesce(
                    socket,
                    last_dropped_at,
                    pending_drop_count,
                    pending_dropped_first,
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
/// Mutates `pending_drop_count` and `pending_dropped_first` in BOTH
/// branches:
/// - On `Suppress`: `pending_drop_count` is incremented by `n`; on the
///   FIRST suppression of a window (when `pending_drop_count` was 0
///   before this call), `pending_dropped_first` captures the
///   `last_delivered_event_id + 1` cursor (or `EventId(0)` sentinel)
///   that will be used when the window eventually emits — see
///   second-round review finding #2.
/// - On `Emit`: the caller resets both `pending_drop_count = 0` and
///   `*pending_dropped_first = None` after a successful wire send.
///
/// `#[must_use]` because dropping the result on the Emit branch would
/// lose the wire-id range.
#[must_use]
fn coalesce_decision(
    now: tokio::time::Instant,
    last_dropped_at: Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    pending_dropped_first: &mut Option<EventId>,
    last_delivered_event_id: Option<EventId>,
    n: u64,
    coalesce_window: Duration,
) -> CoalesceDecision {
    let within_window = last_dropped_at
        .map(|t| now.duration_since(t) <= coalesce_window)
        .unwrap_or(false);

    // Helper: project a cursor to the next dropped-id (cursor+1, or
    // EventId(0) sentinel when no prior Event was delivered).
    let cursor_to_first = |cursor: Option<EventId>| -> EventId {
        match cursor {
            Some(EventId(id)) => EventId(id.saturating_add(1)),
            None => EventId(0),
        }
    };

    if within_window {
        // Capture the gap cursor on the FIRST suppression of this
        // window — the next emission MUST report this as its
        // `first_dropped_event_id`, even if normal Event delivery
        // advances `last_delivered_event_id` before the window expires.
        // Otherwise the eventual Dropped frame describes a range that
        // starts after the presenter's true gap (the second-round
        // review's "stale count, fresh cursor" finding).
        if *pending_drop_count == 0 {
            *pending_dropped_first = Some(cursor_to_first(last_delivered_event_id));
        }
        *pending_drop_count = pending_drop_count.saturating_add(n);
        return CoalesceDecision::Suppress;
    }

    let count = pending_drop_count.saturating_add(n);
    let first = match *pending_dropped_first {
        // Prior suppressions in this window captured the gap origin;
        // use that, NOT the (possibly cursor-advanced) current value.
        Some(captured) => captured,
        // First lag of a window (no prior suppression): use current
        // cursor + 1.
        None => cursor_to_first(last_delivered_event_id),
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
#[allow(clippy::too_many_arguments)]
async fn emit_dropped_or_coalesce(
    socket: &mut WebSocket,
    last_dropped_at: &mut Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    pending_dropped_first: &mut Option<EventId>,
    last_delivered_event_id: Option<EventId>,
    n: u64,
    coalesce_window: Duration,
) -> bool {
    let now = tokio::time::Instant::now();
    let decision = coalesce_decision(
        now,
        *last_dropped_at,
        pending_drop_count,
        pending_dropped_first,
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
    // Reset captured gap cursor for the next window — see Story 2.4
    // second-round review finding #2.
    *pending_dropped_first = None;
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

#[allow(clippy::too_many_arguments)]
async fn close_for_daemon_shutdown(
    socket: &mut WebSocket,
    subscriptions: &HashSet<Topic>,
    snapshotted_keys: &mut HashSet<(String, String)>,
    rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
    last_delivered_event_id: &mut Option<EventId>,
    last_dropped_at: &mut Option<tokio::time::Instant>,
    pending_drop_count: &mut u64,
    pending_dropped_first: &mut Option<EventId>,
    coalesce_window: Duration,
) {
    if !drain_backlog_under_state(
        socket,
        subscriptions,
        snapshotted_keys,
        rx,
        last_delivered_event_id,
        last_dropped_at,
        pending_drop_count,
        pending_dropped_first,
        coalesce_window,
    )
    .await
    {
        tracing::debug!("ws: shutdown backlog drain ended on send failure");
        return;
    }

    let frame = ServerMessage::Close(ProtocolCloseFrame {
        reason: Some("daemon shutdown".to_string()),
    });
    let json = match serde_json::to_string(&frame) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = ?e, "ws: failed to serialize shutdown Close frame");
            return;
        }
    };
    if let Err(e) = socket.send(Message::Text(json.into())).await {
        tracing::debug!(error = ?e, "ws: failed to send protocol shutdown Close frame");
        return;
    }

    let frame = WsCloseFrame {
        code: 1000,
        reason: Utf8Bytes::from("daemon shutdown"),
    };
    if let Err(e) = socket.send(Message::Close(Some(frame))).await {
        tracing::debug!(error = ?e, "ws: failed to send websocket shutdown Close frame");
    }
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
        let mut pending_first: Option<EventId> = None;
        let d = coalesce_decision(
            now,
            None,
            &mut pending,
            &mut pending_first,
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
        // pending_dropped_first is only set on Suppress (first lag of a
        // window). On a first-ever lag that goes straight to Emit, it
        // stays None.
        assert_eq!(pending_first, None);
    }

    #[test]
    fn coalesce_first_lag_uses_cursor_plus_one() {
        let now = tokio::time::Instant::now();
        let mut pending = 0u64;
        let mut pending_first: Option<EventId> = None;
        let d = coalesce_decision(
            now,
            None,
            &mut pending,
            &mut pending_first,
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
        // pending > 0 means we've already captured first for THIS
        // window — simulate that with a pre-set value.
        let mut pending_first: Option<EventId> = Some(EventId(11));
        let d = coalesce_decision(
            base + Duration::from_millis(50),
            last_dropped_at,
            &mut pending,
            &mut pending_first,
            Some(EventId(10)),
            4,
            Duration::from_millis(100),
        );
        assert_eq!(d, CoalesceDecision::Suppress);
        // pending accumulates by n
        assert_eq!(pending, 9);
        // pending_first stays at its captured value (the first
        // suppression of the window already set it).
        assert_eq!(pending_first, Some(EventId(11)));
    }

    #[test]
    fn coalesce_window_expired_emits_pending_plus_n() {
        // Reproduces the AC #4 "window resets after silence" contract:
        // last_dropped_at was 200ms ago; window is 150ms; the next lag
        // emits a fresh Dropped frame whose count includes any
        // accumulated pending. With no captured pending_first the
        // emission uses the current cursor.
        let base = tokio::time::Instant::now();
        let last_dropped_at = Some(base);
        let mut pending = 3u64;
        let mut pending_first: Option<EventId> = None;
        let d = coalesce_decision(
            base + Duration::from_millis(200),
            last_dropped_at,
            &mut pending,
            &mut pending_first,
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
        let mut pending_first: Option<EventId> = None;
        let d = coalesce_decision(
            base + Duration::from_millis(100),
            Some(base),
            &mut pending,
            &mut pending_first,
            Some(EventId(1)),
            1,
            Duration::from_millis(100),
        );
        assert_eq!(d, CoalesceDecision::Suppress);
        assert_eq!(pending, 1);
        // First suppression captures pending_first = cursor + 1.
        assert_eq!(pending_first, Some(EventId(2)));
    }

    #[test]
    fn coalesce_pure_repeated_calls_within_window_accumulate() {
        // Models a tight lag-storm sequence: first lag emits, then
        // three more lags arrive within the window — all three are
        // suppressed and folded into `pending`. The single Emit is the
        // bound the AC #3 "≤ 31 frames over 30s" contract relies on.
        let base = tokio::time::Instant::now();
        let mut pending = 0u64;
        let mut pending_first: Option<EventId> = None;

        // First lag — Emit.
        let d1 = coalesce_decision(
            base,
            None,
            &mut pending,
            &mut pending_first,
            Some(EventId(0)),
            5,
            Duration::from_millis(100),
        );
        assert!(matches!(d1, CoalesceDecision::Emit { .. }));
        // Simulate the caller's post-emit bookkeeping.
        pending = 0;
        pending_first = None;
        let last_dropped_at = Some(base);

        // Three more lags within the window — all suppressed.
        for offset_ms in [10, 30, 60] {
            let d = coalesce_decision(
                base + Duration::from_millis(offset_ms),
                last_dropped_at,
                &mut pending,
                &mut pending_first,
                Some(EventId(0)),
                2,
                Duration::from_millis(100),
            );
            assert_eq!(d, CoalesceDecision::Suppress);
        }
        // 2 + 2 + 2 accumulated.
        assert_eq!(pending, 6);
        // First suppression of the new window captured the gap origin.
        assert_eq!(pending_first, Some(EventId(1)));

        // After window expiry, the next lag emits with count = pending + n
        // and first = the captured pending_first (NOT current cursor).
        let d_final = coalesce_decision(
            base + Duration::from_millis(150),
            last_dropped_at,
            &mut pending,
            &mut pending_first,
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
        let mut pending_first: Option<EventId> = None;
        let d = coalesce_decision(
            now,
            None,
            &mut pending,
            &mut pending_first,
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

    /// Story 2.4 second-round review finding #2 — Pending drops MUST
    /// carry their own cursor through the coalescing window. When a lag
    /// is suppressed, then a normal Event delivery advances
    /// `last_delivered_event_id`, then the window expires and the
    /// suppressed lag finally emits, the emitted `first_dropped_event_id`
    /// MUST reflect the cursor at the time of the suppression — NOT the
    /// post-advance cursor. Otherwise count includes pre-advance gaps but
    /// the range starts after the presenter's true gap, which is
    /// internally inconsistent and weakens recovery diagnostics.
    #[test]
    fn coalesce_emit_after_cursor_advance_uses_captured_first() {
        let base = tokio::time::Instant::now();
        let mut pending = 0u64;
        let mut pending_first: Option<EventId> = None;
        let window = Duration::from_millis(100);

        // 1. First lag — Emit immediately. Cursor at EventId(10).
        let d1 = coalesce_decision(
            base,
            None,
            &mut pending,
            &mut pending_first,
            Some(EventId(10)),
            5,
            window,
        );
        assert_eq!(
            d1,
            CoalesceDecision::Emit {
                count: 5,
                first: EventId(11),
                last: EventId(15),
            }
        );
        // Post-Emit caller bookkeeping.
        pending = 0;
        pending_first = None;
        let last_dropped_at = Some(base);

        // 2. Normal Event delivery: cursor advances to EventId(20).
        //    (No coalesce_decision call — this happens on rx.recv Ok arm.)
        let cursor_after_delivery_1 = Some(EventId(20));

        // 3. Second lag, within window. Cursor is now EventId(20).
        //    Should SUPPRESS and capture pending_first = EventId(21).
        let d2 = coalesce_decision(
            base + Duration::from_millis(30),
            last_dropped_at,
            &mut pending,
            &mut pending_first,
            cursor_after_delivery_1,
            3,
            window,
        );
        assert_eq!(d2, CoalesceDecision::Suppress);
        assert_eq!(pending, 3);
        assert_eq!(
            pending_first,
            Some(EventId(21)),
            "first suppression of window must capture cursor + 1"
        );

        // 4. More normal Event deliveries: cursor advances to EventId(40).
        //    The captured pending_first MUST stay at EventId(21).
        let cursor_after_delivery_2 = Some(EventId(40));

        // 5. Third lag, still within window. Should SUPPRESS without
        //    overwriting pending_first.
        let d3 = coalesce_decision(
            base + Duration::from_millis(70),
            last_dropped_at,
            &mut pending,
            &mut pending_first,
            cursor_after_delivery_2,
            2,
            window,
        );
        assert_eq!(d3, CoalesceDecision::Suppress);
        assert_eq!(pending, 5);
        assert_eq!(
            pending_first,
            Some(EventId(21)),
            "subsequent suppressions in the same window must NOT overwrite \
             pending_first; the original gap origin is the recovery signal"
        );

        // 6. Window expires; fourth lag arrives. Cursor is now EventId(40).
        //    The Emit MUST use the captured first (EventId(21)), NOT the
        //    advanced cursor (would have been EventId(41)).
        let d4 = coalesce_decision(
            base + Duration::from_millis(200),
            last_dropped_at,
            &mut pending,
            &mut pending_first,
            cursor_after_delivery_2,
            4,
            window,
        );
        // Pre-fix: this would have been Emit { count: 9, first: EventId(41), ... }
        // Post-fix: Emit { count: 9, first: EventId(21), ... }
        assert_eq!(
            d4,
            CoalesceDecision::Emit {
                count: 9, // 5 pending + 4 new
                first: EventId(21),
                last: EventId(29),
            },
            "emit-after-cursor-advance MUST use the captured pending_first \
             (EventId(21)), NOT the post-advance current cursor (EventId(41)). \
             Regression of Story 2.4 second-round code-review finding #2."
        );
    }

    /// Story 2.4 second-round review finding #3 — AC #1 says a single
    /// lag burst produces EXACTLY one `Dropped` frame. At the policy
    /// level this is structural: `coalesce_decision` is only called from
    /// the two `Lagged` arms (main `rx.recv()` + drain). One `Lagged(n)`
    /// arrival → one call → one decision. The test documents the
    /// post-Emit state so a regression that "double-fires" by emitting
    /// twice for the same burst would fail loudly.
    #[test]
    fn coalesce_single_lag_burst_produces_exactly_one_emit() {
        let base = tokio::time::Instant::now();
        let mut pending = 0u64;
        let mut pending_first: Option<EventId> = None;
        let window = Duration::from_secs(1);

        // First lag — Emit.
        let d1 = coalesce_decision(
            base,
            None,
            &mut pending,
            &mut pending_first,
            None,
            1, // n = 1 (1025 envelopes vs capacity 1024 → 1-pos lag)
            window,
        );
        assert!(matches!(
            d1,
            CoalesceDecision::Emit {
                count: 1,
                first: EventId(0),
                last: EventId(0),
            }
        ));
        // Caller post-Emit bookkeeping.
        pending = 0;
        pending_first = None;
        let last_dropped_at = Some(base);

        // No further `Lagged` arrives in this scenario — the burst is
        // over. The receiver consumes the rest of the channel normally;
        // those are `rx.recv() Ok` arms that DO NOT call
        // coalesce_decision. Without another Lagged event, no second
        // emit can occur. (We assert this structurally below: simulate
        // a second call within the window with n=0 — would be a bug if
        // it ever happens — and confirm Suppress, NOT Emit.)
        //
        // n=0 is a hypothetical bug-state input. The function still
        // honors the within-window check, so even a "vacuous" lag call
        // suppresses; the upstream invariant is that Lagged(0) never
        // arrives from tokio::broadcast.
        let d2 = coalesce_decision(
            base + Duration::from_millis(10),
            last_dropped_at,
            &mut pending,
            &mut pending_first,
            None,
            0,
            window,
        );
        assert_eq!(d2, CoalesceDecision::Suppress);
    }

    /// Story 2.4 second-round review finding #6 — The AC #3 contract
    /// guarantees "≤ 31 Dropped frames over 30s sustained lag with
    /// `coalesce_window = 1s`." The e2e contract test
    /// `sustained_lag_does_not_storm_dropped_frames` exercises 1s of
    /// wall time with a 100ms window. This pure-policy test simulates
    /// the production default (30s horizon, 1s window) deterministically
    /// — no scheduler, no TCP — so a regression in long-horizon emission
    /// rate bookkeeping fails this test even if the integration smoke
    /// stays green.
    #[test]
    fn coalesce_30s_horizon_with_1s_window_bounded_at_31_emits() {
        let base = tokio::time::Instant::now();
        let mut pending = 0u64;
        let mut pending_first: Option<EventId> = None;
        let mut last_dropped_at: Option<tokio::time::Instant> = None;
        let window = Duration::from_secs(1);

        // Simulate 30 real seconds of sustained lag by calling
        // coalesce_decision every 100ms (300 calls). Each call models
        // one Lagged arrival from the broadcast receiver.
        let mut emit_count: u64 = 0;
        let mut cursor: i64 = 0;
        for i in 0..300 {
            let now = base + Duration::from_millis(100 * i);
            let decision = coalesce_decision(
                now,
                last_dropped_at,
                &mut pending,
                &mut pending_first,
                Some(EventId(cursor)),
                10, // n = 10 envelopes per lag event
                window,
            );
            match decision {
                CoalesceDecision::Emit { .. } => {
                    emit_count += 1;
                    // Mirror the caller's post-Emit bookkeeping.
                    pending = 0;
                    pending_first = None;
                    last_dropped_at = Some(now);
                }
                CoalesceDecision::Suppress => {
                    // Nothing — state mutated in-place by the function.
                }
            }
            // Simulate occasional normal Event delivery advancing cursor.
            // This is the scenario where finding #2 matters: cursor
            // advances even while suppressed lag accumulates.
            cursor += 5;
        }

        // Theoretical ceiling per AC #3: ceil(30s / 1s) = 30 Emit frames,
        // plus possibly 1 for the very first lag that occurs at t=0
        // before any window is established. Spec says "≤ 31."
        assert!(
            emit_count <= 31,
            "AC #3: 30s sustained lag with 1s window must emit ≤ 31 \
             DroppedFrames; got {emit_count}. Coalescing window \
             bookkeeping regressed."
        );
        // Lower bound: at least one emission must happen (sanity check).
        assert!(
            emit_count >= 1,
            "30s of continuous lag must produce at least one Dropped \
             emission; got {emit_count}"
        );
    }
}
