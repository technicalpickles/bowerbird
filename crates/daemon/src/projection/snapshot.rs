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

use protocol::{SessionState, StateFrame};

use crate::broadcast::{BroadcastEnvelope, Topic};
use crate::db::queries::SELECT_NON_SENTINEL_SESSIONS;
use crate::error::{Error, Result};
use crate::projection::state::current_state_for_read;

/// Build the on-subscribe snapshot of `StateFrame`s for `new_topic`.
///
/// Returns an empty vec when `new_topic` is not a state topic, when the
/// projection table has no non-sentinel rows, or when every matching
/// session is already covered by an entry in `pre_existing` (snapshot
/// dedup against overlapping subscriptions).
///
/// Errors propagate reader-pool checkout and `interact` failures. The
/// caller in `handle_text_frame` logs and proceeds with an empty
/// snapshot — a transient DB issue must not close a healthy WS connection.
#[tracing::instrument(skip_all, fields(new_topic = ?new_topic, now_ms))]
pub async fn snapshot_for_topic(
    reader_pool: &deadpool_sqlite::Pool,
    new_topic: &Topic,
    pre_existing: &HashSet<Topic>,
    now_ms: i64,
) -> Result<Vec<StateFrame>> {
    if !matches!(
        new_topic,
        Topic::StateAll | Topic::StateSession(_) | Topic::StateSessionCurrent(_)
    ) {
        return Ok(Vec::new());
    }

    // Cheap pre-query coverage check. Skip the SQL read entirely when
    // `pre_existing` already covers every envelope the new topic could
    // match — every row would be deduped away inside the per-row loop
    // anyway, so the DB and JSON work would be wasted. Covers:
    //   - `StateAll` in pre_existing (covers every State envelope)
    //   - exact `new_topic` already in pre_existing (idempotent re-sub)
    //   - either of `StateSession(id)` / `StateSessionCurrent(id)` in
    //     pre_existing when the new topic targets the same id (the two
    //     variants match identical State envelopes — see
    //     `Topic::matches` in `crates/daemon/src/broadcast/event.rs`).
    if pre_existing.contains(&Topic::StateAll) || pre_existing.contains(new_topic) {
        return Ok(Vec::new());
    }
    if let Some(want_sid) = match new_topic {
        Topic::StateSession(sid) | Topic::StateSessionCurrent(sid) => Some(sid),
        _ => None,
    } {
        let sibling_a = Topic::StateSession(want_sid.clone());
        let sibling_b = Topic::StateSessionCurrent(want_sid.clone());
        if pre_existing.contains(&sibling_a) || pre_existing.contains(&sibling_b) {
            return Ok(Vec::new());
        }
    }

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
        if pre_existing.iter().any(|t| t.matches(&synth)) {
            continue;
        }

        let derived_current = current_state_for_read(&stored, now_ms);
        out.push(StateFrame {
            source,
            session_id,
            state: SessionState {
                current_state: derived_current,
                last_event_kind: stored.last_event_kind,
                last_event_at_ms: stored.last_event_at_ms,
                last_pid: stored.last_pid,
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
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_projection_table_returns_empty_vec() {
        let (tmp, reader) = fresh_db().await;
        let frames = snapshot_for_topic(&reader, &Topic::StateAll, &HashSet::new(), 1_000)
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

        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &HashSet::new(), 3_000)
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
            let frames = snapshot_for_topic(&pools.reader, &t, &HashSet::new(), 3_000)
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

        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &HashSet::new(), 3_000)
            .await
            .expect("snapshot ok");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].session_id, "sess-A");
        assert_ne!(frames[0].source, "__daemon__");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_existing_subscription_dedupes_overlap() {
        // AC #7: subscribing to state.session.* after already holding
        // state.session.sess-A must not re-snapshot sess-A.
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

        let mut pre = HashSet::new();
        pre.insert(Topic::StateSession("sess-A".to_string()));

        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &pre, 3_000)
            .await
            .expect("snapshot ok");
        // sess-B is new, sess-A is dedup'd.
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].session_id, "sess-B");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_existing_wildcard_dedupes_everything() {
        // Wildcard already covers every state envelope, so any second
        // subscribe to a state topic emits zero new snapshot frames.
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

        let mut pre = HashSet::new();
        pre.insert(Topic::StateAll);

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSession("sess-A".to_string()),
            &pre,
            3_000,
        )
        .await
        .expect("snapshot ok");
        assert!(frames.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sibling_state_topic_short_circuits_without_db_read() {
        // `StateSession("sess-A")` and `StateSessionCurrent("sess-A")`
        // match identical State envelopes — they're functionally
        // equivalent. If one is already in `pre_existing`, subscribing
        // to the other yields zero new frames AND must short-circuit
        // before the DB read. To prove the short-circuit fires, we
        // deliberately point the helper at a CLOSED reader pool; a
        // path that touches SQLite would surface a pool error, while
        // the short-circuit returns `Ok(vec![])` without ever calling
        // the pool.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");
        pools.reader.close();

        let mut pre = HashSet::new();
        pre.insert(Topic::StateSession("sess-A".to_string()));

        let frames = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSessionCurrent("sess-A".to_string()),
            &pre,
            1_000,
        )
        .await
        .expect("sibling short-circuit must not touch the (closed) reader pool");
        assert!(frames.is_empty());

        // Symmetric direction.
        let mut pre2 = HashSet::new();
        pre2.insert(Topic::StateSessionCurrent("sess-A".to_string()));
        let frames2 = snapshot_for_topic(
            &pools.reader,
            &Topic::StateSession("sess-A".to_string()),
            &pre2,
            1_000,
        )
        .await
        .expect("symmetric sibling short-circuit must not touch the (closed) reader pool");
        assert!(frames2.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn idempotent_re_subscribe_short_circuits_without_db_read() {
        // Re-subscribing to a topic the connection already holds must
        // not perform a fresh DB read. Same closed-pool trick as
        // `sibling_state_topic_short_circuits_without_db_read`.
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("snap.db");
        let pools = init_pools(&db_path).await.expect("init_pools");
        run_migrations(&pools.writer).await.expect("migrate");
        pools.reader.close();

        let mut pre = HashSet::new();
        pre.insert(Topic::StateAll);

        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &pre, 1_000)
            .await
            .expect("idempotent re-sub must not touch the (closed) reader pool");
        assert!(frames.is_empty());
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
        };
        upsert_session(&pools.writer, "claude", "sess-A", &stored, 0).await;

        // 6 minutes > STALE_WORKING_MS (5 minutes).
        let now_ms = 6 * 60 * 1_000;
        let frames = snapshot_for_topic(&pools.reader, &Topic::StateAll, &HashSet::new(), now_ms)
            .await
            .expect("snapshot ok");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].state.current_state, SessionCurrentState::Idle);
        // Stored fields ride through unchanged.
        assert_eq!(frames[0].state.last_event_kind, EventKind::PreToolUse);
        assert_eq!(frames[0].state.last_event_at_ms, 0);
    }
}
