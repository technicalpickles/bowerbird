//! `bowerbird replay [<file>]` — POST JSONL of `protocol::Event` records to
//! the daemon's `POST /replay` endpoint so they flow through the pub/sub
//! path exactly as if they had arrived via the ingest socket.
//!
//! Two input modes:
//!  1. **Explicit file**: `bowerbird replay <path>` reads the file from disk.
//!  2. **Bundled fixture**: `bowerbird replay` (no arg) replays a fixture
//!     embedded at compile time via [`BUNDLED_FIXTURE`]. The embed makes the
//!     installed binary self-contained — `bowerbird replay` works on a fresh
//!     install without any source-tree access.
//!
//! See `src/commands/status.rs` for the structural reference; this command
//! follows the same "CLI hand-rolled HTTP probe with graceful degradation"
//! pattern.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::Args;
use secrecy::ExposeSecret;
use serde::Deserialize;

use super::auth;
use super::daemon::{self, ReplayResponse};

/// Bundled at compile time from `fixtures/replay-demo.jsonl` (workspace root).
/// The `CARGO_MANIFEST_DIR`-relative path resolves at compile time, so the
/// resulting binary is self-contained — no runtime file lookup needed and
/// `bowerbird replay` (no arg) works against the installed binary regardless
/// of cwd or whether the source tree is present.
const BUNDLED_FIXTURE: &[u8] = include_bytes!("../../fixtures/replay-demo.jsonl");

/// Per-attempt connect/read budget for the replay POST. A replay file with
/// thousands of events is still constrained by the daemon's 1 MiB body cap
/// per request, so the response arrives once the daemon has finished the
/// per-line parse + try_send loop — fast on loopback, but we give it
/// generous headroom so a saturated channel does not surface as a timeout.
const REPLAY_PER_ATTEMPT: Duration = Duration::from_secs(15);

#[derive(Args)]
pub struct ReplayArgs {
    /// Path to a JSONL file of protocol::Event records. Omit to use the
    /// bundled demo fixture embedded in the binary.
    pub file: Option<PathBuf>,
}

/// Response-body shape mirroring `crates/daemon/src/api/replay.rs::ReplayResponse`.
/// Duplicated here because the CLI does NOT depend on the daemon crate —
/// dragging it in would force tokio + axum into the CLI dep tree (Task 8
/// invariant).
#[derive(Debug, Deserialize)]
struct DaemonReplayResponse {
    replayed_count: usize,
    #[serde(default)]
    parse_errors: Vec<DaemonParseError>,
}

#[derive(Debug, Deserialize)]
struct DaemonParseError {
    line: usize,
    error: String,
}

/// Shallow shape used only to count sessions in the bundled fixture
/// preamble. The fixture lines are real `protocol::Event` records, but we
/// only need `source`+`session_id` here; everything else stays untouched
/// and is parsed authoritatively by the daemon.
#[derive(Debug, Deserialize)]
struct FixtureLine {
    source: String,
    session_id: String,
}

pub fn run(args: ReplayArgs) -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;

    let (body, source_label, fixture_preamble) = match args.file.as_deref() {
        Some(path) => {
            let bytes = std::fs::read(path)
                .with_context(|| format!("read replay file {}", path.display()))?;
            let label = path.display().to_string();
            (bytes, label, None)
        }
        None => {
            // Compute event count + distinct session count from the bundled
            // fixture so the preamble line gives the user a sense of what is
            // about to be replayed (AC #3).
            let (event_count, session_count) = count_fixture_shape(BUNDLED_FIXTURE);
            let preamble = format!(
                "using bundled fixture ({event_count} events across {session_count} sessions)"
            );
            (
                BUNDLED_FIXTURE.to_vec(),
                "bundled-fixture".to_string(),
                Some(preamble),
            )
        }
    };

    // Resolve daemon address.
    let info = match daemon::read_server_info(&bowerbird_dir) {
        Some(i) => i,
        None => {
            anyhow::bail!(
                "cannot reach daemon: ~/.bowerbird/server.json missing or unparseable; \
                 is the daemon running? (try 'bowerbird start')"
            );
        }
    };
    let addr: SocketAddr = info
        .bind_addr
        .parse()
        .with_context(|| format!("server.json bind_addr {:?} unparseable", info.bind_addr))?;

    // Resolve bearer token.
    let token = match auth::resolve_token_for_cli() {
        Ok((t, _source)) => t,
        Err(e) => {
            anyhow::bail!("cannot reach daemon: token resolution failed: {e}");
        }
    };

    // Emit the preamble (only for the bundled-fixture case) BEFORE the POST,
    // so a slow/unresponsive daemon does not delay user feedback about which
    // input the replay is using.
    if let Some(preamble) = fixture_preamble {
        println!("{preamble}");
    }

    let outcome = daemon::http_post_replay(addr, token.expose_secret(), &body, REPLAY_PER_ATTEMPT);

    match outcome {
        ReplayResponse::Ok(body) => {
            let resp: DaemonReplayResponse =
                serde_json::from_slice(&body).context("parse /replay response body")?;
            // STDOUT: one progress line. STDERR: per-line errors.
            println!(
                "replayed {} events from {}",
                resp.replayed_count, source_label
            );
            for pe in &resp.parse_errors {
                eprintln!("  line {}: {}", pe.line, pe.error);
            }
            Ok(())
        }
        ReplayResponse::Status(401) => {
            anyhow::bail!(
                "daemon rejected bearer token; check ~/.bowerbird/config.toml or BOWERBIRD_TOKEN"
            );
        }
        ReplayResponse::Status(code) => {
            anyhow::bail!("daemon returned HTTP {code}");
        }
        ReplayResponse::Unreachable => {
            anyhow::bail!("cannot reach daemon at {addr}; is it running? (try 'bowerbird start')");
        }
    }
}

/// Count `(event_count, distinct_session_count)` for the bundled fixture
/// preamble. Mirrors the daemon's per-line policy: blank lines and
/// `#`-comment lines are skipped; malformed lines are ignored for the
/// preamble (the authoritative validation happens at the daemon's
/// /replay handler).
fn count_fixture_shape(bytes: &[u8]) -> (usize, usize) {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    let mut events = 0usize;
    let mut sessions: HashSet<(String, String)> = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match serde_json::from_str::<FixtureLine>(trimmed) {
            Ok(f) => {
                events += 1;
                sessions.insert((f.source, f.session_id));
            }
            Err(_) => {
                // Skip malformed lines in the preamble count; the daemon
                // surfaces them in `parse_errors`. We still want a sensible
                // preamble even for a partially-broken fixture.
            }
        }
    }
    (events, sessions.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_fixture_shape_skips_blank_and_comment_lines() {
        let bytes = b"# header comment\n\n{\"source\":\"claude\",\"session_id\":\"a\"}\n{\"source\":\"claude\",\"session_id\":\"b\"}\n";
        let (events, sessions) = count_fixture_shape(bytes);
        assert_eq!(events, 2);
        assert_eq!(sessions, 2);
    }

    #[test]
    fn count_fixture_shape_counts_distinct_sessions() {
        let bytes = b"{\"source\":\"claude\",\"session_id\":\"a\"}\n{\"source\":\"claude\",\"session_id\":\"a\"}\n{\"source\":\"claude\",\"session_id\":\"b\"}\n";
        let (events, sessions) = count_fixture_shape(bytes);
        assert_eq!(events, 3);
        assert_eq!(sessions, 2);
    }
}
