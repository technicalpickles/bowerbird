//! On-subscribe state snapshot for the WS surface (Story 2.3).
//!
//! `snapshot_for_topic` reads `session_projections` and returns one
//! `StateFrame` per matching session that is NOT already covered by a
//! pre-existing subscription. Called from the `ClientMessage::Subscribe`
//! arm of `api::ws::handle_text_frame` so a tool that connects to an
//! already-running daemon sees the existing world before any live event.
//!
//! Snapshot is state-only: subscriptions to `events.*` topics return an
//! empty vec. Event history is fetched via REST
//! `/sessions/:id/events?since=0` (Story 1.7); the live event stream
//! picks up at subscribe-time.
//!
//! The helper synthesizes a local `BroadcastEnvelope::State` for each
//! row purely to evaluate `Topic::matches` against the new topic and the
//! pre-existing subscription set. The synthetic envelope is never
//! published to the broadcast hub — that would re-fan-out the snapshot
//! to every connected subscriber and burn the hub capacity.

use std::collections::HashSet;

use protocol::{SessionCurrentState, SessionState, StateFrame};

use crate::api::filter::state_matches;
use crate::broadcast::{BroadcastEnvelope, Topic};
use crate::db::queries::SELECT_NON_SENTINEL_SESSIONS;
use crate::error::{Error, Result};
use crate::projection::state::current_state_for_read;

