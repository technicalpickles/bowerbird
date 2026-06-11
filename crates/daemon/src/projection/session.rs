use protocol::{Event, EventEnvelope, EventId, EventKind, SessionCurrentState, SessionState};
use rusqlite::OptionalExtension;

use crate::broadcast::{BroadcastEnvelope, BroadcastHub};
use crate::db::queries::{
    event_kind_as_str, event_kind_from_db_str, reaction_as_db_string, INSERT_EVENT,
    INSERT_RECORDING_SESSION_STARTED, SELECT_DISTINCT_SESSIONS_FROM_EVENTS,
    SELECT_EVENT_KINDS_FOR_SESSION, SELECT_NON_SENTINEL_SESSIONS, SELECT_SESSION_PROJECTION_STATE,
    UPDATE_RECORDING_SESSION_ENDED, UPSERT_SESSION_PROJECTION,
};
use crate::error::{Error, Result};
use crate::projection::liveness::{EndedPayload, EndedReason};
use crate::projection::state::{current_state_for_read, transition};

/// Sentinel `source`/`session_id` for daemon-emitted lifecycle events.
const DAEMON_SENTINEL_SOURCE: &str = "__daemon__";
const DAEMON_SENTINEL_SESSION: &str = "__daemon__";
const EMPTY_PAYLOAD: &str = "{}";

/// Returned by [`write_recording_started`] — the caller passes
/// [`RecordingStarted::recording_session_id`] back to
/// [`write_recording_ended`] so the right `recording_sessions` row is closed.
#[derive(Debug, Clone, Copy)]
pub struct RecordingStarted {
    pub event_id: EventId,
    pub recording_session_id: i64,
}

/// Conditional-write guard for [`write_if_state_matches`]. The synthetic
/// liveness probe's SessionEnded write is only committed if the projection
/// row's `current_state` and `last_pid` still match the snapshot the probe
/// observed — otherwise a real hook event has already moved the session and
/// the probe should yield to it (story 5.3 review finding #2).
///
/// `expected_last_event_at_ms` (Story 5.11 review finding #3) is an OPTIONAL
/// monotonic guard: when `Some(t)`, the write also requires the row's
/// `last_event_at_ms` to still equal `t`. This closes the *same-state*
/// interleaving the `current_state`+`last_pid` pair cannot see — e.g. a victim
/// that emits another event on the same PID without changing `current_state`
/// (`Working` → `Working`). Without it a stale synthetic `SessionEnded` would
/// still pass and end the session that just emitted most recently, violating
/// "whoever emitted most recently on the PID is the survivor." `None` keeps the
/// pre-5.11 behavior (the liveness probe passes `None`; only supersession
/// opts in, where the race matters and `last_event_at_ms` is in hand).
#[derive(Debug, Clone, Copy)]
pub struct WritePrecondition {
    pub expected_current_state: SessionCurrentState,
    pub expected_last_pid: Option<u32>,
    pub expected_last_event_at_ms: Option<i64>,
}

