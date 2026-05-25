//! `bowerbird export <session-id> [-o <path>]` — fetch a session's event
//! history from the daemon and emit it as JSONL of `protocol::Event` records.
//!
//! The output shape is the input shape for `bowerbird replay`, so
//! `bowerbird export <id> | bowerbird replay /dev/stdin` round-trips a
//! session through the pub/sub path on the same daemon (or, after
//! `bowerbird export <id> > session.jsonl`, on a different machine after a
//! fresh `bowerbird install`).
//!
//! Output discipline:
//! - **No `-o`**: JSONL goes to stdout, success summary goes to stderr.
//! - **With `-o`**: JSONL goes to the file (truncating it), success summary
//!   goes to stderr.
//!
//! Stderr-for-summary keeps stdout pipe-safe: `bowerbird export <id> |
//! bowerbird replay /dev/stdin` doesn't get its data contaminated by a
//! diagnostic banner.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::Args;
use protocol::EventListResponse;
use secrecy::ExposeSecret;

use super::auth;
use super::daemon::{self, EventsResponse};

const EXPORT_PER_ATTEMPT: Duration = Duration::from_secs(15);

#[derive(Args)]
pub struct ExportArgs {
    /// The session_id to export. Matches the `session_id` column the daemon
    /// returns via `/sessions/{id}/events`. Source is `claude` in V1; multi-
    /// source disambiguation arrives when a second adapter ships per the
    /// Story 1.7 deferred-work entry.
    pub session_id: String,

    /// Output path. Default: stdout (so the JSONL can be piped into
    /// `bowerbird replay /dev/stdin`).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

pub fn run(args: ExportArgs) -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;

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

    let token = match auth::resolve_token_for_cli() {
        Ok((t, _source)) => t,
        Err(e) => {
            anyhow::bail!("cannot reach daemon: token resolution failed: {e}");
        }
    };

    // Pre-check: the events endpoint returns 200 with an empty list for an
    // unknown session_id (it just queries the events table by session_id).
    // The 404 distinction lives at `GET /sessions/{id}` (session_projections
    // lookup). Probe that first so AC #2's "session not found" stderr is
    // unambiguous; a real session with 0 events is structurally impossible
    // (session_projections rows are created at first-event-write time).
    match daemon::http_get_session_detail(
        addr,
        token.expose_secret(),
        &args.session_id,
        EXPORT_PER_ATTEMPT,
    ) {
        daemon::SessionDetailResponse::Ok => {}
        daemon::SessionDetailResponse::NotFound => {
            anyhow::bail!("session {} not found", args.session_id);
        }
        daemon::SessionDetailResponse::Status(401) => {
            anyhow::bail!(
                "daemon rejected bearer token; check ~/.bowerbird/config.toml or BOWERBIRD_TOKEN"
            );
        }
        daemon::SessionDetailResponse::Status(code) => {
            anyhow::bail!("daemon returned HTTP {code}");
        }
        daemon::SessionDetailResponse::Unreachable => {
            anyhow::bail!("cannot reach daemon at {addr}; is it running? (try 'bowerbird start')");
        }
    }

    // Output sink. BufWriter keeps the per-event write from syscalling.
    let (mut sink, output_label): (BufWriter<Box<dyn Write>>, String) = match args.output.as_deref()
    {
        Some(path) => {
            let f = File::create(path)
                .with_context(|| format!("create output file {}", path.display()))?;
            (BufWriter::new(Box::new(f)), path.display().to_string())
        }
        None => (
            BufWriter::new(Box::new(std::io::stdout().lock())),
            "stdout".to_string(),
        ),
    };

    let mut since: i64 = 0;
    let mut total_events: usize = 0;

    loop {
        let response = daemon::http_get_events(
            addr,
            token.expose_secret(),
            &args.session_id,
            since,
            EXPORT_PER_ATTEMPT,
        );
        let body_bytes = match response {
            EventsResponse::Ok(b) => b,
            EventsResponse::NotFound => {
                anyhow::bail!("session {} not found", args.session_id);
            }
            EventsResponse::Status(401) => {
                anyhow::bail!(
                    "daemon rejected bearer token; check ~/.bowerbird/config.toml or BOWERBIRD_TOKEN"
                );
            }
            EventsResponse::Status(code) => {
                anyhow::bail!("daemon returned HTTP {code}");
            }
            EventsResponse::Unreachable => {
                anyhow::bail!(
                    "cannot reach daemon at {addr}; is it running? (try 'bowerbird start')"
                );
            }
        };

        let page: EventListResponse =
            serde_json::from_slice(&body_bytes).context("parse /sessions/{id}/events response")?;

        for event in &page.events {
            // Per-event serialize. Each line is a complete JSON document.
            let line = serde_json::to_string(event).context("serialize Event to JSONL line")?;
            sink.write_all(line.as_bytes())
                .context("write event line to output sink")?;
            sink.write_all(b"\n")
                .context("write newline to output sink")?;
        }
        total_events += page.events.len();

        // Cursor semantics: `Some(last_event_id)` when non-empty, `None`
        // when no more events. Loop terminates on `None`.
        match page.cursor {
            Some(cursor) => {
                let new_since = cursor.0;
                if new_since <= since {
                    // Defensive: a buggy daemon that returns a non-increasing
                    // cursor would loop forever. Bail loudly rather than spin.
                    anyhow::bail!(
                        "daemon returned non-increasing cursor ({} -> {}); export would loop",
                        since,
                        new_since
                    );
                }
                since = new_since;
            }
            None => break,
        }
    }

    sink.flush().context("flush output sink")?;
    drop(sink); // close file before printing summary

    // Summary to STDERR so stdout stays pipe-safe.
    eprintln!(
        "exported {total_events} events from session {} to {output_label}",
        args.session_id
    );
    Ok(())
}
