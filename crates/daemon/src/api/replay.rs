//! `POST /replay` — accept JSONL of `protocol::Event` records and push them
//! onto the daemon's existing ingest channel.
//!
//! Contract: accepts JSONL of `protocol::Event` records on the request body;
//! strips `event_id` + `created_at`; constructs `EventEnvelope`; pushes to
//! `state.ingest_tx`. Per-line parse failures are collected and returned in
//! the response body rather than failing the entire request — replay is a
//! development tool, not a transactional ingest path. The endpoint is
//! bearer-auth-required (it joins the `authenticated` Router branch in
//! [`super::router`]).
//!
//! Sentinel kinds (`RecordingStarted` / `RecordingEnded`) are rejected at the
//! parse boundary with a per-line error rather than propagated through
//! `projection::session::write` — the runtime guard there returns a generic
//! `Error::Projection` message; rejecting up front yields a clearer per-line
//! error.
//!
//! `event_id` and `created_at` carried in the JSONL lines are silently
//! dropped: the writer reassigns both via AUTOINCREMENT + `current_unix_millis()`
//! at projection-write time. Replayed events therefore reflect replay
//! wall-clock rather than source wall-clock — replay is for development, not
//! performance reproduction.

use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use protocol::{Event, EventEnvelope};
use serde::Serialize;

use crate::state::AppState;

/// Per-line error in the JSONL request body. The `line` field is 1-based to
/// match what a developer sees in `less`/`vim`; an explicit-zero would be
/// confusing.
#[derive(Debug, Serialize)]
pub struct ParseError {
    pub line: usize,
    pub error: String,
}

/// Response body for `POST /replay`. `replayed_count` is the number of
/// envelopes successfully pushed onto `ingest_tx`; downstream commit failures
/// are reported only in the daemon's tracing output (the HTTP response is
/// synchronous but the writer task runs asynchronously — there is no place
/// to surface a post-handler-return commit failure to the caller without
/// blocking the response on the entire writer drain).
#[derive(Debug, Serialize)]
pub struct ReplayResponse {
    pub replayed_count: usize,
    pub parse_errors: Vec<ParseError>,
}

/// Accept any reasonable JSONL content-type. We are not opinionated:
/// `application/json-seq`, `application/x-ndjson`, and `text/plain` are all
/// in the wild for JSONL. The actual decoder treats the body as `\n`-separated
/// bytes regardless of `Content-Type`.
#[tracing::instrument(skip_all, fields(content_length = body.len()))]
pub async fn run(State(state): State<AppState>, body: Bytes) -> Response {
    let mut replayed_count: usize = 0;
    let mut parse_errors: Vec<ParseError> = Vec::new();

    for (idx, line) in body.split(|&b| b == b'\n').enumerate() {
        let line_no = idx + 1;
        // Skip blank lines (including the trailing empty slice after a final
        // `\n`) and `#`-prefixed comment lines (the JSONL ecosystem has no
        // standard comment convention, but we honor it as a convenience for
        // hand-authored replay files).
        let trimmed = trim_ascii(line);
        if trimmed.is_empty() || trimmed.starts_with(b"#") {
            continue;
        }

        let event: Event = match serde_json::from_slice(line) {
            Ok(e) => e,
            Err(e) => {
                parse_errors.push(ParseError {
                    line: line_no,
                    error: e.to_string(),
                });
                continue;
            }
        };

        // Reject sentinel kinds at the parse boundary so the error is clearer
        // than a propagated `Error::Projection` from
        // `projection::session::write`'s runtime guard.
        if matches!(
            event.kind,
            protocol::EventKind::RecordingStarted | protocol::EventKind::RecordingEnded
        ) {
            parse_errors.push(ParseError {
                line: line_no,
                error: "sentinel kind cannot be replayed".to_string(),
            });
            continue;
        }

        // Reject the decode-only `Unknown` catch-all (Story 4.4 / Epic 2 retro
        // AI-4). A replay line with a v1.x-only `kind` value (e.g., a future
        // `"SubAgentSpawn"`) parses to `EventKind::Unknown` via `#[serde(other)]`
        // — replaying it would persist the lossy `"Unknown"` token to SQLite
        // and lose the original kind string. Surface as a parse error so the
        // operator updates their bowerbird before replaying foreign data.
        if matches!(event.kind, protocol::EventKind::Unknown) {
            parse_errors.push(ParseError {
                line: line_no,
                error: "unknown event kind: this build is older than the source daemon".to_string(),
            });
            continue;
        }

        // Story 5.3: re-parse notification_type from payload for Notification
        // events so the replayed envelope drives the same transition() rules
        // as live ingest. Replayed events carry their stored `pid` forward.
        let notification_type = if matches!(event.kind, protocol::EventKind::Notification) {
            serde_json::from_str::<serde_json::Value>(&event.payload)
                .ok()
                .as_ref()
                .and_then(|v| v.get("notification_type"))
                .and_then(|v| v.as_str())
                .map(|s| match s {
                    "permission_prompt" => protocol::NotificationType::PermissionPrompt,
                    "idle_prompt" => protocol::NotificationType::IdlePrompt,
                    "auth_success" => protocol::NotificationType::AuthSuccess,
                    "elicitation_dialog" => protocol::NotificationType::ElicitationDialog,
                    "elicitation_response" => protocol::NotificationType::ElicitationResponse,
                    "elicitation_complete" => protocol::NotificationType::ElicitationComplete,
                    _ => protocol::NotificationType::Unknown,
                })
        } else {
            None
        };
        let envelope = EventEnvelope {
            source: event.source,
            session_id: event.session_id,
            kind: event.kind,
            reaction: event.reaction,
            payload: event.payload,
            pid: event.pid,
            notification_type,
            // Story 5.7: a replayed event carries its stored `cwd` forward so
            // the rebuilt projection matches live ingest. `started_at` is NOT
            // threaded here — it follows replay's existing Story 4.1
            // retimestamping contract: replay drops each JSONL line's
            // `event_id` and `created_at`, and `projection::session::write`
            // stamps a fresh `current_unix_millis()` at write time. So the
            // projected `started_at` is the replay wall-clock of the FIRST
            // replayed write for the session (set-once / keep-earliest in
            // transition), NOT the original exported timestamp. Do not thread
            // the JSONL `created_at` through here unless intentionally changing
            // Story 4.1's replay semantics.
            cwd: event.cwd,
        };

        // try_send rather than send: we never block the HTTP handler on a
        // full channel. A `TrySendError::Full` is reported back as a parse
        // error (with a distinct message so a developer can tell the
        // difference between a malformed line and a saturated queue).
        match state.ingest_tx.try_send(envelope) {
            Ok(()) => {
                replayed_count += 1;
                tracing::debug!(line = line_no, "replay: forwarded envelope");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                parse_errors.push(ParseError {
                    line: line_no,
                    error: "channel full (replay too fast)".to_string(),
                });
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                parse_errors.push(ParseError {
                    line: line_no,
                    error: "ingest channel closed (daemon shutting down)".to_string(),
                });
            }
        }
    }

    tracing::info!(
        replayed_count,
        parse_errors_count = parse_errors.len(),
        "replay request handled"
    );

    Json(ReplayResponse {
        replayed_count,
        parse_errors,
    })
    .into_response()
}

fn trim_ascii(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&b| !b.is_ascii_whitespace())
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &s[start..end]
}
