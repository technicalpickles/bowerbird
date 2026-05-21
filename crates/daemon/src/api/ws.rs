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

use protocol::{ClientMessage, EventFrame, EventId, HelloFrame, ServerMessage, StateFrame};

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
                        if !handle_text_frame(&mut socket, &mut subscriptions, &mut rx, text.as_str()).await {
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
                        if !dispatch_envelope(&mut socket, &subscriptions, &env).await {
                            return;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        // Story 2.4 will project this into a `DroppedFrame`.
                        // Story 2.1 logs and continues; the socket stays open.
                        tracing::warn!(dropped = n, "ws: broadcast receiver lagged");
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
async fn handle_text_frame(
    socket: &mut WebSocket,
    subscriptions: &mut HashSet<Topic>,
    rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
    text: &str,
) -> bool {
    let msg: ClientMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            close_with_bad_message(socket, &e.to_string()).await;
            return false;
        }
    };
    match msg {
        ClientMessage::Subscribe { topic } => match Topic::parse(&topic) {
            Ok(t) => {
                if !drain_backlog_under_state(socket, subscriptions, rx).await {
                    return false;
                }
                subscriptions.insert(t);
                true
            }
            Err(()) => {
                close_with_bad_message(socket, &format!("invalid subscribe topic: {topic}")).await;
                false
            }
        },
        ClientMessage::Unsubscribe { topic } => match Topic::parse(&topic) {
            Ok(t) => {
                if !drain_backlog_under_state(socket, subscriptions, rx).await {
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
/// exit. `Lagged` is logged at WARN (consistent with the main `rx` branch).
async fn drain_backlog_under_state(
    socket: &mut WebSocket,
    subscriptions: &HashSet<Topic>,
    rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
) -> bool {
    use tokio::sync::broadcast::error::TryRecvError;
    loop {
        match rx.try_recv() {
            Ok(env) => {
                if !dispatch_envelope(socket, subscriptions, &env).await {
                    return false;
                }
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Lagged(n)) => {
                tracing::warn!(
                    dropped = n,
                    "ws: broadcast receiver lagged during subscribe drain"
                );
            }
            Err(TryRecvError::Closed) => return true,
        }
    }
}

/// Dispatch a broadcast envelope to the client iff any topic in the
/// subscription set matches. Sends at most one wire frame even when
/// multiple subscription entries match (Topic-match invariant: dedup at
/// dispatch). Returns `false` if the socket failed to send and the task
/// should exit.
async fn dispatch_envelope(
    socket: &mut WebSocket,
    subscriptions: &HashSet<Topic>,
    envelope: &BroadcastEnvelope,
) -> bool {
    let any_match = subscriptions.iter().any(|t| t.matches(envelope));
    if !any_match {
        return true;
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
            return true;
        }
    };
    if let Err(e) = socket.send(Message::Text(json.into())).await {
        tracing::debug!(error = ?e, "ws: send failed; closing");
        return false;
    }
    true
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
}