/// Sole owner of the SQLite write transaction AND the sole publisher of
/// user-facing `BroadcastEnvelope::Event` / `BroadcastEnvelope::State`
/// frames (per story 2.2; refined by story 5.2).
///
/// Inserts one row into `events` and upserts the matching row in
/// `session_projections` inside a single transaction containing exactly those
/// two writes — nothing else. After `tx.commit()` succeeds, publishes one
/// `BroadcastEnvelope::Event` followed by ZERO-OR-ONE `BroadcastEnvelope::State`
/// so any WS subscribers see the event before the resulting projection update
/// IFF the projection update actually changed `current_state` (story 5.2). A
/// first event for a previously-unknown session counts as a transition
/// (`None != Some(new_state.current_state)`) and DOES publish State.
/// Publishing is gated on commit success: if `interact` or `tx.commit` returns
/// `Err`, no envelope is published (story 2.2 AC #6).
///
/// Sentinel writes (`write_recording_started` / `write_recording_ended`) do
/// NOT publish — daemon lifecycle is excluded from the user-facing surface
/// (story 2.2 AC #7).
#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]
pub async fn write(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
) -> Result<EventId> {
    // Story 5.11 / ADR 0009: capture the fields the supersession follow-up
    // needs BEFORE `envelope` is moved into `write_committed`. A `SessionEnded`
    // event does not assert its session is the live PID holder, so it must
    // never supersede others (guards a replayed/odd `SessionEnded`).
    let successor_source = envelope.source.clone();
    let successor_session_id = envelope.session_id.clone();
    let successor_pid = envelope.pid;
    let is_session_ended = matches!(envelope.kind, EventKind::SessionEnded);

    let event_id = write_committed(writer_pool, broadcaster, envelope).await?;

    // Story 5.11 / ADR 0009 §6/§7: event-driven PID supersession. Runs ONLY on
    // LIVE ingest (this `write()` path), AFTER `write_committed` committed the
    // primary event and released its writer-pool connection. `/replay` uses
    // `write_replayed` instead and skips this step — the synthetic
    // `SessionEnded` rows are already in the log being replayed (§7). Gated on
    // the event carrying a PID and not being a `SessionEnded`. A failure here
    // is logged inside `supersede_predecessors` and NEVER propagates — `S′`'s
    // write already succeeded and its `event_id` return value must be
    // unchanged (the best-effort completion model, §6).
    if let Some(pid) = successor_pid {
        if !is_session_ended {
            supersede_predecessors(
                writer_pool,
                broadcaster,
                &successor_source,
                &successor_session_id,
                pid,
            )
            .await;
        }
    }

    Ok(event_id)
}

/// Replay-path write (Story 5.11 / ADR 0009 §7). Identical to [`write`] for the
/// primary event+projection write and broadcast, but does NOT run the
/// PID-supersession follow-up. `/replay` reconstructs events from the stored
/// log, and the synthetic `SessionEnded { reason: pid_superseded }` rows a
/// prior live run produced are already in that log — replay re-applies them
/// faithfully. Re-running supersession here would double-generate those rows
/// and, when replaying co-PID history into a live DB, could end the current
/// live PID holder on replay arrival order. See ADR 0009 §7.
#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]
pub async fn write_replayed(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
) -> Result<EventId> {
    write_committed(writer_pool, broadcaster, envelope).await
}

/// The primary write shared by [`write`] and [`write_replayed`]: commit the
/// event+projection and publish, with no precondition. `write_inner` only
/// returns `None` when a precondition is supplied, so a `None` here is a bug.
async fn write_committed(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
) -> Result<EventId> {
    write_inner(writer_pool, broadcaster, envelope, None)
        .await?
        .ok_or_else(|| {
            Error::Projection("write() with no precondition unexpectedly skipped".into())
        })
}

