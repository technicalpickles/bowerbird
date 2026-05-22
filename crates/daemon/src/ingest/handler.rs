use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use adapter_claude::ClaudeAdapter;
use protocol::{EventEnvelope, SourceAdapter};

/// Maximum bytes accepted for a single newline-terminated ingest line on
/// the daemon's local socket, excluding the trailing newline. Larger
/// than `protocol::SHIM_STDIN_CAP_BYTES` by 4 KiB so the shim's
/// `hook_kind` injection (`,"hook_kind":"<value>"` adds ~30 bytes),
/// JSON whitespace round-trip (`serde_json::to_vec` is compact but may
/// differ from the stdin form), and the trailing newline all fit. A
/// shim-accepted stdin payload near the 1 MiB cap must not be rejected
/// on the daemon side.
///
/// The shim's raw stdin cap is canonical in `protocol::
/// SHIM_STDIN_CAP_BYTES`; both crates import it from there so they
/// cannot drift independently.
///
/// A direct local-socket client can still push more bytes than the shim
/// ever would; this cap rejects those before JSON parse, enqueue, DB
/// write, or broadcast fan-out — see [`handle`].
const MAX_INGEST_LINE_BYTES: usize = protocol::SHIM_STDIN_CAP_BYTES + 4096;

// Wire 400 responses are line-framed. Collapse internal newlines and cap
// length so a multi-line serde_json::Error Display can't desync the client.
fn sanitize_for_wire(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .take(512)
        .collect()
}

#[tracing::instrument(skip_all)]
pub(super) async fn handle(
    stream: UnixStream,
    tx: mpsc::Sender<EventEnvelope>,
    adapter: Arc<ClaudeAdapter>,
) {
    let (read_half, mut write_half) = stream.into_split();
    // Cap the read at MAX + 1 bytes so a payload of exactly the cap plus
    // its terminating newline still fits, but anything larger stops short
    // of the next `\n` and is detectable by `buf.ends_with('\n') == false`.
    // Mirrors the shim's `stdin.lock().take((MAX + 1) as u64)` pattern,
    // but with [`MAX_INGEST_LINE_BYTES`] (wire cap with shim overhead),
    // not the shim's stdin cap.
    let limited = read_half.take(MAX_INGEST_LINE_BYTES as u64 + 1);
    let mut reader = BufReader::new(limited);
    let mut buf = String::new();

    match reader.read_line(&mut buf).await {
        Ok(0) => {
            tracing::debug!("ingest: EOF before newline (client disconnected)");
            return;
        }
        Err(e) => {
            tracing::debug!(error = ?e, "ingest: read_line error");
            return;
        }
        Ok(_) => {}
    }

    // If the read stopped at the take limit without finding a `\n`, the
    // line is over the 1 MiB cap. Reject before JSON parse, enqueue, DB
    // write, or broadcast — a direct socket writer could otherwise bypass
    // the shim's matching cap and push an oversized payload into the
    // broadcast ring.
    if buf.len() > MAX_INGEST_LINE_BYTES && !buf.ends_with('\n') {
        tracing::debug!(
            len = buf.len(),
            cap = MAX_INGEST_LINE_BYTES,
            "ingest: line exceeded max cap"
        );
        let _ = write_half
            .write_all(format!("400 line exceeds {MAX_INGEST_LINE_BYTES}-byte cap\n").as_bytes())
            .await;
        let _ = write_half.flush().await;
        return;
    }

    let trimmed = buf.trim_end_matches('\n');

    let value = match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = ?e, "ingest: invalid JSON");
            let sanitized = sanitize_for_wire(&e.to_string());
            let _ = write_half
                .write_all(format!("400 invalid JSON: {sanitized}\n").as_bytes())
                .await;
            let _ = write_half.flush().await;
            return;
        }
    };

    if !value.is_object() {
        tracing::debug!("ingest: payload is not a JSON object");
        let _ = write_half.write_all(b"400 expected JSON object\n").await;
        let _ = write_half.flush().await;
        return;
    }

    // Story 1.8: hook_kind is required. The shim injects it via --hook-kind on every
    // payload. Absence or wrong JSON type (e.g. number, null) is malformed input — the
    // `as_str()` chain coalesces both into `None`, which we intentionally treat as the
    // same "missing" wire response. See ADR-0002 §Consequences and AC #1/#3.
    let hook_kind = match value.get("hook_kind").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            tracing::debug!("ingest: missing hook_kind");
            let _ = write_half.write_all(b"400 missing hook_kind\n").await;
            let _ = write_half.flush().await;
            return;
        }
    };

    let envelope = match adapter.normalize(hook_kind, trimmed.as_bytes()) {
        Ok(result) => result.envelope,
        Err(protocol::Error::UnknownHookKind(k)) => {
            tracing::debug!(hook_kind = %k, "ingest: unknown hook_kind");
            // Echo the user-supplied bogus kind (not the formatted error message)
            // through sanitize_for_wire to preserve the one-line wire invariant.
            let sanitized = sanitize_for_wire(&k);
            let _ = write_half
                .write_all(format!("400 unknown hook_kind: {sanitized}\n").as_bytes())
                .await;
            let _ = write_half.flush().await;
            return;
        }
        Err(e) => {
            tracing::debug!(error = ?e, "ingest: normalize failed");
            let sanitized = sanitize_for_wire(&e.to_string());
            let _ = write_half
                .write_all(format!("400 normalize error: {sanitized}\n").as_bytes())
                .await;
            let _ = write_half.flush().await;
            return;
        }
    };

    if envelope.session_id.trim().is_empty() {
        tracing::debug!("ingest: session_id is empty");
        let _ = write_half
            .write_all(b"400 session_id must not be empty\n")
            .await;
        let _ = write_half.flush().await;
        return;
    }

    if envelope.payload.contains('\0') {
        tracing::debug!("ingest: payload contains null bytes");
        let _ = write_half
            .write_all(b"400 payload must not contain null bytes\n")
            .await;
        let _ = write_half.flush().await;
        return;
    }

    match tx.try_send(envelope) {
        Ok(()) => {
            tracing::debug!("ingest: 200 accepted");
            let _ = write_half.write_all(b"200\n").await;
            let _ = write_half.flush().await;
        }
        Err(_) => {
            tracing::debug!("ingest: 503 queue full or closed");
            let _ = write_half.write_all(b"503\n").await;
            let _ = write_half.flush().await;
        }
    }
}