/// Build the on-subscribe snapshot of `StateFrame`s for `new_topic`.
///
/// Returns an empty vec when `new_topic` is not a state topic, when the
/// projection table has no non-sentinel rows, or when every matching
/// session was already snapshot-delivered on this connection.
/// `already_snapshotted` is the set of `(source, session_id)` natural keys
/// (project-context "Substrate-not-actor invariants") this connection has
/// already emitted a snapshot frame for; a row whose key is in the set is
/// skipped so the connection never double-delivers a snapshot
/// (`docs/protocol.md` idempotence promise). Keying on the delivered key
/// rather than on the subscribed topic is what lets a filtered subscribe
/// (`state_filter` non-empty, which sent only a subset of its topic's rows)
/// coexist with a later wider subscribe: the wider burst re-sends only the
/// keys the narrow one never covered (Story 5.8, ADR 0008 finding). The
/// caller prunes this set on `Unsubscribe`, so coverage lapses when no
/// active subscription tracks the session and a re-subscribe re-snapshots.
///
/// `state_filter` (Story 5.8, ADR 0008) scopes the burst by the presenter's
/// requested `SessionCurrentState` set, keyed on the read-derived
/// `current_state`. An empty slice matches everything (the v1.0 default).
///
/// Errors propagate reader-pool checkout and `interact` failures. The
/// caller in `handle_text_frame` logs and proceeds with an empty
/// snapshot — a transient DB issue must not close a healthy WS connection.
#[tracing::instrument(skip_all, fields(new_topic = ?new_topic, now_ms))]
pub async fn snapshot_for_topic(
    reader_pool: &deadpool_sqlite::Pool,
    new_topic: &Topic,
    already_snapshotted: &HashSet<(String, String)>,
    now_ms: i64,
    state_filter: &[SessionCurrentState],
) -> Result<Vec<StateFrame>> {
    if !new_topic.is_state_session_family() {
        return Ok(Vec::new());
    }

    // No topic-based pre-query short-circuit: snapshot coverage is tracked
    // per `(source, session_id)` key, not per topic (a topic set cannot
    // express a filtered subscribe's partial coverage). An idempotent or
    // already-covered re-subscribe still reads the rows and dedups them in
    // Rust below — at projection scale (one row per session, ~1MB) that
    // cold read is immaterial, the same tradeoff ADR 0008 accepted for
    // Rust-side `?state=` filtering.

    let conn = reader_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("reader pool get failed: {e}")))?;

    // `updated_at` (column 3) is intentionally dropped — the `StateFrame`
    // wire shape carries `last_event_at_ms` from the deserialized
    // `SessionState`, and the SQL `ORDER BY updated_at DESC, source ASC,
    // session_id ASC` already imposes the row order Task 2 emits.
    let rows = conn
        .interact(|c| -> rusqlite::Result<Vec<(String, String, String)>> {
            let mut stmt = c.prepare(SELECT_NON_SENTINEL_SESSIONS)?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            rows.collect()
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))??;

    let mut out = Vec::with_capacity(rows.len());
    for (source, session_id, state_json) in rows {
        // Per-key snapshot dedup: a session already snapshot-delivered on
        // this connection (under any prior subscribe, filtered or not) is
        // not re-sent — the no-double-delivery contract (protocol.md),
        // keyed on the `(source, session_id)` natural key the StateFrame
        // carries. Checked before the JSON parse so a covered row costs
        // nothing.
        if already_snapshotted.contains(&(source.clone(), session_id.clone())) {
            continue;
        }

        // Skip rows with corrupt JSON rather than 500-ing the whole
        // snapshot — mirrors `api/sessions.rs::list` discipline.
        let stored: SessionState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    source = %source,
                    session_id = %session_id,
                    "snapshot_for_topic: skipping session row with unparseable state JSON"
                );
                continue;
            }
        };

        let synth = BroadcastEnvelope::State {
            source: source.clone(),
            session_id: session_id.clone(),
            state: stored.clone(),
        };

        if !new_topic.matches(&synth) {
            continue;
        }

        let derived_current = current_state_for_read(&stored, now_ms);
        // Story 5.8 (ADR 0008): scope the snapshot burst by the presenter's
        // `states` filter, keyed on the read-derived `current_state` (matching
        // the REST `?state=` semantics). Empty filter = match all (the v1.0
        // default). Only the initial snapshot is scoped — the live stream is
        // untouched.
        if !state_matches(state_filter, derived_current) {
            continue;
        }
        out.push(StateFrame {
            source,
            session_id,
            state: SessionState {
                current_state: derived_current,
                last_event_kind: stored.last_event_kind,
                last_event_at_ms: stored.last_event_at_ms,
                last_pid: stored.last_pid,
                cwd: stored.cwd,
                started_at: stored.started_at,
            },
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::UPSERT_SESSION_PROJECTION;
    use crate::db::{init_pools, run_migrations};
    use protocol::{EventKind, SessionCurrentState, SessionState};
    use std::collections::HashSet;
    use tempfile::TempDir;

    async fn fresh_db() -> (TempDir, deadpool_sqlite::Pool) {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");
        (tmp, pools.reader)
    }

    async fn upsert_session(
        writer_pool: &deadpool_sqlite::Pool,
        source: &str,
        session_id: &str,
        state: &SessionState,
        updated_at: i64,
    ) {
        let conn = writer_pool.get().await.expect("writer get");
        let state_json = serde_json::to_string(state).expect("serialize state");
        let source = source.to_string();
        let session_id = session_id.to_string();
        conn.interact(move |c| -> rusqlite::Result<()> {
            c.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![source, session_id, state_json, updated_at],
            )?;
            Ok(())
        })
        .await
        .expect("interact")
        .expect("upsert");
    }

    fn working_state(now_ms: i64) -> SessionState {
        SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: now_ms,
            last_pid: None,
            cwd: None,
            started_at: None,
        }
    }

    fn state_with(cs: SessionCurrentState, last_event_at_ms: i64) -> SessionState {
        SessionState {
            current_state: cs,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms,
            last_pid: None,
            cwd: None,
            started_at: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_projection_table_returns_empty_vec() {
        let (tmp, reader) = fresh_db().await;
        let frames = snapshot_for_topic(&reader, &Topic::StateAll, &HashSet::new(), 1_000, &[])
            .await
            .expect("snapshot ok");
        assert!(frames.is_empty());
        drop(tmp);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn state_all_returns_one_frame_per_session() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-B",
            &working_state(2_000),
            2_000,
        )
        .await;

        let frames =
            snapshot_for_topic(&pools.reader, &Topic::StateAll, &HashSet::new(), 3_000, &[])
                .await
                .expect("snapshot ok");
        let ids: Vec<String> = frames.iter().map(|f| f.session_id.clone()).collect();
        assert_eq!(ids.len(), 2);
        // updated_at DESC — sess-B was updated more recently.
        assert_eq!(ids, vec!["sess-B".to_string(), "sess-A".to_string()]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn state_session_filters_to_matching_id() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-B",
            &working_state(2_000),
            2_000,
        )
        .await;

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSession("sess-A".to_string()),
            &HashSet::new(),
            3_000,
            &[],
        )
        .await
        .expect("snapshot ok");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].session_id, "sess-A");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn state_session_no_match_returns_empty() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSession("sess-other".to_string()),
            &HashSet::new(),
            3_000,
            &[],
        )
        .await
        .expect("snapshot ok");
        assert!(frames.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn state_current_subscription_emits_full_frame() {
        // Story 2.1 deliberately does not project a smaller frame for
        // `.current_state`; AC #5 asserts the full StateFrame shape.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSessionCurrent("sess-A".to_string()),
            &HashSet::new(),
            1_500,
            &[],
        )
        .await
        .expect("snapshot ok");
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert_eq!(f.session_id, "sess-A");
        assert_eq!(f.source, "claude");
        // Full StateFrame: all three SessionState fields populated.
        assert_eq!(f.state.last_event_kind, EventKind::PreToolUse);
        assert_eq!(f.state.last_event_at_ms, 1_000);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn events_topic_returns_empty_vec() {
        // Snapshot is state-only. `events.*` family subscriptions get an
        // empty vec — event history is REST territory.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;

        for t in [
            Topic::EventsAll,
            Topic::EventsBySource("claude".to_string()),
            Topic::EventsBySourceSession("claude".to_string(), "sess-A".to_string()),
        ] {
            let frames = snapshot_for_topic(&pools.reader, &t, &HashSet::new(), 3_000, &[])
                .await
                .expect("snapshot ok");
            assert!(
                frames.is_empty(),
                "events topic must not snapshot; got {} frames for {t:?}",
                frames.len()
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sentinel_daemon_row_excluded_even_when_injected_directly() {
        // Defense in depth: SELECT_NON_SENTINEL_SESSIONS already filters
        // source = '__daemon__', so a `Topic::matches` check on the
        // synthetic envelope is belt-and-suspenders. This test injects
        // the sentinel row directly to confirm the SQL filter is the
        // gate.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "__daemon__",
            "__daemon__",
            &working_state(1_000),
            1_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(2_000),
            2_000,
        )
        .await;

        let frames =
            snapshot_for_topic(&pools.reader, &Topic::StateAll, &HashSet::new(), 3_000, &[])
                .await
                .expect("snapshot ok");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].session_id, "sess-A");
        assert_ne!(frames[0].source, "__daemon__");
    }

    // Story 5.8: snapshot dedup keys on the `(source, session_id)` rows the
    // connection has already snapshot-delivered, NOT on subscribed topics.
    // A session already in `already_snapshotted` is not re-sent.
    #[tokio::test(flavor = "current_thread")]
    async fn already_snapshotted_key_dedupes_overlap() {
        // sess-A was already delivered (e.g. a prior `state.session.sess-A`
        // subscribe); a later `state.session.*` must not re-snapshot it.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-B",
            &working_state(2_000),
            2_000,
        )
        .await;

        let already = HashSet::from([("claude".to_string(), "sess-A".to_string())]);

        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &already, 3_000, &[])
            .await
            .expect("snapshot ok");
        // sess-B is new, sess-A is dedup'd by key.
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].session_id, "sess-B");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn all_keys_snapshotted_returns_empty() {
        // Every matching session already delivered → zero new frames, the
        // per-key analogue of the documented wildcard-then-specific dedup.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-B",
            &working_state(2_000),
            2_000,
        )
        .await;

        let already = HashSet::from([
            ("claude".to_string(), "sess-A".to_string()),
            ("claude".to_string(), "sess-B".to_string()),
        ]);

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSession("sess-A".to_string()),
            &already,
            3_000,
            &[],
        )
        .await
        .expect("snapshot ok");
        assert!(frames.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sibling_state_topic_dedupes_by_key() {
        // `StateSession("sess-A")` and `StateSessionCurrent("sess-A")` cover
        // the same session. Once sess-A's key is recorded (delivered under
        // one), subscribing to the other re-snapshots nothing — the dedup is
        // by delivered key, so the two siblings are equivalent without any
        // topic-set bookkeeping.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;

        let already = HashSet::from([("claude".to_string(), "sess-A".to_string())]);

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSessionCurrent("sess-A".to_string()),
            &already,
            1_000,
            &[],
        )
        .await
        .expect("snapshot ok");
        assert!(frames.is_empty());

        // Symmetric direction.
        let frames2 = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSession("sess-A".to_string()),
            &already,
            1_000,
            &[],
        )
        .await
        .expect("snapshot ok");
        assert!(frames2.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn idempotent_re_subscribe_dedupes_by_key() {
        // Re-subscribing to a topic whose rows were already delivered emits
        // zero new frames — the no-double-delivery contract, honored by the
        // per-key set rather than a topic short-circuit.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        upsert_session(
            &pools.writer,
            "claude",
            "sess-A",
            &working_state(1_000),
            1_000,
        )
        .await;

        let already = HashSet::from([("claude".to_string(), "sess-A".to_string())]);

        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &already, 1_000, &[])
            .await
            .expect("snapshot ok");
        assert!(frames.is_empty());
    }

    // Story 5.8 finding F1: widening a filter re-sends ONLY the keys the
    // narrower burst never covered. After a `states:["working"]` subscribe
    // recorded the Working key, an unfiltered re-subscribe delivers the rest
    // (here: the Ended row) and NOT the already-sent Working row.
    #[tokio::test(flavor = "current_thread")]
    async fn widening_after_filtered_resends_only_uncovered() {
        let tmp = TempDir::new().expect("tempdir");
        let pools = init_pools(&tmp.path().join("snap.db"))
            .await
            .expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        let now = 10_000;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-working",
            &state_with(SessionCurrentState::Working, now),
            2_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-ended",
            &state_with(SessionCurrentState::Ended, now),
            1_000,
        )
        .await;

        // Narrow burst: only Working. Caller records the delivered key.
        let narrow = snapshot_for_topic(
            &pools.reader,
            &Topic::StateAll,
            &HashSet::new(),
            now,
            &[SessionCurrentState::Working],
        )
        .await
        .expect("snapshot ok");
        assert_eq!(narrow.len(), 1);
        assert_eq!(narrow[0].session_id, "sess-working");

        let already: HashSet<(String, String)> = narrow
            .iter()
            .map(|f| (f.source.clone(), f.session_id.clone()))
            .collect();

        // Wider (unfiltered) re-subscribe: only the uncovered Ended row.
        let wide = snapshot_for_topic(&pools.reader, &Topic::StateAll, &already, now, &[])
            .await
            .expect("snapshot ok");
        assert_eq!(wide.len(), 1, "Working already delivered must not repeat");
        assert_eq!(wide[0].session_id, "sess-ended");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_working_falls_back_to_idle_at_read_time() {
        // 6-minute-old Working session must surface as Idle in the
        // snapshot, matching `api/sessions.rs::list` discipline. Stored
        // row remains Working.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 0,
            last_pid: None,
            cwd: None,
            started_at: None,
        };
        upsert_session(&pools.writer, "claude", "sess-A", &stored, 0).await;

        // 6 minutes > STALE_WORKING_MS (5 minutes).
        let now_ms = 6 * 60 * 1_000;
        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateAll,
            &HashSet::new(),
            now_ms,
            &[],
        )
        .await
        .expect("snapshot ok");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].state.current_state, SessionCurrentState::Idle);
        // Stored fields ride through unchanged.
        assert_eq!(frames[0].state.last_event_kind, EventKind::PreToolUse);
        assert_eq!(frames[0].state.last_event_at_ms, 0);
    }

    // Story 5.8 (ADR 0008) AC #10: a non-empty `states` filter scopes the
    // snapshot burst to sessions whose read-derived current_state is in the
    // set. The `Ended` graveyard is excluded when the presenter asks for the
    // active states.
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_states_filter_excludes_unmatched() {
        let tmp = TempDir::new().expect("tempdir");
        let pools = init_pools(&tmp.path().join("snap.db"))
            .await
            .expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        // Fresh now_ms so the Working row reads as Working (not stale).
        let now = 10_000;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-working",
            &state_with(SessionCurrentState::Working, now),
            4_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-waiting",
            &state_with(SessionCurrentState::WaitingInput, now),
            3_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-idle",
            &state_with(SessionCurrentState::Idle, now),
            2_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-ended",
            &state_with(SessionCurrentState::Ended, now),
            1_000,
        )
        .await;

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateAll,
            &HashSet::new(),
            now,
            &[
                SessionCurrentState::Working,
                SessionCurrentState::WaitingInput,
                SessionCurrentState::Idle,
            ],
        )
        .await
        .expect("snapshot ok");
        let ids: Vec<&str> = frames.iter().map(|f| f.session_id.as_str()).collect();
        assert_eq!(ids.len(), 3, "the Ended graveyard must be excluded");
        assert!(!ids.contains(&"sess-ended"), "Ended must not appear");
        assert!(ids.contains(&"sess-working"));
        assert!(ids.contains(&"sess-waiting"));
        assert!(ids.contains(&"sess-idle"));
    }

    // Story 5.8 AC #11: an empty filter is the v1.0 default — every matching
    // session (including `Ended`) appears.
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_empty_filter_returns_all() {
        let tmp = TempDir::new().expect("tempdir");
        let pools = init_pools(&tmp.path().join("snap.db"))
            .await
            .expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        let now = 10_000;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-working",
            &state_with(SessionCurrentState::Working, now),
            2_000,
        )
        .await;
        upsert_session(
            &pools.writer,
            "claude",
            "sess-ended",
            &state_with(SessionCurrentState::Ended, now),
            1_000,
        )
        .await;

        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &HashSet::new(), now, &[])
            .await
            .expect("snapshot ok");
        assert_eq!(
            frames.len(),
            2,
            "empty filter = unfiltered (includes Ended)"
        );
    }

    // Story 5.8 AC #10: the snapshot filter keys on the read-derived
    // current_state, consistent with the REST `?state=` surface. A stale
    // Working row (renders Idle) is INCLUDED by `&[Idle]` and EXCLUDED by
    // `&[Working]`.
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_states_filter_matches_read_derived() {
        let tmp = TempDir::new().expect("tempdir");
        let pools = init_pools(&tmp.path().join("snap.db"))
            .await
            .expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");

        // Stored Working, last_event_at_ms = 0; read at 6 min > STALE_WORKING_MS
        // renders Idle.
        upsert_session(
            &pools.writer,
            "claude",
            "sess-stale",
            &state_with(SessionCurrentState::Working, 0),
            0,
        )
        .await;
        let now_ms = 6 * 60 * 1_000;

        let as_idle = snapshot_for_topic(
            &pools.reader,
            &Topic::StateAll,
            &HashSet::new(),
            now_ms,
            &[SessionCurrentState::Idle],
        )
        .await
        .expect("snapshot ok");
        assert_eq!(
            as_idle.len(),
            1,
            "stale Working renders Idle and must match the Idle filter"
        );

        let as_working = snapshot_for_topic(
            &pools.reader,
            &Topic::StateAll,
            &HashSet::new(),
            now_ms,
            &[SessionCurrentState::Working],
        )
        .await
        .expect("snapshot ok");
        assert!(
            as_working.is_empty(),
            "the Working filter must NOT match a row that renders Idle"
        );
    }
}