/// Story 5.11 / ADR 0009 — event-driven PID supersession.
///
/// Called from [`write`] (LIVE ingest only — not `/replay`, see §7) after the
/// successor `S′`'s event has committed, when that event carried `pid = Some(P)`
/// and was NOT a `SessionEnded`. Ends every OTHER non-`Ended` session still
/// claiming `P`:
/// `S′`'s event proves it is the live holder of the PID, so any predecessor
/// still on `P` is provably stale — the same observation ADR 0004 makes for
/// PID death, one step earlier in time (ADR 0009 §1).
///
/// CRITICAL placement (do not move down into `write_inner` /
/// `write_if_state_matches`): supersession runs ONLY from [`write`]. The
/// liveness probe and supersession's own synthetic `SessionEnded` writes flow
/// through [`write_if_state_matches`]; keeping supersession out of that path is
/// what makes a synthetic `SessionEnded` unable to trigger another supersession
/// pass — no recursion, no re-entrancy bookkeeping (ADR 0009 §3).
///
/// Each victim is ended through [`write_if_state_matches`] under the same
/// precondition discipline the probe uses (Story 5.2 / ADR 0009 §3, AC4): a
/// concurrent hook or probe write that moved the row makes the synthetic write
/// no-op (`Ok(None)`) rather than stomp the transition. This is a side effect
/// of an already-committed primary write — every failure is logged, never
/// propagated, and never turns `S′`'s successful write into an `Err`.
///
/// Subagent gate (ADR 0009 §"The subagent gate", AC5): a Task-tool (`Agent`)
/// subagent does NOT surface as a distinct co-PID session_id — subagent hooks
/// carry the PARENT's session_id — so the `(source, session_id) != S′`
/// exclusion below never matches the emitter, and a session never supersedes
/// itself on account of its own subagent activity.
async fn supersede_predecessors(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    successor_source: &str,
    successor_session_id: &str,
    pid: u32,
) {
    // CRITICAL: the writer pool is max_size = 1 and `write_if_state_matches`
    // checks out that same pool. Scope this read connection to JUST the SELECT
    // and drop it before the per-victim write loop — exactly as the liveness
    // probe does (liveness.rs:128) — or the loop deadlocks on its own held
    // connection.
    //
    // Perf: `last_pid` lives inside the serialized `SessionState` JSON (no SQL
    // column), so this scans + deserializes every non-sentinel row once per
    // PID-carrying forward event — O(active sessions). Acceptable for V1 (the
    // ingest path already deserializes `prev_state`); revisit with a
    // `json_extract` index only if dogfooding shows a hot-path regression
    // (ADR 0009 / story Dev Notes — a follow-up bean, not scope here).
    let rows: Vec<(String, String, String)> = {
        let conn = match writer_pool.get().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    pid,
                    "supersession: writer pool get failed; skipping (primary write already committed)"
                );
                return;
            }
        };
        // SELECT shape from queries.rs: (source, session_id, state, updated_at).
        let res = conn
            .interact(|c| -> rusqlite::Result<Vec<(String, String, String)>> {
                let mut stmt = c.prepare(SELECT_NON_SENTINEL_SESSIONS)?;
                let mapped = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?;
                mapped.collect::<rusqlite::Result<Vec<_>>>()
            })
            .await;
        match res {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                tracing::error!(error = %e, pid, "supersession: victim SELECT failed; skipping");
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, pid, "supersession: interact failed; skipping");
                return;
            }
        }
        // conn dropped here; the borrow returns to the pool before the loop.
    };

    let observed_at_ms = match current_unix_millis() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = ?e, pid, "supersession: clock read failed; skipping");
            return;
        }
    };

    let mut emitted = 0usize;
    let mut skipped_stale = 0usize;
    let mut failed = 0usize;

    for (source, session_id, state_json) in rows {
        // Never supersede the emitter — only OTHER sessions claiming P
        // (ADR 0009 §5 + the subagent gate, AC5).
        if source == successor_source && session_id == successor_session_id {
            continue;
        }
        let stored: SessionState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    source = %source,
                    session_id = %session_id,
                    "supersession: session_projections.state failed to deserialize; skipping row"
                );
                continue;
            }
        };
        // Victim = a non-`Ended` session still claiming P. An already-`Ended`
        // row is excluded (idempotence, AC2); a different PID is irrelevant.
        if stored.current_state == SessionCurrentState::Ended || stored.last_pid != Some(pid) {
            continue;
        }

        let payload = EndedPayload {
            reason: EndedReason::PidSuperseded,
            pid: Some(pid),
            observed_at_ms,
        };
        let payload_str = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(e) => {
                failed += 1;
                tracing::error!(
                    error = %e,
                    source = %source,
                    session_id = %session_id,
                    "supersession: SessionEnded payload serialize failed"
                );
                continue;
            }
        };
        // Mirror the probe's synthetic envelope (liveness.rs:204-219): the
        // envelope's `pid` is P, so last_pid carry-forward keeps the victim's
        // last_pid intact, and `cwd: None` lets carry-forward preserve its
        // last-known location.
        let envelope = EventEnvelope {
            source: source.clone(),
            session_id: session_id.clone(),
            kind: EventKind::SessionEnded,
            reaction: None,
            payload: payload_str,
            pid: Some(pid),
            notification_type: None,
            cwd: None,
        };
        // Same precondition discipline as the probe: yield to any concurrent
        // write that moved the row between the SELECT above and now (AC4).
        // Story 5.11 review finding #3: also pin `last_event_at_ms` so a
        // same-state interleave (a fresh event for this victim that kept
        // current_state + last_pid unchanged, e.g. Working→Working on P)
        // also makes the synthetic write no-op — the victim emitted more
        // recently than our scan, so it is the survivor, not a predecessor.
        let precondition = WritePrecondition {
            expected_current_state: stored.current_state,
            expected_last_pid: Some(pid),
            expected_last_event_at_ms: Some(stored.last_event_at_ms),
        };
        match write_if_state_matches(writer_pool, broadcaster, envelope, precondition).await {
            Ok(Some(_)) => {
                emitted += 1;
                tracing::info!(
                    source = %source,
                    session_id = %session_id,
                    pid,
                    "supersession: emitted SessionEnded(pid_superseded) for predecessor"
                );
            }
            Ok(None) => {
                skipped_stale += 1;
                tracing::debug!(
                    source = %source,
                    session_id = %session_id,
                    "supersession: row changed since SELECT; yielding to concurrent write"
                );
            }
            Err(e) => {
                failed += 1;
                tracing::error!(
                    error = ?e,
                    source = %source,
                    session_id = %session_id,
                    "supersession: write(SessionEnded) failed; continuing with remaining victims"
                );
            }
        }
    }

    if emitted > 0 || failed > 0 {
        tracing::info!(
            emitted,
            skipped_stale,
            failed,
            pid,
            successor_source = %successor_source,
            successor_session_id = %successor_session_id,
            "supersession: scan complete"
        );
    }
}

