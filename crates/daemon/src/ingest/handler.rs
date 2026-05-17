use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use adapter_claude::ClaudeAdapter;
use protocol::{EventEnvelope, SourceAdapter};

#[tracing::instrument(skip_all)]
pub(super) async fn handle(
    stream: UnixStream,
    tx: mpsc::Sender<EventEnvelope>,
    adapter: Arc<ClaudeAdapter>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
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

    let trimmed = buf.trim_end_matches('\n');

    let value = match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = ?e, "ingest: invalid JSON");
            let _ = write_half
                .write_all(format!("400 invalid JSON: {e}\n").as_bytes())
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

    let hook_kind = value
        .get("hook_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("PreToolUse");

    let envelope = match adapter.normalize(hook_kind, trimmed.as_bytes()) {
        Ok(result) => result.envelope,
        Err(e) => {
            tracing::debug!(error = ?e, "ingest: normalize failed");
            let _ = write_half
                .write_all(format!("400 normalize error: {e}\n").as_bytes())
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
