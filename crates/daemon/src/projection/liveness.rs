//! Story 5.3 — Daemon-observed session liveness probe.
//!
//! Iterates non-sentinel `session_projections` rows on a 5s cadence (and once
//! synchronously at daemon startup). For each row, checks whether the carried
//! `last_pid` is still a live OS process via `kill(pid, 0)`. Dead or never-
//! captured PIDs trigger a synthetic `SessionEnded` event written through the
//! normal `projection::session::write` path — preserving the "exactly two
//! writes per event" invariant (architecture.md §634-641) and the post-commit
//! broadcast envelope semantics from Story 5.2.
//!
//! Per Axiom 4 ("Mechanical facts in the protocol; semantics in the
//! presenter") the probe emits a *mechanical observation* — "this PID is no
//! longer signalable" — and lets each presenter decide how to render the
//! resulting `Ended` state. `Ended` is non-terminal: the next hook event
//! transitions out via `transition()`'s normal arms (e.g. a `UserPromptSubmit`
//! from `claude --resume` → `Working`).

use std::sync::Arc;
use std::time::Duration;

use deadpool_sqlite::Pool;
use protocol::{EventEnvelope, EventKind, SessionCurrentState, SessionState};
use serde::Serialize;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::broadcast::BroadcastHub;
use crate::db::queries::SELECT_NON_SENTINEL_SESSIONS;
use crate::error::{Error, Result};
use crate::projection::session::write;
use crate::time::current_unix_millis;

/// Probe cadence — see ADR 0004 §"Cadence shorter/longer than 5s" for rationale.
pub(crate) const PROBE_CADENCE: Duration = Duration::from_secs(5);

/// Per-row reason carried in the `SessionEnded` event payload.
///
/// The wire payload is mechanical fact only: which observation triggered the
/// emission. Presenter-side interpretation (e.g. "hide ended sessions" vs
/// "dim them") lives in the presenter, per Axiom 1.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum EndedReason {
    /// `last_pid IS NULL` at probe time. Either pre-migration legacy row OR a
    /// session that ingested before Story 5.3 shipped (the `bowerbird_ppid`
    /// injection didn't yet exist).
    NoPidAtUpgrade,
    /// `kill(last_pid, 0)` returned ESRCH — the OS confirms the process is
    /// gone.
    PidDead,
}

#[derive(Debug, Serialize)]
struct EndedPayload {
    reason: EndedReason,
    pid: Option<u32>,
    observed_at_ms: i64,
}

/// `kill(pid, 0)` does not send a signal — it returns 0 if the process exists
/// and the caller can signal it; -1 with errno otherwise. `ESRCH` means the
/// process is gone (the probe's "dead" trigger). `EPERM` means the process
/// exists but we can't signal it — treated as alive (defensive: if we can't
/// tell, don't kill the session).
fn is_pid_alive(pid: u32) -> bool {
    use std::convert::TryFrom;
    let pid_i32 = match i32::try_from(pid) {
        Ok(v) => v,
        Err(_) => return false, // out-of-range PID — treat as dead
    };
    // SAFETY: libc::kill is signal-safe and the (pid, 0) form is a pure
    // existence probe — no signal is delivered. We only inspect errno on
    // failure.
    #[allow(unsafe_code)]
    let r = unsafe { libc::kill(pid_i32, 0) };
    if r == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // EPERM = exists but we can't signal it → treat as alive.
    errno == libc::EPERM
}