/// Conditional variant of [`write`] used by the liveness probe. Re-reads the
/// projection row inside the writer transaction and only commits if
/// `current_state` AND `last_pid` still match `precondition` — otherwise the
/// txn drops without committing, no broadcast is emitted, and `Ok(None)` is
/// returned. Story 5.3 review finding #2: a real hook event may land between
/// the probe's SELECT and the synthetic write; the probe must yield to it.
#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]
pub async fn write_if_state_matches(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
    precondition: WritePrecondition,
) -> Result<Option<EventId>> {
    write_inner(writer_pool, broadcaster, envelope, Some(precondition)).await
}

/// Closure return type from `write_inner`'s `interact` callback. `None`
/// means the precondition (if any) was not met — the txn dropped without
/// committing, no broadcast is owed.
type WriteInteractResult = Option<(
    i64,
    SessionState,
    Option<SessionCurrentState>,
    Option<SessionCurrentState>,
)>;

/// One row read by `rebuild_missing_projections` from
/// `SELECT_EVENT_KINDS_FOR_SESSION`: `(kind, created_at, pid, cwd, payload)`.
type RebuildEventRow = (String, i64, Option<u32>, Option<String>, String);

async fn write_inner(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
    precondition: Option<WritePrecondition>,
) -> Result<Option<EventId>> {
    // Sentinel kinds route through `write_recording_started` /
    // `write_recording_ended` and must never reach this function
    // (architecture.md:634-641). A runtime guard (not just `debug_assert!`)
    // is required so release builds also cannot publish daemon-lifecycle
    // sentinels through the user-facing broadcast path (story 2.2 AC #7).
    // Reject before pool checkout so a misuse cannot insert a row, commit
    // a transaction, or emit a broadcast envelope.
    if matches!(
        envelope.kind,
        protocol::EventKind::RecordingStarted | protocol::EventKind::RecordingEnded
    ) {
        return Err(Error::Projection(format!(
            "sentinel EventKind ({:?}) cannot be written through projection::session::write; \
             use write_recording_started / write_recording_ended",
            envelope.kind
        )));
    }

    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis()?;
    let source = envelope.source;
    let session_id = envelope.session_id;
    let kind = envelope.kind;
    let reaction = envelope.reaction;
    let payload = envelope.payload;
    let pid = envelope.pid;
    let notification_type = envelope.notification_type;
    let cwd = envelope.cwd;

    // The closure moves its captures, so duplicate the fields the post-commit
    // publish path needs. SessionState is returned out of the closure to avoid
    // recomputing `transition` against a post-commit DB (which would race with
    // a future multi-writer world even though today's single-writer pool
    // serializes writes).
    let source_for_closure = source.clone();
    let session_id_for_closure = session_id.clone();
    let kind_for_transition = kind.clone();
    let kind_str = event_kind_as_str(&kind);
    let reaction_str = reaction.as_ref().map(reaction_as_db_string);
    let payload_for_closure = payload.clone();
    let cwd_for_closure = cwd.clone();

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<WriteInteractResult> {
            let tx = c.transaction()?;

            // Read the prior state inside the transaction so a concurrent
            // writer cannot interleave between SELECT and UPSERT. Reads do
            // not break the "exactly two writes" invariant from
            // architecture.md:634-641 — the invariant is about *writes*.
            let prev_state: Option<SessionState> = tx
                .query_row(
                    SELECT_SESSION_PROJECTION_STATE,
                    rusqlite::params![&source_for_closure, &session_id_for_closure],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .and_then(|raw| match serde_json::from_str::<SessionState>(&raw) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            raw = %raw,
                            "session_projections.state failed to deserialize; \
                             treating as fresh — next event will overwrite",
                        );
                        None
                    }
                });

            // Story 5.3 review finding #2: if the caller supplied a
            // precondition (only the liveness probe does), bail before any
            // write if the row no longer matches the snapshot we observed.
            // A real hook event has already moved the session; the probe
            // yields to it. Dropping `tx` here rolls back (no rows
            // touched), so this branch leaves the DB and the broadcast
            // channel untouched.
            if let Some(pc) = precondition {
                let matches = match prev_state.as_ref() {
                    Some(s) => {
                        s.current_state == pc.expected_current_state
                            && s.last_pid == pc.expected_last_pid
                            // Story 5.11 review finding #3: optional monotonic
                            // guard. When the caller pins `last_event_at_ms`,
                            // any newer event for this row (even one that left
                            // current_state + last_pid unchanged) advances it
                            // and fails the match — so the stale synthetic
                            // write yields. `None` skips the guard (probe).
                            && pc
                                .expected_last_event_at_ms
                                .is_none_or(|t| s.last_event_at_ms == t)
                    }
                    // No prev row means the projection was deleted (or
                    // never existed) after the probe SELECT — definitely
                    // changed since snapshot. Skip.
                    None => false,
                };
                if !matches {
                    return Ok(None);
                }
            }

            // Capture prev's READ-FACING current_state BEFORE the closure
            // consumes `prev_state` into `transition` — the post-commit
            // publish path needs both prev and new current_state to decide
            // whether to emit a `BroadcastEnvelope::State` (story 5.2).
            //
            // The read-facing value (via `current_state_for_read`) folds in
            // the `STALE_WORKING_MS` fallback so a stale stored `Working`
            // that subscribers were seeing as `Idle` (via snapshot, REST,
            // or any read path) triggers a State envelope when a new event
            // restores live `Working`. Comparing raw stored states would
            // miss that transition because the stored row sat at
            // `Working` the whole time.
            let prev_raw_current_state = prev_state.as_ref().map(|s| s.current_state);
            let prev_read_current_state = prev_state
                .as_ref()
                .map(|s| current_state_for_read(s, now_ms));

            // Story 5.7 / correct-course 2026-06-02 (Option D): `started_at` is
            // set-once in `transition` (`prev.started_at.or(Some(now_ms))`). A
            // `started_at: None`-with-prior row is only reachable from a pre-5.7
            // projection blob; in a fresh v5.7+ db every session sets it on its
            // first event. bowerbird is pre-release and the documented upgrade
            // path is "nuke ~/.bowerbird/bower.db and restart," so that legacy
            // case is unsupported — no event-log backfill here. A real
            // migration-era backfill strategy lands when bowerbird ships a
            // release whose dbs must survive upgrades (deferred-work.md). See
            // docs/bmad/planning-artifacts/started-at-backfill-reconsideration-2026-06-02.md.

            let new_state = transition(
                prev_state.as_ref(),
                kind_for_transition,
                notification_type,
                pid,
                cwd_for_closure.clone(),
                now_ms,
            );
            let state_json = serde_json::to_string(&new_state).map_err(|e| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("SessionState serialize failed: {e}"),
                )))
            })?;

            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![
                    source_for_closure,
                    session_id_for_closure,
                    state_json,
                    now_ms
                ],
            )?;
            tx.execute(
                INSERT_EVENT,
                rusqlite::params![
                    source_for_closure,
                    session_id_for_closure,
                    kind_str,
                    reaction_str,
                    payload_for_closure,
                    now_ms,
                    pid,
                    cwd_for_closure,
                ],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(Some((
                id,
                new_state,
                prev_raw_current_state,
                prev_read_current_state,
            )))
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    let (event_id_raw, new_state, prev_raw_current_state, prev_read_current_state) =
        match interact_res? {
            Some(t) => t,
            None => {
                tracing::debug!(
                    "write_if_state_matches: precondition not met; skipping synthetic write"
                );
                return Ok(None);
            }
        };
    let event_id = EventId(event_id_raw);

    // Post-commit publish. Event BEFORE State so a presenter consuming both
    // topics sees the triggering event before the resulting projection
    // update. `tokio::sync::broadcast` preserves per-channel order across
    // sequential publishes, so every subscriber sees Event → State in the
    // same relative order; different subscribers may see them interleaved
    // with other publishes but never reversed within their own stream.
    let event = Event {
        event_id,
        source: source.clone(),
        session_id: session_id.clone(),
        kind,
        reaction,
        payload,
        created_at: now_ms,
        pid,
        cwd,
    };
    broadcaster.publish(BroadcastEnvelope::Event(event));

    // Story 5.2: only publish State when `current_state` actually changed.
    // First-event semantics fall out naturally — `prev_raw_current_state` is
    // `None` for a new session, `None != Some(new_state.current_state)`
    // returns true, and the State envelope publishes.
    //
    // Compare both the stored state and the read-facing state:
    // - raw catches a delayed `Stop` after stale stored `Working` (Working→Idle)
    // - read-facing catches renewed activity after stale stored `Working`
    //   (Idle-as-read→Working)
    let state_changed = prev_raw_current_state != Some(new_state.current_state)
        || prev_read_current_state != Some(new_state.current_state);
    if state_changed {
        broadcaster.publish(BroadcastEnvelope::State {
            source,
            session_id,
            state: new_state,
        });
    }
    tracing::debug!(
        event_id = event_id.0,
        state_published = state_changed,
        "ws: published event envelope; state envelope gated on transition"
    );

    Ok(Some(event_id))
}

/// Write the daemon's `RecordingStarted` sentinel atomically with the
/// `recording_sessions` row. Three writes in one transaction: projection
/// upsert, event insert, recording-session insert — a deliberate
/// exception to the two-statement rule for `write`, justified because the
/// lifecycle marker must be inseparable from the event that opened it.
pub async fn write_recording_started(
    writer_pool: &deadpool_sqlite::Pool,
) -> Result<RecordingStarted> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis()?;
    let kind_str = event_kind_as_str(&protocol::EventKind::RecordingStarted);
    // Sentinel rows have no meaningful `current_state`. The `__daemon__/__daemon__`
    // row is excluded from session-listing queries (Story 1.7), so the placeholder
    // `"{}"` is intentional and does not flow to any presenter.
    let state_json = EMPTY_PAYLOAD.to_string();
    let payload = EMPTY_PAYLOAD.to_string();

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<(i64, i64)> {
            let tx = c.transaction()?;
            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    state_json,
                    now_ms
                ],
            )?;
            tx.execute(
                INSERT_EVENT,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    kind_str,
                    None::<String>,
                    payload,
                    now_ms,
                    None::<u32>,
                    None::<String>,
                ],
            )?;
            let event_id = tx.last_insert_rowid();
            tx.execute(
                INSERT_RECORDING_SESSION_STARTED,
                rusqlite::params![event_id],
            )?;
            let recording_session_id = tx.last_insert_rowid();
            tx.commit()?;
            Ok((event_id, recording_session_id))
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    let (event_id, recording_session_id) = interact_res?;
    Ok(RecordingStarted {
        event_id: EventId(event_id),
        recording_session_id,
    })
}