/// One probe iteration. Reads every non-sentinel `session_projections` row;
/// for each row not already `Ended`, decides liveness from `last_pid`:
///   - `None`      → emit `SessionEnded` with `reason: no_pid_at_upgrade`
///   - `Some(pid)` AND `!is_pid_alive(pid)` → emit `SessionEnded` with `reason: pid_dead`
///   - `Some(pid)` AND `is_pid_alive(pid)`  → skip
///
/// Emissions route through `projection::session::write`, so the resulting
/// state transition is broadcast by Story 5.2's gating logic naturally.
/// Returns the number of `SessionEnded` events emitted.
#[tracing::instrument(skip_all)]
pub async fn probe_once(writer_pool: &Pool, broadcaster: &BroadcastHub) -> Result<usize> {
    // CRITICAL: do NOT hold a writer-pool connection across the per-row
    // `write()` calls below — the writer pool has max_size = 1, and `write()`
    // checks out the same pool. Holding the read connection here would
    // deadlock the loop. Scope the connection checkout to just the SELECT.
    let rows: Vec<(String, String, String)> = {
        let conn = writer_pool
            .get()
            .await
            .map_err(|e| Error::Pool(format!("writer pool get failed for liveness probe: {e}")))?;
        // Read non-sentinel session rows. Sentinel filtering matches the rest
        // of the daemon's session-listing queries — the `__daemon__/__daemon__`
        // row has no PID and no current_state to maintain.
        conn.interact(|c| -> rusqlite::Result<Vec<(String, String, String)>> {
            let mut stmt = c.prepare(SELECT_NON_SENTINEL_SESSIONS)?;
            // SELECT shape from queries.rs: (source, session_id, state, updated_at)
            let mapped = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed for liveness probe: {e}")))?
        .map_err(Error::Sqlite)?
        // conn dropped here; the borrow returns to the pool.
    };

    let observed_at_ms = current_unix_millis()?;
    let mut emitted = 0usize;

    for (source, session_id, state_json) in rows {
        let stored: SessionState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    source = %source,
                    session_id = %session_id,
                    "liveness probe: session_projections.state failed to deserialize; skipping row"
                );
                continue;
            }
        };
        if stored.current_state == SessionCurrentState::Ended {
            // Already Ended — do not re-emit. A resume hook event drives the
            // transition out via projection::session::write's normal path.
            continue;
        }
        let reason = match stored.last_pid {
            None => EndedReason::NoPidAtUpgrade,
            Some(pid) if !is_pid_alive(pid) => EndedReason::PidDead,
            Some(_) => continue,
        };
        let payload = EndedPayload {
            reason,
            pid: stored.last_pid,
            observed_at_ms,
        };
        let payload_str = serde_json::to_string(&payload).map_err(|e| {
            Error::Projection(format!("SessionEnded payload serialize failed: {e}"))
        })?;
        let envelope = EventEnvelope {
            source: source.clone(),
            session_id: session_id.clone(),
            kind: EventKind::SessionEnded,
            reaction: None,
            payload: payload_str,
            // The synthetic envelope's `pid` is the dead PID we observed —
            // last_pid carry-forward keeps the projection's value intact,
            // matching the prior row.
            pid: stored.last_pid,
            notification_type: None,
        };
        match write(writer_pool, broadcaster, envelope).await {
            Ok(_) => {
                emitted += 1;
                tracing::info!(
                    source = %source,
                    session_id = %session_id,
                    reason = ?reason,
                    pid = ?stored.last_pid,
                    "liveness probe: emitted SessionEnded"
                );
            }
            Err(e) => {
                tracing::error!(
                    error = ?e,
                    source = %source,
                    session_id = %session_id,
                    "liveness probe: write(SessionEnded) failed; continuing with remaining rows"
                );
            }
        }
    }
    Ok(emitted)
}

/// Periodic probe loop. `MissedTickBehavior::Skip` so a slow iteration does
/// not queue catch-up ticks — see ADR 0004 §"Cadence shorter/longer than 5s."
/// Exits on shutdown cancellation.
pub async fn run(writer_pool: Pool, broadcaster: Arc<BroadcastHub>, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(PROBE_CADENCE);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // The first tick fires immediately. The startup synchronous probe in
    // main.rs has already covered t=0, so skip the immediate fire.
    interval.tick().await;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("liveness probe: shutdown requested; exiting");
                return;
            }
            _ = interval.tick() => {
                if let Err(e) = probe_once(&writer_pool, &broadcaster).await {
                    tracing::error!(error = ?e, "liveness probe: iteration failed");
                }
            }
        }
    }
}