/// Close the `recording_sessions` row identified by `recording_session_id`
/// atomically with the `RecordingEnded` sentinel event. Three writes in one
/// transaction (same exception as [`write_recording_started`]).
pub async fn write_recording_ended(
    writer_pool: &deadpool_sqlite::Pool,
    recording_session_id: i64,
) -> Result<EventId> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis()?;
    let kind_str = event_kind_as_str(&protocol::EventKind::RecordingEnded);
    // Sentinel rows have no meaningful `current_state` — see
    // `write_recording_started` for rationale.
    let state_json = EMPTY_PAYLOAD.to_string();
    let payload = EMPTY_PAYLOAD.to_string();

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<i64> {
            let tx = c.transaction()?;
            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    state_json,
                    now_ms
                ],
            )?;
            tx.execute(
                INSERT_EVENT,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    kind_str,
                    None::<String>,
                    payload,
                    now_ms,
                    None::<u32>,
                    None::<String>,
                ],
            )?;
            let event_id = tx.last_insert_rowid();
            let rows = tx.execute(
                UPDATE_RECORDING_SESSION_ENDED,
                rusqlite::params![event_id, recording_session_id],
            )?;
            if rows != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            tx.commit()?;
            Ok(event_id)
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    let event_id = interact_res?;
    Ok(EventId(event_id))
}

/// Replay the event log to rebuild any missing `session_projections` rows.
///
/// Iterates distinct non-sentinel `(source, session_id)` pairs in `events`,
/// rebuilds only the ones that have no projection row, and UPSERTs the
/// derived [`SessionState`] computed by folding [`transition`] over the
/// event stream. Returns the number of projections rebuilt.
///
/// Best-effort: a per-session failure is logged but does not abort the
/// remaining rebuild. Callers should treat a `Err` only as a data-correctness
/// warning, never as a startup blocker (see story 1.6 Task 6 rationale).
///
/// Runs in a single transaction so a partial crash mid-rebuild does not
/// leave a half-populated projection table.
#[tracing::instrument(skip_all)]
pub async fn rebuild_missing_projections(writer_pool: &deadpool_sqlite::Pool) -> Result<usize> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let rebuilt = conn
        .interact(move |c| -> rusqlite::Result<usize> {
            let tx = c.transaction()?;

            let pairs: Vec<(String, String)> = {
                let mut stmt = tx.prepare(SELECT_DISTINCT_SESSIONS_FROM_EVENTS)?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            let mut rebuilt = 0usize;
            for (source, session_id) in pairs {
                let existing: Option<String> = tx
                    .query_row(
                        SELECT_SESSION_PROJECTION_STATE,
                        rusqlite::params![&source, &session_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if existing.is_some() {
                    continue;
                }

                // Story 5.3 / 5.7: SELECT returns (kind, created_at, pid, cwd,
                // payload) so rebuild threads last_pid + cwd carry-forward AND
                // parses notification_type from Notification payloads (the
                // typed value lives in events.payload, not on the stored
                // Event). `started_at` needs no extra column — the set-once
                // rule in `transition` derives it from each event's stored
                // `created_at` (passed as `now_ms`), so the first replayed
                // event sets it and the rest preserve it.
                let kinds: Vec<RebuildEventRow> = {
                    let mut stmt = tx.prepare(SELECT_EVENT_KINDS_FOR_SESSION)?;
                    let rows = stmt.query_map(rusqlite::params![&source, &session_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Option<u32>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    })?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };

                let mut state: Option<SessionState> = None;
                let mut last_created_at: i64 = 0;
                let mut bad_kind = false;
                for (kind_str, created_at, pid, cwd, payload) in kinds {
                    last_created_at = created_at;
                    let kind = match event_kind_from_db_str(&kind_str) {
                        Ok(k) => k,
                        Err(e) => {
                            tracing::error!(
                                source = %source,
                                session_id = %session_id,
                                kind = %kind_str,
                                error = %e,
                                "rebuild_missing_projections: unknown EventKind in events.kind; \
                                 skipping this session"
                            );
                            bad_kind = true;
                            break;
                        }
                    };
                    // Parse notification_type from payload only for Notification
                    // events — the same logic the adapter applies at ingest
                    // (Task 3) but here we read from stored payload at rebuild.
                    let notification_type = if matches!(kind, protocol::EventKind::Notification) {
                        serde_json::from_str::<serde_json::Value>(&payload)
                            .ok()
                            .as_ref()
                            .and_then(|v| v.get("notification_type"))
                            .and_then(|v| v.as_str())
                            .map(|s| match s {
                                "permission_prompt" => protocol::NotificationType::PermissionPrompt,
                                "idle_prompt" => protocol::NotificationType::IdlePrompt,
                                "auth_success" => protocol::NotificationType::AuthSuccess,
                                "elicitation_dialog" => {
                                    protocol::NotificationType::ElicitationDialog
                                }
                                "elicitation_response" => {
                                    protocol::NotificationType::ElicitationResponse
                                }
                                "elicitation_complete" => {
                                    protocol::NotificationType::ElicitationComplete
                                }
                                _ => protocol::NotificationType::Unknown,
                            })
                    } else {
                        None
                    };
                    // Sentinel kinds should be filtered by the source != '__daemon__' clause
                    // on SELECT_DISTINCT_SESSIONS_FROM_EVENTS, but defend against future shape
                    // changes by routing them through transition's defensive branch anyway.
                    let next = transition(
                        state.as_ref(),
                        kind,
                        notification_type,
                        pid,
                        cwd,
                        created_at,
                    );
                    state = Some(next);
                }
                if bad_kind {
                    continue;
                }
                let Some(state) = state else { continue };
                let state_json = match serde_json::to_string(&state) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(
                            source = %source,
                            session_id = %session_id,
                            error = %e,
                            "rebuild_missing_projections: SessionState serialize failed"
                        );
                        continue;
                    }
                };
                tx.execute(
                    UPSERT_SESSION_PROJECTION,
                    rusqlite::params![&source, &session_id, &state_json, last_created_at],
                )?;
                rebuilt += 1;
                tracing::info!(
                    source = %source,
                    session_id = %session_id,
                    "rebuilt session projection"
                );
            }

            tx.commit()?;
            Ok(rebuilt)
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))??;

    Ok(rebuilt)
}

use crate::time::current_unix_millis;
