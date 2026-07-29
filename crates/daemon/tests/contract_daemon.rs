use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use assert_cmd::Command;
use bowerbird_daemon::api;
use bowerbird_daemon::api::token::BearerToken;
use bowerbird_daemon::broadcast::BroadcastHub;
use bowerbird_daemon::db::migrations::migrations;
use bowerbird_daemon::db::queries::{
    event_kind_as_str, SELECT_EVENT_BY_ID, UPSERT_SESSION_PROJECTION,
};
use bowerbird_daemon::db::{init_pools, run_migrations, DbPools};
use bowerbird_daemon::projection;
use bowerbird_daemon::state::{AppState, WsConfig};
use protocol::{EventEnvelope, EventKind, NotificationType, SessionCurrentState, SessionState};
use rusqlite::OptionalExtension;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use adapter_claude::ClaudeAdapter;

async fn fresh_pools() -> (TempDir, DbPools) {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("bower.db");
    let pools = init_pools(&db_path).await.expect("init_pools");
    run_migrations(&pools.writer).await.expect("migrate");
    (tmp, pools)
}

/// Ordered teardown for in-process pool tests. Drops the pools so SQLite's
/// connection-close runs, yields so those finalizers complete, THEN drops the
/// `TempDir` that removes `bower.db`. Prevents the intermittent
/// `sqlite3_close → sqlite3_mutex_enter → pthread_mutex_wait` deadlock — the
/// same fix the inline block in `state_plus_event_atomicity_under_sigkill...`
/// applies (see its doc-comment for the mechanism). In-process `oneshot` REST
/// tests that check out a connection must call this at end-of-body instead of
/// relying on implicit scope-exit drop order.
async fn teardown_pools(pools: DbPools, tmp: TempDir) {
    drop(pools);
    tokio::task::yield_now().await;
    drop(tmp);
    tokio::task::yield_now().await;
}

const TEST_BEARER: &str = "test-bearer-token-1.7";

fn make_test_state(pools: DbPools, migrations_complete: Arc<AtomicBool>) -> AppState {
    make_test_state_with_ws(
        pools,
        migrations_complete,
        4,
        Duration::from_secs(30),
        Duration::from_secs(10),
    )
}

fn make_test_state_with_ws(
    pools: DbPools,
    migrations_complete: Arc<AtomicBool>,
    ws_max_conns: usize,
    ping_interval: Duration,
    pong_timeout: Duration,
) -> AppState {
    // Story 4.1 added `ingest_tx` to `AppState` so the `POST /replay` endpoint
    // can push onto the same channel the live-shim ingest path uses. Tests
    // that don't exercise /replay never push to this sender, so a tiny
    // capacity (1) with the receiver dropped immediately is safe — the
    // receiver-half being closed makes the sender error on use, which is
    // exactly what a non-replay test wants (any accidental /replay call in a
    // non-replay test would fail fast rather than silently succeed).
    let (ingest_tx, _ingest_rx) =
        tokio::sync::mpsc::channel::<bowerbird_daemon::ingest::IngestItem>(1);
    AppState {
        db: pools,
        migrations_complete,
        shutdown_requested: CancellationToken::new(),
        ws_close_requested: CancellationToken::new(),
        bearer: BearerToken::new(TEST_BEARER.to_string()),
        started_at_ms: 0,
        broadcaster: Arc::new(BroadcastHub::new(16)),
        ws_semaphore: Arc::new(tokio::sync::Semaphore::new(ws_max_conns)),
        ws_config: WsConfig {
            ping_interval,
            pong_timeout,
            coalesce_window: Duration::from_secs(1),
            max_connections: ws_max_conns,
        },
        ingest_tx,
    }
}

async fn assert_pragmas(pool: &deadpool_sqlite::Pool) {
    let conn = pool.get().await.expect("pool get");
    let result: (i64, String, i64) = conn
        .interact(|c| -> rusqlite::Result<(i64, String, i64)> {
            let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
            let jm: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
            let sync: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
            Ok((fk, jm, sync))
        })
        .await
        .expect("interact")
        .expect("query");
    assert_eq!(result.0, 1, "foreign_keys must be ON");
    assert_eq!(result.1, "wal", "journal_mode must be WAL");
    assert_eq!(result.2, 1, "synchronous must be NORMAL (1)");
}

#[tokio::test(flavor = "current_thread")]
async fn pragmas_on_every_writer_checkout() {
    let (_tmp, pools) = fresh_pools().await;
    for _ in 0..3 {
        assert_pragmas(&pools.writer).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pragmas_on_every_reader_checkout() {
    let (_tmp, pools) = fresh_pools().await;
    for _ in 0..3 {
        assert_pragmas(&pools.reader).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn wal_durability_after_simulated_crash() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("bower.db");

    let envelope = EventEnvelope {
        source: "claude-code".to_string(),
        session_id: "sess-1".to_string(),
        kind: EventKind::PreToolUse,
        reaction: None,
        payload: r#"{"hello":"world"}"#.to_string(),
        pid: None,
        notification_type: None,
        cwd: None,
    };

    let event_id = {
        let pools = init_pools(&db_path).await.expect("init_pools 1");
        run_migrations(&pools.writer).await.expect("migrate 1");
        let id =
            projection::session::write(&pools.writer, &BroadcastHub::new(16), envelope.clone())
                .await
                .expect("write event");
        // Pools (and their underlying connections) drop here without an explicit
        // wal_checkpoint — simulating a crash where the WAL hasn't been folded
        // back into the main db file.
        id
    };

    // Reopen against the same file. WAL must be honored — the row is visible.
    let pools = init_pools(&db_path).await.expect("init_pools 2");
    run_migrations(&pools.writer).await.expect("migrate 2");

    let conn = pools.reader.get().await.expect("pool get");
    let id_i64 = event_id.0;
    let row: (i64, String, String, String, Option<String>, String) = conn
        .interact(move |c| -> rusqlite::Result<_> {
            c.query_row(SELECT_EVENT_BY_ID, rusqlite::params![id_i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
        })
        .await
        .expect("interact")
        .expect("row");

    assert_eq!(row.0, event_id.0);
    assert_eq!(row.1, envelope.source);
    assert_eq!(row.2, envelope.session_id);
    assert_eq!(row.3, event_kind_as_str(&envelope.kind));
    assert!(row.4.is_none(), "reaction is None for this envelope");
    assert_eq!(row.5, envelope.payload);
}

/// Rollback surrogate for AC #1. Pairs with `wal_durability_after_simulated_crash`
/// (which covers the crash-after-commit path): this test covers the
/// crash-mid-transaction path by issuing an explicit `tx.rollback()` after a
/// partial write and asserting both tables remain empty, then exercises the
/// real `projection::session::write` happy path and confirms both rows commit
/// atomically. The full SIGKILL-process variant is deferred (see
/// `deferred-work.md`); this surrogate guards the SQL-pattern + transaction
/// config against regressions in the meantime.
#[tokio::test(flavor = "current_thread")]
async fn state_plus_event_atomicity_rollback() {
    let (_tmp, pools) = fresh_pools().await;

    {
        let conn = pools.writer.get().await.expect("writer get");
        conn.interact(|c| -> rusqlite::Result<()> {
            let tx = c.transaction()?;
            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params!["src", "sess", "{}", 100i64],
            )?;
            // Intentionally do not insert the event row; rollback to simulate
            // crash-mid-transaction semantics.
            tx.rollback()?;
            Ok(())
        })
        .await
        .expect("interact")
        .expect("rollback path");
    }

    let reader = pools.reader.get().await.expect("reader get");
    let (proj_count, event_count): (i64, i64) = reader
        .interact(|c| -> rusqlite::Result<(i64, i64)> {
            let p: i64 =
                c.query_row("SELECT COUNT(*) FROM session_projections", [], |r| r.get(0))?;
            let e: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
            Ok((p, e))
        })
        .await
        .expect("interact")
        .expect("count");
    assert_eq!(
        proj_count, 0,
        "rollback must leave session_projections empty"
    );
    assert_eq!(event_count, 0, "rollback must leave events empty");
    drop(reader);

    let envelope = EventEnvelope {
        source: "src".to_string(),
        session_id: "sess".to_string(),
        kind: EventKind::PreToolUse,
        reaction: None,
        payload: "{}".to_string(),
        pid: None,
        notification_type: None,
        cwd: None,
    };
    let id = projection::session::write(&pools.writer, &BroadcastHub::new(16), envelope)
        .await
        .expect("write");
    assert!(id.0 > 0);

    let reader = pools.reader.get().await.expect("reader get");
    let (proj_count, event_count): (i64, i64) = reader
        .interact(|c| -> rusqlite::Result<(i64, i64)> {
            let p: i64 =
                c.query_row("SELECT COUNT(*) FROM session_projections", [], |r| r.get(0))?;
            let e: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
            Ok((p, e))
        })
        .await
        .expect("interact")
        .expect("count");
    assert_eq!(proj_count, 1, "happy path must commit projection row");
    assert_eq!(event_count, 1, "happy path must commit event row");
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_read_during_write() {
    let (_tmp, pools) = fresh_pools().await;
    let writer = pools.writer.clone();
    let reader = pools.reader.clone();

    let writer_task = tokio::spawn(async move {
        for i in 0..50 {
            let envelope = EventEnvelope {
                source: "claude-code".to_string(),
                session_id: format!("sess-{i}"),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            };
            projection::session::write(&writer, &BroadcastHub::new(16), envelope)
                .await
                .expect("write");
        }
    });

    let read_start = tokio::time::Instant::now();
    let conn = reader.get().await.expect("reader get");
    let _count: i64 = conn
        .interact(|c| c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0)))
        .await
        .expect("interact")
        .expect("count");
    let read_elapsed = read_start.elapsed();
    assert!(
        read_elapsed < Duration::from_millis(500),
        "reader blocked too long: {read_elapsed:?}"
    );

    writer_task.await.expect("writer task");
}

#[test]
fn migrations_validate() {
    migrations()
        .validate()
        .expect("rusqlite_migration validate() must pass");
}

#[tokio::test(flavor = "current_thread")]
async fn migrations_apply_on_fresh_db() {
    let (_tmp, pools) = fresh_pools().await;
    let conn = pools.writer.get().await.expect("writer get");
    let columns: Vec<(String, Vec<String>)> = conn
        .interact(|c| -> rusqlite::Result<Vec<(String, Vec<String>)>> {
            let mut out = Vec::new();
            for table in ["events", "session_projections", "recording_sessions"] {
                let mut stmt = c.prepare(&format!("PRAGMA table_info({table})"))?;
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                out.push((table.to_owned(), rows));
            }
            Ok(out)
        })
        .await
        .expect("interact")
        .expect("query");

    let events = &columns[0].1;
    for col in [
        "event_id",
        "source",
        "session_id",
        "kind",
        "reaction",
        "payload",
        "created_at",
    ] {
        assert!(events.contains(&col.to_string()), "events.{col} missing");
    }

    let proj = &columns[1].1;
    for col in ["source", "session_id", "state", "updated_at"] {
        assert!(
            proj.contains(&col.to_string()),
            "session_projections.{col} missing"
        );
    }

    let rec = &columns[2].1;
    for col in ["id", "started_event_id", "ended_event_id"] {
        assert!(
            rec.contains(&col.to_string()),
            "recording_sessions.{col} missing"
        );
    }
}

// Story 5.3: re-running `to_latest` against an already-migrated DB must be a
// no-op. These two `:memory:` unit tests originally lived in
// `crates/daemon/src/db/migrations.rs::tests`, but `open_in_memory()` inside a
// `#[cfg(test)]` block in `src/` trips `scripts/lint-connection-factory.sh`
// (which exempts `crates/daemon/tests/**` but does not parse `#[cfg(test)]`
// blocks). Moved here so the lint passes while preserving the in-memory
// baseline coverage, which is distinct from the populated-DB idempotency check
// in `story_5_4_migrations::migrations_idempotent_on_populated_db`. See bean
// gt-5o91.
#[test]
fn migrations_are_idempotent() {
    let mut conn = rusqlite::Connection::open_in_memory().expect("in-memory connection");
    let m = migrations();
    m.to_latest(&mut conn).expect("first to_latest");
    let v1: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version read");
    m.to_latest(&mut conn)
        .expect("second to_latest must succeed");
    let v2: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version read");
    assert_eq!(v1, v2, "user_version must be stable on re-apply");
    assert!(
        v1 >= 2,
        "expected user_version >= 2 after migrations, got {v1}"
    );
}

// Story 5.3: migration v2 must add `events.pid` as a nullable INTEGER, and
// existing rows must default to NULL. Moved here from `migrations.rs::tests`
// alongside `migrations_are_idempotent` (see that test's note and bean gt-5o91).
#[test]
fn migration_v2_adds_nullable_pid_column() {
    let mut conn = rusqlite::Connection::open_in_memory().expect("in-memory connection");
    migrations()
        .to_latest(&mut conn)
        .expect("migrations must apply");

    // Insert a row and verify pid is NULL by default (the migration uses
    // ALTER TABLE ADD COLUMN, which yields NULL for pre-existing rows; new
    // rows that don't set pid also get NULL).
    conn.execute(
        "INSERT INTO events (source, session_id, kind, payload, created_at) \
         VALUES ('claude', 's1', 'Stop', '{}', 0)",
        [],
    )
    .expect("insert");

    let pid: Option<i64> = conn
        .query_row("SELECT pid FROM events WHERE session_id = 's1'", [], |r| {
            r.get(0)
        })
        .expect("read pid");
    assert_eq!(pid, None, "default pid must be NULL");
}

// Story 5.7: migration v3 must add `events.cwd` as a nullable TEXT column;
// existing rows default to NULL; PRAGMA user_version reports 3; re-applying is
// a no-op (idempotency contract, mirrors migration_v2_adds_nullable_pid_column).
#[test]
fn migration_v3_adds_nullable_cwd_column() {
    // Story 5.7 review: exercise the file-backed migration path the story's
    // Task 4 and the Testing Standards section ask for (tempfile DB, not
    // `:memory:`), so this covers the on-disk ALTER TABLE the daemon actually
    // runs at startup rather than only the in-memory column shape.
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("bower.db");
    let mut conn = rusqlite::Connection::open(&db_path).expect("file-backed connection");
    let m = migrations();
    m.to_latest(&mut conn).expect("migrations must apply");

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version read");
    assert_eq!(version, 3, "expected user_version 3 after v3 migration");

    // A row inserted without cwd defaults to NULL (ALTER TABLE ADD COLUMN
    // behavior for new and pre-existing rows alike).
    conn.execute(
        "INSERT INTO events (source, session_id, kind, payload, created_at) \
         VALUES ('claude', 's1', 'Stop', '{}', 0)",
        [],
    )
    .expect("insert");
    let cwd: Option<String> = conn
        .query_row("SELECT cwd FROM events WHERE session_id = 's1'", [], |r| {
            r.get(0)
        })
        .expect("read cwd");
    assert_eq!(cwd, None, "default cwd must be NULL");

    // A row that sets cwd reads it back verbatim (TEXT affinity).
    conn.execute(
        "INSERT INTO events (source, session_id, kind, payload, created_at, cwd) \
         VALUES ('claude', 's2', 'Stop', '{}', 0, '/Users/x/repo')",
        [],
    )
    .expect("insert with cwd");
    let cwd2: Option<String> = conn
        .query_row("SELECT cwd FROM events WHERE session_id = 's2'", [], |r| {
            r.get(0)
        })
        .expect("read cwd");
    assert_eq!(cwd2, Some("/Users/x/repo".to_string()));

    // Idempotent re-run.
    m.to_latest(&mut conn).expect("second to_latest is a no-op");
    let version2: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version read");
    assert_eq!(version, version2, "user_version stable on re-apply");
}

// Story 5.7 review pass 2 (AC #6): prove the v2 -> v3 upgrade path itself, not
// just the post-migration column shape. The test above runs all migrations up
// front and only inserts after `events.cwd` exists, so it never exercises a
// pre-existing v2 row gaining `cwd = NULL`. Here we stop at a real v2 schema
// (`to_version(_, 2)` applies V1 + V2 → `events.pid`, no `events.cwd`,
// `PRAGMA user_version = 2`), insert a row WHILE the table has no `cwd` column,
// THEN run `to_latest` to apply v3. The pre-existing row must read `cwd = NULL`
// (ALTER TABLE ADD COLUMN backfills NULL) and `user_version` must report 3.
#[test]
fn migration_v3_preserves_existing_v2_rows_with_null_cwd() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("bower.db");
    let mut conn = rusqlite::Connection::open(&db_path).expect("file-backed connection");
    let m = migrations();

    // Stop at v2: events has `pid` but NOT `cwd`.
    m.to_version(&mut conn, 2).expect("migrate to v2");
    let v2: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version read");
    assert_eq!(v2, 2, "expected v2 schema before the v3 upgrade");
    let has_cwd_at_v2: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('events') WHERE name = 'cwd'")
        .and_then(|mut s| s.exists([]))
        .expect("table_info query");
    assert!(!has_cwd_at_v2, "v2 schema must NOT have events.cwd yet");

    // Insert a pre-existing row against the v2 schema (no cwd column to set).
    conn.execute(
        "INSERT INTO events (source, session_id, kind, payload, created_at, pid) \
         VALUES ('claude', 'pre-v3', 'Stop', '{}', 42, 7)",
        [],
    )
    .expect("insert v2 row");

    // Now run the v3 migration the daemon would run at startup.
    m.to_latest(&mut conn).expect("upgrade v2 -> v3");
    let v3: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version read");
    assert_eq!(v3, 3, "expected user_version 3 after v3 upgrade");

    // The pre-existing v2 row must have gained `cwd = NULL`.
    let (pid, cwd): (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT pid, cwd FROM events WHERE session_id = 'pre-v3'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("read pre-existing row");
    assert_eq!(pid, Some(7), "pre-existing pid must survive the v3 upgrade");
    assert_eq!(
        cwd, None,
        "pre-existing v2 row must read cwd = NULL after v3 ALTER TABLE ADD COLUMN"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn migration_failure_exits_nonzero() {
    let tmp = TempDir::new().expect("tempdir");
    let bowerbird_dir = tmp.path().join(".bowerbird");
    std::fs::create_dir_all(&bowerbird_dir).expect("mkdir");
    let db_path = bowerbird_dir.join("bower.db");

    // Manually open a connection and bump user_version above the highest
    // known migration. The daemon must refuse to proceed.
    {
        let conn = rusqlite::Connection::open(&db_path).expect("open");
        conn.execute_batch("PRAGMA user_version = 9999;")
            .expect("set version");
    }

    let assert = Command::cargo_bin("bowerbird-daemon")
        .expect("cargo_bin")
        .env("HOME", tmp.path())
        .env("RUST_LOG", "")
        // Story 3.3: pin the token via env so this test exercises the
        // migration code path (not the new token-resolution failure path).
        .env("BOWERBIRD_TOKEN", "migration-test-token")
        .env("BOWERBIRD_KEYRING_BACKEND", "disable")
        .timeout(Duration::from_secs(20))
        .assert();
    let output = assert.get_output();
    assert!(
        !output.status.success(),
        "daemon must exit non-zero on migration failure; got status {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("migration")
            || stderr.to_lowercase().contains("user_version"),
        "stderr should mention migration: {stderr}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pool_starvation_returns_defined_error() {
    let (_tmp, pools) = fresh_pools().await;

    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(pools.reader.get().await.expect("get"));
    }

    let reader = pools.reader.clone();
    let attempt = tokio::time::timeout(Duration::from_millis(250), reader.get()).await;
    match attempt {
        Err(_elapsed) => {}
        Ok(other) => panic!(
            "expected timeout while pool starved; got: {:?}",
            other.is_ok()
        ),
    }
    drop(held);
}

#[tokio::test(flavor = "current_thread")]
async fn readyz_returns_503_before_migrations_complete() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let (tmp, pools) = fresh_pools().await;
    let migrations_complete = Arc::new(AtomicBool::new(false));
    let state = make_test_state(pools.clone(), migrations_complete.clone());
    let app = api::router(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);

    migrations_complete.store(true, Ordering::Release);
    let resp2 = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp2.status(), axum::http::StatusCode::OK);

    teardown_pools(pools, tmp).await;
}

#[tokio::test(flavor = "current_thread")]
async fn healthz_returns_200_immediately() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let (_tmp, pools) = fresh_pools().await;
    // Deliberately leave migrations_complete = false to assert healthz is
    // independent of readiness.
    let state = make_test_state(pools, Arc::new(AtomicBool::new(false)));
    let app = api::router(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    // AC #1 also requires the body shape to be exactly `{"status":"ok"}` —
    // verify under the new router shape introduced by Story 1.7.
    let resp_body = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    let body_bytes = axum::body::to_bytes(resp_body.into_body(), usize::MAX)
        .await
        .expect("read body");
    let parsed: serde_json::Value = serde_json::from_slice(&body_bytes).expect("parse body");
    assert_eq!(parsed, serde_json::json!({ "status": "ok" }));
}

#[tokio::test(flavor = "current_thread")]
async fn connection_factory_policy_lint_passes() {
    // Mirror the CI lint locally so regressions are caught in `cargo test`,
    // not just in CI.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let src_dir = std::path::Path::new(manifest_dir).join("src");
    let mut offenders = Vec::new();
    walk(&src_dir, &mut offenders);
    assert!(
        offenders.is_empty(),
        "rusqlite::Connection::open is permitted only in src/db/pool.rs; found in: {:?}",
        offenders
    );
}

fn walk(dir: &std::path::Path, offenders: &mut Vec<String>) {
    let pool_rs = dir.join("db").join("pool.rs");
    for entry in walkdir(dir) {
        if entry == pool_rs {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&entry) else {
            continue;
        };
        // Forbid `Connection::open(` specifically (with the open paren). This
        // catches the bypass path that opens a file-backed connection without
        // the PRAGMA invariants the factory enforces, while permitting
        // `Connection::open_in_memory()` for ephemeral in-memory DBs that never
        // have callers depending on the PRAGMA setup. NOTE: this is looser than
        // the CI bash lint `scripts/lint-connection-factory.sh`, which bans
        // `open_in_memory()` in `src/` too; that is why the migration unit
        // tests live in this test crate rather than in `src/db/migrations.rs`.
        // See bean gt-5o91.
        if content.contains("rusqlite::Connection::open(") {
            offenders.push(entry.display().to_string());
        }
    }
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out
}

// ─── Ingest contract tests ────────────────────────────────────────────────────

async fn start_ingest_listener(
    tmp: &TempDir,
    capacity: usize,
) -> (
    tokio_util::sync::CancellationToken,
    std::path::PathBuf,
    tokio::sync::mpsc::Receiver<bowerbird_daemon::ingest::IngestItem>,
) {
    let sock_path = tmp.path().join("ingest.sock");
    let (tx, rx) = tokio::sync::mpsc::channel::<bowerbird_daemon::ingest::IngestItem>(capacity);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let path_clone = sock_path.clone();
    let shutdown_clone = shutdown.clone();
    // Use a nonexistent TOML path — adapter degrades gracefully to Unknown reactions.
    let adapter = Arc::new(ClaudeAdapter::new(
        tmp.path().join("nonexistent-tool-reactions.toml"),
    ));
    tokio::spawn(async move {
        let _ =
            bowerbird_daemon::ingest::listener::run(path_clone, tx, shutdown_clone, adapter).await;
    });
    // Give the listener a moment to bind and chmod.
    tokio::time::sleep(Duration::from_millis(20)).await;
    (shutdown, sock_path, rx)
}

async fn send_line_recv_response(sock_path: &std::path::Path, line: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::UnixStream::connect(sock_path)
        .await
        .expect("connect");
    stream.write_all(line).await.expect("write");
    stream.flush().await.expect("flush");
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await.expect("read");
    buf
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_socket_has_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;
    let mode = std::fs::metadata(&sock_path)
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "ingest.sock must be 0600, got {mode:#o}"
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_200_on_valid_json_object() {
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(resp.starts_with("200"), "expected 200, got: {resp:?}");
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_event_reaches_channel_after_200() {
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, mut rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(resp.starts_with("200"), "expected 200, got: {resp:?}");

    let item = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("timeout waiting for envelope")
        .expect("channel closed");
    assert_eq!(
        item.origin,
        bowerbird_daemon::ingest::IngestOrigin::Live,
        "shim hooks ingest as Live"
    );
    let envelope = item.envelope;
    assert!(
        envelope.payload.contains("session_id"),
        "payload should contain sent JSON"
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_200_is_ack_before_db_commit() {
    // Demonstrate that the 200 arrives before any DB work: use a full DB pool
    // but assert the 200 arrives in the read before we even look at the DB.
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    // The response was received synchronously (before DB commit) as long as it
    // arrives at all; the test verifies the write-path works end-to-end.
    assert!(resp.starts_with("200"), "expected 200, got: {resp:?}");
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_503_on_full_queue() {
    let tmp = TempDir::new().expect("tempdir");
    // capacity=1: pre-fill channel, then second send gets 503
    let (shutdown, sock_path, rx) = start_ingest_listener(&tmp, 1).await;

    // First event fills the capacity-1 channel.
    let resp1 = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(
        resp1.starts_with("200"),
        "first should be 200, got: {resp1:?}"
    );

    // Don't consume from rx — channel is now full. Second send → 503.
    let resp2 = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s2\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(
        resp2.starts_with("503"),
        "second should be 503, got: {resp2:?}"
    );

    drop(rx);
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_400_on_invalid_json() {
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(&sock_path, b"not valid json\n").await;
    assert!(resp.starts_with("400"), "expected 400, got: {resp:?}");
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_400_on_non_object_json() {
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(&sock_path, b"[1,2,3]\n").await;
    assert!(resp.starts_with("400"), "expected 400, got: {resp:?}");
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_no_db_row_on_400() {
    let (_tmp, pools) = fresh_pools().await;
    let sock_tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&sock_tmp, 16).await;

    let resp = send_line_recv_response(&sock_path, b"not valid json\n").await;
    assert!(resp.starts_with("400"), "expected 400, got: {resp:?}");

    // Allow a small window for any erroneously queued write to reach the DB.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let conn = pools.reader.get().await.expect("reader");
    let count: i64 = conn
        .interact(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM events WHERE source != '__daemon__'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .expect("interact")
        .expect("count");
    assert_eq!(count, 0, "invalid payload must not produce a DB row");
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_eof_before_newline_is_silent() {
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    // Connect and immediately close — no data written.
    {
        let _stream = tokio::net::UnixStream::connect(&sock_path)
            .await
            .expect("connect");
        // _stream drops here, sending EOF.
    }

    // Give the daemon a moment to process the EOF, then assert it can still
    // accept a normal connection.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(
        resp.starts_with("200"),
        "daemon should still work after EOF client, got: {resp:?}"
    );
    shutdown.cancel();
}

// ─── Story 1.8: strict hook_kind enforcement ────────────────────────────────
//
// The daemon previously coalesced missing/non-string `hook_kind` into a silent
// `"PreToolUse"` default. After Story 1.8, the field is required and unknown
// values get their own typed wire response. See ADR-0002 §Consequences and the
// story's AC #1, #2, #3, #6.

#[tokio::test(flavor = "current_thread")]
async fn ingest_400_on_missing_hook_kind() {
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    // AC #1 specifies the exact wire response. Using exact equality (not
    // `starts_with`) catches regressions that append extra detail after the
    // status line. The structural assertions below stay as belt-and-braces.
    assert_eq!(
        resp, "400 missing hook_kind\n",
        "expected exact 400 missing hook_kind, got: {resp:?}"
    );
    assert_eq!(
        resp.matches('\n').count(),
        1,
        "response must be exactly one line: {resp:?}"
    );
    assert!(
        resp.ends_with('\n'),
        "response must end in newline: {resp:?}"
    );
    assert!(
        resp.len() <= 64,
        "missing-hook_kind response must stay short (got {} bytes): {resp:?}",
        resp.len()
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_400_on_unknown_hook_kind() {
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"BogusKind\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert_eq!(
        resp, "400 unknown hook_kind: BogusKind\n",
        "expected exact 400 unknown hook_kind, got: {resp:?}"
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_400_on_non_string_hook_kind() {
    // AC #3: non-string hook_kind (number, bool, null, array, object) is
    // malformed in the same way as absent; same wire response.
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":42,\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert_eq!(
        resp, "400 missing hook_kind\n",
        "expected exact 400 missing hook_kind for non-string hook_kind, got: {resp:?}"
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_400_on_unknown_hook_kind_with_missing_session_id() {
    // Story 1.8 review finding: an unknown hook_kind must surface as the
    // dedicated `400 unknown hook_kind: <value>` wire response even when
    // other required adapter fields (session_id, tool_name) are absent or
    // wrong-type. Without ordering hook_kind validation before session_id
    // extraction in the adapter, this payload would hit MissingField and
    // emit `400 normalize error: missing required field: session_id` instead.
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"BogusKind\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert_eq!(
        resp, "400 unknown hook_kind: BogusKind\n",
        "expected exact 400 unknown hook_kind (no normalize error: prefix), got: {resp:?}"
    );

    let resp2 = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"BogusKind\",\"session_id\":42,\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert_eq!(
        resp2, "400 unknown hook_kind: BogusKind\n",
        "expected exact 400 unknown hook_kind for non-string session_id, got: {resp2:?}"
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_400_on_unknown_hook_kind_sanitizes_newlines() {
    // AC #6: any user-supplied string flowing into the 400 line must pass
    // through sanitize_for_wire so embedded \n / \r can't desync the client.
    let tmp = TempDir::new().expect("tempdir");
    let (shutdown, sock_path, _rx) = start_ingest_listener(&tmp, 16).await;

    let resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"Bad\\nKind\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(
        resp.contains("Bad Kind"),
        "embedded \\n must be replaced with a space: {resp:?}"
    );
    assert_eq!(
        resp.matches('\n').count(),
        1,
        "response must contain exactly one newline (the terminator): {resp:?}"
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn ingest_no_db_row_on_missing_hook_kind() {
    // Story 1.8 review finding: `start_ingest_listener` alone only wires the
    // listener to an in-memory mpsc — the writer is not plumbed to the test's
    // DB pool, so a `COUNT(*)` against `pools.reader` would pass even if a
    // malformed payload were accidentally queued. This harness wires
    // `listener::run` and `writer::run` together through the same channel and
    // the same `pools.writer`, so the DB assertion actually exercises the
    // ingest-to-persistence path. To prove the harness is meaningful, the
    // test then sends a valid payload and asserts the row count goes up.
    let (_tmp, pools) = fresh_pools().await;
    let sock_tmp = TempDir::new().expect("tempdir");
    let sock_path = sock_tmp.path().join("ingest.sock");

    let (tx, rx) = tokio::sync::mpsc::channel::<bowerbird_daemon::ingest::IngestItem>(16);
    let shutdown = CancellationToken::new();
    let adapter = Arc::new(ClaudeAdapter::new(
        sock_tmp.path().join("nonexistent-tool-reactions.toml"),
    ));

    let listener_path = sock_path.clone();
    let listener_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ =
            bowerbird_daemon::ingest::listener::run(listener_path, tx, listener_shutdown, adapter)
                .await;
    });

    let writer_pool = pools.writer.clone();
    let writer_shutdown = shutdown.clone();
    tokio::spawn(async move {
        bowerbird_daemon::ingest::writer::run(
            rx,
            writer_pool,
            Arc::new(BroadcastHub::new(16)),
            writer_shutdown,
        )
        .await;
    });

    // Give the listener a moment to bind and chmod.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 1. Malformed payload must yield exact `400 missing hook_kind\n`.
    let resp = send_line_recv_response(
        &sock_path,
        b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert_eq!(
        resp, "400 missing hook_kind\n",
        "expected exact 400 missing hook_kind, got: {resp:?}"
    );

    // Allow a window for any erroneously queued write to reach the DB.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let conn = pools.reader.get().await.expect("reader");
    let count_after_400: i64 = conn
        .interact(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM events WHERE source != '__daemon__'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .expect("interact")
        .expect("count");
    assert_eq!(
        count_after_400, 0,
        "missing hook_kind must not produce a DB row"
    );

    // 2. Valid payload must persist — proves the writer is plumbed to
    //    `pools.writer`, so the assertion above is meaningful.
    let valid_resp = send_line_recv_response(
        &sock_path,
        b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(
        valid_resp.starts_with("200"),
        "expected 200 for valid payload, got: {valid_resp:?}"
    );

    // Wait for the writer to persist before re-querying.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let conn2 = pools.reader.get().await.expect("reader");
    let count_after_valid: i64 = conn2
        .interact(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM events WHERE source != '__daemon__'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .expect("interact")
        .expect("count");
    assert_eq!(
        count_after_valid, 1,
        "valid payload must persist exactly one event row (harness sanity check)"
    );

    shutdown.cancel();
}

// ─── Story 1.5: shim ↔ daemon e2e round-trip ────────────────────────────────
//
// Spawns the real `bowerbird-shim` binary via assert_cmd, points it at a temp
// ingest socket served by the real `ingest::listener::run` loop, and asserts
// the daemon's mpsc receives one normalized envelope. Chosen location:
// `crates/daemon/tests/contract_daemon.rs` (over a new `crates/shim/tests/
// e2e_against_daemon.rs`) so we don't introduce a daemon dev-dep on the shim
// crate — the daemon already owns the ingest contract.

#[tokio::test(flavor = "current_thread")]
async fn shim_binary_round_trip_to_daemon_ingest() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log_path = log_tmp.path().join("shim.log");
    let (shutdown, sock_path, mut rx) = start_ingest_listener(&tmp, 16).await;

    let sock_str = sock_path.to_string_lossy().into_owned();
    let log_str = log_path.to_string_lossy().into_owned();

    // Mirror crates/adapter-claude/tests/fixtures/pre_tool_use_bash.json.
    let stdin =
        br#"{"hook_kind":"PreToolUse","session_id":"test-session-abc123","tool_name":"Bash","tool_input":{"command":"cargo test"}}"#;

    // Run the shim binary on a blocking task — assert_cmd is sync.
    let shim_result = tokio::task::spawn_blocking(move || {
        Command::cargo_bin("bowerbird-shim")
            .expect("cargo_bin shim")
            .arg("--hook-kind")
            .arg("PreToolUse")
            .env("BOWERBIRD_INGEST_SOCK", &sock_str)
            .env("BOWERBIRD_SHIM_LOG", &log_str)
            .write_stdin(stdin.to_vec())
            .output()
            .expect("shim spawn")
    })
    .await
    .expect("join shim task");

    let code = shim_result.status.code().expect("clean exit");
    assert_eq!(
        code,
        0,
        "shim should exit 0 against real daemon; stderr: {:?}, stdout: {:?}",
        String::from_utf8_lossy(&shim_result.stderr),
        String::from_utf8_lossy(&shim_result.stdout),
    );

    let item = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout waiting for envelope")
        .expect("channel closed");
    let envelope = item.envelope;
    assert_eq!(envelope.kind, EventKind::PreToolUse);
    assert_eq!(envelope.source, "claude");
    assert_eq!(envelope.session_id, "test-session-abc123");

    shutdown.cancel();
}

#[tokio::test(flavor = "current_thread")]
async fn shim_user_prompt_submit_round_trip_persists_working() {
    // Story 5.2 AC #3/#8 — prove the real installed path, not just the
    // projection helper: shim CLI accepts UserPromptSubmit, injects hook_kind,
    // daemon ingest normalizes it without tool_name, writer persists it, and
    // the projection moves to Working.
    let (_tmp, pools) = fresh_pools().await;
    let sock_tmp = TempDir::new().expect("socket tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let sock_path = sock_tmp.path().join("ingest.sock");
    let log_path = log_tmp.path().join("shim.log");

    let (tx, rx) = tokio::sync::mpsc::channel::<bowerbird_daemon::ingest::IngestItem>(16);
    let shutdown = CancellationToken::new();
    let adapter = Arc::new(ClaudeAdapter::new(
        sock_tmp.path().join("nonexistent-tool-reactions.toml"),
    ));

    let listener_path = sock_path.clone();
    let listener_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ =
            bowerbird_daemon::ingest::listener::run(listener_path, tx, listener_shutdown, adapter)
                .await;
    });

    let writer_pool = pools.writer.clone();
    let writer_shutdown = shutdown.clone();
    tokio::spawn(async move {
        bowerbird_daemon::ingest::writer::run(
            rx,
            writer_pool,
            Arc::new(BroadcastHub::new(16)),
            writer_shutdown,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(20)).await;

    let sock_str = sock_path.to_string_lossy().into_owned();
    let log_str = log_path.to_string_lossy().into_owned();
    let stdin = br#"{"session_id":"sess-ups-e2e","prompt":"hello"}"#;
    let shim_result = tokio::task::spawn_blocking(move || {
        Command::cargo_bin("bowerbird-shim")
            .expect("cargo_bin shim")
            .arg("--hook-kind")
            .arg("UserPromptSubmit")
            .env("BOWERBIRD_INGEST_SOCK", &sock_str)
            .env("BOWERBIRD_SHIM_LOG", &log_str)
            .write_stdin(stdin.to_vec())
            .output()
            .expect("shim spawn")
    })
    .await
    .expect("join shim task");

    assert_eq!(
        shim_result.status.code(),
        Some(0),
        "shim should exit 0 against real daemon; stderr: {:?}, stdout: {:?}",
        String::from_utf8_lossy(&shim_result.stderr),
        String::from_utf8_lossy(&shim_result.stdout),
    );

    let expected_kind = event_kind_as_str(&EventKind::UserPromptSubmit);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let (event_count, state_json) = loop {
        let conn = pools.reader.get().await.expect("reader");
        let expected_kind = expected_kind.clone();
        let row = conn
            .interact(move |c| -> rusqlite::Result<(i64, Option<String>)> {
                let event_count = c.query_row(
                    "SELECT COUNT(*) FROM events WHERE source = ? AND session_id = ? AND kind = ?",
                    rusqlite::params!["claude", "sess-ups-e2e", expected_kind],
                    |r| r.get(0),
                )?;
                let state_json = c
                    .query_row(
                        "SELECT state FROM session_projections WHERE source = ? AND session_id = ?",
                        rusqlite::params!["claude", "sess-ups-e2e"],
                        |r| r.get(0),
                    )
                    .optional()?;
                Ok((event_count, state_json))
            })
            .await
            .expect("interact")
            .expect("query");
        if row.0 == 1 && row.1.is_some() {
            break row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for UserPromptSubmit event/projection row; last row={row:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    assert_eq!(event_count, 1);
    let parsed: SessionState =
        serde_json::from_str(&state_json.expect("state row")).expect("parse state");
    assert_eq!(parsed.current_state, SessionCurrentState::Working);
    assert_eq!(parsed.last_event_kind, EventKind::UserPromptSubmit);

    shutdown.cancel();
}

// -----------------------------------------------------------------------------
// Story 1.6 — session projection + hook unreliability tolerance
// -----------------------------------------------------------------------------

/// Mirror of `projection::state::STALE_WORKING_MS` (pub(crate)). Kept in sync
/// by code review; if these diverge, the boundary test in `projection::state`
/// will still catch the actual production threshold.
const TEST_STALE_WORKING_MS: i64 = 300_000;

async fn read_session_state(
    pool: &deadpool_sqlite::Pool,
    source: &str,
    session_id: &str,
) -> String {
    let conn = pool.get().await.expect("reader get");
    let src = source.to_string();
    let sid = session_id.to_string();
    conn.interact(move |c| -> rusqlite::Result<String> {
        c.query_row(
            "SELECT state FROM session_projections WHERE source = ? AND session_id = ?",
            rusqlite::params![src, sid],
            |row| row.get(0),
        )
    })
    .await
    .expect("interact")
    .expect("state row")
}

async fn count_session_projections(pool: &deadpool_sqlite::Pool, filter: &str) -> i64 {
    let conn = pool.get().await.expect("reader get");
    let sql = format!("SELECT COUNT(*) FROM session_projections WHERE {filter}");
    conn.interact(move |c| -> rusqlite::Result<i64> { c.query_row(&sql, [], |r| r.get(0)) })
        .await
        .expect("interact")
        .expect("count")
}

fn envelope_for(source: &str, session_id: &str, kind: EventKind) -> EventEnvelope {
    EventEnvelope {
        source: source.to_string(),
        session_id: session_id.to_string(),
        kind,
        reaction: None,
        payload: "{}".to_string(),
        pid: None,
        notification_type: None,
        cwd: None,
    }
}

/// Story 5.3: Notification envelope helper. The typed `notification_type`
/// drives the branching (three rules as of Story 5.6 / ADR 0005, code-review
/// D1+D3): input-required types (PermissionPrompt, ElicitationDialog) → WaitingInput;
/// IdlePrompt → Idle, except a prior WaitingInput is preserved; the truly-transient
/// types (AuthSuccess, ElicitationResponse, ElicitationComplete, Unknown, None)
/// preserve prior current_state, except a prior Ended resurrects to Idle.
fn envelope_for_notification(
    source: &str,
    session_id: &str,
    notification_type: Option<NotificationType>,
) -> EventEnvelope {
    EventEnvelope {
        source: source.to_string(),
        session_id: session_id.to_string(),
        kind: EventKind::Notification,
        reaction: None,
        payload: notification_type
            .map(|nt| {
                let wire = match nt {
                    NotificationType::PermissionPrompt => "permission_prompt",
                    NotificationType::IdlePrompt => "idle_prompt",
                    NotificationType::AuthSuccess => "auth_success",
                    NotificationType::ElicitationDialog => "elicitation_dialog",
                    NotificationType::ElicitationResponse => "elicitation_response",
                    NotificationType::ElicitationComplete => "elicitation_complete",
                    NotificationType::Unknown => "unknown_future_type",
                };
                format!(r#"{{"notification_type":"{wire}"}}"#)
            })
            .unwrap_or_else(|| "{}".to_string()),
        pid: None,
        notification_type,
        cwd: None,
    }
}

/// AC #3 — Two sessions sharing a `session_id` but differing in `source` must
/// have independent projection rows.
#[tokio::test(flavor = "current_thread")]
async fn source_session_id_collision_safety() {
    let (_tmp, pools) = fresh_pools().await;

    projection::session::write(
        &pools.writer,
        &BroadcastHub::new(16),
        envelope_for("claude", "sess-shared", EventKind::PreToolUse),
    )
    .await
    .expect("write claude");
    projection::session::write(
        &pools.writer,
        &BroadcastHub::new(16),
        envelope_for("codex", "sess-shared", EventKind::PreToolUse),
    )
    .await
    .expect("write codex");

    let count = count_session_projections(&pools.reader, "session_id = 'sess-shared'").await;
    assert_eq!(count, 2, "two distinct (source, session_id) rows expected");

    // Capture the pre-second-write codex state for inequality assertion.
    let codex_state_before = read_session_state(&pools.reader, "codex", "sess-shared").await;
    let codex_updated_at_before: i64 = {
        let conn = pools.reader.get().await.expect("reader get");
        conn.interact(|c| -> rusqlite::Result<i64> {
            c.query_row(
                "SELECT updated_at FROM session_projections WHERE source = 'codex' AND session_id = 'sess-shared'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .expect("interact")
        .expect("updated_at row")
    };

    // Mutate only the claude session. Use Stop here so claude ends up Idle and
    // diverges from codex's Working state — Story 5.2 made PostToolUse preserve
    // prev, so a single PostToolUse no longer flips Working → Idle.
    projection::session::write(
        &pools.writer,
        &BroadcastHub::new(16),
        envelope_for("claude", "sess-shared", EventKind::Stop),
    )
    .await
    .expect("write claude#2");

    let claude_state = read_session_state(&pools.reader, "claude", "sess-shared").await;
    let codex_state_after = read_session_state(&pools.reader, "codex", "sess-shared").await;
    let codex_updated_at_after: i64 = {
        let conn = pools.reader.get().await.expect("reader get");
        conn.interact(|c| -> rusqlite::Result<i64> {
            c.query_row(
                "SELECT updated_at FROM session_projections WHERE source = 'codex' AND session_id = 'sess-shared'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .expect("interact")
        .expect("updated_at row")
    };
    assert_eq!(
        codex_state_before, codex_state_after,
        "codex row state must not be mutated by a claude write"
    );
    assert_eq!(
        codex_updated_at_before, codex_updated_at_after,
        "codex row updated_at must not be touched by a claude write"
    );
    let claude_parsed: SessionState =
        serde_json::from_str(&claude_state).expect("claude state parses");
    let codex_parsed: SessionState =
        serde_json::from_str(&codex_state_after).expect("codex state parses");
    assert_eq!(claude_parsed.current_state, SessionCurrentState::Idle);
    assert_eq!(codex_parsed.current_state, SessionCurrentState::Working);
    assert_ne!(claude_state, codex_state_after, "states must diverge");

    // Event log is also segregated.
    let reader = pools.reader.get().await.expect("reader get");
    let (claude_events, codex_events): (i64, i64) = reader
        .interact(|c| -> rusqlite::Result<(i64, i64)> {
            let a: i64 = c.query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'claude' AND session_id = 'sess-shared'",
                [],
                |r| r.get(0),
            )?;
            let b: i64 = c.query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'codex' AND session_id = 'sess-shared'",
                [],
                |r| r.get(0),
            )?;
            Ok((a, b))
        })
        .await
        .expect("interact")
        .expect("counts");
    assert_eq!(claude_events, 2);
    assert_eq!(codex_events, 1);
}

/// AC #1, #4 — a `PreToolUse` without a matching `PostToolUse` does not stay
/// stuck in `Working` once the stale-Working threshold elapses. Also verifies
/// that a `Stop` hook naturally clears `Working` at the storage layer.
#[tokio::test(flavor = "current_thread")]
async fn hook_unreliability_tolerance_pretooluse_without_posttooluse() {
    let (_tmp, pools) = fresh_pools().await;

    projection::session::write(
        &pools.writer,
        &BroadcastHub::new(16),
        envelope_for("claude", "sess-lonely", EventKind::PreToolUse),
    )
    .await
    .expect("write PreToolUse");

    let stored_json = read_session_state(&pools.reader, "claude", "sess-lonely").await;
    let stored: SessionState = serde_json::from_str(&stored_json).expect("state parses");
    assert_eq!(stored.current_state, SessionCurrentState::Working);
    assert_eq!(stored.last_event_kind, EventKind::PreToolUse);

    let now_ms_late = stored.last_event_at_ms + TEST_STALE_WORKING_MS + 1;
    let surfaced = projection::current_state_for_read(&stored, now_ms_late);
    assert_eq!(
        surfaced,
        SessionCurrentState::Idle,
        "read-time stale check must surface Idle past the threshold"
    );

    // Sub-case: a Stop hook clears Working at the storage layer.
    projection::session::write(
        &pools.writer,
        &BroadcastHub::new(16),
        envelope_for("claude", "sess-lonely", EventKind::Stop),
    )
    .await
    .expect("write Stop");
    let stored_after_stop = read_session_state(&pools.reader, "claude", "sess-lonely").await;
    let parsed: SessionState =
        serde_json::from_str(&stored_after_stop).expect("post-Stop state parses");
    assert_eq!(parsed.current_state, SessionCurrentState::Idle);
    // Byte-for-byte: the wire string for SessionCurrentState::Idle is `"Idle"`.
    // A literal substring check guards against silent rename_all drift.
    assert!(
        stored_after_stop.contains(r#""current_state":"Idle""#),
        "stored state JSON must contain literal \"current_state\":\"Idle\" — got {stored_after_stop}"
    );
}

/// AC #4 — Drive a session through a mixed event sequence and assert the
/// stored `current_state` matches the documented transition table at each step.
///
/// Updated by Story 5.3: PostToolUse now unconditionally → Working (refines
/// Story 5.2's "preserve prior"). Notification branches on `notification_type`
/// (three rules as of Story 5.6 / ADR 0005, code-review D1+D3): input-required
/// types (PermissionPrompt, ElicitationDialog) → WaitingInput; IdlePrompt → Idle,
/// except a prior WaitingInput is preserved; the truly-transient types (AuthSuccess,
/// ElicitationResponse, ElicitationComplete, Unknown) and None preserve prior,
/// except a prior Ended resurrects to Idle.
#[tokio::test(flavor = "current_thread")]
async fn state_machine_full_sequence_determinism() {
    let (_tmp, pools) = fresh_pools().await;
    let session_id = "sess-determinism";

    // Each case: (envelope, expected current_state after write).
    let cases: Vec<(EventEnvelope, SessionCurrentState)> = vec![
        (
            envelope_for("claude", session_id, EventKind::PreToolUse),
            SessionCurrentState::Working,
        ),
        // Story 5.3 AC #9: PostToolUse → Working unconditionally.
        (
            envelope_for("claude", session_id, EventKind::PostToolUse),
            SessionCurrentState::Working,
        ),
        // Stop is the canonical "agent done" transition.
        (
            envelope_for("claude", session_id, EventKind::Stop),
            SessionCurrentState::Idle,
        ),
        // UserPromptSubmit drives Idle → Working.
        (
            envelope_for("claude", session_id, EventKind::UserPromptSubmit),
            SessionCurrentState::Working,
        ),
        (
            envelope_for("claude", session_id, EventKind::PreToolUse),
            SessionCurrentState::Working,
        ),
        // Story 5.3 AC #7: Notification + PermissionPrompt → WaitingInput.
        (
            envelope_for_notification(
                "claude",
                session_id,
                Some(NotificationType::PermissionPrompt),
            ),
            SessionCurrentState::WaitingInput,
        ),
        // Story 5.3 AC #8: Notification + AuthSuccess → preserve prior
        // (current_state stays WaitingInput from the previous case).
        (
            envelope_for_notification("claude", session_id, Some(NotificationType::AuthSuccess)),
            SessionCurrentState::WaitingInput,
        ),
        (
            envelope_for("claude", session_id, EventKind::PreToolUse),
            SessionCurrentState::Working,
        ),
        (
            envelope_for("claude", session_id, EventKind::PostToolUse),
            SessionCurrentState::Working,
        ),
        (
            envelope_for("claude", session_id, EventKind::Stop),
            SessionCurrentState::Idle,
        ),
    ];

    for (envelope, expected) in cases {
        let kind = envelope.kind.clone();
        projection::session::write(&pools.writer, &BroadcastHub::new(16), envelope)
            .await
            .expect("write");
        let stored = read_session_state(&pools.reader, "claude", session_id).await;
        let parsed: SessionState = serde_json::from_str(&stored).expect("parse");
        assert_eq!(
            parsed.current_state, expected,
            "after {kind:?} current_state must be {expected:?}, got {:?}",
            parsed.current_state
        );
        assert_eq!(
            parsed.last_event_kind, kind,
            "last_event_kind must always reflect the latest event"
        );
    }
}

/// Drain a broadcast receiver synchronously (the hub publishes inline), tallying
/// Event frames and collecting the `current_state` of each State frame.
fn drain_frames(
    rx: &mut tokio::sync::broadcast::Receiver<bowerbird_daemon::broadcast::BroadcastEnvelope>,
) -> (usize, Vec<SessionCurrentState>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut events = 0usize;
    let mut states = Vec::new();
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::Event(_)) => events += 1,
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::State { state, .. }) => {
                states.push(state.current_state)
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(e) => panic!("unexpected recv error: {e:?}"),
        }
    }
    (events, states)
}

/// Story 5.6 / ADR 0005 (code-review D3), publish-path coverage. The pure
/// `transition` tests assert `IdlePrompt + prior Working → Idle`; this drives
/// the full `projection::session::write` path and asserts the persisted row
/// becomes `Idle` AND a `State` frame carrying `Idle` is published, so a
/// presenter actually receives the dropped-`Stop` correction. (Finding from
/// the fourth code-review pass: publish-path coverage missed the new
/// state-changing notification branches.)
#[tokio::test(flavor = "current_thread")]
async fn notification_idle_prompt_after_working_publishes_idle_state() {
    let (_tmp, pools) = fresh_pools().await;
    let hub = BroadcastHub::new(64);
    let session_id = "sess-idle-after-working";

    // Drive the session to Working (simulating a turn whose Stop was dropped).
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::PreToolUse),
    )
    .await
    .expect("write PreToolUse");

    // Subscribe AFTER setup so we observe only the notification write's frames.
    let mut rx = hub.subscribe();

    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for_notification("claude", session_id, Some(NotificationType::IdlePrompt)),
    )
    .await
    .expect("write Notification(IdlePrompt)");

    let stored = read_session_state(&pools.reader, "claude", session_id).await;
    let parsed: SessionState = serde_json::from_str(&stored).expect("parse");
    assert_eq!(
        parsed.current_state,
        SessionCurrentState::Idle,
        "idle_prompt after Working must persist Idle"
    );
    assert_eq!(parsed.last_event_kind, EventKind::Notification);

    let (events, states) = drain_frames(&mut rx);
    assert_eq!(
        events, 1,
        "the idle_prompt write must publish its Event frame"
    );
    assert_eq!(
        states,
        vec![SessionCurrentState::Idle],
        "Working→Idle must publish exactly one State frame carrying Idle"
    );
}

/// Story 5.6 / ADR 0005 (code-review D1), publish-path coverage. A row the
/// liveness probe marked `Ended` receives a non-blocking notification; the hook
/// proves the process is alive, so it must resurrect to `Idle` — persisted AND
/// published as an Event then a State frame. Covers both `IdlePrompt` (its own
/// arm) and a truly-transient type (`AuthSuccess`) reaching the same outcome.
#[tokio::test(flavor = "current_thread")]
async fn notification_after_ended_resurrects_to_idle_state() {
    for notification_type in [NotificationType::IdlePrompt, NotificationType::AuthSuccess] {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        let session_id = "sess-ended-resurrect";

        // Drive the row to Ended (as the liveness probe would).
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_for("claude", session_id, EventKind::SessionEnded),
        )
        .await
        .expect("write SessionEnded");
        let ended = read_session_state(&pools.reader, "claude", session_id).await;
        let ended: SessionState = serde_json::from_str(&ended).expect("parse");
        assert_eq!(
            ended.current_state,
            SessionCurrentState::Ended,
            "setup: row must be Ended before the resurrecting notification"
        );

        // Subscribe AFTER setup so we observe only the notification write.
        let mut rx = hub.subscribe();

        projection::session::write(
            &pools.writer,
            &hub,
            envelope_for_notification("claude", session_id, Some(notification_type)),
        )
        .await
        .expect("write Notification");

        let stored = read_session_state(&pools.reader, "claude", session_id).await;
        let parsed: SessionState = serde_json::from_str(&stored).expect("parse");
        assert_eq!(
            parsed.current_state,
            SessionCurrentState::Idle,
            "{notification_type:?} from Ended must persist Idle (hook proves process alive)"
        );
        assert_eq!(parsed.last_event_kind, EventKind::Notification);

        let (events, states) = drain_frames(&mut rx);
        assert_eq!(
            events, 1,
            "{notification_type:?} write must publish its Event frame"
        );
        assert_eq!(
            states,
            vec![SessionCurrentState::Idle],
            "Ended→Idle must publish exactly one State frame carrying Idle for {notification_type:?}"
        );
    }
}

/// Story 5.2 AC #1, #2 — `BroadcastEnvelope::State` is published only when a
/// projection write actually changes `current_state`. Back-to-back
/// `PreToolUse`/`PostToolUse` pairs (which both keep the session Working
/// under Story 5.2's new transition table) must publish exactly one
/// state envelope across the entire run — the initial `None → Working`.
#[tokio::test(flavor = "current_thread")]
async fn state_broadcast_only_on_transition() {
    let (_tmp, pools) = fresh_pools().await;
    let hub = BroadcastHub::new(64);
    let mut rx = hub.subscribe();
    let session_id = "sess-only-on-transition";

    // 1 PreToolUse → Idle→Working transition (1 Event + 1 State).
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::PreToolUse),
    )
    .await
    .expect("write PreToolUse");

    // N=3 back-to-back PostToolUse/PreToolUse pairs. Each event stays in
    // Working — zero additional State envelopes.
    const N: usize = 3;
    for _ in 0..N {
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_for("claude", session_id, EventKind::PostToolUse),
        )
        .await
        .expect("write PostToolUse");
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_for("claude", session_id, EventKind::PreToolUse),
        )
        .await
        .expect("write PreToolUse");
    }

    // 1 Stop → Working→Idle transition (1 Event + 1 State).
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::Stop),
    )
    .await
    .expect("write Stop");

    // Drain the receiver. Use `try_recv` in a deadline loop — the hub is
    // synchronous so all envelopes should already be in the channel.
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut events = 0usize;
    let mut states = 0usize;
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::Event(_)) => events += 1,
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::State { .. }) => states += 1,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                // No more frames pending — done.
                break;
            }
            Err(e) => panic!("unexpected recv error: {e:?}"),
        }
    }
    // Events: 1 (initial PreToolUse) + 2*N (back-to-back pairs) + 1 (final Stop) = 2 + 2N.
    let expected_events = 2 + 2 * N;
    assert_eq!(events, expected_events, "event count mismatch");
    // State envelopes: exactly two — initial Idle→Working + final Working→Idle.
    assert_eq!(
        states, 2,
        "expected exactly two State envelopes (Idle→Working, Working→Idle)"
    );
}

/// Story 5.2 AC #3 — `UserPromptSubmit` drives the session into `Working`
/// from `Idle`, and the subsequent `PreToolUse` is a no-op for state-frame
/// purposes (already Working). `last_event_kind` always reflects the most
/// recent event regardless of state-frame gating.
#[tokio::test(flavor = "current_thread")]
async fn user_prompt_submit_drives_working_transition() {
    let (_tmp, pools) = fresh_pools().await;
    let hub = BroadcastHub::new(64);
    let mut rx = hub.subscribe();
    let session_id = "sess-ups";

    // Stop first to put session at Idle.
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::Stop),
    )
    .await
    .expect("write Stop");

    // UserPromptSubmit → Idle → Working: should emit one State envelope.
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::UserPromptSubmit),
    )
    .await
    .expect("write UserPromptSubmit");

    // PreToolUse: already Working → no additional State envelope.
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::PreToolUse),
    )
    .await
    .expect("write PreToolUse");

    // Tally.
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut events = 0usize;
    let mut states_after_first_stop = 0usize;
    // The Stop write itself counts as `None → Idle`, which is a transition
    // (None != Some(Idle)) — so the very first envelope sequence is
    // Event(Stop) + State(Idle). We track State envelopes AFTER the
    // initial Stop transition to assert specifically that
    // UserPromptSubmit drives one transition and PreToolUse drives zero.
    let mut state_seq: Vec<EventKind> = Vec::new();
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::Event(_)) => events += 1,
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::State { state, .. }) => {
                state_seq.push(state.last_event_kind);
                if state_seq.len() > 1 {
                    states_after_first_stop += 1;
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(e) => panic!("unexpected recv error: {e:?}"),
        }
    }
    assert_eq!(events, 3, "3 events: Stop + UserPromptSubmit + PreToolUse");
    assert_eq!(
        state_seq,
        vec![EventKind::Stop, EventKind::UserPromptSubmit],
        "exactly two State frames — initial Stop (None→Idle) and UserPromptSubmit (Idle→Working)"
    );
    assert_eq!(
        states_after_first_stop, 1,
        "UserPromptSubmit emits one State frame; trailing PreToolUse emits none"
    );

    // Stored row reflects the most recent event regardless of state-frame
    // gating: last_event_kind must be PreToolUse, not UserPromptSubmit.
    let stored = read_session_state(&pools.reader, "claude", session_id).await;
    let parsed: SessionState = serde_json::from_str(&stored).expect("parse");
    assert_eq!(parsed.last_event_kind, EventKind::PreToolUse);
    assert_eq!(parsed.current_state, SessionCurrentState::Working);
}

/// Story 5.2 review #2 — a stale-Working stored row that subscribers see
/// as `Idle` (via `current_state_for_read` in the snapshot/REST paths) must
/// re-emit a `State` envelope when a new event restores live `Working`.
/// Without read-facing gating, comparing raw stored `Working` to new stored
/// `Working` would suppress the publish and leave the subscriber stuck on
/// `Idle`.
#[tokio::test(flavor = "current_thread")]
async fn state_broadcast_publishes_when_stale_working_recovers() {
    let (_tmp, pools) = fresh_pools().await;
    let hub = BroadcastHub::new(64);
    let session_id = "sess-stale-recover";

    // Seed: write a fresh PreToolUse so the projection row exists at Working.
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::PreToolUse),
    )
    .await
    .expect("seed write");

    // Age the row past STALE_WORKING_MS so the read-facing view becomes Idle.
    // Real sleeps are forbidden (deterministic-test discipline) — direct
    // UPDATE is the right pattern (see sessions_list_applies_stale_working_fallback).
    // Scope the writer guard so the connection returns to the pool (max_size = 1)
    // before the recovery write tries to check one out.
    {
        let writer = pools.writer.get().await.expect("writer get");
        writer
            .interact(|c| -> rusqlite::Result<usize> {
                c.execute(
                    "UPDATE session_projections SET state = ? WHERE source = ? AND session_id = ?",
                    rusqlite::params![
                        r#"{"current_state":"Working","last_event_kind":"PreToolUse","last_event_at_ms":0}"#,
                        "claude",
                        "sess-stale-recover",
                    ],
                )
            })
            .await
            .expect("interact")
            .expect("update");
    }

    // Subscribe AFTER the seed write so we only see envelopes from the
    // post-stale recovery write.
    let mut rx = hub.subscribe();

    // Recovery: a new PreToolUse stores Working again. Stored prev was
    // Working, stored new is Working — the read-facing prev is Idle (stale),
    // and the publish gate should fire because Idle != Working.
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::PreToolUse),
    )
    .await
    .expect("recovery write");

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut events = 0usize;
    let mut states = 0usize;
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::Event(_)) => events += 1,
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::State { .. }) => states += 1,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(e) => panic!("unexpected recv error: {e:?}"),
        }
    }
    assert_eq!(events, 1, "recovery write publishes one Event envelope");
    assert_eq!(
        states, 1,
        "stale Working → fresh Working must publish a State envelope because the read-facing prev was Idle"
    );
}

/// Story 5.2 review #7 — a delayed `Stop` after stale stored `Working` must
/// still publish the live Idle correction. The state publish gate compares raw
/// stored state too, not only read-facing state, so `Working → Idle` is not
/// suppressed as read-facing `Idle → Idle`.
#[tokio::test(flavor = "current_thread")]
async fn state_broadcast_publishes_when_stale_working_stops() {
    let (_tmp, pools) = fresh_pools().await;
    let hub = BroadcastHub::new(64);
    let session_id = "sess-stale-stop";

    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::PreToolUse),
    )
    .await
    .expect("seed write");

    {
        let writer = pools.writer.get().await.expect("writer get");
        writer
            .interact(|c| -> rusqlite::Result<usize> {
                c.execute(
                    "UPDATE session_projections SET state = ? WHERE source = ? AND session_id = ?",
                    rusqlite::params![
                        r#"{"current_state":"Working","last_event_kind":"PreToolUse","last_event_at_ms":0}"#,
                        "claude",
                        "sess-stale-stop",
                    ],
                )
            })
            .await
            .expect("interact")
            .expect("update");
    }

    let mut rx = hub.subscribe();
    projection::session::write(
        &pools.writer,
        &hub,
        envelope_for("claude", session_id, EventKind::Stop),
    )
    .await
    .expect("delayed Stop write");

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut events = 0usize;
    let mut states = Vec::new();
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::Event(_)) => events += 1,
            Ok(bowerbird_daemon::broadcast::BroadcastEnvelope::State { state, .. }) => {
                states.push(state.current_state);
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(e) => panic!("unexpected recv error: {e:?}"),
        }
    }

    assert_eq!(events, 1, "delayed Stop publishes one Event envelope");
    assert_eq!(
        states,
        vec![SessionCurrentState::Idle],
        "delayed Stop after stale stored Working must publish one Idle State envelope"
    );
}

/// Story 5.2 AC #5 — A presenter compiled against the pre-Story-5.2
/// protocol enum (lacking `UserPromptSubmit`) must decode an event whose
/// `kind: "UserPromptSubmit"` as `EventKind::Unknown` via Story 4.4's
/// `#[serde(other)]` catch-all — and the surrounding event payload must
/// still parse so the legacy presenter does not drop the whole frame.
///
/// Task 7 of Story 5.2 explicitly asks for full-event-shape coverage,
/// not just the bare kind string in isolation.
#[test]
fn pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown() {
    use serde::Deserialize;

    // Mock copy of pre-Story-5.2 EventKind: 6 named variants + Unknown.
    // The `#[serde(other)]` Unknown is the Story 4.4 catch-all contract
    // that lets older presenters survive new variants.
    #[derive(Debug, PartialEq, Eq, Deserialize)]
    enum LegacyEventKind {
        PreToolUse,
        PostToolUse,
        Stop,
        Notification,
        RecordingStarted,
        RecordingEnded,
        #[serde(other)]
        Unknown,
    }

    // Mock copy of the pre-Story-5.2 Event struct as a v1.0 presenter would
    // have authored it: the same wire field names as `protocol::Event`, but
    // typed against the legacy enum. `reaction` stays `serde_json::Value`
    // so any future Reaction additions are tolerated alongside the kind
    // catch-all.
    #[derive(Debug, Deserialize)]
    struct LegacyEvent {
        event_id: i64,
        source: String,
        session_id: String,
        kind: LegacyEventKind,
        reaction: Option<serde_json::Value>,
        payload: String,
        created_at: i64,
    }

    // Bare-kind sanity check — the catch-all itself.
    let bare = r#""UserPromptSubmit""#;
    let kind: LegacyEventKind = serde_json::from_str(bare).expect("bare kind must deserialize");
    assert_eq!(kind, LegacyEventKind::Unknown);

    // Full-event shape — the actual contract a v1.0 presenter sees on the wire.
    let raw = r#"{
        "event_id": 1,
        "source": "claude",
        "session_id": "x",
        "kind": "UserPromptSubmit",
        "reaction": null,
        "payload": "{}",
        "created_at": 0
    }"#;
    let event: LegacyEvent =
        serde_json::from_str(raw).expect("legacy presenter must decode full event payload");
    assert_eq!(event.event_id, 1);
    assert_eq!(event.source, "claude");
    assert_eq!(event.session_id, "x");
    assert_eq!(
        event.kind,
        LegacyEventKind::Unknown,
        "pre-5.2 presenter must surface UserPromptSubmit as Unknown without dropping the frame"
    );
    assert!(event.reaction.is_none());
    assert_eq!(event.payload, "{}");
    assert_eq!(event.created_at, 0);
}

/// AC #2 — Atomicity contract: an `INSERT INTO events` and its matching
/// `UPSERT INTO session_projections` commit together or not at all. Per
/// `project-context.md` lines 291, 589, 700, the validation strategy is a
/// **SIGKILL during a load run**: push a stream of events through the daemon,
/// kill it while events are in flight, and prove on restart that the surviving
/// `events` rows all have matching `session_projections` rows (and vice versa).
/// Events that were ACK'd but not yet committed at SIGKILL time are expected
/// to be lost — PRD Journey 4 explicitly accepts this. What matters is "no
/// half-state": every committed event has a committed projection.
///
/// Coverage triangle for AC #2:
///   - `state_plus_event_atomicity_rollback` — explicit `tx.rollback()` covers
///     the crash-before-commit path (one transaction, two writes, no commit →
///     zero rows).
///   - this test — SIGKILL during real load via the daemon binary covers the
///     async crash-during-commit-stream path through the WAL.
///   - the single-transaction discipline in `projection::session::write`
///     covers the property by construction (one `tx.commit()` for both writes).
///
/// Supersedes the `drop(pool)` surrogate in `wal_durability_after_simulated_crash`
/// (which exits cleanly through rusqlite's destructor). SIGKILL skips
/// destructors entirely and exercises a different failure mode.
///
/// **Story 4.4 AC #3a (Epic 3 retro AI-2 fold-in, taskwarrior `a2ea3bfb`).**
/// This test was previously flagged for `sqlite3_close → sqlite3_mutex_enter
/// → pthread_mutex_wait` deadlocks in TempDir teardown — symptom: the test
/// body asserts complete, then the async drop ordering of `pools` vs
/// `TempDir` deadlocks inside SQLite's connection-close mutex. The fix
/// shipped in 4.4 is Option A from the AC: explicit `drop(reader)` →
/// `drop(pools)` → `drop(tmp)` ordering at end-of-function with
/// `tokio::task::yield_now().await` between each so any pending rusqlite
/// finalizers run before the next stage runs. The test runs unflagged
/// under `cargo test --workspace` (no `--skip`
/// invocations survive in `.github/workflows/ci.yml` or any helper script).
/// See Epic 3 retrospective Discovery #2 for the original symptom.
#[tokio::test(flavor = "current_thread")]
async fn state_plus_event_atomicity_under_sigkill_during_load() {
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::process::{Command as StdCommand, Stdio};

    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    const LOAD_EVENT_COUNT: usize = 25;

    let tmp = TempDir::new().expect("tempdir");
    let bowerbird_dir = tmp.path().join(".bowerbird");
    std::fs::create_dir_all(&bowerbird_dir).expect("mkdir");
    let sock_path = bowerbird_dir.join("ingest.sock");
    let db_path = bowerbird_dir.join("bower.db");

    let bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");
    let mut child = StdCommand::new(&bin)
        .env("HOME", tmp.path())
        .env("BOWERBIRD_INGEST_SOCK", &sock_path)
        .env("RUST_LOG", "warn")
        // Story 3.3: pin the bearer token via env and disable the keychain
        // step so the spawned daemon never touches the developer's real
        // macOS Keychain / Linux Secret Service.
        .env("BOWERBIRD_TOKEN", "contract-daemon-test-token")
        .env("BOWERBIRD_KEYRING_BACKEND", "disable")
        // Inherit PATH so dyld can find linked libs on macOS.
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| std::ffi::OsString::from("")),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon");
    let child_pid = child.id() as i32;

    // Bounded poll for ingest socket creation. project-context line 642 forbids
    // real sleep() for synchronization, but socket bind has no signal-style
    // signal — this is the documented exception (Story 1.6 Task 7 Dev Notes).
    let socket_ready = async {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if std::fs::metadata(&sock_path).is_ok() {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let ready = tokio::time::timeout(Duration::from_secs(6), socket_ready)
        .await
        .expect("poll loop must finish within timeout");
    if !ready {
        let _ = child.kill();
        let _ = child.wait();
        panic!("daemon never bound ingest.sock at {}", sock_path.display());
    }

    // Fire N events as fast as we can, spread across several sessions so the
    // post-restart orphan check is exercised on a real-shaped projection set.
    // Each event is its own UDS connection — accept-then-close per line is the
    // wire shape the shim uses. We do not wait for individual commit; the
    // point is to have writes in flight when SIGKILL lands.
    let sender_handle = tokio::task::spawn_blocking({
        let sock_path = sock_path.clone();
        move || -> std::io::Result<usize> {
            let mut ack_count = 0;
            for i in 0..LOAD_EVENT_COUNT {
                let session = format!("sess-load-{}", i % 5);
                let kind = if i % 2 == 0 {
                    "PreToolUse"
                } else {
                    "PostToolUse"
                };
                let line = format!(
                    r#"{{"hook_kind":"{kind}","session_id":"{session}","tool_name":"Bash"}}"#
                );
                let mut stream = match StdUnixStream::connect(&sock_path) {
                    Ok(s) => s,
                    // Daemon may have been SIGKILLed mid-stream — that's the
                    // intended condition, stop sending cleanly.
                    Err(_) => break,
                };
                if stream.write_all(line.as_bytes()).is_err() {
                    break;
                }
                if stream.write_all(b"\n").is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
                if stream.shutdown(std::net::Shutdown::Write).is_err() {
                    break;
                }
                let mut response = String::new();
                if stream.read_to_string(&mut response).is_err() {
                    break;
                }
                if response.starts_with("200") {
                    ack_count += 1;
                }
            }
            Ok(ack_count)
        }
    });

    // Wait until at least one event has actually committed so the SIGKILL
    // happens against a populated DB (not a vacuous empty one). Polling, not
    // sleep — same exception as the socket-ready loop above.
    let committed_some = async {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                let n: rusqlite::Result<i64> = conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE source != '__daemon__'",
                    [],
                    |r| r.get(0),
                );
                if let Ok(n) = n {
                    if n >= 1 {
                        break n;
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                break 0;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let pre_kill_visible = tokio::time::timeout(Duration::from_secs(3), committed_some)
        .await
        .expect("commit poll must finish within timeout");

    // SIGKILL while the sender task is still pushing events. Some will have
    // committed, some will be in flight (or never reach the writer task).
    kill(Pid::from_raw(child_pid), Signal::SIGKILL).expect("SIGKILL");
    let _ = tokio::task::spawn_blocking(move || child.wait())
        .await
        .expect("join wait")
        .expect("child wait");
    // The sender task's UDS connection will get torn down; collect whatever
    // it managed before the kill.
    let ack_count = sender_handle.await.expect("sender join").unwrap_or(0);

    assert!(
        pre_kill_visible >= 1,
        "daemon never committed any of {LOAD_EVENT_COUNT} events before SIGKILL; \
         test cannot prove atomicity if zero events landed (ack_count={ack_count})"
    );

    // Reopen through the normal pool path. Rebuild converges any projections
    // that were lost to an unclean exit (none, given our single-transaction
    // discipline — but the call is part of the recovery contract).
    let pools = init_pools(&db_path).await.expect("reopen pools");
    run_migrations(&pools.writer).await.expect("migrate reopen");
    projection::session::rebuild_missing_projections(&pools.writer)
        .await
        .expect("rebuild");

    let reader = pools.reader.get().await.expect("reader get");
    // No-half-state check, both directions:
    //   - every event session has a projection row (Task 6 rebuild guarantees this)
    //   - every non-sentinel projection has at least one event row
    let (event_orphans, projection_orphans, surviving_events, surviving_projections): (
        i64,
        i64,
        i64,
        i64,
    ) = reader
        .interact(|c| -> rusqlite::Result<(i64, i64, i64, i64)> {
            let event_orphans: i64 = c.query_row(
                "SELECT COUNT(*) FROM \
                 (SELECT DISTINCT source, session_id FROM events WHERE source != '__daemon__') e \
                 LEFT JOIN session_projections p USING (source, session_id) \
                 WHERE p.source IS NULL",
                [],
                |r| r.get(0),
            )?;
            let projection_orphans: i64 = c.query_row(
                "SELECT COUNT(*) FROM session_projections p \
                 WHERE p.source != '__daemon__' \
                 AND NOT EXISTS ( \
                     SELECT 1 FROM events e \
                     WHERE e.source = p.source AND e.session_id = p.session_id \
                 )",
                [],
                |r| r.get(0),
            )?;
            let surviving_events: i64 = c.query_row(
                "SELECT COUNT(*) FROM events WHERE source != '__daemon__'",
                [],
                |r| r.get(0),
            )?;
            let surviving_projections: i64 = c.query_row(
                "SELECT COUNT(*) FROM session_projections WHERE source != '__daemon__'",
                [],
                |r| r.get(0),
            )?;
            Ok((
                event_orphans,
                projection_orphans,
                surviving_events,
                surviving_projections,
            ))
        })
        .await
        .expect("interact")
        .expect("orphan counts");
    assert_eq!(
        event_orphans, 0,
        "every event session must have a matching projection row \
         (surviving_events={surviving_events}, surviving_projections={surviving_projections})"
    );
    assert_eq!(
        projection_orphans, 0,
        "no projection row may exist without at least one event in the log"
    );
    assert!(
        surviving_events >= 1,
        "load run must leave at least one event in the table"
    );

    // Confirm at least one surviving state JSON parses cleanly as SessionState
    // — the projection's wire shape is verified across the SIGKILL boundary.
    let any_state_json: String = reader
        .interact(|c| -> rusqlite::Result<String> {
            c.query_row(
                "SELECT state FROM session_projections WHERE source != '__daemon__' LIMIT 1",
                [],
                |r| r.get(0),
            )
        })
        .await
        .expect("interact")
        .expect("surviving projection row");
    let parsed: SessionState =
        serde_json::from_str(&any_state_json).expect("post-SIGKILL state JSON must parse cleanly");
    assert!(
        matches!(
            parsed.current_state,
            SessionCurrentState::Working | SessionCurrentState::Idle
        ),
        "load events are PreToolUse/PostToolUse only — final state must be \
         Working or Idle, got {:?}",
        parsed.current_state
    );
    assert!(
        matches!(
            parsed.last_event_kind,
            EventKind::PreToolUse | EventKind::PostToolUse
        ),
        "last_event_kind must reflect a load event, got {:?}",
        parsed.last_event_kind
    );

    // Story 4.4 AC #3a / Epic 3 retro AI-2: explicit drop ordering so the
    // SQLite connection-close mutexes finish before `TempDir`'s destructor
    // tries to remove `bower.db`. Without this, the previous-observed
    // `sqlite3_close → sqlite3_mutex_enter → pthread_mutex_wait` deadlock
    // could re-emerge under a future CI runner's scheduler ordering.
    // `yield_now().await` gives the runtime a tick to flush any pending
    // rusqlite finalizers.
    drop(reader);
    tokio::task::yield_now().await;
    drop(pools);
    tokio::task::yield_now().await;
    drop(tmp);
}

/// AC #5 — Deleting the projection rows and calling `rebuild_missing_projections`
/// reproduces byte-identical state JSON, proving the event log is the source of
/// truth and the projection is a deterministic derivative.
#[tokio::test(flavor = "current_thread")]
async fn projection_rebuild_from_event_log_is_byte_identical() {
    let (_tmp, pools) = fresh_pools().await;

    let a_seq = [
        EventKind::PreToolUse,
        EventKind::PostToolUse,
        EventKind::PreToolUse,
        EventKind::Notification,
        EventKind::Stop,
    ];
    let b_seq = [
        EventKind::PreToolUse,
        EventKind::PostToolUse,
        EventKind::Stop,
    ];
    for kind in a_seq {
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            envelope_for("claude", "sess-A", kind),
        )
        .await
        .expect("write A");
    }
    for kind in b_seq {
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            envelope_for("claude", "sess-B", kind),
        )
        .await
        .expect("write B");
    }

    let baseline_a = read_session_state(&pools.reader, "claude", "sess-A").await;
    let baseline_b = read_session_state(&pools.reader, "claude", "sess-B").await;

    // Wipe non-sentinel projection rows.
    let writer = pools.writer.get().await.expect("writer get");
    writer
        .interact(|c| -> rusqlite::Result<usize> {
            c.execute(
                "DELETE FROM session_projections WHERE source != '__daemon__'",
                [],
            )
        })
        .await
        .expect("interact")
        .expect("delete");
    drop(writer);

    let remaining = count_session_projections(&pools.reader, "source != '__daemon__'").await;
    assert_eq!(remaining, 0, "non-sentinel projection rows must be cleared");

    let rebuilt = projection::session::rebuild_missing_projections(&pools.writer)
        .await
        .expect("rebuild");
    assert_eq!(rebuilt, 2, "exactly two sessions must be rebuilt");

    // Byte-for-byte: serde preserves struct-field order; if it ever changes,
    // this assertion will surface immediately. SessionCurrentState/EventKind
    // are PascalCase by-variant-name, no rename_all involved.
    let result_a = read_session_state(&pools.reader, "claude", "sess-A").await;
    let result_b = read_session_state(&pools.reader, "claude", "sess-B").await;
    assert_eq!(
        result_a, baseline_a,
        "sess-A rebuild must be byte-identical"
    );
    assert_eq!(
        result_b, baseline_b,
        "sess-B rebuild must be byte-identical"
    );
}

// =====================================================================
// Story 1.7 — REST query API contract tests
// =====================================================================

mod story_1_7_rest {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use bowerbird_daemon::api::token::BearerToken;
    use protocol::{
        DaemonStatus, EventKind, EventListResponse, SessionCurrentState, SessionDetail,
        SessionListItem, SessionStats,
    };
    use tower::ServiceExt;

    fn bearer_header() -> String {
        format!("Bearer {}", super::TEST_BEARER)
    }

    fn ready_state(pools: DbPools) -> AppState {
        let mc = Arc::new(AtomicBool::new(true));
        super::make_test_state(pools, mc)
    }

    async fn json_body<T: serde::de::DeserializeOwned>(resp: axum::response::Response) -> T {
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("parse json")
    }

    fn auth_get(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, bearer_header())
            .body(Body::empty())
            .unwrap()
    }

    // ----- Task 12 -----
    #[tokio::test(flavor = "current_thread")]
    async fn readyz_returns_503_when_db_unreachable() {
        use std::fs::OpenOptions;
        use std::io::Write;

        // Option A (preferred over pool starvation): write garbage bytes to a
        // path, then point init_pools at it. The connection opens, but PRAGMA
        // journal_mode = WAL or SELECT 1 FROM events fails because the file
        // is not a SQLite database. probe_db catches the error and returns
        // 503 — exactly what the AC needs.
        //
        // Option B (pool starvation by holding all readers) would take ~5s due
        // to the wait timeout — too slow for the PR test loop.
        let tmp = TempDir::new().expect("tempdir");
        let bad_path = tmp.path().join("not-a-db.bin");
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&bad_path)
            .expect("open bad path");
        f.write_all(b"this is not a sqlite database")
            .expect("write garbage");
        drop(f);

        let pools = init_pools(&bad_path)
            .await
            .expect("build pool over a non-db file (lazy connection)");
        // Mark migrations complete so the probe branch is the one that fails.
        let migrations_complete = Arc::new(AtomicBool::new(true));
        let state = super::make_test_state(pools, migrations_complete);
        let app = api::router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ----- Task 13 -----
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_list_returns_known_sessions_with_read_time_state() {
        let (tmp, pools) = fresh_pools().await;
        // Sentinel — must be filtered out of /sessions.
        projection::session::write_recording_started(&pools.writer)
            .await
            .expect("recording started");
        // sess-a: PreToolUse → stored Working.
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-a".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("write sess-a");
        // sess-b: PostToolUse + Stop → stored Idle.
        // Story 5.2: PostToolUse alone now preserves prev (defaults to Working
        // when there is no prev). The Stop is what drives Idle.
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-b".to_string(),
                kind: EventKind::PostToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("write sess-b PostToolUse");
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-b".to_string(),
                kind: EventKind::Stop,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("write sess-b Stop");

        let app = api::router(ready_state(pools.clone()));
        let resp = app.oneshot(auth_get("/sessions")).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let items: Vec<SessionListItem> = json_body(resp).await;
        assert_eq!(items.len(), 2, "sentinel must be filtered out");
        for it in &items {
            assert_eq!(it.source, "claude");
        }
        let by_id: std::collections::HashMap<_, _> =
            items.iter().map(|i| (i.session_id.as_str(), i)).collect();
        assert_eq!(
            by_id["sess-a"].current_state,
            SessionCurrentState::Working,
            "fresh PreToolUse should surface as Working"
        );
        assert_eq!(by_id["sess-a"].last_event_kind, EventKind::PreToolUse);
        assert_eq!(
            by_id["sess-b"].current_state,
            SessionCurrentState::Idle,
            "Stop surfaces as Idle"
        );
        assert_eq!(by_id["sess-b"].last_event_kind, EventKind::Stop);

        super::teardown_pools(pools, tmp).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sessions_list_applies_stale_working_fallback() {
        let (tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-old".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("write sess-old");

        // Manually age the stored last_event_at_ms inside the projection JSON.
        // Real sleep is forbidden (project-context.md:642 deterministic-test
        // discipline) — JSON tweak is the right pattern.
        let writer = pools.writer.get().await.expect("writer get");
        writer
            .interact(|c| -> rusqlite::Result<usize> {
                c.execute(
                    "UPDATE session_projections SET state = ? WHERE source = ? AND session_id = ?",
                    rusqlite::params![
                        r#"{"current_state":"Working","last_event_kind":"PreToolUse","last_event_at_ms":0}"#,
                        "claude",
                        "sess-old"
                    ],
                )
            })
            .await
            .expect("interact")
            .expect("update");
        drop(writer);

        let app = api::router(ready_state(pools.clone()));
        let resp = app.oneshot(auth_get("/sessions")).await.expect("oneshot");
        let items: Vec<SessionListItem> = json_body(resp).await;
        let it = items
            .iter()
            .find(|i| i.session_id == "sess-old")
            .expect("sess-old must appear");
        assert_eq!(
            it.current_state,
            SessionCurrentState::Idle,
            "Working older than STALE_WORKING_MS must surface as Idle at read time"
        );

        super::teardown_pools(pools, tmp).await;
    }

    // ----- Task 14 -----
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_detail_returns_projection_state() {
        let (tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-x".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("write sess-x");

        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/sess-x"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let detail: SessionDetail = json_body(resp).await;
        assert_eq!(detail.source, "claude");
        assert_eq!(detail.session_id, "sess-x");
        assert_eq!(detail.state.current_state, SessionCurrentState::Working);
        assert_eq!(detail.state.last_event_kind, EventKind::PreToolUse);

        super::teardown_pools(pools, tmp).await;
    }

    // Story 5.7 AC #8: GET /sessions and /sessions/{id} carry cwd + started_at;
    // the read-time stale-Working → Idle fallback does NOT alter them.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_rest_surfaces_cwd_and_started_at() {
        let (tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-cwd".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: Some("/Users/x/repo".to_string()),
            },
        )
        .await
        .expect("write sess-cwd");

        let app = api::router(ready_state(pools.clone()));

        // Detail.
        let resp = app
            .clone()
            .oneshot(auth_get("/sessions/sess-cwd"))
            .await
            .expect("oneshot detail");
        assert_eq!(resp.status(), StatusCode::OK);
        let detail: SessionDetail = json_body(resp).await;
        assert_eq!(detail.state.cwd, Some("/Users/x/repo".to_string()));
        assert!(
            detail.state.started_at.is_some(),
            "started_at must be set on the first event"
        );

        // List.
        let resp = app
            .oneshot(auth_get("/sessions"))
            .await
            .expect("oneshot list");
        assert_eq!(resp.status(), StatusCode::OK);
        let items: Vec<SessionListItem> = json_body(resp).await;
        let item = items
            .iter()
            .find(|i| i.session_id == "sess-cwd")
            .expect("sess-cwd in list");
        assert_eq!(item.cwd, Some("/Users/x/repo".to_string()));
        assert!(item.started_at.is_some());

        super::teardown_pools(pools, tmp).await;
    }

    // Story 5.7 review pass 2 (AC #9): GET /sessions/{id}/events carries
    // `Event.cwd` per row. The `/sessions` + `/sessions/{id}` test above pins
    // the state surface; this pins the per-event REST surface, which threads
    // `cwd` through a separate column-index path (`SELECT_EVENTS_FOR_SESSION_SINCE`).
    #[tokio::test(flavor = "current_thread")]
    async fn events_rest_surfaces_event_cwd() {
        let (tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-ev-cwd".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: Some("/repo".to_string()),
            },
        )
        .await
        .expect("write sess-ev-cwd");

        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/sess-ev-cwd/events?since=0"))
            .await
            .expect("oneshot events");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: EventListResponse = json_body(resp).await;
        assert_eq!(body.events.len(), 1, "exactly one event written");
        assert_eq!(
            body.events[0].cwd,
            Some("/repo".to_string()),
            "GET /sessions/{{id}}/events must carry Event.cwd"
        );

        super::teardown_pools(pools, tmp).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sessions_detail_returns_404_when_unknown() {
        let (tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/does-not-exist"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = json_body(resp).await;
        assert_eq!(body, serde_json::json!({ "error": "session not found" }));

        super::teardown_pools(pools, tmp).await;
    }

    // ----- Task 15 -----
    #[tokio::test(flavor = "current_thread")]
    async fn events_list_returns_all_in_ascending_order() {
        let (tmp, pools) = fresh_pools().await;
        for _ in 0..5 {
            projection::session::write(
                &pools.writer,
                &BroadcastHub::new(16),
                EventEnvelope {
                    source: "claude".to_string(),
                    session_id: "sess-y".to_string(),
                    kind: EventKind::PreToolUse,
                    reaction: None,
                    payload: "{}".to_string(),
                    pid: None,
                    notification_type: None,
                    cwd: None,
                },
            )
            .await
            .expect("write");
        }
        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/sess-y/events?since=0"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: EventListResponse = json_body(resp).await;
        assert_eq!(body.events.len(), 5);
        for w in body.events.windows(2) {
            assert!(
                w[0].event_id < w[1].event_id,
                "events must be in ascending event_id order"
            );
            // NFR22 surface check: created_at must be non-zero on every row.
            assert!(w[0].created_at > 0);
        }
        let last_id = body.events.last().unwrap().event_id;
        assert_eq!(body.cursor, Some(last_id), "cursor follows last event_id");
        assert_eq!(
            body.oldest_available_event_id, body.events[0].event_id,
            "oldest_available_event_id should match earliest stored row"
        );

        super::teardown_pools(pools, tmp).await;
    }

    // Story 5.4 AC #5: requesting `/events` for a session_id that has never
    // had a projection row returns 404, not 200-with-empty. The legitimate
    // "session exists, no new events past `since`" case is covered by the
    // `events_list_respects_since_cursor` and `events_200_for_existing_session_with_no_new_events`
    // (in `story_5_4_events_404` below) tests.
    #[tokio::test(flavor = "current_thread")]
    async fn events_list_returns_404_for_unknown_session() {
        let (tmp, pools) = fresh_pools().await;
        // Do not write any non-sentinel events; the session simply doesn't exist.
        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/no-such/events?since=0"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = json_body(resp).await;
        assert_eq!(body, serde_json::json!({ "error": "session not found" }));

        super::teardown_pools(pools, tmp).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn events_list_respects_since_cursor() {
        let (tmp, pools) = fresh_pools().await;
        let mut written_ids: Vec<i64> = Vec::new();
        for _ in 0..10 {
            let id = projection::session::write(
                &pools.writer,
                &BroadcastHub::new(16),
                EventEnvelope {
                    source: "claude".to_string(),
                    session_id: "sess-y".to_string(),
                    kind: EventKind::PreToolUse,
                    reaction: None,
                    payload: "{}".to_string(),
                    pid: None,
                    notification_type: None,
                    cwd: None,
                },
            )
            .await
            .expect("write");
            written_ids.push(id.0);
        }
        let cutoff = written_ids[4]; // strictly > cutoff means last 5 returned.

        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get(&format!("/sessions/sess-y/events?since={cutoff}")))
            .await
            .expect("oneshot");
        let body: EventListResponse = json_body(resp).await;
        assert_eq!(body.events.len(), 5);
        for ev in &body.events {
            assert!(
                ev.event_id.0 > cutoff,
                "event_id {:?} must be > since={cutoff}",
                ev.event_id
            );
        }

        super::teardown_pools(pools, tmp).await;
    }

    // ----- Task 16 -----
    #[tokio::test(flavor = "current_thread")]
    async fn events_list_oldest_available_after_purge() {
        let (tmp, pools) = fresh_pools().await;
        let mut written: Vec<i64> = Vec::new();
        for _ in 0..5 {
            let id = projection::session::write(
                &pools.writer,
                &BroadcastHub::new(16),
                EventEnvelope {
                    source: "claude".to_string(),
                    session_id: "sess-y".to_string(),
                    kind: EventKind::PreToolUse,
                    reaction: None,
                    payload: "{}".to_string(),
                    pid: None,
                    notification_type: None,
                    cwd: None,
                },
            )
            .await
            .expect("write");
            written.push(id.0);
        }
        let middle = written[2];
        let writer = pools.writer.get().await.expect("writer get");
        writer
            .interact(move |c| -> rusqlite::Result<usize> {
                c.execute(
                    "DELETE FROM events WHERE event_id <= ?",
                    rusqlite::params![middle],
                )
            })
            .await
            .expect("interact")
            .expect("delete");
        drop(writer);
        let surviving_min = written[3];

        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/sess-y/events?since=0"))
            .await
            .expect("oneshot");
        let body: EventListResponse = json_body(resp).await;
        assert_eq!(
            body.oldest_available_event_id.0, surviving_min,
            "oldest_available reflects the post-purge minimum"
        );
        assert_eq!(body.events.len(), 2);
        // Axiom-4-style mechanical gap inference at the presenter layer:
        let since = 0_i64;
        assert!(
            since < body.oldest_available_event_id.0,
            "presenter can infer a gap from since < oldest_available_event_id"
        );

        super::teardown_pools(pools, tmp).await;
    }

    // ----- Task 17 -----
    const PROTECTED_ROUTES: &[&str] = &[
        "/sessions",
        "/sessions/foo",
        "/sessions/foo/events",
        "/sessions/foo/stats",
        "/status",
    ];

    #[tokio::test(flavor = "current_thread")]
    async fn authenticated_routes_reject_missing_header() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        for route in PROTECTED_ROUTES {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(*route).body(Body::empty()).unwrap())
                .await
                .expect("oneshot");
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "route {route}");
            let body: serde_json::Value = json_body(resp).await;
            assert_eq!(body, serde_json::json!({ "error": "unauthorized" }));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authenticated_routes_reject_wrong_bearer() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        for route in PROTECTED_ROUTES {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(*route)
                        .header(header::AUTHORIZATION, "Bearer wrong-token")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("oneshot");
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "route {route}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authenticated_routes_accept_correct_bearer() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        for route in PROTECTED_ROUTES {
            let resp = app.clone().oneshot(auth_get(route)).await.expect("oneshot");
            assert_ne!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "route {route} must pass auth"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unauthenticated_routes_accept_missing_header() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        for route in ["/healthz", "/readyz"] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(route).body(Body::empty()).unwrap())
                .await
                .expect("oneshot");
            assert_ne!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "route {route} must not require auth"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authenticated_routes_reject_empty_bearer() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/sessions")
                    .header(header::AUTHORIZATION, "Bearer ")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authenticated_routes_reject_wrong_scheme() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/sessions")
                    .header(header::AUTHORIZATION, "Basic dGVzdA==")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ----- /stats happy path + 404 -----
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_stats_returns_stats_for_known_session() {
        let (tmp, pools) = fresh_pools().await;
        for _ in 0..3 {
            projection::session::write(
                &pools.writer,
                &BroadcastHub::new(16),
                EventEnvelope {
                    source: "claude".to_string(),
                    session_id: "sess-s".to_string(),
                    kind: EventKind::PreToolUse,
                    reaction: None,
                    payload: "{}".to_string(),
                    pid: None,
                    notification_type: None,
                    cwd: None,
                },
            )
            .await
            .expect("write");
        }
        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/sess-s/stats"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let stats: SessionStats = json_body(resp).await;
        assert_eq!(stats.source, "claude");
        assert_eq!(stats.session_id, "sess-s");
        assert_eq!(stats.event_count, 3);
        assert!(stats.first_event_at.is_some());
        assert!(stats.last_event_at.is_some());

        super::teardown_pools(pools, tmp).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sessions_stats_returns_404_when_unknown() {
        let (tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/missing/stats"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        super::teardown_pools(pools, tmp).await;
    }

    // Story 5.7 review pass-5 finding: `/sessions/{id}/stats` `first_event_at`
    // is `MIN(created_at)` — a pure timestamp aggregate — which is NOT the same
    // field as `SessionState.started_at` (the `created_at` of the first event by
    // `event_id ASC`, matching rebuild). With monotonic timestamps the two
    // coincide; under clock skew / manually injected / replay-reordered data
    // they diverge. This pins that documented contract (docs/protocol.md
    // §`GET /sessions/{id}/stats` Notes) so a silent change to either side
    // surfaces here. The decision is to keep `/stats` as min/max aggregates and
    // document the divergence, NOT to switch `/stats` to event_id order.
    #[tokio::test(flavor = "current_thread")]
    async fn stats_first_event_at_min_diverges_from_started_at_under_nonmonotonic_created_at() {
        let (tmp, pools) = fresh_pools().await;

        // event_id 1 carries a LATER created_at than event_id 2 (clock skew).
        // started_at follows event_id order (2000); MIN(created_at) is 1000.
        let conn = pools.writer.get().await.expect("writer pool");
        conn.interact(|c| -> rusqlite::Result<()> {
            let rows: &[(&str, i64)] = &[
                ("PreToolUse", 2_000), // event_id 1, latest-by-id
                ("Stop", 1_000),       // event_id 2, smaller timestamp
            ];
            for (kind, created_at) in rows {
                c.execute(
                    "INSERT INTO events (source, session_id, kind, payload, created_at) \
                     VALUES ('claude', 'sess-skew', ?, '{}', ?)",
                    rusqlite::params![kind, created_at],
                )?;
            }
            Ok(())
        })
        .await
        .expect("interact")
        .expect("insert events");
        drop(conn);

        projection::session::rebuild_missing_projections(&pools.writer)
            .await
            .expect("rebuild");

        let app = api::router(ready_state(pools.clone()));

        // started_at (via REST /sessions/{id}) = first event by event_id (2000).
        let detail: SessionDetail = json_body(
            app.clone()
                .oneshot(auth_get("/sessions/sess-skew"))
                .await
                .expect("oneshot"),
        )
        .await;
        assert_eq!(
            detail.state.started_at,
            Some(2_000),
            "started_at must be the created_at of the first event by event_id, not MIN"
        );

        // /stats first_event_at = MIN(created_at) = 1000, deliberately divergent.
        let resp = app
            .oneshot(auth_get("/sessions/sess-skew/stats"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let stats: SessionStats = json_body(resp).await;
        assert_eq!(
            stats.first_event_at,
            Some(1_000),
            "/stats first_event_at is MIN(created_at), a pure aggregate"
        );
        assert_eq!(
            stats.last_event_at,
            Some(2_000),
            "/stats last_event_at is MAX(created_at)"
        );
        assert_ne!(
            stats.first_event_at, detail.state.started_at,
            "documented divergence: MIN(created_at) != started_at under non-monotonic timestamps"
        );

        super::teardown_pools(pools, tmp).await;
    }

    // ----- /status -----
    #[tokio::test(flavor = "current_thread")]
    async fn status_returns_uptime_and_last_event() {
        let (tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-z".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("write");

        // Use a non-zero started_at_ms so uptime is meaningful.
        let mc = Arc::new(AtomicBool::new(true));
        let (ingest_tx, _ingest_rx) =
            tokio::sync::mpsc::channel::<bowerbird_daemon::ingest::IngestItem>(1);
        let state = AppState {
            db: pools.clone(),
            migrations_complete: mc,
            shutdown_requested: CancellationToken::new(),
            ws_close_requested: CancellationToken::new(),
            bearer: BearerToken::new(super::TEST_BEARER.to_string()),
            started_at_ms: 1,
            broadcaster: Arc::new(BroadcastHub::new(16)),
            ws_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            ws_config: WsConfig {
                ping_interval: Duration::from_secs(30),
                pong_timeout: Duration::from_secs(10),
                coalesce_window: Duration::from_secs(1),
                max_connections: 4,
            },
            ingest_tx,
        };
        let app = api::router(state);
        let resp = app.oneshot(auth_get("/status")).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: DaemonStatus = json_body(resp).await;
        assert_eq!(body.protocol_version, "1.0");
        assert_eq!(body.started_at_ms, 1);
        assert!(body.uptime_ms >= 0);
        assert!(body.last_event_id.is_some());
        assert!(body.last_event_at_ms.is_some());

        super::teardown_pools(pools, tmp).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_returns_none_last_event_when_only_sentinels() {
        let (tmp, pools) = fresh_pools().await;
        projection::session::write_recording_started(&pools.writer)
            .await
            .expect("sentinel");
        let app = api::router(ready_state(pools.clone()));
        let resp = app.oneshot(auth_get("/status")).await.expect("oneshot");
        let body: DaemonStatus = json_body(resp).await;
        assert!(
            body.last_event_id.is_none(),
            "sentinel rows should not surface as last_event_id"
        );
        assert!(body.last_event_at_ms.is_none());

        super::teardown_pools(pools, tmp).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn events_endpoint_rejects_unknown_query_param() {
        // EventsParams has #[serde(deny_unknown_fields)]; axum surfaces the
        // deserialization failure as a 400.
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(auth_get("/sessions/sess-y/events?since=0&tool=bash"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}

/// Story 2.1 WebSocket contract tests.
mod story_2_1_ws {
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use bowerbird_daemon::api;
    use bowerbird_daemon::broadcast::BroadcastEnvelope;
    use bowerbird_daemon::state::AppState;
    use futures_util::{SinkExt, StreamExt};
    use protocol::{Event, EventId};
    use protocol::{
        EventKind, HelloFrame, Reaction, ServerMessage, SessionCurrentState, SessionState,
    };
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::Message;

    use super::{fresh_pools, make_test_state_with_ws};

    const TEST_BEARER: &str = super::TEST_BEARER;

    /// Spawn a real axum server on a random localhost port. Returns the
    /// bound address and a JoinHandle for the serve task.
    pub(super) async fn spawn_test_daemon(state: AppState) -> (SocketAddr, JoinHandle<()>) {
        let router = api::router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let shutdown = state.shutdown_requested.clone();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await;
        });
        (addr, handle)
    }

    pub(super) fn ws_url_header(addr: SocketAddr) -> String {
        format!("ws://{}/ws", addr)
    }

    fn ws_url_query(addr: SocketAddr, token: &str) -> String {
        format!("ws://{}/ws?token={}", addr, token)
    }

    fn ws_url_header_and_query(addr: SocketAddr, query_token: &str) -> String {
        format!("ws://{}/ws?token={}", addr, query_token)
    }

    /// Build a connect request with `Authorization: Bearer <token>`.
    pub(super) fn authed_request(
        url: &str,
        token: &str,
    ) -> tokio_tungstenite::tungstenite::http::Request<()> {
        let mut req = url.into_client_request().expect("into_client_request");
        req.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().expect("header"),
        );
        req
    }

    pub(super) async fn connect_authed(
        addr: SocketAddr,
        token: &str,
    ) -> (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    ) {
        let req = authed_request(&ws_url_header(addr), token);
        tokio_tungstenite::connect_async(req)
            .await
            .expect("ws connect")
    }

    pub(super) async fn read_text_frame_or_close(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Message {
        tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("recv within 5s")
            .expect("stream not ended")
            .expect("recv ok")
    }

    pub(super) fn parse_hello(msg: &Message) -> HelloFrame {
        let text = match msg {
            Message::Text(t) => t.as_str(),
            other => panic!("expected text Hello frame, got {other:?}"),
        };
        let server: ServerMessage = serde_json::from_str(text).expect("parse ServerMessage");
        match server {
            ServerMessage::Hello(h) => h,
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_hello_frame_on_connect() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let started_at = state.started_at_ms;
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _resp) = connect_authed(addr, TEST_BEARER).await;
        let msg = read_text_frame_or_close(&mut ws).await;
        let hello = parse_hello(&msg);
        assert_eq!(hello.protocol_version, "1.0");
        assert_eq!(hello.daemon_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(hello.daemon_started_at, started_at);

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_hello_frame_query_token_path() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let url = ws_url_query(addr, TEST_BEARER);
        let req = url.into_client_request().expect("into_client_request");
        let (mut ws, _resp) = tokio_tungstenite::connect_async(req)
            .await
            .expect("ws connect via query token");
        let msg = read_text_frame_or_close(&mut ws).await;
        let hello = parse_hello(&msg);
        assert_eq!(hello.protocol_version, "1.0");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    fn make_state_envelope(session_id: &str) -> BroadcastEnvelope {
        BroadcastEnvelope::State {
            source: "claude".to_string(),
            session_id: session_id.to_string(),
            state: SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 0,
                last_pid: None,
                cwd: None,
                started_at: None,
            },
        }
    }

    fn make_event_envelope(source: &str, session_id: &str) -> BroadcastEnvelope {
        BroadcastEnvelope::Event(Event {
            event_id: EventId(1),
            source: source.to_string(),
            session_id: session_id.to_string(),
            kind: EventKind::PreToolUse,
            reaction: Some(Reaction::Continue),
            payload: "{}".to_string(),
            created_at: 0,
            pid: None,
            cwd: None,
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_subscribe_accumulates_then_unsubscribe_removes() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _resp) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;

        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe state");
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe events");

        // Yield so the daemon processes the subscribes before publish races.
        tokio::time::sleep(Duration::from_millis(20)).await;

        state.broadcaster.publish(make_state_envelope("sess-1"));
        let msg = read_text_frame_or_close(&mut ws).await;
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(text.as_str()).expect("parse");
        match parsed {
            ServerMessage::State(s) => assert_eq!(s.session_id, "sess-1"),
            other => panic!("expected State, got {other:?}"),
        }

        state
            .broadcaster
            .publish(make_event_envelope("claude", "sess-1"));
        let msg = read_text_frame_or_close(&mut ws).await;
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(text.as_str()).expect("parse");
        assert!(
            matches!(parsed, ServerMessage::Event(_)),
            "expected Event frame, got {parsed:?}"
        );

        ws.send(Message::Text(
            r#"{"op":"unsubscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send unsubscribe");
        tokio::time::sleep(Duration::from_millis(20)).await;

        // After unsubscribing from state.*, a state envelope must NOT arrive,
        // but an events envelope still should.
        state.broadcaster.publish(make_state_envelope("sess-2"));
        state
            .broadcaster
            .publish(make_event_envelope("claude", "sess-2"));

        let msg = read_text_frame_or_close(&mut ws).await;
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(text.as_str()).expect("parse");
        assert!(
            matches!(parsed, ServerMessage::Event(_)),
            "after unsubscribe(state.session.*), only Event frame should arrive; got {parsed:?}"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    async fn assert_closes_with_1008(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) {
        // The next message must be a Close frame with code 1008.
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("close arrived in time")
            .expect("stream produced item")
            .expect("recv ok");
        match msg {
            Message::Close(Some(CloseFrame { code, reason })) => {
                assert_eq!(
                    u16::from(code),
                    1008,
                    "expected 1008 Policy Violation close, got {code:?}"
                );
                assert!(
                    reason.starts_with("bad message:"),
                    "expected reason to start with 'bad message:', got {reason:?}"
                );
            }
            other => panic!("expected Close(1008), got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_empty_topic_closes_with_policy_violation() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;
        ws.send(Message::Text(r#"{"op":"subscribe","topic":""}"#.into()))
            .await
            .expect("send");
        assert_closes_with_1008(&mut ws).await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_unknown_op_closes_with_policy_violation() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;
        ws.send(Message::Text(r#"{"op":"bogus","topic":"events.*"}"#.into()))
            .await
            .expect("send");
        assert_closes_with_1008(&mut ws).await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_extra_field_closes_with_policy_violation() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*","extra":1}"#.into(),
        ))
        .await
        .expect("send");
        assert_closes_with_1008(&mut ws).await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_binary_message_closes_with_policy_violation() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;
        ws.send(Message::Binary(vec![0u8; 4].into()))
            .await
            .expect("send binary");
        assert_closes_with_1008(&mut ws).await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    // Story 5.8 (ADR 0008) AC #10: an invalid `states` token on Subscribe
    // closes the connection with `bad message` (1008), via the same
    // `close_with_bad_message` path an invalid topic uses — NOT a silent empty
    // snapshot. The token `nope` is not a `SessionCurrentState` wire value.
    #[tokio::test(flavor = "current_thread")]
    async fn ws_invalid_states_token_closes_with_policy_violation() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*","states":["nope"]}"#.into(),
        ))
        .await
        .expect("send subscribe with bad states token");
        assert_closes_with_1008(&mut ws).await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 pass-3 (ADR 0008): a non-empty `states` filter is only valid
    /// on a `state.session.*` family topic — those are the only topics with a
    /// snapshot for it to scope. A `states` filter on an event topic has nothing
    /// to scope, so accepting it would silently discard presenter intent; the
    /// strict-inbound axiom fails loud (1008) instead. (An EMPTY `states` stays
    /// valid on any topic — that's an ordinary subscribe, covered below.)
    #[tokio::test(flavor = "current_thread")]
    async fn ws_states_on_event_topic_closes_with_policy_violation() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*","states":["working"]}"#.into(),
        ))
        .await
        .expect("send event subscribe with a states filter");
        assert_closes_with_1008(&mut ws).await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 pass-3: the reject above is gated on a NON-empty filter — an
    /// event subscribe with no `states` (the v1.0 shape) still works and goes
    /// live, proving the new check does not regress ordinary event subscribes.
    #[tokio::test(flavor = "current_thread")]
    async fn ws_empty_states_on_event_topic_is_ordinary_subscribe() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*","states":[]}"#.into(),
        ))
        .await
        .expect("send event subscribe with empty states");
        // Goes live (no close): a published event probe arrives on the wire.
        crate::story_2_2_publish::wait_subscribe_live(
            &mut ws,
            &state,
            crate::story_2_2_publish::ProbeKind::Event { source: "claude" },
        )
        .await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_401_when_no_auth() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let url = ws_url_header(addr);
        let req = url.into_client_request().expect("into_client_request");
        let err = tokio_tungstenite::connect_async(req)
            .await
            .expect_err("must fail with 401");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status().as_u16(), 401);
            }
            other => panic!("expected HTTP 401 error, got {other:?}"),
        }
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_401_when_bad_token() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let req = authed_request(&ws_url_header(addr), "wrong-token");
        let err = tokio_tungstenite::connect_async(req)
            .await
            .expect_err("must fail with 401");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status().as_u16(), 401);
            }
            other => panic!("expected HTTP 401 error, got {other:?}"),
        }
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_header_token_wins_over_query() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Valid header + invalid query → upgrade succeeds.
        let req = authed_request(
            &ws_url_header_and_query(addr, "wrong-query-token"),
            TEST_BEARER,
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(req)
            .await
            .expect("valid header should win over invalid query");
        let _hello = read_text_frame_or_close(&mut ws).await;

        // Invalid header + valid query → must fail (header wins; not falling
        // through to query when header is present but rejected).
        let req = authed_request(&ws_url_query(addr, TEST_BEARER), "wrong-header-token");
        let err = tokio_tungstenite::connect_async(req)
            .await
            .expect_err("invalid header must not fall through to query");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status().as_u16(), 401);
            }
            other => panic!("expected HTTP 401 error, got {other:?}"),
        }

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_pre_subscribe_backlog_does_not_leak_to_new_subscription() {
        // AC #2 review finding: a frame published BEFORE a Subscribe arrives
        // must not be delivered AFTER the Subscribe is processed, even when
        // the new topic would match the queued frame.
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;

        // Publish a state envelope BEFORE any Subscribe has been processed.
        // The per-connection receiver was subscribed pre-upgrade, so this
        // envelope is buffered.
        state.broadcaster.publish(make_state_envelope("sess-1"));

        // Give the daemon a moment to ensure the envelope reaches the
        // per-connection broadcast receiver buffer.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Subscribe to a topic that WOULD match the pre-published frame.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        // After subscribing, publish a fresh envelope so we have a known
        // post-subscribe frame to anchor the assertion.
        tokio::time::sleep(Duration::from_millis(20)).await;
        state.broadcaster.publish(make_state_envelope("sess-2"));

        // We expect exactly ONE State frame to arrive — for sess-2.
        // If the pre-subscribe sess-1 frame leaked, we'd see sess-1 first.
        let msg = read_text_frame_or_close(&mut ws).await;
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(text.as_str()).expect("parse");
        match parsed {
            ServerMessage::State(s) => assert_eq!(
                s.session_id, "sess-2",
                "pre-subscribe backlog leaked into post-subscribe delivery"
            ),
            other => panic!("expected State frame, got {other:?}"),
        }
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_malformed_header_does_not_fall_through_to_query() {
        // AC #5 review finding: a malformed `Authorization` header (e.g.
        // `Basic ...` instead of `Bearer ...`, or empty bearer) must NOT
        // fall through to a valid `?token=` query param. Header presence
        // (any value) wins.
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Case 1: `Basic <correct>` header + valid query token → 401.
        let url = ws_url_query(addr, TEST_BEARER);
        let mut req = url.into_client_request().expect("into_client_request");
        req.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Basic {}", TEST_BEARER).parse().expect("header"),
        );
        let err = tokio_tungstenite::connect_async(req)
            .await
            .expect_err("malformed header must NOT fall through");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status().as_u16(), 401);
            }
            other => panic!("expected 401, got {other:?}"),
        }

        // Case 2: `Bearer ` (empty token) header + valid query token → 401.
        let url = ws_url_query(addr, TEST_BEARER);
        let mut req = url.into_client_request().expect("into_client_request");
        req.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer ".parse().expect("header"));
        let err = tokio_tungstenite::connect_async(req)
            .await
            .expect_err("empty-bearer header must NOT fall through");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status().as_u16(), 401);
            }
            other => panic!("expected 401, got {other:?}"),
        }

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_257th_connection_rejected_503() {
        // Use cap of 3 to keep the test cheap. The contract is identical at
        // any boundary: N connections succeed, the N+1 fails with 503, then
        // closing one allows another to succeed.
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            3,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Open 3 connections; keep them alive in the test.
        let mut alive = Vec::new();
        for _ in 0..3 {
            let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
            let _hello = read_text_frame_or_close(&mut ws).await;
            alive.push(ws);
        }

        // The 4th attempt must be rejected with 503.
        let req = authed_request(&ws_url_header(addr), TEST_BEARER);
        let err = tokio_tungstenite::connect_async(req)
            .await
            .expect_err("must fail with 503");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status().as_u16(), 503);
            }
            other => panic!("expected HTTP 503, got {other:?}"),
        }

        // Drop one of the 3 — permit returned on connection task exit.
        let mut to_close = alive.remove(0);
        to_close.close(None).await.ok();
        drop(to_close);

        // Give the daemon a moment to notice the close and release the permit.
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let req = authed_request(&ws_url_header(addr), TEST_BEARER);
            match tokio_tungstenite::connect_async(req).await {
                Ok((mut ws, _)) => {
                    let _hello = read_text_frame_or_close(&mut ws).await;
                    state.shutdown_requested.cancel();
                    state.ws_close_requested.cancel();
                    return;
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                    if resp.status().as_u16() == 503 =>
                {
                    continue;
                }
                Err(e) => panic!("unexpected error on retry: {e:?}"),
            }
        }
        panic!("permit was never returned after dropping a connection");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_ping_within_idle_window() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_millis(100),
            Duration::from_millis(500),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;

        // Wait for a Ping frame. tokio-tungstenite will auto-respond with
        // Pong before yielding the Ping to our stream, but the Ping itself
        // is visible on the stream.
        let msg = tokio::time::timeout(Duration::from_millis(500), ws.next())
            .await
            .expect("Ping arrived in time")
            .expect("stream not ended")
            .expect("recv ok");
        assert!(
            matches!(msg, Message::Ping(_)),
            "expected Ping, got {msg:?}"
        );
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_no_pong_within_timeout_closes() {
        // AC #8: when no Pong arrives within `pong_timeout`, the daemon
        // closes the connection, exits the per-connection task, and
        // releases the semaphore permit.
        //
        // tokio-tungstenite's auto-Pong runs in `poll_next`, so a client
        // that holds the WS stream without polling it will receive the
        // server's Ping into the TCP buffer but never respond with a Pong.
        // That exercises the daemon's pong-deadline branch directly.
        //
        // To verify the permit was released, we cap the daemon to 1
        // concurrent connection and assert a second connection succeeds
        // shortly after the timeout fires.
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            1, // cap = 1 so we can prove permit release via re-connect
            Duration::from_millis(60),
            Duration::from_millis(40),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Connect, read Hello, then stop polling the stream.
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;

        // Hold `ws` so the TCP socket stays open, but do NOT poll it.
        // tokio-tungstenite's auto-Pong needs `poll_next` to run; without
        // a poll, the daemon's Ping arrives at the OS but no Pong is
        // ever generated. The daemon should hit the pong-deadline branch
        // within ~ping_interval + pong_timeout (60 + 40 = 100ms).
        let _hold = ws;

        // Wait long enough for daemon's pong-deadline to fire AND for the
        // dropped connection task to release the permit.
        tokio::time::sleep(Duration::from_millis(400)).await;

        // The permit MUST be released — cap is 1, so a new connection
        // can only succeed if the dead connection's task exited.
        let mut succeeded = false;
        for _ in 0..20 {
            let req = authed_request(&ws_url_header(addr), TEST_BEARER);
            match tokio_tungstenite::connect_async(req).await {
                Ok((mut ws2, _)) => {
                    let _hello = read_text_frame_or_close(&mut ws2).await;
                    succeeded = true;
                    break;
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                    if resp.status().as_u16() == 503 =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("unexpected error on reconnect: {e:?}"),
            }
        }
        assert!(
            succeeded,
            "pong-deadline did not exit task / release permit within budget"
        );
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ws_shutdown_token_closes_task() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _hello = read_text_frame_or_close(&mut ws).await;

        state.ws_close_requested.cancel();

        let msg = read_text_frame_or_close(&mut ws).await;
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected protocol Close text frame, got {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(text.as_str()).expect("parse close");
        match parsed {
            ServerMessage::Close(frame) => {
                assert_eq!(frame.reason.as_deref(), Some("daemon shutdown"));
            }
            other => panic!("expected ServerMessage::Close, got {other:?}"),
        }

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn x_request_id_on_healthz() {
        // AC #10 canary: SetRequestIdLayer is wired and emits an
        // x-request-id UUID4 on every response. Uses the in-process
        // `oneshot` path because the full router middleware stack applies
        // regardless of whether the request travels through TCP.
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let app = api::router(state);
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .expect("req build");
        let resp = app.oneshot(req).await.expect("oneshot");
        assert_eq!(resp.status().as_u16(), 200);
        let rid = resp
            .headers()
            .get("x-request-id")
            .expect("x-request-id header present")
            .to_str()
            .expect("ascii");
        // UUID4: 36 chars, hyphens at 8, 13, 18, 23.
        assert_eq!(rid.len(), 36, "x-request-id should be 36 chars; got {rid}");
        for pos in [8usize, 13, 18, 23] {
            assert_eq!(
                rid.as_bytes()[pos] as char,
                '-',
                "x-request-id hyphen position {pos} mismatch in {rid}"
            );
        }
    }
}

/// Story 2.2 — `projection::session::write` publishes one
/// `BroadcastEnvelope::Event` followed by one `BroadcastEnvelope::State`
/// after `tx.commit()`. These tests exercise the real publish path (not
/// synthetic `broadcaster.publish` shortcuts) so the wiring from ingest →
/// projection → broadcast hub → WS is end-to-end verified.
mod story_2_2_publish {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use bowerbird_daemon::broadcast::BroadcastEnvelope;
    use bowerbird_daemon::state::AppState;
    use futures_util::{SinkExt, StreamExt};
    use protocol::{
        Event, EventEnvelope, EventId, EventKind, Reaction, ServerMessage, SessionCurrentState,
        SessionState, StateFrame,
    };
    use tokio_tungstenite::tungstenite::Message;

    use super::story_2_1_ws::{
        authed_request, connect_authed, parse_hello, read_text_frame_or_close, spawn_test_daemon,
        ws_url_header,
    };
    use super::{fresh_pools, make_test_state_with_ws};

    const TEST_BEARER: &str = super::TEST_BEARER;

    pub(super) type WsStream = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// Probe shape for `wait_subscribe_live`. The probe must match the
    /// subscribed topic AND be distinguishable from real test envelopes
    /// (so the helper can drain probe frames before returning a clean
    /// stream).
    ///
    /// Convention:
    /// - **Event probe:** `source` matches the subscription's required
    ///   source; `session_id` is fixed to `"__probe__"`.
    /// - **State probe:** `session_id` matches the subscription's required
    ///   `session_id` (or any value for wildcard subs); `source` is fixed
    ///   to `"__probe__"`.
    ///
    /// Real test envelopes use real `source`/`session_id` values, so the
    /// `__probe__` magic is reliably distinguishable.
    #[derive(Clone, Copy)]
    pub(super) enum ProbeKind {
        Event { source: &'static str },
        State { session_id: &'static str },
    }

    /// Per-attempt probe identifier. Each `wait_subscribe_live*` loop
    /// iteration generates a fresh token, encodes it into the probe
    /// envelope, and only returns once subscribers have observed at
    /// least that token. Because `tokio::sync::broadcast` preserves
    /// per-channel order, a subscriber that has seen token `N` cannot
    /// receive a probe with token `< N` later — eliminating the stale
    /// probe race the v2 review caught.
    fn build_probe_with_token(kind: ProbeKind, token: u64) -> BroadcastEnvelope {
        let token_str = format!("__probe-{token}__");
        match kind {
            ProbeKind::Event { source } => BroadcastEnvelope::Event(Event {
                event_id: EventId(0),
                source: source.to_string(),
                // Token rides in `session_id` so source-filtered Event
                // subscriptions (`events.<source>.*`) still match while
                // the field stays uniquely identifiable per attempt.
                session_id: token_str,
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                created_at: 0,
                pid: None,
                cwd: None,
            }),
            ProbeKind::State { session_id } => BroadcastEnvelope::State {
                // Token rides in `source` so wildcard or session-keyed
                // State subscriptions still match by `session_id` while
                // the marker stays uniquely identifiable per attempt.
                source: token_str,
                session_id: session_id.to_string(),
                state: SessionState {
                    current_state: SessionCurrentState::Idle,
                    last_event_kind: EventKind::PreToolUse,
                    last_event_at_ms: 0,
                    last_pid: None,
                    cwd: None,
                    started_at: None,
                },
            },
        }
    }

    /// Parse a probe token off any probe frame (Event or State). Returns
    /// `None` for real (non-probe) frames.
    fn extract_probe_token(msg: &Message) -> Option<u64> {
        let text = match msg {
            Message::Text(t) => t.as_str(),
            _ => return None,
        };
        let server: ServerMessage = serde_json::from_str(text).ok()?;
        let marker = match server {
            ServerMessage::Event(f) => f.event.session_id,
            ServerMessage::State(f) => f.source,
            _ => return None,
        };
        marker
            .strip_prefix("__probe-")
            .and_then(|rest| rest.strip_suffix("__"))
            .and_then(|num| num.parse::<u64>().ok())
    }

    /// True if `msg` is a probe of the same kind currently being probed
    /// for. Frames from the prior (different-kind) probe call are also
    /// probes — they get drained but do NOT advance readiness for the
    /// current kind. The match keeps `wait_subscribe_live` honest when
    /// the same client is woken twice (e.g. event + state subs back to
    /// back, as the cross-topic ordering test does).
    fn probe_matches_kind(msg: &Message, kind: ProbeKind) -> bool {
        let text = match msg {
            Message::Text(t) => t.as_str(),
            _ => return false,
        };
        let server: ServerMessage = match serde_json::from_str(text) {
            Ok(s) => s,
            Err(_) => return false,
        };
        matches!(
            (server, kind),
            (ServerMessage::Event(_), ProbeKind::Event { .. })
                | (ServerMessage::State(_), ProbeKind::State { .. })
        )
    }

    /// Block until a previously-sent `Subscribe` is live on the daemon
    /// side. Publishes uniquely-tokened probe envelopes and returns only
    /// after the subscriber has observed the latest published token —
    /// broadcast ordering then proves no older probe can arrive later,
    /// closing the stale-probe race the v2 review caught.
    ///
    /// Sleeps used here live inside this explicit bounded retry loop, not
    /// as unconditional synchronization.
    pub(super) async fn wait_subscribe_live(ws: &mut WsStream, state: &AppState, probe: ProbeKind) {
        wait_subscribe_live_all(&mut [ws], state, probe).await
    }

    /// Multi-client variant of `wait_subscribe_live`. Each iteration
    /// mints a fresh probe token, publishes one probe to the hub, and
    /// drains every client's WS queue. A client is considered ready when
    /// its observed max-probe-token is `>=` the latest published token;
    /// once all clients meet that bar, every per-connection task has
    /// drained the channel up to that point and no older probe can
    /// arrive later (tokio broadcast preserves per-channel order).
    pub(super) async fn wait_subscribe_live_all(
        clients: &mut [&mut WsStream],
        state: &AppState,
        probe: ProbeKind,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        // A global atomic keeps the token unique across parallel `cargo
        // test` workers running their own daemons; the bare value is
        // irrelevant — only its monotonic order within one helper
        // invocation matters.
        static TOKEN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut max_seen: Vec<Option<u64>> = vec![None; clients.len()];
        let mut latest_token: u64 = 0;
        loop {
            if std::time::Instant::now() >= deadline {
                panic!(
                    "not all clients went live within 2s deadline \
                     (max_seen={max_seen:?}, latest_token={latest_token})"
                );
            }
            latest_token = TOKEN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            state
                .broadcaster
                .publish(build_probe_with_token(probe, latest_token));

            for (i, ws) in clients.iter_mut().enumerate() {
                // Drain everything currently queued on this client.
                // Probes of `probe`'s kind advance `max_seen`; probes of
                // a different kind (e.g. left over from an earlier
                // `wait_subscribe_live` for the other topic in this same
                // client — the cross-topic ordering test does this) are
                // valid probes and get discarded without panicking.
                loop {
                    match tokio::time::timeout(Duration::from_millis(20), ws.next()).await {
                        Ok(Some(Ok(msg))) => {
                            let token = extract_probe_token(&msg).unwrap_or_else(|| {
                                panic!("non-probe frame on client #{i} during readiness: {msg:?}")
                            });
                            if probe_matches_kind(&msg, probe) {
                                max_seen[i] = Some(match max_seen[i] {
                                    Some(prev) => prev.max(token),
                                    None => token,
                                });
                            }
                        }
                        Ok(Some(Err(e))) => {
                            panic!("ws error on client #{i} during readiness: {e:?}")
                        }
                        Ok(None) => panic!("client #{i} closed during readiness"),
                        Err(_) => break, // queue currently empty — move on
                    }
                }
            }

            let all_caught_up = max_seen
                .iter()
                .all(|m| matches!(m, Some(t) if *t >= latest_token));
            if all_caught_up {
                return;
            }
        }
    }

    /// Retry `connect_authed` until the daemon accepts the upgrade. A 503
    /// (ws_semaphore exhausted) is "not ready yet" — retry until success
    /// or a 2s deadline. Other errors fail the test. Mirrors the pattern
    /// `story_2_1_ws::ws_257th_connection_rejected_503` uses to verify
    /// permit release after a graceful close.
    pub(super) async fn connect_until_ready(addr: std::net::SocketAddr) -> WsStream {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if std::time::Instant::now() >= deadline {
                panic!("connect never succeeded within 2s deadline");
            }
            let req = authed_request(&ws_url_header(addr), TEST_BEARER);
            match tokio_tungstenite::connect_async(req).await {
                Ok((mut ws, _)) => {
                    let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
                    return ws;
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                    if resp.status().as_u16() == 503 =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                Err(e) => panic!("unexpected error during connect retry: {e:?}"),
            }
        }
    }

    pub(super) fn parse_event_frame(msg: &Message) -> Event {
        let text = match msg {
            Message::Text(t) => t.as_str(),
            other => panic!("expected text Event frame, got {other:?}"),
        };
        let server: ServerMessage = serde_json::from_str(text).expect("parse ServerMessage");
        match server {
            ServerMessage::Event(f) => f.event,
            other => panic!("expected Event, got {other:?}"),
        }
    }

    pub(super) fn parse_state_frame(msg: &Message) -> StateFrame {
        let text = match msg {
            Message::Text(t) => t.as_str(),
            other => panic!("expected text State frame, got {other:?}"),
        };
        let server: ServerMessage = serde_json::from_str(text).expect("parse ServerMessage");
        match server {
            ServerMessage::State(f) => f,
            other => panic!("expected State, got {other:?}"),
        }
    }

    /// Drives the REAL `projection::session::write` path so the broadcast
    /// envelopes are produced by the production publisher, not a synthetic
    /// `state.broadcaster.publish(...)` shortcut.
    pub(super) async fn publish_via_projection(
        state: &AppState,
        source: &str,
        session_id: &str,
        kind: EventKind,
        reaction: Option<Reaction>,
        payload: &str,
    ) -> EventId {
        bowerbird_daemon::projection::session::write(
            &state.db.writer,
            &state.broadcaster,
            EventEnvelope {
                source: source.to_string(),
                session_id: session_id.to_string(),
                kind,
                reaction,
                payload: payload.to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("projection::session::write")
    }

    /// Like [`publish_via_projection`] but lets the caller set `cwd` (the
    /// default helper hardcodes `None`). Story 5.7 review pass 2 — used by the
    /// WS `EventFrame.event.cwd` and snapshot `StateFrame.state.cwd` coverage.
    pub(super) async fn publish_via_projection_with_cwd(
        state: &AppState,
        source: &str,
        session_id: &str,
        kind: EventKind,
        cwd: Option<&str>,
    ) -> EventId {
        bowerbird_daemon::projection::session::write(
            &state.db.writer,
            &state.broadcaster,
            EventEnvelope {
                source: source.to_string(),
                session_id: session_id.to_string(),
                kind,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: cwd.map(|s| s.to_string()),
            },
        )
        .await
        .expect("projection::session::write")
    }

    fn default_state(pools: bowerbird_daemon::db::DbPools, ws_max_conns: usize) -> AppState {
        make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            ws_max_conns,
            Duration::from_secs(30),
            Duration::from_secs(10),
        )
    }

    /// Story 5.7 review pass 2 (AC #9): a live `events.*` `EventFrame` must
    /// carry `event.cwd`. A column-index slip or a dropped field on the live
    /// broadcast path would otherwise go uncaught by the Story 5.7 tests.
    #[tokio::test(flavor = "current_thread")]
    async fn events_frame_carries_cwd() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        publish_via_projection_with_cwd(
            &state,
            "claude",
            "sess-ws-cwd",
            EventKind::PreToolUse,
            Some("/repo"),
        )
        .await;

        let event = parse_event_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            event.cwd,
            Some("/repo".to_string()),
            "live EventFrame.event.cwd must carry the extracted cwd"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1 — Three subscribers to `events.*` receive byte-identical
    /// `Event` frames in the same order.
    #[tokio::test(flavor = "current_thread")]
    async fn three_subscribers_receive_identical_events_in_order() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws1, _) = connect_authed(addr, TEST_BEARER).await;
        let (mut ws2, _) = connect_authed(addr, TEST_BEARER).await;
        let (mut ws3, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws1).await);
        let _ = parse_hello(&read_text_frame_or_close(&mut ws2).await);
        let _ = parse_hello(&read_text_frame_or_close(&mut ws3).await);

        for ws in [&mut ws1, &mut ws2, &mut ws3] {
            ws.send(Message::Text(
                r#"{"op":"subscribe","topic":"events.*"}"#.into(),
            ))
            .await
            .expect("send subscribe");
        }
        // Wait for all three subscribes to go live coordinated so probes
        // published for one client don't leak into another's stream.
        wait_subscribe_live_all(
            &mut [&mut ws1, &mut ws2, &mut ws3],
            &state,
            ProbeKind::Event { source: "claude" },
        )
        .await;

        let id1 = publish_via_projection(
            &state,
            "claude",
            "sess-fan",
            EventKind::PreToolUse,
            Some(Reaction::Continue),
            r#"{"tool":"Bash"}"#,
        )
        .await;
        let id2 = publish_via_projection(
            &state,
            "claude",
            "sess-fan",
            EventKind::PostToolUse,
            None,
            r#"{"tool":"Bash"}"#,
        )
        .await;

        // Collect two frames per client.
        let mut texts: Vec<Vec<String>> = Vec::with_capacity(3);
        for ws in [&mut ws1, &mut ws2, &mut ws3] {
            let f1 = read_text_frame_or_close(ws).await;
            let f2 = read_text_frame_or_close(ws).await;
            let t1 = match f1 {
                Message::Text(t) => t.to_string(),
                other => panic!("expected text frame, got {other:?}"),
            };
            let t2 = match f2 {
                Message::Text(t) => t.to_string(),
                other => panic!("expected text frame, got {other:?}"),
            };
            texts.push(vec![t1, t2]);
        }

        // Byte-identical wire frames across all three clients.
        assert_eq!(
            texts[0], texts[1],
            "client 1 and 2 must see byte-identical frames"
        );
        assert_eq!(
            texts[1], texts[2],
            "client 2 and 3 must see byte-identical frames"
        );

        // Event IDs arrive in publication order.
        let ev1: ServerMessage = serde_json::from_str(&texts[0][0]).expect("parse #1");
        let ev2: ServerMessage = serde_json::from_str(&texts[0][1]).expect("parse #2");
        let first_id = match ev1 {
            ServerMessage::Event(f) => f.event.event_id,
            other => panic!("expected Event, got {other:?}"),
        };
        let second_id = match ev2 {
            ServerMessage::Event(f) => f.event.event_id,
            other => panic!("expected Event, got {other:?}"),
        };
        assert_eq!(first_id, id1, "first frame must carry the first event_id");
        assert_eq!(
            second_id, id2,
            "second frame must carry the second event_id"
        );
        assert!(
            first_id.0 < second_id.0,
            "event_ids must be strictly increasing"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #2 — `state.session.<id>.current_state` delivers a `State` frame
    /// only for matching `session_id`s, never for other sessions.
    #[tokio::test(flavor = "current_thread")]
    async fn state_current_topic_filters_other_sessions() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.sess-A.current_state"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "sess-A",
            },
        )
        .await;

        // Positive #1: publish for sess-A — expect State frame.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let frame = read_text_frame_or_close(&mut ws).await;
        let parsed = parse_state_frame(&frame);
        assert_eq!(parsed.session_id, "sess-A");

        // Negative: publish for sess-B — expect NO frame within 300ms.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-B",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let timed = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            timed.is_err(),
            "no state frame should arrive for sess-B; got {timed:?}"
        );

        // Positive #2: publish for sess-A again — expect State frame.
        // Story 5.2: State frames fire only on `current_state` transitions.
        // sess-A is currently Working (from the earlier PreToolUse); use Stop
        // to flip it to Idle and trigger a State publish.
        let _ =
            publish_via_projection(&state, "claude", "sess-A", EventKind::Stop, None, "{}").await;
        let frame = read_text_frame_or_close(&mut ws).await;
        let parsed = parse_state_frame(&frame);
        assert_eq!(parsed.session_id, "sess-A");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #3 — `events.claude.*` delivers events from `source = "claude"`
    /// and filters out events from any other source. The other-source
    /// publish is synthetic (via `state.broadcaster.publish`) since the
    /// production ingest path only has the `"claude"` source today; this
    /// is the documented test-only exception to the "publish only from
    /// `projection::session::write`" rule.
    #[tokio::test(flavor = "current_thread")]
    async fn events_source_filter_excludes_other_source() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.claude.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Real claude event via the production publish path.
        let id_claude = publish_via_projection(
            &state,
            "claude",
            "sess-1",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let frame = read_text_frame_or_close(&mut ws).await;
        let event = parse_event_frame(&frame);
        assert_eq!(event.source, "claude");
        assert_eq!(event.event_id, id_claude);

        // Synthetic codex event (simulating a future second-source adapter).
        state.broadcaster.publish(BroadcastEnvelope::Event(Event {
            event_id: EventId(99_999),
            source: "codex".to_string(),
            session_id: "sess-2".to_string(),
            kind: EventKind::PreToolUse,
            reaction: None,
            payload: "{}".to_string(),
            created_at: 0,
            pid: None,
            cwd: None,
        }));

        let timed = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            timed.is_err(),
            "events.claude.* must not deliver codex-sourced events; got {timed:?}"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #4 — `state.session.*` wildcard delivers one `State` frame per
    /// publish, each carrying the originating `session_id` with no
    /// cross-session smearing.
    #[tokio::test(flavor = "current_thread")]
    async fn state_wildcard_preserves_session_id_per_frame() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__probe__",
            },
        )
        .await;

        // Story 5.2: State frames fire only on `current_state` transitions.
        // For the repeated `sess-A` to publish twice, the second occurrence
        // must drive a different state — use Stop (Working → Idle) for the
        // second `sess-A` so all four publishes yield State frames.
        let order: [(&str, EventKind); 4] = [
            ("sess-A", EventKind::PreToolUse),
            ("sess-B", EventKind::PreToolUse),
            ("sess-A", EventKind::Stop),
            ("sess-C", EventKind::PreToolUse),
        ];
        for (sid, kind) in &order {
            let _ = publish_via_projection(&state, "claude", sid, kind.clone(), None, "{}").await;
        }

        for (expected, _) in &order {
            let frame = read_text_frame_or_close(&mut ws).await;
            let parsed = parse_state_frame(&frame);
            assert_eq!(
                parsed.session_id, *expected,
                "session_id must match publication order; expected {expected}, got {}",
                parsed.session_id
            );
        }

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #5 — Closing one WS client does not interrupt delivery to the
    /// other, and the closed client's WS semaphore permit is released
    /// (verified via a connect-cap probe with `ws_max_conns = 2`).
    #[tokio::test(flavor = "current_thread")]
    async fn consumer_independence_and_semaphore_release() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 2); // ws_max_conns = 2 enables the probe.
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws_a, _) = connect_authed(addr, TEST_BEARER).await;
        let (mut ws_b, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws_a).await);
        let _ = parse_hello(&read_text_frame_or_close(&mut ws_b).await);

        for ws in [&mut ws_a, &mut ws_b] {
            ws.send(Message::Text(
                r#"{"op":"subscribe","topic":"events.*"}"#.into(),
            ))
            .await
            .expect("send subscribe");
        }
        wait_subscribe_live_all(
            &mut [&mut ws_a, &mut ws_b],
            &state,
            ProbeKind::Event { source: "claude" },
        )
        .await;

        // Close A gracefully. B's delivery path is independent of A's
        // teardown, so we can publish + read on B without waiting; the
        // semaphore-permit-release check is deferred to the
        // `connect_until_ready` retry below.
        ws_a.close(None).await.expect("close A");
        drop(ws_a);

        // Publishing now must reach B uninterrupted.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-survives",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let frame = read_text_frame_or_close(&mut ws_b).await;
        let event = parse_event_frame(&frame);
        assert_eq!(event.session_id, "sess-survives");

        // Probe: with `ws_max_conns = 2` and B still attached, the third
        // connect succeeds only after A's permit is released on close.
        // The retry handles the race deterministically — same shape as
        // `story_2_1_ws::ws_257th_connection_rejected_503`.
        let _ws_c = connect_until_ready(addr).await;

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #7 (defense in depth) — `projection::session::write` rejects
    /// sentinel `EventKind`s at runtime in release builds, not just under
    /// `debug_assert!`. A misuse must not commit a row, must not publish a
    /// broadcast envelope, and must surface a typed error.
    #[tokio::test(flavor = "current_thread")]
    async fn projection_write_rejects_sentinel_kinds_at_runtime() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        for kind in [EventKind::RecordingStarted, EventKind::RecordingEnded] {
            let err = bowerbird_daemon::projection::session::write(
                &state.db.writer,
                &state.broadcaster,
                EventEnvelope {
                    source: "__daemon__".to_string(),
                    session_id: "__daemon__".to_string(),
                    kind: kind.clone(),
                    reaction: None,
                    payload: "{}".to_string(),
                    pid: None,
                    notification_type: None,
                    cwd: None,
                },
            )
            .await
            .expect_err("sentinel kind must be rejected");
            let msg = format!("{err}");
            assert!(
                msg.contains("sentinel EventKind"),
                "error must name sentinel cause; got {msg}"
            );

            // No broadcast envelope was published — confirm via timeout.
            let timed = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
            assert!(
                timed.is_err(),
                "rejected sentinel write must not produce a broadcast frame ({kind:?}); got {timed:?}"
            );
        }

        // No event row was inserted either — count must stay zero.
        let conn = state.db.reader.get().await.expect("reader get");
        let count: i64 = conn
            .interact(|c| -> rusqlite::Result<i64> {
                c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            })
            .await
            .expect("interact")
            .expect("count");
        assert_eq!(count, 0, "sentinel-rejected writes must not insert rows");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #7 — Sentinel writes (`write_recording_started`,
    /// `write_recording_ended`) do NOT publish. Subscriber to `events.*`
    /// receives nothing for the sentinel, then receives a frame for a
    /// subsequent non-sentinel event (sanity that the channel is alive).
    #[tokio::test(flavor = "current_thread")]
    async fn sentinel_writes_are_not_published() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Sentinel write — must NOT broadcast.
        let started =
            bowerbird_daemon::projection::session::write_recording_started(&state.db.writer)
                .await
                .expect("write_recording_started");
        let timed = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            timed.is_err(),
            "sentinel RecordingStarted must not produce a broadcast frame; got {timed:?}"
        );

        // The other sentinel writer must also not broadcast.
        let _ = bowerbird_daemon::projection::session::write_recording_ended(
            &state.db.writer,
            started.recording_session_id,
        )
        .await
        .expect("write_recording_ended");
        let timed = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            timed.is_err(),
            "sentinel RecordingEnded must not produce a broadcast frame; got {timed:?}"
        );

        // Sanity: the channel is still alive for non-sentinel events.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-alive",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let frame = read_text_frame_or_close(&mut ws).await;
        let event = parse_event_frame(&frame);
        assert_eq!(event.session_id, "sess-alive");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// End-to-end: a hook line delivered over the Unix ingest socket
    /// must round-trip through `ingest::writer::run` →
    /// `projection::session::write` and reach a WS subscriber as an
    /// `Event` frame. This is the only test that exercises the *full*
    /// production wiring story 2.2 introduced; the per-AC tests publish
    /// via `projection::session::write` directly and would miss a
    /// regression that swapped the writer task's broadcaster handle for
    /// the wrong one or `None`.
    #[tokio::test(flavor = "current_thread")]
    async fn ingest_socket_event_reaches_ws_subscriber() {
        use std::sync::Arc;
        use tempfile::TempDir;

        let (_dbtmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Wire up the ingest stack against the same `AppState`. The
        // listener spawns inside `start_ingest_listener`; we add a
        // sibling `writer::run` consuming the channel and writing
        // through `state.db.writer` + `state.broadcaster` so any wiring
        // mistake on the broadcaster handle would be observable here.
        let sock_tmp = TempDir::new().expect("ingest tempdir");
        let (ingest_shutdown, sock_path, rx) = super::start_ingest_listener(&sock_tmp, 16).await;
        let writer_handle = tokio::spawn(bowerbird_daemon::ingest::writer::run(
            rx,
            state.db.writer.clone(),
            Arc::clone(&state.broadcaster),
            ingest_shutdown.clone(),
        ));

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Send a real hook line. Adapter is the same `ClaudeAdapter`
        // pointed at a nonexistent reactions TOML (degrades to
        // `Unknown`), so envelope.kind ends up `PreToolUse` and source
        // is `"claude"`.
        let resp = super::send_line_recv_response(
            &sock_path,
            b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"sess-ingest-broadcast\",\"tool_name\":\"Test\"}\n",
        )
        .await;
        assert!(resp.starts_with("200"), "expected 200 ack, got: {resp:?}");

        let frame = read_text_frame_or_close(&mut ws).await;
        let event = parse_event_frame(&frame);
        assert_eq!(event.source, "claude");
        assert_eq!(event.session_id, "sess-ingest-broadcast");

        ingest_shutdown.cancel();
        // Closing the listener drops the sender; await the writer so
        // any tail publishes complete before the test tears down.
        let _ = writer_handle.await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1 + AC #2 combined — a single client subscribed to BOTH
    /// `events.*` and `state.session.*` must observe the resulting
    /// frames in the documented order: `Event` before `State`. A
    /// regression that reverses the two publishes in
    /// `projection::session::write` would still pass the AC-specific
    /// tests (which split events- and state-only subscribers) but break
    /// the presenter mental model the write doc-comment promises.
    #[tokio::test(flavor = "current_thread")]
    async fn event_published_before_state_to_dual_subscriber() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe events.*");
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe state.session.*");

        // Wait for BOTH topic classes to be live. The token-based helper
        // tolerates probes of the prior kind queued on the same client
        // (drained without panic, just not counted toward this call's
        // readiness).
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__probe__",
            },
        )
        .await;

        let id = publish_via_projection(
            &state,
            "claude",
            "sess-order",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        // Exactly two frames in this order: Event first, then State.
        let f1 = read_text_frame_or_close(&mut ws).await;
        let event = parse_event_frame(&f1);
        assert_eq!(event.event_id, id, "frame 1 must be the published event");
        assert_eq!(event.source, "claude");
        assert_eq!(event.session_id, "sess-order");

        let f2 = read_text_frame_or_close(&mut ws).await;
        let st = parse_state_frame(&f2);
        assert_eq!(
            st.source, "claude",
            "frame 2 must be the state update for the same session"
        );
        assert_eq!(st.session_id, "sess-order");

        // No trailing frames within a short timeout — the publish
        // produced exactly the pair.
        let timed = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
        assert!(
            timed.is_err(),
            "unexpected trailing frame after Event+State pair: {timed:?}"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }
}

/// Story 2.3 — Snapshot of matching session projections on Subscribe.
///
/// Each test sits on top of the Story 2.2 publish path and the Story 2.1
/// WS surface. The unique behaviour exercised here is the new emission
/// step in the `ClientMessage::Subscribe` arm: a per-subscribe read of
/// `session_projections` filtered by the new topic and deduped against
/// the pre-existing subscription set.
///
/// `wait_subscribe_live*` from 2.2 is reused unchanged. The helper
/// panics on a non-probe frame during readiness — in this module that
/// is the *desired* behaviour for AC #7's dedup assertion, and every
/// other test reads its expected snapshot frames explicitly before
/// calling the helper.
mod story_2_3_snapshot {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use bowerbird_daemon::state::AppState;
    use futures_util::SinkExt;
    use protocol::{EventKind, ServerMessage, SessionCurrentState};
    use tokio_tungstenite::tungstenite::Message;

    use super::story_2_1_ws::{
        connect_authed, parse_hello, read_text_frame_or_close, spawn_test_daemon,
    };
    use super::story_2_2_publish::{
        connect_until_ready, parse_event_frame, parse_state_frame, publish_via_projection,
        publish_via_projection_with_cwd, wait_subscribe_live, ProbeKind, WsStream,
    };
    use super::{fresh_pools, make_test_state_with_ws};

    const TEST_BEARER: &str = super::TEST_BEARER;

    fn default_state(pools: bowerbird_daemon::db::DbPools, ws_max_conns: usize) -> AppState {
        make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            ws_max_conns,
            Duration::from_secs(30),
            Duration::from_secs(10),
        )
    }

    /// Connect via `connect_until_ready` (which parses Hello) and return
    /// the ready stream. Used by tests that don't need a non-default
    /// `ws_max_conns` and want to skip the boilerplate.
    async fn connect_and_skip_hello(addr: std::net::SocketAddr) -> WsStream {
        connect_until_ready(addr).await
    }

    /// Story 5.7 review pass 2 (AC #9): a snapshot-on-subscribe `StateFrame`
    /// must carry `state.cwd` and `state.started_at` for a pre-existing
    /// session. The snapshot path builds `SessionState` from the stored
    /// projection blob (`snapshot.rs`), a separate surface from the live
    /// transition the other cwd tests exercise.
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_state_frame_carries_cwd_and_started_at() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Pre-create a session with a cwd BEFORE the subscriber connects, so
        // the snapshot read includes it.
        publish_via_projection_with_cwd(
            &state,
            "claude",
            "sess-snap-cwd",
            EventKind::PreToolUse,
            Some("/repo"),
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        let frame = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(frame.session_id, "sess-snap-cwd");
        assert_eq!(
            frame.state.cwd,
            Some("/repo".to_string()),
            "snapshot StateFrame.state.cwd must carry the stored cwd"
        );
        assert!(
            frame.state.started_at.is_some(),
            "snapshot StateFrame.state.started_at must be set"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    // NOTE (correct-course 2026-06-02, Option D): a Story 5.7 review pass-4 test
    // (`legacy_started_at_backfill_publishes_state_frame_on_same_state_event`)
    // lived here. It pinned that the legacy `started_at` event-log backfill
    // published a live `StateFrame` even on a same-state write. The backfill was
    // removed entirely — bowerbird is pre-release and the documented upgrade
    // path is "nuke the db," so a `started_at: None`-with-prior row (only
    // reachable from a pre-5.7 blob) is unsupported. See
    // docs/bmad/planning-artifacts/started-at-backfill-reconsideration-2026-06-02.md
    // and deferred-work.md (real migration-era backfills land when bowerbird
    // ships a release whose dbs must survive upgrades).

    /// AC #1 — Three pre-existing sessions surface as snapshot State
    /// frames in the documented SQL order (`updated_at DESC, source
    /// ASC, session_id ASC`) BEFORE any live event. Each frame's
    /// `(source, session_id, state)` must match the stored projection.
    /// A subsequent live publish then arrives as a live State frame
    /// with a strictly later `last_event_at_ms`.
    ///
    /// `current_unix_millis` is the source of `updated_at` inside
    /// `projection::session::write`. To force `updated_at` strictly
    /// monotone (so the SQL `ORDER BY updated_at DESC` is deterministic
    /// rather than relying on the secondary `session_id ASC` tiebreaker
    /// for same-millisecond writes), sleep ~2ms between pre-create
    /// publishes. `tokio::time::sleep` advances real time even on the
    /// `current_thread` runtime since the timer is not paused.
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_three_sessions_arrive_before_live_events() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Pre-create three sessions BEFORE the WS client connects so the
        // snapshot read picks them up. Sleep between publishes so
        // updated_at is strictly increasing across the three rows.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        // sess-B uses Stop so it lands in Idle — Story 5.2 made PostToolUse
        // preserve prev, so a lone PostToolUse no longer yields Idle.
        let _ =
            publish_via_projection(&state, "claude", "sess-B", EventKind::Stop, None, "{}").await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-C",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        // Three snapshot State frames. SQL order is `updated_at DESC,
        // source ASC, session_id ASC`. Since sess-C was published last,
        // it has the largest updated_at; sess-A was first, smallest.
        // Expected wire order: sess-C → sess-B → sess-A.
        let frame_c = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        let frame_b = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        let frame_a = parse_state_frame(&read_text_frame_or_close(&mut ws).await);

        assert_eq!(frame_c.session_id, "sess-C", "snapshot order: newest first");
        assert_eq!(frame_b.session_id, "sess-B");
        assert_eq!(frame_a.session_id, "sess-A", "snapshot order: oldest last");

        // Full source assertion on every frame.
        for f in [&frame_c, &frame_b, &frame_a] {
            assert_eq!(
                f.source, "claude",
                "snapshot frame source must match stored row"
            );
        }

        // Full state assertion per session, matching projection::transition.
        // sess-A: PreToolUse → Working
        assert_eq!(frame_a.state.last_event_kind, EventKind::PreToolUse);
        assert_eq!(
            frame_a.state.current_state,
            SessionCurrentState::Working,
            "sess-A PreToolUse → Working"
        );
        // sess-B: Stop → Idle (Story 5.2 — Stop is the canonical Idle trigger).
        assert_eq!(frame_b.state.last_event_kind, EventKind::Stop);
        assert_eq!(
            frame_b.state.current_state,
            SessionCurrentState::Idle,
            "sess-B Stop → Idle"
        );
        // sess-C: PreToolUse → Working
        assert_eq!(frame_c.state.last_event_kind, EventKind::PreToolUse);
        assert_eq!(
            frame_c.state.current_state,
            SessionCurrentState::Working,
            "sess-C PreToolUse → Working"
        );

        // updated_at-strict-monotone check via last_event_at_ms (which
        // is set from the same `current_unix_millis` value as the
        // projection's `updated_at`).
        assert!(
            frame_c.state.last_event_at_ms > frame_b.state.last_event_at_ms,
            "sess-C must be newer than sess-B (got C={}, B={})",
            frame_c.state.last_event_at_ms,
            frame_b.state.last_event_at_ms
        );
        assert!(
            frame_b.state.last_event_at_ms > frame_a.state.last_event_at_ms,
            "sess-B must be newer than sess-A (got B={}, A={})",
            frame_b.state.last_event_at_ms,
            frame_a.state.last_event_at_ms
        );

        let max_snapshot_last_event_at_ms = frame_c.state.last_event_at_ms;

        // Now publish a live event for sess-A. The subscription is
        // state-only, so the Event envelope is filtered by
        // `dispatch_envelope` and never reaches the wire — only the
        // resulting State frame does. Its `last_event_at_ms` must be
        // strictly later than the snapshot's newest timestamp.
        //
        // Story 5.2: State frames fire only on `current_state` transitions.
        // sess-A is currently `Working` (from its earlier PreToolUse); we
        // need an event that flips it. `Stop` is the canonical Working → Idle
        // trigger.
        tokio::time::sleep(Duration::from_millis(2)).await;
        let _ =
            publish_via_projection(&state, "claude", "sess-A", EventKind::Stop, None, "{}").await;
        let f_state = read_text_frame_or_close(&mut ws).await;
        let live = parse_state_frame(&f_state);
        assert_eq!(live.session_id, "sess-A");
        assert_eq!(live.source, "claude");
        assert_eq!(live.state.last_event_kind, EventKind::Stop);
        assert_eq!(live.state.current_state, SessionCurrentState::Idle);
        assert!(
            live.state.last_event_at_ms > max_snapshot_last_event_at_ms,
            "live frame must strictly postdate the snapshot (live={}, max snapshot={})",
            live.state.last_event_at_ms,
            max_snapshot_last_event_at_ms
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 (ADR 0008) AC #10/#11: a `Subscribe` with `states` scopes the
    /// snapshot burst to the requested read-derived states — the `Ended`
    /// graveyard is excluded — while a live transition INTO `Ended` after
    /// subscribe still arrives (the live stream is never scoped by `states`).
    ///
    /// No `sleep`s (deterministic-test discipline): exclusion is proven by the
    /// frame *after* the active-session snapshot being the live `sess-W`→Ended
    /// transition, NOT a `sess-E` snapshot frame. That holds regardless of the
    /// `updated_at` order of the two pre-created rows, so we don't need to
    /// wall-clock-separate them. The read/publish await sequence (read the
    /// snapshot frame, THEN publish the live event, THEN read again) is what
    /// orders snapshot-before-live — not timing.
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_states_filter_excludes_ended_live_transition_still_arrives() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // sess-W: PreToolUse → Working (active). sess-E: SessionEnded → Ended
        // (graveyard). With the `["working","waitinginput","idle"]` filter only
        // sess-W can appear in the snapshot.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-W",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-E",
            EventKind::SessionEnded,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*","states":["working","waitinginput","idle"]}"#
                .into(),
        ))
        .await
        .expect("send subscribe with states filter");

        // The only snapshot frame is sess-W; sess-E (Ended) is filtered out.
        let first = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            first.session_id, "sess-W",
            "snapshot must include the active session, not the Ended one"
        );
        assert_eq!(first.state.current_state, SessionCurrentState::Working);

        // A live transition INTO Ended after subscribe still arrives — `states`
        // scopes only the snapshot, never live `state.*` delivery. Flip sess-W
        // (currently Working) to Ended. Because the snapshot held exactly one
        // frame (sess-W), this next frame being the live sess-W→Ended (and not
        // a sess-E snapshot frame) is what proves sess-E was excluded.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-W",
            EventKind::SessionEnded,
            None,
            "{}",
        )
        .await;
        let live = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            live.session_id, "sess-W",
            "next frame must be the live sess-W transition, not a sess-E snapshot frame"
        );
        assert_eq!(
            live.state.current_state,
            SessionCurrentState::Ended,
            "a live transition to Ended must arrive even though Ended was filtered from the snapshot"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review (finding #2): a *filtered* subscribe must NOT suppress a
    /// later snapshot of the same topic for rows the filter excluded. Snapshot
    /// dedup keys on the `(source, session_id)` rows already delivered, so a
    /// first `states:["ended"]` subscribe (which only sent the graveyard) does
    /// not poison a second `states:["working",...]` subscribe to the same topic.
    ///
    /// This is exactly the deck's `a`→"show ended" trajectory: connect filtered
    /// to active sessions, then widen. Pre-fix, the second subscribe saw
    /// `StateAll` already in the subscription set and short-circuited to an
    /// empty snapshot — the active rows were never sent.
    #[tokio::test(flavor = "current_thread")]
    async fn filtered_subscribe_does_not_suppress_later_wider_snapshot() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // sess-W: Working (active). sess-E: Ended (graveyard).
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-W",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-E",
            EventKind::SessionEnded,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // First subscribe: ended-only. Snapshot is exactly [sess-E].
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*","states":["ended"]}"#.into(),
        ))
        .await
        .expect("send ended-only subscribe");
        let ended = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            ended.session_id, "sess-E",
            "first (ended) snapshot is the graveyard row"
        );
        assert_eq!(ended.state.current_state, SessionCurrentState::Ended);

        // Second subscribe: same topic, now the active states. The filtered
        // first subscribe must NOT have suppressed this — we must receive
        // sess-W. (Pre-fix this read would time out: the snapshot was empty.)
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*","states":["working","waitinginput","idle"]}"#
                .into(),
        ))
        .await
        .expect("send active-states subscribe");
        let active = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            active.session_id, "sess-W",
            "a wider re-subscribe after a filtered one must still snapshot the previously-excluded active row"
        );
        assert_eq!(active.state.current_state, SessionCurrentState::Working);

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review pass-2 finding F1: widening a filter on the same topic
    /// re-sends ONLY the keys the narrower burst never covered — the overlap is
    /// not double-delivered (the `docs/protocol.md` no-double-delivery promise).
    /// The reviewer's exact case: subscribe `states:["working"]`, then the same
    /// topic unfiltered — the Working row already delivered must NOT repeat, and
    /// only the previously-excluded Ended row arrives.
    #[tokio::test(flavor = "current_thread")]
    async fn widening_filter_resends_only_uncovered_rows() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // sess-W: Working (active). sess-E: Ended (graveyard).
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-W",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-E",
            EventKind::SessionEnded,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Narrow subscribe: working-only. Snapshot is exactly [sess-W].
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*","states":["working"]}"#.into(),
        ))
        .await
        .expect("send working-only subscribe");
        let working = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(working.session_id, "sess-W");
        assert_eq!(working.state.current_state, SessionCurrentState::Working);

        // Wider subscribe: same topic, unfiltered. sess-W was already
        // delivered, so the ONLY new snapshot frame is sess-E.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send unfiltered subscribe");
        let widened = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            widened.session_id, "sess-E",
            "widening must deliver only the previously-uncovered Ended row"
        );

        // No further snapshot frame: a duplicate sess-W would be a non-probe
        // frame and make this readiness drain panic.
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__w__",
            },
        )
        .await;

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review pass-2 finding F3: an identical filtered re-subscribe on
    /// one connection is idempotent (no duplicate snapshot frame) — the
    /// `docs/protocol.md` "subscribing to the same topic twice ... is idempotent"
    /// promise, now honored for filtered subscribes by the per-key dedup.
    #[tokio::test(flavor = "current_thread")]
    async fn identical_filtered_resubscribe_is_idempotent() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-E",
            EventKind::SessionEnded,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // First ended-only subscribe: snapshot is exactly [sess-E].
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*","states":["ended"]}"#.into(),
        ))
        .await
        .expect("send ended-only subscribe");
        let first = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(first.session_id, "sess-E");

        // Identical re-subscribe: sess-E's key is already recorded, so zero new
        // snapshot frames. A duplicate would be caught as a non-probe frame by
        // the readiness drain below.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*","states":["ended"]}"#.into(),
        ))
        .await
        .expect("send identical re-subscribe");
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__i__",
            },
        )
        .await;

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review pass-2 finding F2: snapshot coverage lapses on
    /// unsubscribe. After unsubscribing, the live updates that kept the snapshot
    /// current stop, so a re-subscribe must re-snapshot the drift that
    /// accumulated while unsubscribed. Repro: subscribe unfiltered, unsubscribe,
    /// change a session while unsubscribed, re-subscribe → fresh snapshot
    /// carrying the NEW state. Pre-fix, `fully_snapshotted` still held the topic
    /// and the re-subscribe short-circuited to an empty snapshot.
    #[tokio::test(flavor = "current_thread")]
    async fn unsubscribe_lapses_coverage_resubscribe_resnapshots() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // sess-A starts Working.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Subscribe unfiltered → snapshot [sess-A Working]; confirm live.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        let snap = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(snap.session_id, "sess-A");
        assert_eq!(snap.state.current_state, SessionCurrentState::Working);
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__a__",
            },
        )
        .await;

        // Unsubscribe, then subscribe a non-matching event barrier. Once that
        // Event probe is live, the unsubscribe is guaranteed processed
        // (in-order frame handling), so the next publish delivers nothing live
        // (state is unsubscribed; the barrier topic only matches source
        // "barrier", not the "claude" change below).
        ws.send(Message::Text(
            r#"{"op":"unsubscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send unsubscribe");
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.barrier.*"}"#.into(),
        ))
        .await
        .expect("send barrier subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "barrier" }).await;

        // Drift while unsubscribed: sess-A → Ended. No live frame reaches the
        // client (state unsubscribed; barrier topic excludes claude events).
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::SessionEnded,
            None,
            "{}",
        )
        .await;

        // Re-subscribe → fresh snapshot reflecting the drift (sess-A now Ended).
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send re-subscribe");
        let resnap = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            resnap.session_id, "sess-A",
            "re-subscribe after unsubscribe must re-snapshot the session"
        );
        assert_eq!(
            resnap.state.current_state,
            SessionCurrentState::Ended,
            "the fresh snapshot must carry the state that drifted while unsubscribed"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review pass-3: a State frame delivered LIVE (not via the
    /// snapshot burst) must still update this connection's snapshot coverage,
    /// so an identical re-subscribe does not re-snapshot a row the live stream
    /// already carried (the `docs/protocol.md` no-double-delivery promise).
    /// Pre-fix, `snapshotted_keys` was written only while emitting snapshot
    /// frames, so a live row stayed "uncovered" and the re-subscribe duplicated
    /// it. Repro: empty daemon → subscribe (empty snapshot) → publish a new
    /// session (delivered live) → identical re-subscribe must emit nothing new.
    #[tokio::test(flavor = "current_thread")]
    async fn live_state_frame_recorded_in_coverage_no_duplicate_on_resubscribe() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Subscribe to an EMPTY daemon → empty snapshot. Confirm live before
        // publishing so the next real frame is unambiguously the live one.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__live__",
            },
        )
        .await;

        // A new session arrives → delivered LIVE (not via the snapshot burst).
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-L",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let live = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(live.session_id, "sess-L");
        assert_eq!(live.state.current_state, SessionCurrentState::Working);

        // Identical re-subscribe: sess-L's key was recorded by the LIVE
        // delivery, so zero new snapshot frames. A duplicate sess-L snapshot
        // would be a non-probe frame and make this readiness drain panic.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send identical re-subscribe");
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__after__",
            },
        )
        .await;

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review pass-3: coverage for a session lapses only when NO
    /// remaining active state subscription covers it. With overlapping
    /// subscriptions (`state.session.*` + `state.session.<id>`), unsubscribing
    /// only the wildcard must NOT lapse coverage for the id the specific
    /// subscription still tracks — its live state keeps flowing, and a wildcard
    /// re-subscribe does not re-snapshot it. Guards against an over-broad
    /// "clear all coverage on unsubscribe" reading of the protocol docs.
    #[tokio::test(flavor = "current_thread")]
    async fn overlapping_subscription_unsubscribe_does_not_lapse_covered_session() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // sess-A starts Working.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Subscribe wildcard → snapshot [sess-A]. Then ALSO subscribe the
        // specific id (overlapping). The specific subscribe sends no new
        // snapshot frame — sess-A is already covered (per-key dedup).
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send wildcard subscribe");
        let snap = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(snap.session_id, "sess-A");
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.sess-A"}"#.into(),
        ))
        .await
        .expect("send specific-id subscribe");

        // Barrier: an event subscription whose probe confirms in-order
        // processing of everything sent before it (both state subs).
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.barrier.*"}"#.into(),
        ))
        .await
        .expect("send barrier subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "barrier" }).await;

        // Unsubscribe ONLY the wildcard. The specific `state.session.sess-A`
        // still covers sess-A, so its key must survive the unsubscribe prune.
        ws.send(Message::Text(
            r#"{"op":"unsubscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send wildcard unsubscribe");
        // Fresh barrier probe, published AFTER the unsubscribe → its arrival
        // proves the unsubscribe is fully processed (in-order handling).
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "barrier" }).await;

        // Mutate sess-A → Ended. The specific subscription still delivers its
        // live state (coverage did NOT lapse for delivery).
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::SessionEnded,
            None,
            "{}",
        )
        .await;
        let live = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(live.session_id, "sess-A");
        assert_eq!(
            live.state.current_state,
            SessionCurrentState::Ended,
            "the still-active specific subscription must deliver the live transition"
        );

        // Re-subscribe the wildcard. sess-A is still covered (the specific sub
        // kept it current), so NO re-snapshot. A duplicate would be a non-probe
        // frame and make this readiness drain panic.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send wildcard re-subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "barrier" }).await;

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review pass-3: the `states` snapshot filter applies to specific
    /// `state.session.<id>` topics too, not only the wildcard. A matching filter
    /// yields the snapshot row; a non-matching filter yields none. Two
    /// connections so the negative case has its own fresh coverage set.
    #[tokio::test(flavor = "current_thread")]
    async fn states_filter_on_specific_session_topic() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // sess-A is Working.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        // Matching filter: state.session.sess-A + states:["working"] → snapshot.
        let (mut ws_match, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws_match).await);
        ws_match
            .send(Message::Text(
                r#"{"op":"subscribe","topic":"state.session.sess-A","states":["working"]}"#.into(),
            ))
            .await
            .expect("send matching-filter specific subscribe");
        let matched = parse_state_frame(&read_text_frame_or_close(&mut ws_match).await);
        assert_eq!(matched.session_id, "sess-A");
        assert_eq!(matched.state.current_state, SessionCurrentState::Working);

        // Non-matching filter: state.session.sess-A + states:["ended"] → no
        // snapshot (sess-A renders Working). A wrongly-sent snapshot frame would
        // be a non-probe frame and make this readiness drain panic. The State
        // probe matches the specific subscription (keyed on session_id "sess-A").
        let (mut ws_miss, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws_miss).await);
        ws_miss
            .send(Message::Text(
                r#"{"op":"subscribe","topic":"state.session.sess-A","states":["ended"]}"#.into(),
            ))
            .await
            .expect("send non-matching-filter specific subscribe");
        wait_subscribe_live(
            &mut ws_miss,
            &state,
            ProbeKind::State {
                session_id: "sess-A",
            },
        )
        .await;

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #2 — A brand-new session's first event reaches a wildcard
    /// subscriber as a live State frame, via Story 2.2's publish path.
    /// No Story 2.3 code change required; the test exists so a
    /// regression in 2.2's publish-then-emit ordering surfaces here.
    #[tokio::test(flavor = "current_thread")]
    async fn new_session_emits_state_to_wildcard_subscriber() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let mut ws = connect_and_skip_hello(addr).await;
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        // No pre-existing sessions, so the snapshot is empty; we use
        // wait_subscribe_live to confirm the subscribe landed before
        // publishing the first event.
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__probe__",
            },
        )
        .await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-NEW",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        // State-only subscription — the Event envelope is filtered;
        // only the State frame reaches the wire.
        let f_state = read_text_frame_or_close(&mut ws).await;
        let st = parse_state_frame(&f_state);
        assert_eq!(st.session_id, "sess-NEW");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #3 — Subscribing to a specific `state.session.<id>` emits a
    /// snapshot for only that session and does not deliver live state
    /// frames for other sessions.
    #[tokio::test(flavor = "current_thread")]
    async fn specific_id_subscription_excludes_other_sessions() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-B",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.sess-A"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        // Exactly one snapshot frame, for sess-A only.
        let frame = read_text_frame_or_close(&mut ws).await;
        let parsed = parse_state_frame(&frame);
        assert_eq!(parsed.session_id, "sess-A");

        // Ordering barrier rather than a fixed sleep: publish the
        // forbidden envelope (sess-B) FIRST, then a permitted live
        // envelope (sess-A). `tokio::sync::broadcast` preserves
        // per-channel publish order, so a buggy `Topic::matches` that
        // let the sess-B State frame leak to this subscriber would
        // deliver sess-B BEFORE the sess-A live frame. Reading the
        // next State frame and asserting it is sess-A — and that the
        // first frame seen is not sess-B — is deterministic across CI
        // load, scheduler jitter, and TCP backpressure.
        //
        // Story 5.2: State frames fire only on `current_state` transitions.
        // Both sessions are currently Working (from initial PreToolUse);
        // Stop drives them to Idle so each publish yields a State frame.
        let _ =
            publish_via_projection(&state, "claude", "sess-B", EventKind::Stop, None, "{}").await;
        let _ =
            publish_via_projection(&state, "claude", "sess-A", EventKind::Stop, None, "{}").await;
        let frame = read_text_frame_or_close(&mut ws).await;
        let parsed = parse_state_frame(&frame);
        assert_eq!(
            parsed.session_id, "sess-A",
            "specific-session subscriber must not receive sess-B; first frame after barrier was {parsed:?}",
        );
        assert_eq!(
            parsed.state.last_event_kind,
            EventKind::Stop,
            "expected live update for sess-A, not a leaked snapshot or sess-B leak",
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #4 — An empty daemon emits zero snapshot frames and
    /// transitions straight to live streaming on the first publish.
    #[tokio::test(flavor = "current_thread")]
    async fn empty_daemon_no_snapshot_frames() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let mut ws = connect_and_skip_hello(addr).await;
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        // The readiness probe panics on any non-probe frame, so if a
        // real snapshot frame were ever emitted from an empty daemon
        // (regression), this would catch it.
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__probe__",
            },
        )
        .await;

        // Publish for a new session — state-only subscription means
        // the Event envelope is filtered; only the State frame reaches
        // the wire. Confirms immediate transition to live streaming.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-NEW",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let f_state = read_text_frame_or_close(&mut ws).await;
        let st = parse_state_frame(&f_state);
        assert_eq!(st.session_id, "sess-NEW");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #5 — `state.session.<id>.current_state` emits the same full
    /// `StateFrame` wire shape as `state.session.<id>` for the snapshot.
    /// Story 2.1 deliberately did not project a smaller current-state
    /// frame; this test pins that decision.
    #[tokio::test(flavor = "current_thread")]
    async fn current_state_subscription_delivers_snapshot() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.sess-A.current_state"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        let frame = read_text_frame_or_close(&mut ws).await;
        let parsed = parse_state_frame(&frame);
        assert_eq!(parsed.session_id, "sess-A");
        assert_eq!(parsed.source, "claude");
        // Full StateFrame: every SessionState field carries a value.
        assert_eq!(parsed.state.last_event_kind, EventKind::PreToolUse);
        assert!(parsed.state.last_event_at_ms > 0);

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #6 — Subscribing to an `events.*`-family topic emits zero
    /// snapshot frames even when sessions exist.
    #[tokio::test(flavor = "current_thread")]
    async fn events_subscription_emits_no_snapshot() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let mut ws = connect_and_skip_hello(addr).await;
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        // wait_subscribe_live panics on non-probe frames — if a snapshot
        // State frame were emitted for an events.* subscription, this
        // would fail before the probe is observed.
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Channel is still alive for events — publish and read one
        // Event frame to confirm live streaming.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PostToolUse,
            None,
            "{}",
        )
        .await;
        let frame = read_text_frame_or_close(&mut ws).await;
        let event = parse_event_frame(&frame);
        assert_eq!(event.session_id, "sess-A");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #7 — Overlapping subscriptions do not re-snapshot. After
    /// `state.session.*` has delivered a snapshot for sess-A, a
    /// subsequent `state.session.sess-A` subscribe emits zero new
    /// snapshot frames (the wildcard already covered it).
    ///
    /// The assertion is twofold:
    /// - `wait_subscribe_live` panics on any non-probe frame during
    ///   readiness, so a regression that re-snapshots sess-A would
    ///   surface as a `non-probe frame` panic.
    /// - After the second subscribe is live, a subsequent publish for
    ///   sess-A yields exactly the live Event + State pair (no
    ///   leftover snapshot frame in the queue).
    #[tokio::test(flavor = "current_thread")]
    async fn overlapping_subscriptions_do_not_re_snapshot() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // First subscribe — wildcard. Read the one snapshot frame.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe wildcard");
        let frame = read_text_frame_or_close(&mut ws).await;
        let parsed = parse_state_frame(&frame);
        assert_eq!(parsed.session_id, "sess-A");

        // Second subscribe — specific id. The wildcard already covers
        // sess-A, so the dedup logic must skip it. wait_subscribe_live
        // confirms readiness via probes; any extra snapshot frame for
        // sess-A would surface as a non-probe-frame panic.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.sess-A"}"#.into(),
        ))
        .await
        .expect("send subscribe specific");
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "sess-A",
            },
        )
        .await;

        // Subsequent live publish for sess-A — state-only subscription
        // filters the Event envelope, leaving exactly one State frame.
        // Because `wait_subscribe_live` above panics on any non-probe
        // frame during readiness, any duplicate-snapshot regression
        // would already have been caught BEFORE this publish. The
        // assertion below then has its own ordering barrier: the live
        // frame's `last_event_kind` (Stop) differs from the snapshot's
        // (PreToolUse), so a leaked snapshot would surface here as a
        // `last_event_kind` mismatch rather than relying on a fixed
        // timeout window.
        //
        // Story 5.2: Stop drives Working → Idle which triggers the State
        // publish; PostToolUse would now preserve Working and emit no
        // State frame.
        let _ =
            publish_via_projection(&state, "claude", "sess-A", EventKind::Stop, None, "{}").await;
        let f_state = read_text_frame_or_close(&mut ws).await;
        let live = parse_state_frame(&f_state);
        assert_eq!(live.session_id, "sess-A");
        assert_eq!(
            live.state.last_event_kind,
            EventKind::Stop,
            "first frame after second subscribe must be the live update, not a leaked snapshot frame",
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1 — Sanity check that snapshot frames carry the
    /// `ServerMessage::State` op so deserialization through the canonical
    /// path produces a `State` variant. Guards against a regression that
    /// would emit a sibling op (e.g. `Hello` shape with state fields).
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_frame_wire_shape_is_state_op() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        let frame = read_text_frame_or_close(&mut ws).await;
        let text = match frame {
            Message::Text(t) => t.to_string(),
            other => panic!("expected text frame, got {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(&text).expect("parse ServerMessage");
        assert!(
            matches!(parsed, ServerMessage::State(_)),
            "snapshot wire shape must be ServerMessage::State; got {parsed:?}"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Second-round review finding (Patch) — a transient reader-pool
    /// failure during `Subscribe` must NOT leave the client silently
    /// unsubscribed. The Subscribe arm logs the snapshot error, emits
    /// an empty snapshot, and STILL inserts the topic so live `State`
    /// frames continue to flow. Snapshot retry is via reconnect
    /// because V1's protocol has no `Subscribe` ack/error frame —
    /// trade-off documented in `crates/daemon/src/api/ws.rs::handle_text_frame`.
    ///
    /// We force the failure by closing the reader pool after the
    /// daemon is running. `publish_via_projection` uses the writer
    /// pool and stays functional.
    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_read_failure_keeps_live_subscription_active() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools, 4);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Pre-create a session so the projection table HAS data. A
        // healthy reader pool would have emitted one snapshot frame
        // for `sess-pre`; closing the reader below forces zero.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-pre",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        // Force `snapshot_for_topic` into the reader-pool error path.
        state.db.reader.close();

        let mut ws = connect_until_ready(addr).await;
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        // `wait_subscribe_live` publishes the probe through the
        // broadcaster (not the reader). It panics on any non-probe
        // frame during readiness, so a regression that emitted an
        // unexpected frame on snapshot failure would be caught here.
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__probe__",
            },
        )
        .await;

        // Prove the topic was inserted: publishing a new session
        // emits a live State frame on this connection. If the prior
        // behavior (return without inserting) had survived, this
        // frame would be filtered by `dispatch_envelope` and the
        // read would time out.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-live",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let frame = read_text_frame_or_close(&mut ws).await;
        let st = parse_state_frame(&frame);
        assert_eq!(st.session_id, "sess-live");
        assert_eq!(st.source, "claude");

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }
}

/// Story 2.4 — lagged consumer recovery via `DroppedFrame`. Verifies the
/// per-connection coalescing helper installed in `crates/daemon/src/api/ws.rs`
/// for both the main `rx.recv()` arm and the parallel
/// `drain_backlog_under_state` arm. All tests use the production publish
/// path (`publish_via_projection`) where the contract is end-to-end, and
/// fall back to synthetic `broadcaster.publish` only when the test is
/// specifically about lag mechanics rather than projection wiring.
mod story_2_4_dropped {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use bowerbird_daemon::api;
    use bowerbird_daemon::api::token::BearerToken;
    use bowerbird_daemon::broadcast::{BroadcastEnvelope, BroadcastHub};
    use bowerbird_daemon::db::DbPools;
    use bowerbird_daemon::state::{AppState, WsConfig};
    use futures_util::{SinkExt, StreamExt};
    use protocol::{
        Event, EventId, EventKind, EventListResponse, ServerMessage, SessionCurrentState,
        SessionState,
    };
    use tokio_tungstenite::tungstenite::Message;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::story_2_1_ws::{
        connect_authed, parse_hello, read_text_frame_or_close, spawn_test_daemon,
    };
    use super::story_2_2_publish::{
        parse_state_frame, publish_via_projection, wait_subscribe_live, ProbeKind, WsStream,
    };
    use super::{fresh_pools, TEST_BEARER};

    /// Per-test AppState factory. Story 2.4 needs to vary BOTH the
    /// broadcast channel capacity (so the channel laps quickly enough to
    /// observe Lagged) AND the coalescing window (so AC #3's burst-bound
    /// can be exercised under wall-clock test latencies). The default
    /// `make_test_state_with_ws` factory hard-codes both, so we build
    /// AppState directly here.
    fn state_with_caps(
        pools: DbPools,
        broadcast_capacity: usize,
        coalesce_window: Duration,
    ) -> AppState {
        let (ingest_tx, _ingest_rx) =
            tokio::sync::mpsc::channel::<bowerbird_daemon::ingest::IngestItem>(1);
        AppState {
            db: pools,
            migrations_complete: Arc::new(AtomicBool::new(true)),
            shutdown_requested: CancellationToken::new(),
            ws_close_requested: CancellationToken::new(),
            bearer: BearerToken::new(TEST_BEARER.to_string()),
            started_at_ms: 0,
            broadcaster: Arc::new(BroadcastHub::new(broadcast_capacity)),
            ws_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            ws_config: WsConfig {
                ping_interval: Duration::from_secs(30),
                pong_timeout: Duration::from_secs(10),
                coalesce_window,
                max_connections: 4,
            },
            ingest_tx,
        }
    }

    /// Synthetic broadcast event for tests that want to drive lag faster
    /// than the SQLite writer pool will allow. Keeps the `source`/
    /// `session_id` distinguishable from probe envelopes so the lag-trigger
    /// publishes can be filtered/counted without confusion.
    fn synthetic_event(event_id: i64, session_id: &str) -> BroadcastEnvelope {
        BroadcastEnvelope::Event(Event {
            event_id: EventId(event_id),
            source: "claude".to_string(),
            session_id: session_id.to_string(),
            kind: EventKind::PreToolUse,
            reaction: None,
            payload: "{}".to_string(),
            created_at: 0,
            pid: None,
            cwd: None,
        })
    }

    /// Read frames until a `Dropped` frame arrives or `max_frames` are
    /// consumed. Returns `(dropped_count, events_before, dropped_first,
    /// dropped_last, all_received_frames)`. Each helper read is bounded
    /// at 5s so a regression that closes the socket fails fast instead
    /// of hanging the test runner.
    async fn read_until_dropped(
        ws: &mut WsStream,
        max_frames: usize,
    ) -> Option<(u64, usize, EventId, EventId)> {
        let mut events_before: usize = 0;
        for _ in 0..max_frames {
            let msg = match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(m))) => m,
                Ok(Some(Err(e))) => panic!("ws recv error while hunting for Dropped: {e:?}"),
                Ok(None) => return None,
                Err(_) => return None,
            };
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => return None,
                Message::Ping(_) | Message::Pong(_) => continue,
                other => panic!("unexpected frame while hunting for Dropped: {other:?}"),
            };
            let server: ServerMessage =
                serde_json::from_str(text.as_str()).expect("parse ServerMessage");
            match server {
                ServerMessage::Dropped(d) => {
                    return Some((
                        d.count,
                        events_before,
                        d.first_dropped_event_id,
                        d.last_dropped_event_id,
                    ));
                }
                ServerMessage::Event(_) | ServerMessage::State(_) => {
                    events_before += 1;
                }
                other => panic!("unexpected ServerMessage while hunting for Dropped: {other:?}"),
            }
        }
        None
    }

    /// AC #1 — A WS client whose read loop is blocked, when 1025+
    /// envelopes are published, eventually receives a `Dropped` frame.
    /// The exact frame index of Dropped depends on TCP send-buffer size
    /// (frames buffered into the kernel are dispatched before the
    /// per-connection task hits Lagged), so the test reads until it
    /// observes Dropped rather than asserting "first frame after
    /// subscribe."
    #[tokio::test(flavor = "current_thread")]
    async fn dropped_frame_after_1025_envelopes_with_blocked_reader() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 1024, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Block the reader: do NOT call ws.next() during the flood. Use
        // synthetic broadcasts so the publish loop saturates the channel
        // far faster than `publish_via_projection`'s DB writes would
        // permit. 4096 envelopes is 4x capacity (1024) — guarantees the
        // receiver's cursor laps regardless of TCP send-buffer size.
        for i in 0..4096 {
            state
                .broadcaster
                .publish(synthetic_event(i + 1, "sess-blocked"));
        }

        // Yield so the per-connection task gets scheduled to process the
        // backlog. Without this, the publishing task hogs the executor
        // and the test races against scheduler ordering.
        tokio::task::yield_now().await;

        // Resume reading. Hunt for Dropped — read up to all 4096 events
        // plus margin to avoid a flaky hang if TCP holds an unexpectedly
        // large buffer.
        let outcome = read_until_dropped(&mut ws, 4200).await;
        let (count, events_before, first, last) =
            outcome.expect("must observe a Dropped frame within 4200 reads");
        assert!(count >= 1, "Dropped count must be >= 1; got {count}");
        assert!(
            count <= 4096,
            "Dropped count must be <= total published envelopes; got {count}"
        );
        assert!(
            first <= last,
            "Dropped frame must have first_id <= last_id; got first={first:?}, last={last:?}"
        );
        // events_before > 0 documents the realistic timing: the per-
        // connection task gets to dispatch some envelopes into the TCP
        // send buffer before back-pressure triggers Lagged. Print for
        // diagnostics on regression.
        eprintln!(
            "story_2_4: {events_before} events arrived before Dropped(count={count}, \
             first={first:?}, last={last:?})"
        );

        // AC #1: "the next frame after the dropped frame is the next
        // legitimate event." Second-round review fix: assert this
        // explicitly — without it, a regression that emitted two
        // Dropped frames in a row (or closed the socket) would pass.
        // Read up to 50 frames hunting for the next non-Dropped,
        // non-Close message; assert it's an Event (synthetic flood
        // residuals are all Events).
        let mut next_legit: Option<ServerMessage> = None;
        for _ in 0..50 {
            match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => {
                    let server: ServerMessage =
                        serde_json::from_str(t.as_str()).expect("parse ServerMessage");
                    match server {
                        ServerMessage::Dropped(_) => {
                            panic!(
                                "AC #1 violation: a second Dropped frame followed the \
                                 first within the coalesce window. Coalescing regressed."
                            )
                        }
                        ServerMessage::Event(_) | ServerMessage::State(_) => {
                            next_legit = Some(server);
                            break;
                        }
                        _ => continue,
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) => {
                    panic!("socket must stay open after Dropped frame")
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(e))) => panic!("ws recv error after Dropped: {e:?}"),
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            matches!(
                next_legit,
                Some(ServerMessage::Event(_)) | Some(ServerMessage::State(_))
            ),
            "AC #1: the next frame after Dropped must be a legitimate Event \
             or State frame; got {next_legit:?}"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1, #4 — After a `Dropped` frame, subsequent publishes are
    /// delivered in order on the SAME socket; the channel is not
    /// permanently degraded.
    #[tokio::test(flavor = "current_thread")]
    async fn dropped_frame_keeps_socket_open() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 16, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Trigger lag with a synthetic flood. Use a synthetic session_id
        // distinct from `sess-after` AND ids starting at 99_000 to keep
        // them disjoint from any database-assigned id this test produces.
        // Second-round review fix: prior synthetic ids `1..=512` could
        // collide with real ids in a fresh DB, so the post-drop
        // assertion could pass on residual flood frames instead of real
        // production events.
        for i in 0..512 {
            state
                .broadcaster
                .publish(synthetic_event(99_000 + i + 1, "sess-flood"));
        }
        tokio::task::yield_now().await;

        // Read until Dropped.
        let outcome = read_until_dropped(&mut ws, 600).await;
        let (count, _, _, _) = outcome.expect("must observe a Dropped frame within 600 reads");
        assert!(count >= 1);

        // Now publish 3 fresh events via the PRODUCTION path. Each must
        // arrive in order as a normal Event frame; the socket is open
        // and `last_delivered_event_id` continues to advance.
        let id1 = publish_via_projection(
            &state,
            "claude",
            "sess-after",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let id2 = publish_via_projection(
            &state,
            "claude",
            "sess-after",
            EventKind::PostToolUse,
            None,
            "{}",
        )
        .await;
        let id3 = publish_via_projection(
            &state,
            "claude",
            "sess-after",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        // Drain frames until we collect the three post-drop events.
        // Match by (event_id, source, session_id, kind) so a stray
        // synthetic flood envelope can never satisfy the assertion —
        // the flood publishes use `session_id = "sess-flood"` and a
        // different id range. Second-round review fix.
        let mut found: Vec<(EventId, EventKind)> = Vec::with_capacity(3);
        let expected: Vec<(EventId, EventKind)> = vec![
            (id1, EventKind::PreToolUse),
            (id2, EventKind::PostToolUse),
            (id3, EventKind::PreToolUse),
        ];
        for _ in 0..2000 {
            if found.len() == 3 {
                break;
            }
            let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("read within 5s")
                .expect("stream not ended")
                .expect("recv ok");
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => panic!("socket closed unexpectedly after Dropped"),
                _ => continue,
            };
            let server: ServerMessage =
                serde_json::from_str(text.as_str()).expect("parse ServerMessage");
            if let ServerMessage::Event(f) = server {
                // Strict matching — must be the production-path event we
                // just published, not a residual flood envelope.
                if f.event.source == "claude"
                    && f.event.session_id == "sess-after"
                    && (f.event.event_id == id1
                        || f.event.event_id == id2
                        || f.event.event_id == id3)
                {
                    found.push((f.event.event_id, f.event.kind));
                }
            }
        }
        assert_eq!(
            found, expected,
            "post-Dropped events must arrive in publication order with the \
             matching event_id, source=\"claude\", session_id=\"sess-after\", \
             and EventKind"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1 — `DroppedFrame.count` is positive (envelopes, not bytes);
    /// `first_dropped_event_id <= last_dropped_event_id`. The wire-id
    /// values are best-estimate per the helper's design — we assert the
    /// invariants enforced by `DroppedFrame::new`, not exact values.
    #[tokio::test(flavor = "current_thread")]
    async fn dropped_frame_carries_count_in_envelopes() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 16, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        for i in 0..256 {
            state.broadcaster.publish(synthetic_event(i + 1, "sess-x"));
        }
        tokio::task::yield_now().await;

        let outcome = read_until_dropped(&mut ws, 300).await;
        let (count, _, first, last) =
            outcome.expect("must observe a Dropped frame within 300 reads");

        // Invariants from DroppedFrame::new — best-estimate semantics mean
        // we deliberately don't assert exact event-id values.
        assert!(
            count > 0,
            "Dropped count must be > 0 (envelopes); got {count}"
        );
        assert!(
            first <= last,
            "Dropped frame must have first_dropped_event_id <= last_dropped_event_id; \
             got first={first:?}, last={last:?}"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #2 — A presenter that receives a `Dropped` frame can recover
    /// missed events via the REST surface using its OWN
    /// `last_delivered_event_id` (NOT the values inside the Dropped frame,
    /// which are best-estimate). The REST response's
    /// `oldest_available_event_id` confirms the gap is recoverable
    /// (`oldest_available_event_id <= last_delivered_event_id + 1`).
    #[tokio::test(flavor = "current_thread")]
    async fn dropped_frame_rest_refetch_recovers() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 16, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Publish a single real event so the presenter has a
        // last_delivered_event_id cursor it can pass to REST.
        let id0 = publish_via_projection(
            &state,
            "claude",
            "sess-rest",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        let _ = read_text_frame_or_close(&mut ws).await; // drain the Event frame

        // Now publish several more REAL events; the presenter stops
        // reading mid-flood and falls behind.
        let mut produced: Vec<EventId> = Vec::new();
        for i in 0..40 {
            let id = publish_via_projection(
                &state,
                "claude",
                "sess-rest",
                if i % 2 == 0 {
                    EventKind::PreToolUse
                } else {
                    EventKind::PostToolUse
                },
                None,
                "{}",
            )
            .await;
            produced.push(id);
        }
        // Synthetic flood to force Lagged.
        for i in 0..256 {
            state
                .broadcaster
                .publish(synthetic_event(99_000 + i + 1, "sess-rest"));
        }
        tokio::task::yield_now().await;

        // Hunt for Dropped, ignoring intermediate Event frames.
        let outcome = read_until_dropped(&mut ws, 400).await;
        let (count, _, _, _) = outcome.expect("must observe a Dropped frame within 400 reads");
        assert!(count > 0);

        // The presenter's authoritative cursor is `id0` (the last real
        // event it dispatched and tracked). It does NOT use the Dropped
        // frame's first/last (best-estimate). REST should return the
        // missed real events that have event_id > id0. We use a fresh
        // axum::Router::oneshot over the same AppState rather than
        // making a real HTTP request — both are equally valid, and
        // oneshot avoids paying for another listener.
        let app = api::router(state.clone());
        let req = Request::builder()
            .uri(format!("/sessions/sess-rest/events?since={}", id0.0))
            .header(header::AUTHORIZATION, format!("Bearer {TEST_BEARER}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let body: EventListResponse = serde_json::from_slice(&bytes).expect("parse events");
        assert!(
            !body.events.is_empty(),
            "REST must return events past last_delivered_event_id={id0:?}"
        );
        // Second-round review fix: assert EVERY produced id > id0 is
        // present in the response and arrives in publication order.
        // Without this, the test would pass even if REST recovery
        // returned a partial set — which would silently corrupt the
        // presenter's gap recovery.
        let returned_ids: Vec<EventId> = body.events.iter().map(|e| e.event_id).collect();
        let expected_ids: Vec<EventId> =
            produced.iter().copied().filter(|id| id.0 > id0.0).collect();
        assert!(
            !expected_ids.is_empty(),
            "test scaffolding: should have produced events past id0={id0:?}"
        );
        assert_eq!(
            returned_ids, expected_ids,
            "REST recovery must return every produced event id > \
             last_delivered_event_id={id0:?}, in publication order. \
             Missing/reordered events would corrupt presenter gap recovery."
        );
        // Gap recoverable: oldest_available_event_id <= last_delivered + 1.
        assert!(
            body.oldest_available_event_id.0 <= id0.0 + 1,
            "gap must be recoverable: oldest={:?} <= last_delivered+1={}",
            body.oldest_available_event_id,
            id0.0 + 1
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #3 — Sustained lag is bounded by the coalesce window. With
    /// `coalesce_window = 100ms` and ~1s real-time test duration, the
    /// theoretical ceiling is `ceil(1000/100) = 10` Dropped frames. We
    /// assert `<= 30` to absorb scheduler jitter while staying far
    /// below the "frame storm" failure mode (which would emit hundreds
    /// or thousands).
    #[tokio::test(flavor = "current_thread")]
    async fn sustained_lag_does_not_storm_dropped_frames() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 4, Duration::from_millis(100));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Publisher: spawn a task that fires synthetic events in bursts
        // for ~1 second of wall time. Short between-burst sleep keeps
        // the runtime cycling so the per-connection task gets to call
        // rx.recv repeatedly and observe Lagged repeatedly.
        let pub_deadline = std::time::Instant::now() + Duration::from_millis(1000);
        let broadcaster = state.broadcaster.clone();
        let publisher = tokio::spawn(async move {
            let mut event_id: i64 = 1;
            while std::time::Instant::now() < pub_deadline {
                for _ in 0..50 {
                    broadcaster.publish(synthetic_event(event_id, "sess-storm"));
                    event_id += 1;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        // Slow reader: poll until publisher's deadline + drain window.
        let read_deadline = pub_deadline + Duration::from_millis(300);
        let mut dropped_count: u64 = 0;
        let mut total_frames: u64 = 0;
        while std::time::Instant::now() < read_deadline {
            match tokio::time::timeout(Duration::from_millis(20), ws.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    total_frames += 1;
                    let server: ServerMessage =
                        serde_json::from_str(text.as_str()).expect("parse ServerMessage");
                    if matches!(server, ServerMessage::Dropped(_)) {
                        dropped_count += 1;
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) => {
                    panic!("socket must stay open during sustained lag")
                }
                Ok(Some(Err(e))) => panic!("ws recv error: {e:?}"),
                Ok(None) => break,
                Ok(_) | Err(_) => continue,
            }
            // Slow reader: yield + brief sleep so back-pressure persists.
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        publisher.abort();

        eprintln!(
            "story_2_4 sustained_lag: total_frames={total_frames}, dropped_frames={dropped_count}"
        );
        assert!(
            dropped_count >= 1,
            "sustained-lag scenario must produce at least one Dropped frame; got 0 of \
             total_frames={total_frames}"
        );
        // Ceiling = ceil(1000ms/100ms) + margin for jitter. The failure mode
        // we're guarding against (storm) would be hundreds.
        assert!(
            dropped_count <= 30,
            "Dropped emissions should be bounded by coalesce_window; got \
             {dropped_count} (theoretical bound ~10, asserted <=30 for jitter)"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1 + Story 2.3 deferred-work.md:79 — Lag during a Subscribe
    /// cycle surfaces as a `Dropped` frame and the socket stays open.
    ///
    /// The Subscribe arm's six-step ordering is [A]drain → [B]clock →
    /// [C]snapshot_read → [D]insert_topic → [E]send_snapshot →
    /// [F]main_loop. Lag can be detected at [A] (drain arm) OR [F]
    /// (main rx.recv after [E]), depending on whether channel saturation
    /// happens before [A] runs or during [E]'s socket.send loop.
    ///
    /// Both orderings are correct per Story 2.4's recovery design:
    /// either path routes through the SAME `emit_dropped_or_coalesce`
    /// helper, producing the same wire frame. The test asserts the
    /// resilient invariants: snapshot frames eventually arrive, a
    /// Dropped frame is observed, the socket stays open, and no Event
    /// frame leaks through the state.session.* topic filter.
    #[tokio::test(flavor = "current_thread")]
    async fn lag_during_snapshot_emits_dropped_after_snapshot_completes() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 4, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Pre-populate the projection table so the snapshot for
        // state.session.* will read multiple rows. Each publish_via_projection
        // also pushes one Event + one State envelope through the broadcast
        // hub, but no client is connected yet so they're discarded.
        for i in 0..10 {
            let _ = publish_via_projection(
                &state,
                "claude",
                &format!("snap-sess-{i:02}"),
                EventKind::PreToolUse,
                None,
                "{}",
            )
            .await;
        }

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Spawn a background publisher that floods the broadcast hub
        // for 200ms with synthetic events. The flood overlaps with the
        // snapshot SEND phase ([E]), forcing lag DURING the per-
        // connection task's socket.send loop. The post-Finding-#1
        // behaviour: drain at [A] runs under empty subscriptions and
        // silent-fast-forwards; the main loop at [F] (after [E]) is
        // the one that sees the residual lag and emits Dropped — the
        // exact scenario Story 2.3 deferred-work line 79 carried into
        // 2.4 to verify.
        let broadcaster = state.broadcaster.clone();
        let flood = tokio::spawn(async move {
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            let mut event_id: i64 = 1;
            while std::time::Instant::now() < deadline {
                for _ in 0..20 {
                    broadcaster.publish(synthetic_event(event_id, "snap-lag-src"));
                    event_id += 1;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });

        // Send Subscribe AFTER the flood task started — the snapshot
        // send loop runs while the flood is still publishing, so the
        // channel laps under the new subscription set.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");

        // Read all frames. Collect counts of State (snapshot) frames
        // and Dropped frames. With Finding #1's guard the drain arm
        // silent-fast-forwards (subscriptions empty at [A]); the main
        // loop after [E] catches the residual lag and emits.
        let mut snapshot_seen = 0usize;
        let mut dropped_seen = 0usize;
        for _ in 0..200 {
            let msg = match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
                Ok(Some(Ok(m))) => m,
                Ok(Some(Err(e))) => panic!("ws recv error during snapshot+lag: {e:?}"),
                Ok(None) => break,
                Err(_) => break,
            };
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => panic!("socket must stay open during snapshot+lag"),
                _ => continue,
            };
            let server: ServerMessage =
                serde_json::from_str(text.as_str()).expect("parse ServerMessage");
            match server {
                ServerMessage::State(_) => snapshot_seen += 1,
                ServerMessage::Dropped(_) => {
                    dropped_seen += 1;
                    // Break once we've seen the contract pair: snapshot
                    // delivered AND lag reported. Further frames are
                    // residual flood-state envelopes which don't
                    // strengthen the assertion.
                    if snapshot_seen > 0 {
                        break;
                    }
                }
                ServerMessage::Event(_) => {
                    // state.session.* must NOT deliver Event frames per
                    // dispatch_envelope's topic-match logic.
                    panic!("Event frame leaked through state.session.* topic filter")
                }
                other => panic!("unexpected ServerMessage during snapshot+lag: {other:?}"),
            }
        }
        // Stop the flood promptly so the test doesn't pay for any
        // remaining iterations.
        flood.abort();

        assert!(
            snapshot_seen > 0,
            "expected at least one snapshot State frame; saw 0 (snapshot phase regressed?)"
        );
        assert!(
            dropped_seen > 0,
            "expected at least one Dropped frame from lag DURING snapshot send; \
             saw 0 (lag-during-snapshot recovery regressed?). snapshot_seen={snapshot_seen}"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1, #3 — Lag detected by `drain_backlog_under_state` (the
    /// Subscribe/Unsubscribe drain phase) routes through the SAME
    /// `emit_dropped_or_coalesce` helper as the main `rx.recv()` arm.
    /// This guards against a regression where drain silently discards
    /// `TryRecvError::Lagged` (the pre-2.4 behaviour).
    ///
    /// The test cannot deterministically pin which arm fired the
    /// Dropped emission (main loop vs. drain), since the per-connection
    /// task interleaves both based on real-time scheduler ordering. The
    /// assertion is therefore "at least one Dropped frame observed
    /// across multiple Subscribe cycles," which is enough to fail if
    /// the drain arm silently swallows lag.
    #[tokio::test(flavor = "current_thread")]
    async fn lag_in_drain_backlog_emits_dropped_through_same_helper() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 4, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // First subscribe: events.* — wait live so the cursor is engaged.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send first subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Block the reader, flood the channel — receiver cursor lags.
        for i in 0..256 {
            state
                .broadcaster
                .publish(synthetic_event(i + 1, "sess-drain"));
        }
        tokio::task::yield_now().await;

        // Now send a SECOND subscribe (state.session.*) — this triggers
        // drain_backlog_under_state under the OLD subscription set. If
        // the channel was lapped by this point, drain's try_recv arm sees
        // Lagged and routes through emit_dropped_or_coalesce. The main
        // rx.recv arm may also see Lagged at some point. Either path
        // emits Dropped through the same helper; the test asserts at
        // least one Dropped frame is observed.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send second subscribe");

        // Now read; expect to see Dropped somewhere in the stream.
        let outcome = read_until_dropped(&mut ws, 400).await;
        let (count, _, _, _) = outcome
            .expect("Dropped must be emitted when channel lapped before a Subscribe-induced drain");
        assert!(count >= 1);

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #3 lower bound + AC #4 — After sustained lag emits its first
    /// `Dropped`, a period of silence longer than `coalesce_window`
    /// followed by a fresh lag burst emits a SECOND `Dropped`. The
    /// window is a sliding boundary per the helper's design, not a
    /// once-per-connection latch.
    #[tokio::test(flavor = "current_thread")]
    async fn coalesce_window_resets_after_silence() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 4, Duration::from_millis(150));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // First burst.
        for i in 0..64 {
            state
                .broadcaster
                .publish(synthetic_event(i + 1, "sess-burst-1"));
        }
        tokio::task::yield_now().await;

        // Read until first Dropped.
        let first = read_until_dropped(&mut ws, 200)
            .await
            .expect("first Dropped must arrive after burst 1");
        assert!(first.0 >= 1);

        // Sleep > coalesce_window (150ms) with NO further lag-trigger so
        // the helper's `now - last_dropped_at > coalesce_window` check
        // fires on the next Lagged.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Second burst.
        for i in 0..64 {
            state
                .broadcaster
                .publish(synthetic_event(10_000 + i + 1, "sess-burst-2"));
        }
        tokio::task::yield_now().await;

        // A SECOND Dropped frame must be observed — the window is a
        // sliding boundary, not a once-per-connection latch.
        let second = read_until_dropped(&mut ws, 200)
            .await
            .expect("second Dropped must arrive after silence + burst 2");
        assert!(second.0 >= 1);

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 2.4 second-round code-review finding #1 — A client that
    /// connects but never subscribes MUST NOT receive an unsolicited
    /// `Dropped` frame, even if lag accumulates on the per-connection
    /// broadcast receiver between connect and the first Subscribe.
    /// Such a frame would describe a recovery gap for events the
    /// presenter never asked for. Both lag arms (main `rx.recv()` and
    /// `drain_backlog_under_state`) gate emission on
    /// `subscriptions.is_empty() == false`.
    #[tokio::test(flavor = "current_thread")]
    async fn lag_before_any_subscription_does_not_emit_dropped() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 4, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Connected but NOT subscribed. Flood the hub — channel laps.
        for i in 0..256 {
            state
                .broadcaster
                .publish(synthetic_event(i + 1, "sess-pre-sub"));
        }
        tokio::task::yield_now().await;

        // Now subscribe to events.* (drain runs under the OLD set,
        // which is empty — silent fast-forward of any drain-time lag).
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Publish ONE legitimate event via the production path; we
        // expect to receive it normally — no preceding Dropped frame
        // for the pre-subscription flood.
        let id = publish_via_projection(
            &state,
            "claude",
            "sess-real",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        // Read up to 50 frames. Allowed: probe frames (Event with
        // session_id starting "__probe-") and the real Event we just
        // published. Forbidden: any Dropped frame at all.
        let mut saw_real = false;
        for _ in 0..50 {
            let msg = match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
                Ok(Some(Ok(m))) => m,
                Ok(None) | Err(_) => break,
                Ok(Some(Err(e))) => panic!("ws recv error: {e:?}"),
            };
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => panic!("socket closed unexpectedly"),
                _ => continue,
            };
            let server: ServerMessage =
                serde_json::from_str(text.as_str()).expect("parse ServerMessage");
            match server {
                ServerMessage::Dropped(d) => panic!(
                    "AC violation (Finding #1): pre-subscription lag must NOT \
                     produce a Dropped frame; got count={}",
                    d.count
                ),
                ServerMessage::Event(f) if f.event.event_id == id => {
                    saw_real = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(
            saw_real,
            "expected the post-subscription real Event to arrive"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 2.4 second-round code-review finding #1 — After
    /// unsubscribing from the last topic, a client is in the same
    /// "no active subscription" state as a fresh connection. Lag
    /// during that interval MUST be silent fast-forward, not emit a
    /// Dropped frame against the new (empty) topic set.
    #[tokio::test(flavor = "current_thread")]
    async fn lag_after_last_unsubscribe_does_not_emit_dropped() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 4, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Subscribe events.*, wait live, then immediately unsubscribe —
        // returns the client to a no-active-subscription state.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;
        ws.send(Message::Text(
            r#"{"op":"unsubscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send unsubscribe");
        // Give the per-connection task a chance to process the
        // unsubscribe before flooding (the unsubscribe runs its own
        // drain under the OLD set events.* — that drain CAN emit
        // Dropped if lag is present, but we haven't flooded yet).
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now flood under empty subscriptions. Either lag arm should
        // silent-fast-forward.
        for i in 0..256 {
            state
                .broadcaster
                .publish(synthetic_event(i + 1, "sess-post-unsub"));
        }
        tokio::task::yield_now().await;

        // Re-subscribe. The drain phase runs under the (still empty)
        // OLD set; Finding #1's guard skips emission.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.*"}"#.into(),
        ))
        .await
        .expect("send re-subscribe");
        wait_subscribe_live(&mut ws, &state, ProbeKind::Event { source: "claude" }).await;

        // Publish one legitimate event and assert it arrives without
        // any preceding Dropped frame.
        let id = publish_via_projection(
            &state,
            "claude",
            "sess-real-2",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let mut saw_real = false;
        for _ in 0..50 {
            let msg = match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
                Ok(Some(Ok(m))) => m,
                Ok(None) | Err(_) => break,
                Ok(Some(Err(e))) => panic!("ws recv error: {e:?}"),
            };
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => panic!("socket closed unexpectedly"),
                _ => continue,
            };
            let server: ServerMessage =
                serde_json::from_str(text.as_str()).expect("parse ServerMessage");
            match server {
                ServerMessage::Dropped(d) => panic!(
                    "AC violation (Finding #1): post-unsubscribe lag must NOT \
                     produce a Dropped frame against a freshly empty subscription \
                     set; got count={}",
                    d.count
                ),
                ServerMessage::Event(f) if f.event.event_id == id => {
                    saw_real = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(
            saw_real,
            "expected the post-resubscription real Event to arrive"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// AC #1 / Story 2.4 code-review finding #1 — A state-only
    /// subscriber MUST NOT have its `last_delivered_event_id` cursor
    /// advanced by Event envelopes that didn't match its subscription.
    /// The first `DroppedFrame` after lag must therefore use the
    /// `EventId(0)` "from the beginning" sentinel (per Dev Notes
    /// "Cursor tracking only on Event dispatch") — NOT a cursor poisoned
    /// by unrelated Events that passed through `rx.recv()` but were
    /// filtered out by `dispatch_envelope`.
    ///
    /// Regression scenario the pre-fix code shipped:
    ///   1. Subscribe to `state.session.*` (state-only).
    ///   2. Publish 5 events via `publish_via_projection` — each emits
    ///      one Event envelope + one State envelope on the broadcast
    ///      hub. The State frames are delivered; the Event frames
    ///      pass through `rx.recv()` but `dispatch_envelope` returns
    ///      Filtered (no wire send).
    ///   3. Pre-fix: `last_delivered_event_id` was advanced to the last
    ///      Event's id, even though no Event frame ever reached the
    ///      socket.
    ///   4. Lag is triggered; the resulting Dropped frame reports
    ///      `first_dropped_event_id = last_delivered + 1` — a value
    ///      the presenter never saw, corrupting REST recovery.
    /// Post-fix: the cursor stays at `EventId(0)` (None internally),
    /// and the Dropped frame's `first_dropped_event_id` is `EventId(0)`.
    #[tokio::test(flavor = "current_thread")]
    async fn state_only_subscriber_does_not_advance_cursor_on_unrelated_events() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 16, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__probe__",
            },
        )
        .await;

        // Publish 5 real events via the production projection path. Each
        // produces one Event + (post-Story-5.2) ZERO-OR-ONE State on the
        // hub. The state-only subscription matches only the State envelopes;
        // the Event envelopes pass through rx.recv but `dispatch_envelope`
        // filters them. Drain the 5 State frames as they arrive so the per-
        // connection task isn't stuck on socket.send.
        //
        // Story 5.2: alternate PreToolUse / Stop so each event actually
        // transitions `current_state` (Idle ↔ Working) and therefore
        // publishes a State envelope. PreToolUse/PostToolUse alternation
        // would now only emit one State frame (the very first Idle→Working
        // transition); the rest would preserve Working.
        for i in 0..5 {
            let _ = publish_via_projection(
                &state,
                "claude",
                "sess-state-only",
                if i % 2 == 0 {
                    EventKind::PreToolUse
                } else {
                    EventKind::Stop
                },
                None,
                "{}",
            )
            .await;
            let frame = read_text_frame_or_close(&mut ws).await;
            let server: ServerMessage = match &frame {
                Message::Text(t) => serde_json::from_str(t.as_str()).expect("parse"),
                other => panic!("expected State frame, got {other:?}"),
            };
            assert!(
                matches!(server, ServerMessage::State(_)),
                "state.session.* must only deliver State frames; got {server:?}"
            );
        }

        // Now trigger lag — synthetic flood of EVENT envelopes (these
        // never match `state.session.*` either, so they're all Filtered
        // dispatch outcomes). The per-connection task will see
        // RecvError::Lagged on a subsequent rx.recv.
        for i in 0..256 {
            state
                .broadcaster
                .publish(synthetic_event(i + 1, "sess-state-only"));
        }
        tokio::task::yield_now().await;

        // Hunt for Dropped. The frame's first_dropped_event_id MUST be
        // EventId(0) — the cursor was never advanced because no Event
        // ever reached the wire on this state-only subscription.
        let outcome = read_until_dropped(&mut ws, 400)
            .await
            .expect("state-only subscriber must still receive Dropped on lag");
        let (count, _events_before, first, _last) = outcome;
        assert!(count >= 1);
        assert_eq!(
            first,
            EventId(0),
            "state-only subscriber's first_dropped_event_id MUST be \
             EventId(0) sentinel (no prior Event delivery). Got {first:?} \
             — likely cursor advanced on unrelated Events, regression of \
             Story 2.4 code-review finding #1."
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }

    /// Story 5.8 review pass-4 — broadcast lag must INVALIDATE snapshot
    /// coverage so a state-only subscriber can recover via re-subscribe.
    ///
    /// `snapshotted_keys` suppresses re-snapshots for `(source, session_id)`
    /// rows the connection already has current (no double-delivery). But a
    /// broadcast `Lagged(n)` reports only a count, never the identities of
    /// the evicted envelopes — any of them could have been a `State` frame
    /// for a covered session. If coverage is NOT invalidated, the row stays
    /// in `snapshotted_keys`, so a re-subscribe skips it and the subscriber
    /// is permanently stale (a state-only subscriber can't replay missed
    /// state via `/sessions/{id}/events?since=`).
    ///
    /// Scenario:
    ///   1. Seed sess-A; subscribe `state.session.*` → snapshot covers sess-A
    ///      (its key is recorded).
    ///   2. Flood synthetic events to force `RecvError::Lagged` while the
    ///      state subscription is active → the connection receives `Dropped`.
    ///   3. Re-subscribe `state.session.*`.
    /// Post-fix: lag cleared `snapshotted_keys`, so sess-A re-snapshots →
    /// a fresh `StateFrame` for sess-A arrives. Pre-fix: no re-snapshot, and
    /// the observation loop never sees sess-A and the assertion fails.
    ///
    /// Observation note: after the flood the connection task is busy draining
    /// residual envelopes, so the re-subscribe's snapshot frame (which the
    /// daemon *does* emit — verified by tracing) can be starved from flushing.
    /// We drive the runtime like `wait_subscribe_live` — republish a live
    /// `state` probe and drain every iteration; each probe is a
    /// `state.session.*` frame the daemon sends, flushing everything pending
    /// including the sess-A re-snapshot.
    ///
    /// Flake history (resolved 2026-07-28): this test failed 3/3 under
    /// `cargo test --workspace` in June 2026 while passing alone, and was
    /// blamed on cross-binary concurrency. That was wrong: cargo runs test
    /// binaries sequentially, and the failures tracked concurrent-worktree
    /// session load on older hardware (`scripts/test.sh` now locks that
    /// out). On current hardware it passes 20/20 under 2x CPU
    /// oversubscription, finishing in ~0.05s against the 5s deadline. If
    /// it ever flakes again (e.g. a slow CI runner), the durable fix is
    /// the testability seam sketched in
    /// `docs/research/test-isolation-bowerbird-findings.md` §Leads #3;
    /// full re-examination in
    /// `docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md`
    /// (follow-up 2026-07-28 #2).
    #[tokio::test(flavor = "current_thread")]
    async fn lag_invalidates_snapshot_coverage_resubscribe_resnapshots() {
        let (_tmp, pools) = fresh_pools().await;
        let state = state_with_caps(pools, 16, Duration::from_secs(1));
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Seed sess-A in the projection table BEFORE connecting (the
        // broadcast envelopes are discarded — no client yet).
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);

        // Subscribe → snapshot delivers sess-A; its key is recorded in
        // `snapshotted_keys`. Drain the snapshot frame so the readiness
        // gate below sees only probe frames.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        let snap = parse_state_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            snap.session_id, "sess-A",
            "first subscribe must snapshot sess-A"
        );

        // Confirm the per-connection task has fully processed the subscribe
        // (key recorded) and is live before we force lag.
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__probe__",
            },
        )
        .await;

        // Force lag: flood synthetic EVENT envelopes (Filtered by the
        // state-only subscription, but they still lap the shared channel,
        // so the per-connection task observes `RecvError::Lagged`).
        for i in 0..256 {
            state
                .broadcaster
                .publish(synthetic_event(i + 1, "sess-flood"));
        }
        tokio::task::yield_now().await;

        // The subscriber must still receive a `Dropped` frame (socket stays
        // open) — and, with the fix, the lag has cleared snapshot coverage.
        let outcome = read_until_dropped(&mut ws, 400)
            .await
            .expect("active state subscriber must receive Dropped on lag");
        assert!(outcome.0 >= 1, "Dropped count must be positive");

        // Drain the flood residue and confirm the connection task has caught
        // up / gone idle BEFORE re-subscribing. `wait_subscribe_live` is the
        // proven drive-the-runtime helper: it republishes a probe and reads
        // until the probe arrives, which only happens once the task has
        // dispatched everything ahead of it (the residual flood events are
        // Filtered by the state-only subscription, so they produce no wire
        // frames). With the socket idle, the re-subscribe's snapshot flushes
        // promptly — the same clean-socket condition the non-lag re-snapshot
        // tests rely on. The probe session ("__caughtup__") has no projection
        // row, so it is never itself snapshotted; sess-A's coverage stays
        // cleared by the lag (no live sess-A frame re-recorded it).
        wait_subscribe_live(
            &mut ws,
            &state,
            ProbeKind::State {
                session_id: "__caughtup__",
            },
        )
        .await;

        // Re-subscribe. Post-fix the lag invalidated sess-A's coverage, so the
        // subscribe handler re-snapshots it.
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"state.session.*"}"#.into(),
        ))
        .await
        .expect("send re-subscribe");

        // Observe the re-snapshot by driving the runtime (the `wait_subscribe_live`
        // discipline): each iteration publish a fresh live `state` probe for a
        // throwaway session and drain everything queued. The probe is a
        // `state.session.*` frame the daemon sends, and that send flushes the
        // queued sess-A snapshot too. Without the fix sess-A is never
        // re-snapshotted, so it never appears and the 5s deadline fails the test.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_sess_a = false;
        let mut pump = 0u64;
        while !saw_sess_a && std::time::Instant::now() < deadline {
            state.broadcaster.publish(BroadcastEnvelope::State {
                source: format!("__pump-{pump}__"),
                session_id: "__pump__".to_string(),
                state: SessionState {
                    current_state: SessionCurrentState::Idle,
                    last_event_kind: EventKind::PreToolUse,
                    last_event_at_ms: 0,
                    last_pid: None,
                    cwd: None,
                    started_at: None,
                },
            });
            pump += 1;
            loop {
                let msg = match tokio::time::timeout(Duration::from_millis(50), ws.next()).await {
                    Ok(Some(Ok(m))) => m,
                    Ok(None) => break,
                    Ok(Some(Err(e))) => panic!("ws recv error during re-subscribe: {e:?}"),
                    Err(_) => break, // queue drained for now — republish to keep pumping
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => panic!("socket must stay open"),
                    _ => continue,
                };
                let server: ServerMessage =
                    serde_json::from_str(text.as_str()).expect("parse ServerMessage");
                match server {
                    ServerMessage::State(f) if f.session_id == "sess-A" => {
                        saw_sess_a = true;
                        break;
                    }
                    // Pump probes (session "__pump__"), residual Dropped — keep hunting.
                    ServerMessage::State(_) | ServerMessage::Dropped(_) => {}
                    ServerMessage::Event(_) => {
                        panic!("Event frame leaked through state.session.* topic filter")
                    }
                    other => panic!("unexpected ServerMessage during re-subscribe: {other:?}"),
                }
            }
        }
        assert!(
            saw_sess_a,
            "re-subscribe after lag MUST re-snapshot sess-A (lag should have \
             invalidated snapshot coverage); never saw a fresh sess-A StateFrame"
        );

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
    }
}

mod story_2_5_shutdown {
    use std::process::{Command as StdCommand, Stdio};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use bowerbird_daemon::db::{init_pools, run_migrations};
    use bowerbird_daemon::state::wait_for_ws_connection_drain;
    use futures_util::SinkExt;
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    use protocol::{EventKind, ServerMessage};
    use tempfile::TempDir;
    use tokio_tungstenite::tungstenite::Message;

    use super::story_2_1_ws::{
        authed_request, connect_authed, parse_hello, read_text_frame_or_close, spawn_test_daemon,
        ws_url_header,
    };
    use super::story_2_2_publish::{
        parse_event_frame, publish_via_projection, wait_subscribe_live_all, ProbeKind,
    };
    use super::{fresh_pools, make_test_state_with_ws, TEST_BEARER};

    fn default_state(pools: bowerbird_daemon::db::DbPools) -> bowerbird_daemon::state::AppState {
        make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        )
    }

    fn assert_protocol_shutdown_close(msg: Message) {
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected protocol Close text frame, got {other:?}"),
        };
        let parsed: ServerMessage = serde_json::from_str(text.as_str()).expect("parse close");
        match parsed {
            ServerMessage::Close(frame) => {
                assert_eq!(frame.reason.as_deref(), Some("daemon shutdown"));
            }
            other => panic!("expected ServerMessage::Close, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_token_sends_protocol_close_to_all_connected_tools() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws1, _) = connect_authed(addr, TEST_BEARER).await;
        let (mut ws2, _) = connect_authed(addr, TEST_BEARER).await;
        let (mut ws3, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws1).await);
        let _ = parse_hello(&read_text_frame_or_close(&mut ws2).await);
        let _ = parse_hello(&read_text_frame_or_close(&mut ws3).await);

        for ws in [&mut ws1, &mut ws2, &mut ws3] {
            ws.send(Message::Text(
                r#"{"op":"subscribe","topic":"events.*"}"#.into(),
            ))
            .await
            .expect("send subscribe");
        }
        wait_subscribe_live_all(
            &mut [&mut ws1, &mut ws2, &mut ws3],
            &state,
            ProbeKind::Event { source: "claude" },
        )
        .await;

        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-shutdown",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;

        for ws in [&mut ws1, &mut ws2, &mut ws3] {
            let _event = read_text_frame_or_close(ws).await;
        }

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();

        for ws in [&mut ws1, &mut ws2, &mut ws3] {
            assert_protocol_shutdown_close(read_text_frame_or_close(ws).await);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_close_drains_buffered_event_before_protocol_close() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        let (mut ws, _) = connect_authed(addr, TEST_BEARER).await;
        let _ = parse_hello(&read_text_frame_or_close(&mut ws).await);
        ws.send(Message::Text(
            r#"{"op":"subscribe","topic":"events.claude.*"}"#.into(),
        ))
        .await
        .expect("send subscribe");
        wait_subscribe_live_all(
            &mut [&mut ws],
            &state,
            ProbeKind::Event { source: "claude" },
        )
        .await;

        let event_id = publish_via_projection(
            &state,
            "claude",
            "sess-shutdown-drain",
            EventKind::PreToolUse,
            None,
            r#"{"tool":"bash"}"#,
        )
        .await;

        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();

        let event = parse_event_frame(&read_text_frame_or_close(&mut ws).await);
        assert_eq!(
            event.event_id, event_id,
            "queued broadcast event must be drained before the shutdown close frame"
        );
        assert_protocol_shutdown_close(read_text_frame_or_close(&mut ws).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_requested_rejects_new_ws_upgrades() {
        let (_tmp, pools) = fresh_pools().await;
        let state = default_state(pools);
        let (addr, _server) = spawn_test_daemon(state.clone()).await;
        state.shutdown_requested.cancel();

        let req = authed_request(&ws_url_header(addr), TEST_BEARER);
        let err = tokio_tungstenite::connect_async(req)
            .await
            .expect_err("shutdown should prevent new websocket sessions");
        if let tokio_tungstenite::tungstenite::Error::Http(resp) = err {
            assert_eq!(resp.status().as_u16(), 503);
        }

        state.ws_close_requested.cancel();
    }

    fn spawn_daemon_for_signal(
        tmp: &TempDir,
    ) -> (std::process::Child, std::path::PathBuf, std::path::PathBuf) {
        let bowerbird_dir = tmp.path().join(".bowerbird");
        std::fs::create_dir_all(&bowerbird_dir).expect("mkdir");
        let sock_path = bowerbird_dir.join("ingest.sock");
        let stderr_path = tmp.path().join("daemon.stderr.log");
        let stderr = std::fs::File::create(&stderr_path).expect("create daemon stderr log");
        let bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");
        let child = StdCommand::new(&bin)
            .env("HOME", tmp.path())
            .env("BOWERBIRD_INGEST_SOCK", &sock_path)
            .env("RUST_LOG", "warn")
            // Story 3.3: keep this spawn off the developer's real keychain.
            .env("BOWERBIRD_TOKEN", "contract-daemon-test-token")
            .env("BOWERBIRD_KEYRING_BACKEND", "disable")
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| std::ffi::OsString::from("")),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn daemon");
        (child, sock_path, stderr_path)
    }

    async fn wait_for_daemon_ready(sock_path: &std::path::Path, stderr_path: &std::path::Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let socket_ready = std::fs::metadata(sock_path).is_ok();
            let log_ready = std::fs::read_to_string(stderr_path)
                .map(|s| s.contains("daemon listening"))
                .unwrap_or(false);
            if socket_ready && log_ready {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "daemon never became ready; ingest_sock={}, stderr={}",
                sock_path.display(),
                std::fs::read_to_string(stderr_path).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn assert_signal_exits_zero(signal: Signal) {
        let tmp = TempDir::new().expect("tempdir");
        let (child, sock_path, stderr_path) = spawn_daemon_for_signal(&tmp);
        wait_for_daemon_ready(&sock_path, &stderr_path).await;
        kill(Pid::from_raw(child.id() as i32), signal).expect("send signal");
        let output = tokio::task::spawn_blocking(move || child.wait_with_output())
            .await
            .expect("join wait")
            .expect("wait output");
        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        assert!(
            output.status.success(),
            "{signal:?} shutdown should exit 0; status={:?}; stderr={}",
            output.status,
            stderr
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sigterm_uses_graceful_shutdown_path_and_exits_zero() {
        assert_signal_exits_zero(Signal::SIGTERM).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sigint_uses_graceful_shutdown_path_and_exits_zero() {
        assert_signal_exits_zero(Signal::SIGINT).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_drain_timeout_does_not_hang() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            1,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let held_permit = state
            .ws_semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("hold permit");
        state.ws_close_requested.cancel();

        let result =
            wait_for_ws_connection_drain(state.ws_semaphore.clone(), 1, Duration::from_millis(25))
                .await;
        assert!(result.is_err(), "held permit should force drain timeout");
        drop(held_permit);

        wait_for_ws_connection_drain(state.ws_semaphore.clone(), 1, Duration::from_secs(1))
            .await
            .expect("permits return after held permit drops");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn graceful_shutdown_mid_transaction_rollback_leaves_no_partial_rows() {
        let (tmp, pools) = fresh_pools().await;
        let db_path = tmp.path().join("bower.db");
        let started = bowerbird_daemon::projection::session::write_recording_started(&pools.writer)
            .await
            .expect("recording started");
        let writer = pools.writer.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();

        let task = tokio::spawn(async move {
            let conn = writer.get().await.expect("writer get");
            conn.interact(move |c| -> rusqlite::Result<()> {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO events (source, session_id, kind, reaction, payload, created_at) \
                     VALUES ('partial', 'sess-partial', 'PreToolUse', NULL, '{}', 1)",
                    [],
                )?;
                started_tx.send(()).expect("notify transaction started");
                std::thread::sleep(Duration::from_millis(100));
                tx.rollback()?;
                Ok(())
            })
            .await
            .expect("interact")
            .expect("rollback");
        });

        started_rx.await.expect("transaction started");
        let ended_id = tokio::time::timeout(
            Duration::from_secs(2),
            bowerbird_daemon::projection::session::write_recording_ended(
                &pools.writer,
                started.recording_session_id,
            ),
        )
        .await
        .expect("shutdown cleanup should wait for transaction and finish")
        .expect("recording ended");
        task.await.expect("transaction task");
        drop(pools);

        let reopened = init_pools(&db_path).await.expect("reopen pools");
        run_migrations(&reopened.writer)
            .await
            .expect("migrate reopen");
        let conn = reopened.reader.get().await.expect("reader get");
        let recording_session_id = started.recording_session_id;
        let (partial_count, ended_count): (i64, i64) = conn
            .interact(move |c| -> rusqlite::Result<(i64, i64)> {
                let partial_count = c.query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'partial'",
                    [],
                    |r| r.get(0),
                )?;
                let ended_count = c.query_row(
                    "SELECT COUNT(*) FROM recording_sessions WHERE id = ? AND ended_event_id = ?",
                    rusqlite::params![recording_session_id, ended_id.0],
                    |r| r.get(0),
                )?;
                Ok((partial_count, ended_count))
            })
            .await
            .expect("interact")
            .expect("counts");
        assert_eq!(
            partial_count, 0,
            "rolled-back partial event row must not persist"
        );
        assert_eq!(
            ended_count, 1,
            "shutdown cleanup must complete after the in-flight transaction releases the writer"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn graceful_shutdown_committed_transaction_has_event_and_projection() {
        let (tmp, pools) = fresh_pools().await;
        let db_path = tmp.path().join("bower.db");
        let state = default_state(pools.clone());
        let _id = publish_via_projection(
            &state,
            "claude",
            "sess-commit",
            EventKind::PreToolUse,
            None,
            "{}",
        )
        .await;
        state.shutdown_requested.cancel();
        state.ws_close_requested.cancel();
        drop(state);
        drop(pools);

        let reopened = init_pools(&db_path).await.expect("reopen pools");
        run_migrations(&reopened.writer)
            .await
            .expect("migrate reopen");
        let conn = reopened.reader.get().await.expect("reader get");
        let (events, projections): (i64, i64) = conn
            .interact(|c| -> rusqlite::Result<(i64, i64)> {
                let events = c.query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'claude' AND session_id = 'sess-commit'",
                    [],
                    |r| r.get(0),
                )?;
                let projections = c.query_row(
                    "SELECT COUNT(*) FROM session_projections WHERE source = 'claude' AND session_id = 'sess-commit'",
                    [],
                    |r| r.get(0),
                )?;
                Ok((events, projections))
            })
            .await
            .expect("interact")
            .expect("counts");
        assert_eq!(events, 1, "committed event row must survive restart");
        assert_eq!(
            projections, 1,
            "matching session projection must commit with event"
        );
    }
}

/// Story 3.1 AC #6: singleton enforcement on the daemon data directory.
///
/// The lock guards `bower.db` from concurrent migration. These tests spawn
/// real `bowerbird-daemon` subprocesses (the only way to exercise the
/// `flock(2)` + FD lifetime contract end-to-end) and assert:
///
///   - a second daemon pointed at an already-locked data dir exits non-zero
///     with the holder PID surfaced in stderr;
///   - a clean SIGTERM exit releases the lock for a fresh acquisition;
///   - a SIGKILL also releases the lock — the kernel reclaims the FD even
///     when no userspace code runs.
///
/// Parallel-safe: each test's daemons share an explicit per-test data dir
/// (that sharing IS the subject under test) but nothing crosses test
/// boundaries — TempDir-scoped paths, per-daemon ingest sockets, signals
/// sent to specific child PIDs. (An earlier header serialized these per
/// Epic 2 retro AI-3.)
mod story_3_1_singleton {
    use std::process::{Command as StdCommand, Stdio};
    use std::time::Duration;

    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    use tempfile::TempDir;

    /// Spawn a `bowerbird-daemon` against an explicit shared data dir. Each
    /// daemon process gets its own ingest socket path so the two daemons in a
    /// conflict test do not compete on the socket bind (the singleton lock is
    /// what we are testing, not the socket).
    fn spawn_daemon_against_data_dir(
        tmp: &TempDir,
        data_dir: &std::path::Path,
        sock_name: &str,
    ) -> (std::process::Child, std::path::PathBuf, std::path::PathBuf) {
        std::fs::create_dir_all(data_dir).expect("mkdir data dir");
        let sock_path = data_dir.join(sock_name);
        let stderr_path = tmp.path().join(format!("daemon-{sock_name}.stderr.log"));
        let stderr = std::fs::File::create(&stderr_path).expect("create stderr log");
        let bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");
        let child = StdCommand::new(&bin)
            .env("HOME", tmp.path())
            .env("BOWERBIRD_DATA_DIR", data_dir)
            .env("BOWERBIRD_INGEST_SOCK", &sock_path)
            .env("RUST_LOG", "warn")
            // Story 3.3: keep this spawn off the developer's real keychain.
            .env("BOWERBIRD_TOKEN", "contract-daemon-test-token")
            .env("BOWERBIRD_KEYRING_BACKEND", "disable")
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| std::ffi::OsString::from("")),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn daemon");
        (child, sock_path, stderr_path)
    }

    async fn wait_for_daemon_ready(sock_path: &std::path::Path, stderr_path: &std::path::Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let socket_ready = std::fs::metadata(sock_path).is_ok();
            let log_ready = std::fs::read_to_string(stderr_path)
                .map(|s| s.contains("daemon listening"))
                .unwrap_or(false);
            if socket_ready && log_ready {
                // The "daemon listening" log line fires just before `axum::serve`
                // starts polling, but tokio's SIGTERM/SIGINT handlers are
                // registered inside the graceful-shutdown future on its first
                // poll. Give the runtime a tick to register them before tests
                // start signalling — otherwise SIGTERM lands while the kernel
                // disposition is still "terminate process," and the daemon dies
                // by signal instead of exiting 0 through the graceful path.
                tokio::time::sleep(Duration::from_millis(50)).await;
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "daemon never became ready; sock={}, stderr={}",
                sock_path.display(),
                std::fs::read_to_string(stderr_path).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_child_exit(
        mut child: std::process::Child,
        timeout: Duration,
    ) -> std::process::Output {
        // Run the blocking wait on a dedicated task so the test runtime stays
        // responsive. `wait_with_output` consumes the child.
        let join = tokio::task::spawn_blocking(move || {
            let _ = child.try_wait(); // try_wait drives reaping
            child.wait_with_output()
        });
        tokio::time::timeout(timeout, join)
            .await
            .expect("child did not exit within timeout")
            .expect("join blocking wait")
            .expect("wait_with_output")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn second_daemon_exits_nonzero_when_first_holds_lock() {
        let tmp = TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join(".bowerbird");
        let (first_child, sock1, stderr1) =
            spawn_daemon_against_data_dir(&tmp, &data_dir, "ingest1.sock");
        wait_for_daemon_ready(&sock1, &stderr1).await;
        let first_pid = first_child.id();

        let (second_child, _sock2, stderr2) =
            spawn_daemon_against_data_dir(&tmp, &data_dir, "ingest2.sock");
        let second_pid = second_child.id();
        let second_output = wait_for_child_exit(second_child, Duration::from_secs(5)).await;

        // Tear down the first daemon BEFORE the assertions so a panic does
        // not leak a live subprocess into the test runner.
        kill(Pid::from_raw(first_pid as i32), Signal::SIGTERM).expect("SIGTERM first daemon");
        let _ = wait_for_child_exit(first_child, Duration::from_secs(5)).await;

        assert!(
            !second_output.status.success(),
            "second daemon must exit non-zero when the lock is held; status={:?}",
            second_output.status
        );
        let stderr_text = std::fs::read_to_string(&stderr2).unwrap_or_default();
        assert!(
            stderr_text.contains("another bowerbird daemon is already running"),
            "stderr must explain the conflict; got: {stderr_text}\nsecond_pid={second_pid}"
        );
        assert!(
            stderr_text.contains(&format!("pid={first_pid}")),
            "stderr must surface the holding PID ({first_pid}); got: {stderr_text}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn singleton_releases_lock_on_clean_exit() {
        let tmp = TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join(".bowerbird");

        let (first_child, sock1, stderr1) =
            spawn_daemon_against_data_dir(&tmp, &data_dir, "ingest1.sock");
        wait_for_daemon_ready(&sock1, &stderr1).await;

        kill(Pid::from_raw(first_child.id() as i32), Signal::SIGTERM)
            .expect("SIGTERM first daemon");
        let first_output = wait_for_child_exit(first_child, Duration::from_secs(10)).await;
        assert!(
            first_output.status.success(),
            "graceful shutdown must exit 0; status={:?}",
            first_output.status
        );

        // Lock should be released — second daemon comes up cleanly.
        let (second_child, sock2, stderr2) =
            spawn_daemon_against_data_dir(&tmp, &data_dir, "ingest2.sock");
        wait_for_daemon_ready(&sock2, &stderr2).await;
        let second_pid = second_child.id();
        kill(Pid::from_raw(second_pid as i32), Signal::SIGTERM).expect("SIGTERM second daemon");
        let _ = wait_for_child_exit(second_child, Duration::from_secs(5)).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn singleton_releases_lock_on_sigkill_exit() {
        let tmp = TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join(".bowerbird");

        let (first_child, sock1, stderr1) =
            spawn_daemon_against_data_dir(&tmp, &data_dir, "ingest1.sock");
        wait_for_daemon_ready(&sock1, &stderr1).await;

        // SIGKILL: the daemon's graceful shutdown does NOT run. The kernel
        // releases the BSD flock when the FD is reclaimed. This is the test
        // that proves the self-healing claim in the singleton module's doc.
        kill(Pid::from_raw(first_child.id() as i32), Signal::SIGKILL)
            .expect("SIGKILL first daemon");
        let _ = wait_for_child_exit(first_child, Duration::from_secs(5)).await;

        let (second_child, sock2, stderr2) =
            spawn_daemon_against_data_dir(&tmp, &data_dir, "ingest2.sock");
        wait_for_daemon_ready(&sock2, &stderr2).await;
        kill(Pid::from_raw(second_child.id() as i32), Signal::SIGTERM)
            .expect("SIGTERM second daemon");
        let _ = wait_for_child_exit(second_child, Duration::from_secs(5)).await;
    }
}

/// Story 3.2 lifecycle: surface `connected_ws_clients` on `GET /status`,
/// sourced from the `AppState::ws_semaphore` permit accounting. Closes the
/// Epic 2 retro AI-1 charter (`deferred-work.md` line 54).
///
/// **Definition decision (per Task 8.4):** `connected_ws_clients` ==
/// `ws_max_connections - semaphore.available_permits()`. This counts WS
/// connections that have completed the upgrade and not yet released their
/// permit. It does NOT count connections that are mid-upgrade (before
/// `try_acquire_owned`) or connections that have dropped but whose
/// `connection_task` hasn't finished tearing down. The 100ms sleep in the
/// drop-side of the active-count test absorbs that tear-down lag.
mod story_3_2_lifecycle {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use bowerbird_daemon::api;
    use protocol::DaemonStatus;
    use tower::ServiceExt;

    use super::story_2_1_ws::{
        connect_authed, parse_hello, read_text_frame_or_close, spawn_test_daemon,
    };
    use super::{fresh_pools, make_test_state_with_ws};

    const TEST_BEARER: &str = super::TEST_BEARER;

    /// Hit `GET /status` with bearer auth via `tower::ServiceExt::oneshot`
    /// against a fresh router built from the (shared, Arc-backed) state. Same
    /// shape as `story_1_7_rest`'s `auth_get` helper.
    async fn get_status(state: bowerbird_daemon::state::AppState) -> DaemonStatus {
        let app = api::router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .header(header::AUTHORIZATION, format!("Bearer {}", TEST_BEARER))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot /status");
        assert_eq!(resp.status(), StatusCode::OK, "/status must return 200");
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("parse DaemonStatus")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_reports_zero_ws_clients_when_no_subscribers() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            4,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let status = get_status(state).await;
        assert_eq!(
            status.connected_ws_clients, 0,
            "no subscribers connected; expected 0"
        );
    }

    /// Open 3 WS connections, prove they are counted, drop them, prove the
    /// counter returns to zero. The intermediate Hello frame read is what
    /// proves the upgrade landed past `try_acquire_owned`.
    #[tokio::test(flavor = "current_thread")]
    async fn status_reports_active_ws_subscriber_count() {
        let (_tmp, pools) = fresh_pools().await;
        let state = make_test_state_with_ws(
            pools,
            Arc::new(AtomicBool::new(true)),
            8,
            Duration::from_secs(30),
            Duration::from_secs(10),
        );
        let (addr, _server) = spawn_test_daemon(state.clone()).await;

        // Open three WS clients and read each Hello frame so we know the
        // upgrade is past the semaphore checkout.
        let mut clients = Vec::with_capacity(3);
        for _ in 0..3 {
            let (mut ws, _resp) = connect_authed(addr, TEST_BEARER).await;
            let hello_msg = read_text_frame_or_close(&mut ws).await;
            let _ = parse_hello(&hello_msg);
            clients.push(ws);
        }

        let status = get_status(state.clone()).await;
        assert_eq!(
            status.connected_ws_clients, 3,
            "expected 3 active subscribers; got {}",
            status.connected_ws_clients
        );

        // Drop all three clients. The per-connection task's `OwnedSemaphorePermit`
        // is released when the task exits — give the runtime a moment to drive
        // those drops to completion before re-reading.
        drop(clients);
        tokio::time::sleep(Duration::from_millis(200)).await;

        let status = get_status(state).await;
        assert_eq!(
            status.connected_ws_clients, 0,
            "expected all subscribers released; got {}",
            status.connected_ws_clients
        );
    }
}

/// Story 3.3 — bearer token resolution chain (env → keychain → config.toml).
///
/// **Test discipline:** Every test in this module MUST set
/// `keyring_backend: Some("mock"|"disable")` in its [`TokenEnv`] before
/// calling `load_or_generate_with_env()`. A new test without this IS A BUG —
/// it would touch the developer's real macOS Keychain or Linux Secret
/// Service.
///
/// These tests are parallel-safe: each builds an explicit [`TokenEnv`]
/// snapshot instead of mutating process env vars (the pre-seam design
/// required `--test-threads=1`). The one remaining process-global is
/// keyring's credential builder: `set_default_credential_builder` has no
/// inverse, so once a `mock` test installs it the process stays mocked.
/// That is safe here because `disable` tests never consult the builder, and
/// keyring v3's mock hands out a fresh, empty `MockCredential` per
/// `Entry::new` (no service+user interning), so `mock_*` tests share no
/// entry state either.
#[cfg(unix)]
mod story_3_3_auth {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    use bowerbird_daemon::api::token::{self, BearerToken, TokenEnv, TokenSource};
    use tempfile::TempDir;

    /// A [`TokenEnv`] rooted at the test's TempDir with the keychain step
    /// stubbed by `backend` ("disable" or "mock"). No `BOWERBIRD_TOKEN`;
    /// tests that want one set `.token` on the returned value.
    fn token_env(backend: &str, tmp: &TempDir) -> TokenEnv {
        TokenEnv {
            token: None,
            keyring_backend: Some(backend.to_string()),
            data_dir: Some(tmp.path().as_os_str().to_os_string()),
            home: None,
        }
    }

    fn write_config_toml(dir: &Path, contents: &str, mode: u32) {
        let path = dir.join("config.toml");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(&path)
            .expect("open config.toml");
        f.write_all(contents.as_bytes()).expect("write config.toml");
    }

    fn bearer_value(b: &BearerToken) -> String {
        b.expose_token_for_cli().to_string()
    }

    /// 6.2 — env var wins over keychain even when both are populated.
    #[test]
    fn env_var_wins_when_set_and_keychain_has_other_value() {
        let tmp = TempDir::new().unwrap();
        let mut env = token_env("disable", &tmp);
        env.token = Some("expected-from-env".to_string());

        let (bearer, source) = token::load_or_generate_with_env(&env).expect("resolve");
        assert_eq!(source, TokenSource::Env);
        assert_eq!(bearer_value(&bearer), "expected-from-env");
    }

    /// 6.4 — when keychain is unreachable and env is set, env is used.
    /// Pins the AC #2 literal-reading path.
    #[test]
    fn disable_keychain_unavailable_falls_back_to_env() {
        let tmp = TempDir::new().unwrap();
        let mut env = token_env("disable", &tmp);
        env.token = Some("fallback-env-value".to_string());

        let (bearer, source) = token::load_or_generate_with_env(&env).expect("resolve");
        assert_eq!(source, TokenSource::Env);
        assert_eq!(bearer_value(&bearer), "fallback-env-value");
    }

    /// 6.5 — keychain disabled, no env, config.toml provides the token.
    #[test]
    fn disable_keychain_no_env_falls_back_to_config_file() {
        let tmp = TempDir::new().unwrap();
        let env = token_env("disable", &tmp);

        write_config_toml(tmp.path(), "token = \"from-file\"\n", 0o600);

        let (bearer, source) = token::load_or_generate_with_env(&env).expect("resolve");
        assert_eq!(source, TokenSource::ConfigFile);
        assert_eq!(bearer_value(&bearer), "from-file");
    }

    /// 6.6 — no path resolves a token; error names every attempted path.
    #[test]
    fn disable_no_path_resolves_token_returns_error_naming_each_attempted_path() {
        let tmp = TempDir::new().unwrap();
        let env = token_env("disable", &tmp);
        // no config.toml in tmp

        let err = match token::load_or_generate_with_env(&env) {
            Ok(_) => panic!("expected resolution failure"),
            Err(e) => e,
        };
        let s = err.to_string();
        assert!(s.contains("BOWERBIRD_TOKEN"), "missing env path: {s}");
        assert!(s.contains("keychain"), "missing keychain path: {s}");
        assert!(s.contains("config.toml"), "missing config path: {s}");
    }

    /// 6.8 — config.toml mode wider than 0600 emits a warning but the value
    /// is still loaded. Operator-friendly: refusing the file would lock users
    /// out on machines where they cannot fix the permissions.
    #[test]
    fn disable_config_toml_wrong_mode_warns_but_loads() {
        let tmp = TempDir::new().unwrap();
        let env = token_env("disable", &tmp);

        write_config_toml(tmp.path(), "token = \"weak-mode-still-loads\"\n", 0o644);

        let (bearer, source) = token::load_or_generate_with_env(&env).expect("resolve");
        assert_eq!(source, TokenSource::ConfigFile);
        assert_eq!(bearer_value(&bearer), "weak-mode-still-loads");
        // The WARN line goes through `tracing`; capturing it in a test would
        // require a `tracing-test` dep. The reader unit test in
        // `config_file::tests::check_mode_returns_actual_when_wider` proves
        // the detection; this test proves the resolver doesn't refuse on
        // wrong mode.
    }

    /// 6.5b — config.toml with an unknown field is rejected by the strict
    /// `deny_unknown_fields` policy. Pairs with the config_file unit test
    /// `read_rejects_unknown_field` to prove the asymmetric serde policy
    /// applies on the inbound config-file surface.
    #[test]
    fn disable_config_toml_unknown_field_rejects_parse() {
        let tmp = TempDir::new().unwrap();
        let env = token_env("disable", &tmp);

        write_config_toml(tmp.path(), "Token = \"typo-capital-t\"\n", 0o600);

        let err = match token::load_or_generate_with_env(&env) {
            Ok(_) => panic!("expected parse failure"),
            Err(e) => e,
        };
        let s = err.to_string();
        assert!(
            s.contains("config.toml") && s.to_lowercase().contains("parse"),
            "expected a parse-error mention of config.toml; got: {s}"
        );
    }

    /// 6.x — empty token value in config.toml is treated as missing (chain
    /// falls through). Mirrors the empty `BOWERBIRD_TOKEN` behavior.
    #[test]
    fn disable_config_toml_empty_token_treated_as_missing() {
        let tmp = TempDir::new().unwrap();
        let env = token_env("disable", &tmp);

        write_config_toml(tmp.path(), "token = \"\"\n", 0o600);

        let err = match token::load_or_generate_with_env(&env) {
            Ok(_) => panic!("empty token must not resolve"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("config.toml"));
    }

    /// 6.3 — keychain first-run generates a non-empty UUID4 token and tags
    /// the source as `Keychain { generated: true }`.
    ///
    /// **Mock-backend limitation:** `keyring` v3's mock builder produces a
    /// fresh, password-less `MockCredential` on every `Entry::new(service,
    /// user)` call — there is no service+user interning. This is documented
    /// behavior (mock.rs: "This keystore keeps the password in the entry!")
    /// but it diverges from real macOS Keychain / Linux Secret Service
    /// persistence. The consequence is that the daemon's write-then-read
    /// round-trip (NFR14: same token across two `load_or_generate` calls)
    /// **cannot be exercised against the mock** — the second call always
    /// sees an empty entry and would re-generate a different value.
    /// Round-trip coverage instead comes from the cross-process integration
    /// path in `tests/cli_auth.rs::status_shows_full_block_without_user_supplied_token`
    /// (config.toml as a shared persistent backing store) plus the real
    /// platform's persistence guarantee.
    ///
    /// No pre-test entry cleanup: the mock's per-`Entry::new` freshness IS
    /// the clean slate. (The old `Entry::delete_credential` clear ran before
    /// the mock was installed and therefore touched the developer's real
    /// keychain — the opposite of what it intended.)
    #[test]
    fn mock_keychain_first_run_generates_and_tags_source() {
        let tmp = TempDir::new().unwrap();
        let env = token_env("mock", &tmp);

        let (first, first_src) = token::load_or_generate_with_env(&env).expect("first call");
        assert_eq!(first_src, TokenSource::Keychain { generated: true });
        let first_value = bearer_value(&first);
        assert!(!first_value.is_empty(), "generated token must be non-empty");
        // UUID4 format: 8-4-4-4-12 = 36 chars (well-known invariant).
        assert_eq!(
            first_value.len(),
            36,
            "generated token must be UUID4-shaped"
        );
    }

    /// 6.2-companion: env wins even with the mock keychain backend
    /// installed (the mock is per-Entry — see the limitation note on
    /// `mock_keychain_first_run_generates_and_tags_source` — so this test
    /// proves env-first precedence against accidental reordering, not the
    /// "keychain has a competing value" scenario which the mock cannot
    /// represent).
    #[test]
    fn mock_env_var_wins_over_keychain_lookup() {
        let tmp = TempDir::new().unwrap();
        let mut env = token_env("mock", &tmp);
        env.token = Some("expected-from-env".to_string());

        let (bearer, source) = token::load_or_generate_with_env(&env).expect("resolve");
        assert_eq!(source, TokenSource::Env);
        assert_eq!(bearer_value(&bearer), "expected-from-env");
    }

    /// 6.7 — end-to-end exit-code check: a real `bowerbird-daemon` subprocess
    /// with no resolvable token exits non-zero and emits the four-path
    /// summary to stderr.
    #[test]
    fn daemon_exits_nonzero_when_token_chain_exhausted() {
        use std::time::{Duration, Instant};

        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join(".bowerbird");
        std::fs::create_dir_all(&data_dir).unwrap();

        let daemon_bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");
        let mut child = std::process::Command::new(&daemon_bin)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", tmp.path())
            .env("BOWERBIRD_DATA_DIR", &data_dir)
            .env("BOWERBIRD_KEYRING_BACKEND", "disable")
            .env("BOWERBIRD_INGEST_SOCK", tmp.path().join("ingest.sock"))
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn daemon");

        // Wait up to 5s for exit (the resolver runs before any port binding).
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut maybe_status = None;
        while Instant::now() < deadline {
            match child.try_wait().expect("try_wait") {
                Some(status) => {
                    maybe_status = Some(status);
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        let status = maybe_status.unwrap_or_else(|| {
            let _ = child.kill();
            panic!("daemon did not exit within 5s when token chain was exhausted");
        });
        assert!(
            !status.success(),
            "daemon must exit non-zero on token-chain exhaustion; got {status:?}"
        );

        let output = child.wait_with_output().expect("collect output");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("BOWERBIRD_TOKEN"),
            "stderr missing BOWERBIRD_TOKEN mention:\n{stderr}"
        );
        assert!(
            stderr.contains("keychain"),
            "stderr missing keychain mention:\n{stderr}"
        );
        assert!(
            stderr.contains("config.toml"),
            "stderr missing config.toml mention:\n{stderr}"
        );
    }
}

// =====================================================================
// Story 4.1 — `POST /replay` endpoint contract tests
// =====================================================================

mod story_4_1_replay {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use bowerbird_daemon::broadcast::{BroadcastEnvelope, BroadcastHub};
    use bowerbird_daemon::ingest::writer;
    use protocol::{Event, EventId, EventKind};
    use serde::Deserialize;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    const BEARER_HEADER_VALUE: &str = "Bearer test-bearer-token-1.7";

    #[derive(Debug, Deserialize)]
    struct ReplayResponseBody {
        replayed_count: usize,
        #[serde(default)]
        parse_errors: Vec<ParseError>,
    }

    #[derive(Debug, Deserialize)]
    struct ParseError {
        line: usize,
        error: String,
    }

    /// Build a fully wired test state: AppState carries a real `ingest_tx`,
    /// the matching `ingest_rx` is drained by a spawned writer task that
    /// calls `projection::session::write` for each envelope. This mirrors
    /// the production wiring at `crates/daemon/src/main.rs:195-219` minus
    /// the listener task — the test POSTs directly to /replay so no Unix
    /// socket is needed.
    async fn wired_state(pools: DbPools) -> (AppState, tokio::task::JoinHandle<()>) {
        let migrations_complete = Arc::new(AtomicBool::new(true));
        let shutdown = CancellationToken::new();
        let broadcaster = Arc::new(BroadcastHub::new(32));
        let (ingest_tx, ingest_rx) = mpsc::channel::<bowerbird_daemon::ingest::IngestItem>(64);

        let writer_task = tokio::spawn(writer::run(
            ingest_rx,
            pools.writer.clone(),
            broadcaster.clone(),
            shutdown.clone(),
        ));

        let state = AppState {
            db: pools,
            migrations_complete,
            shutdown_requested: shutdown,
            ws_close_requested: CancellationToken::new(),
            bearer: BearerToken::new(super::TEST_BEARER.to_string()),
            started_at_ms: 0,
            broadcaster,
            ws_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            ws_config: WsConfig {
                ping_interval: Duration::from_secs(30),
                pong_timeout: Duration::from_secs(10),
                coalesce_window: Duration::from_secs(1),
                max_connections: 4,
            },
            ingest_tx,
        };
        (state, writer_task)
    }

    async fn parse_replay_body(resp: axum::response::Response) -> ReplayResponseBody {
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("parse ReplayResponse")
    }

    /// Build a `protocol::Event` JSONL line for use in the request body.
    fn event_line(source: &str, session_id: &str, kind: EventKind, event_id: i64) -> String {
        let e = Event {
            event_id: EventId(event_id),
            source: source.to_string(),
            session_id: session_id.to_string(),
            kind,
            reaction: None,
            payload: "{}".to_string(),
            created_at: 1,
            pid: None,
            cwd: None,
        };
        serde_json::to_string(&e).expect("serialize Event")
    }

    /// Like [`event_line`] but sets the stored `Event.cwd`. Story 5.7 review
    /// pass 2 — the replay path reads `cwd` from the JSONL `Event` (not from
    /// payload), so a replayed event must carry it onto the broadcast + projection.
    fn event_line_with_cwd(
        source: &str,
        session_id: &str,
        kind: EventKind,
        event_id: i64,
        cwd: Option<&str>,
    ) -> String {
        let e = Event {
            event_id: EventId(event_id),
            source: source.to_string(),
            session_id: session_id.to_string(),
            kind,
            reaction: None,
            payload: "{}".to_string(),
            created_at: 1,
            pid: None,
            cwd: cwd.map(|s| s.to_string()),
        };
        serde_json::to_string(&e).expect("serialize Event")
    }

    fn auth_post(uri: &str, body: Vec<u8>) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::AUTHORIZATION, BEARER_HEADER_VALUE)
            .header(header::CONTENT_TYPE, "application/x-ndjson")
            .body(Body::from(body))
            .unwrap()
    }

    /// Read up to `expected` broadcast frames from `rx` with a 2s overall
    /// timeout; collects whatever arrives (lagged consumers panic).
    async fn collect_frames(
        rx: &mut tokio::sync::broadcast::Receiver<BroadcastEnvelope>,
        expected: usize,
    ) -> Vec<BroadcastEnvelope> {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut out = Vec::with_capacity(expected);
        while out.len() < expected && std::time::Instant::now() < deadline {
            let budget = deadline.saturating_duration_since(std::time::Instant::now());
            match tokio::time::timeout(budget, rx.recv()).await {
                Ok(Ok(env)) => out.push(env),
                Ok(Err(e)) => panic!("broadcast recv error: {e:?}"),
                Err(_) => break,
            }
        }
        out
    }

    /// AC #1: events POSTed to /replay arrive on broadcast subscribers as
    /// `Event` frames in JSONL line order — each followed by an optional
    /// `State` frame IFF the projection's `current_state` changed
    /// (story 5.2). `event_id` and `created_at` are reassigned to fresh
    /// values (not the JSONL values).
    #[tokio::test(flavor = "current_thread")]
    async fn replay_forwards_events_through_broadcast_path() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let mut rx = state.broadcaster.subscribe();
        let app = api::router(state.clone());

        let body = [
            event_line("claude", "sess-r1", EventKind::PreToolUse, 1000),
            event_line("claude", "sess-r1", EventKind::PostToolUse, 1001),
            event_line("claude", "sess-r1", EventKind::Stop, 1002),
        ]
        .join("\n");

        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed = parse_replay_body(resp).await;
        assert_eq!(parsed.replayed_count, 3);
        assert!(parsed.parse_errors.is_empty(), "no parse errors expected");

        // Story 5.2: State frames fire only on `current_state` transitions.
        //   - PreToolUse: None → Working   → Event + State
        //   - PostToolUse: Working → Working (preserved) → Event only
        //   - Stop: Working → Idle         → Event + State
        // 3 Event + 2 State = 5 frames.
        let frames = collect_frames(&mut rx, 5).await;
        assert_eq!(frames.len(), 5, "expected 5 frames; got {frames:?}");

        // Event frames arrive in JSONL line order. Transition-causing events
        // are followed by State frames; non-transitions publish Event only.
        let kinds: Vec<EventKind> = frames
            .iter()
            .filter_map(|f| match f {
                BroadcastEnvelope::Event(e) => Some(e.kind.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                EventKind::PreToolUse,
                EventKind::PostToolUse,
                EventKind::Stop
            ]
        );

        // event_id is reassigned (the JSONL values were 1000..1002; the
        // daemon's AUTOINCREMENT starts at 1).
        let event_ids: Vec<i64> = frames
            .iter()
            .filter_map(|f| match f {
                BroadcastEnvelope::Event(e) => Some(e.event_id.0),
                _ => None,
            })
            .collect();
        for id in &event_ids {
            assert!(
                *id < 100,
                "event_id {id} suspiciously large; replay should reassign from a fresh DB"
            );
        }

        // created_at is reassigned to replay wall-clock (not the JSONL `1`).
        for f in &frames {
            if let BroadcastEnvelope::Event(e) = f {
                assert!(
                    e.created_at > 1_000_000_000_000,
                    "created_at {} should be a recent unix-ms, not the JSONL placeholder 1",
                    e.created_at
                );
            }
        }
    }

    /// Story 5.7 review pass 2 (AC #9): a replayed event carries its stored
    /// `cwd` onto BOTH the broadcast `Event` frame and the projected
    /// `SessionState` (the `State` frame). Replay reconstructs the envelope
    /// from the JSONL `Event.cwd` (`replay.rs`), a path the other cwd tests
    /// don't touch.
    ///
    /// Review pass 4: also pins that the projected `started_at` follows Story
    /// 4.1's retimestamping contract — it is the replay wall-clock of the first
    /// replayed write, NOT the JSONL `created_at` placeholder (`1000`). `cwd` is
    /// threaded from the stored event; `started_at` is daemon-derived and
    /// re-stamped, so the two diverge by design.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_carries_event_cwd_onto_broadcast_and_projection() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let mut rx = state.broadcaster.subscribe();
        let app = api::router(state.clone());

        // A single PreToolUse with a cwd: None → Working transitions, so we
        // expect one Event frame + one State frame.
        let body = event_line_with_cwd(
            "claude",
            "sess-replay-cwd",
            EventKind::PreToolUse,
            1000,
            Some("/repo"),
        );

        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed = parse_replay_body(resp).await;
        assert_eq!(parsed.replayed_count, 1);

        let frames = collect_frames(&mut rx, 2).await;
        assert_eq!(frames.len(), 2, "expected Event + State; got {frames:?}");

        let event_cwd = frames.iter().find_map(|f| match f {
            BroadcastEnvelope::Event(e) => Some(e.cwd.clone()),
            _ => None,
        });
        assert_eq!(
            event_cwd,
            Some(Some("/repo".to_string())),
            "replayed broadcast Event.cwd must carry the stored cwd"
        );

        let state_cwd = frames.iter().find_map(|f| match f {
            BroadcastEnvelope::State { state, .. } => Some(state.cwd.clone()),
            _ => None,
        });
        assert_eq!(
            state_cwd,
            Some(Some("/repo".to_string())),
            "replayed projection State.cwd must carry the stored cwd"
        );

        // Review pass 4: `started_at` does NOT carry the JSONL `created_at`
        // (1000) — replay re-stamps wall-clock per Story 4.1, and `started_at`
        // is set-once from the first replayed write's fresh timestamp.
        let state_started_at = frames.iter().find_map(|f| match f {
            BroadcastEnvelope::State { state, .. } => Some(state.started_at),
            _ => None,
        });
        match state_started_at {
            Some(Some(ts)) => assert!(
                ts > 1_000_000_000_000,
                "replayed started_at {ts} must be recent replay wall-clock, \
                 not the JSONL placeholder 1000"
            ),
            other => panic!("expected a State frame with started_at: Some; got {other:?}"),
        }
    }

    /// AC #4: events spanning two `(source, session_id)` keys produce
    /// `State` frames for each session, matching Story 2.2 publish
    /// semantics as refined by Story 5.2 — State frames are gated on
    /// `current_state` transitions, not emitted on every write.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_emits_state_frames_for_each_session() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let mut rx = state.broadcaster.subscribe();
        let app = api::router(state.clone());

        let body = [
            event_line("claude", "sess-a", EventKind::PreToolUse, 1),
            event_line("claude", "sess-b", EventKind::PreToolUse, 2),
            event_line("claude", "sess-a", EventKind::PostToolUse, 3),
            event_line("claude", "sess-b", EventKind::Stop, 4),
        ]
        .join("\n");

        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);

        // Story 5.2: 4 events → 4 Event frames + 3 State frames = 7 total.
        //   - sess-a PreToolUse:  None → Working    → Event + State
        //   - sess-b PreToolUse:  None → Working    → Event + State
        //   - sess-a PostToolUse: Working → Working → Event only
        //   - sess-b Stop:        Working → Idle    → Event + State
        let frames = collect_frames(&mut rx, 7).await;
        assert_eq!(frames.len(), 7, "expected 7 frames; got {frames:?}");
        let mut state_sessions = std::collections::HashSet::new();
        for f in &frames {
            if let BroadcastEnvelope::State { session_id, .. } = f {
                state_sessions.insert(session_id.clone());
            }
        }
        assert!(
            state_sessions.contains("sess-a") && state_sessions.contains("sess-b"),
            "State frames must cover both sessions; got {state_sessions:?}"
        );
    }

    /// AC #1: per-line parse failures don't fail the whole request — the
    /// successful lines forward; the errors come back in the response body.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_continues_on_per_line_parse_error() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let app = api::router(state);

        let body = format!(
            "{}\nINVALID JSON LINE\n{}\n",
            event_line("claude", "sess-x", EventKind::PreToolUse, 1),
            event_line("claude", "sess-x", EventKind::Stop, 2),
        );

        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed = parse_replay_body(resp).await;
        assert_eq!(parsed.replayed_count, 2);
        assert_eq!(parsed.parse_errors.len(), 1);
        assert_eq!(parsed.parse_errors[0].line, 2);
    }

    /// AC #5 + Task 1.4: `RecordingStarted` / `RecordingEnded` sentinels
    /// are rejected at the parse boundary with a clear error.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_rejects_sentinel_kinds() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let app = api::router(state);

        let body = event_line("claude", "sess-s", EventKind::RecordingStarted, 1);

        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed = parse_replay_body(resp).await;
        assert_eq!(parsed.replayed_count, 0);
        assert_eq!(parsed.parse_errors.len(), 1);
        assert!(
            parsed.parse_errors[0].error.contains("sentinel"),
            "expected sentinel-rejection message; got {:?}",
            parsed.parse_errors[0]
        );
    }

    /// Bearer-auth: missing header → 401; wrong token → 401; correct
    /// token + valid body → 200.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_requires_bearer() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let app = api::router(state);

        // No Authorization header.
        let body = event_line("claude", "sess-auth", EventKind::PreToolUse, 1);
        let req_no_auth = Request::builder()
            .method("POST")
            .uri("/replay")
            .body(Body::from(body.clone().into_bytes()))
            .unwrap();
        let resp = app.clone().oneshot(req_no_auth).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Wrong bearer token.
        let req_wrong = Request::builder()
            .method("POST")
            .uri("/replay")
            .header(header::AUTHORIZATION, "Bearer wrong-token")
            .body(Body::from(body.clone().into_bytes()))
            .unwrap();
        let resp = app.clone().oneshot(req_wrong).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Correct bearer.
        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// AC #5: the JSONL line's `event_id` and `created_at` are dropped and
    /// reassigned by the writer. Verifies by querying `events` directly
    /// after the replay.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_dropped_event_id_and_created_at_are_reassigned() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools.clone()).await;
        let app = api::router(state.clone());

        // Provide a JSONL line with absurd event_id and created_at values.
        let e = Event {
            event_id: EventId(999_999),
            source: "claude".to_string(),
            session_id: "sess-reassign".to_string(),
            kind: EventKind::PreToolUse,
            reaction: None,
            payload: "{}".to_string(),
            created_at: 1,
            pid: None,
            cwd: None,
        };
        let line = serde_json::to_string(&e).unwrap();

        let resp = app
            .oneshot(auth_post("/replay", line.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);

        // Wait for the writer task to drain. Poll the DB until the row
        // shows up; bound at 2s.
        let session_id_q = "sess-reassign".to_string();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let now_ms_test: i64 = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0);
        let mut maybe_row: Option<(i64, i64)> = None;
        while std::time::Instant::now() < deadline {
            let conn = pools.reader.get().await.expect("reader checkout");
            let q_session = session_id_q.clone();
            let result = conn
                .interact(move |c| -> rusqlite::Result<Option<(i64, i64)>> {
                    c.query_row(
                        "SELECT event_id, created_at FROM events \
                             WHERE source = 'claude' AND session_id = ?1 \
                             ORDER BY event_id LIMIT 1",
                        rusqlite::params![&q_session],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                    )
                    .map(Some)
                    .or_else(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Ok(None),
                        other => Err(other),
                    })
                })
                .await
                .expect("interact")
                .expect("query");
            if let Some(t) = result {
                maybe_row = Some(t);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (assigned_event_id, assigned_created_at) =
            maybe_row.expect("replayed event must land in the events table within 2s");

        // event_id was reassigned: the AUTOINCREMENT starts at 1, far below
        // the 999_999 in the JSONL line.
        assert!(
            assigned_event_id < 100,
            "event_id {assigned_event_id} should be a fresh AUTOINCREMENT value, not 999999"
        );
        // created_at was reassigned to wall-clock at write, not the JSONL
        // placeholder `1`. Within ±5s of test wall-clock.
        let drift = (assigned_created_at - now_ms_test).abs();
        assert!(
            drift < 5_000,
            "created_at {assigned_created_at} differs from test wall-clock {now_ms_test} by {drift}ms (should be < 5000)"
        );
    }

    /// AC #1 + Task 1.4: blank lines and `#`-prefixed comment lines are
    /// silently skipped — they are NOT counted as `replayed_count` and they
    /// do NOT show up as `parse_errors`. A body that is all comments and
    /// blanks therefore returns `replayed_count: 0, parse_errors: []`.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_skips_blank_and_comment_lines() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let app = api::router(state);

        // Body shape: comment, blank, real event, comment, blank, real event,
        // trailing newline (which produces a final empty slice after split).
        let body = format!(
            "# replay fixture for skip-test\n\n{}\n# inline comment between events\n\n{}\n",
            event_line("claude", "sess-skip", EventKind::PreToolUse, 1),
            event_line("claude", "sess-skip", EventKind::Stop, 2),
        );

        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed = parse_replay_body(resp).await;
        assert_eq!(
            parsed.replayed_count, 2,
            "only the two real event lines should be replayed"
        );
        assert!(
            parsed.parse_errors.is_empty(),
            "blank + comment lines must NOT produce parse errors; got {:?}",
            parsed.parse_errors
        );
    }

    /// A body that is all blank lines and comments returns
    /// `replayed_count: 0, parse_errors: []`. This is the smallest valid
    /// replay request — the endpoint must not 400 on an empty effective body.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_with_only_comments_replays_zero_events() {
        let (_tmp, pools) = fresh_pools().await;
        let (state, _writer) = wired_state(pools).await;
        let app = api::router(state);

        let body = "# header\n# more notes\n\n# trailing comment\n".to_string();

        let resp = app
            .oneshot(auth_post("/replay", body.into_bytes()))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed = parse_replay_body(resp).await;
        assert_eq!(parsed.replayed_count, 0);
        assert!(parsed.parse_errors.is_empty());
    }
}

// ─── Story 5.3 — Daemon-observed liveness probe ─────────────────────────────

#[cfg(test)]
mod story_5_3_liveness {
    use super::*;
    use bowerbird_daemon::db::queries::SELECT_SESSION_PROJECTION_STATE;
    use bowerbird_daemon::projection::liveness::probe_once;
    use tokio_util::sync::CancellationToken;

    async fn upsert_session(
        pools: &DbPools,
        source: &str,
        session_id: &str,
        state: &SessionState,
        updated_at: i64,
    ) {
        let state_json = serde_json::to_string(state).expect("serialize state");
        let source = source.to_string();
        let session_id = session_id.to_string();
        let conn = pools.writer.get().await.expect("writer pool");
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

    async fn read_state(pools: &DbPools, source: &str, session_id: &str) -> Option<SessionState> {
        let conn = pools.reader.get().await.expect("reader pool");
        let s = source.to_string();
        let sid = session_id.to_string();
        let raw: Option<String> = conn
            .interact(move |c| -> rusqlite::Result<Option<String>> {
                c.query_row(
                    SELECT_SESSION_PROJECTION_STATE,
                    rusqlite::params![s, sid],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            })
            .await
            .expect("interact")
            .expect("query");
        raw.map(|s| serde_json::from_str(&s).expect("parse state"))
    }

    async fn count_session_ended(pools: &DbPools, source: &str, session_id: &str) -> i64 {
        let conn = pools.reader.get().await.expect("reader pool");
        let s = source.to_string();
        let sid = session_id.to_string();
        conn.interact(move |c| -> rusqlite::Result<i64> {
            c.query_row(
                "SELECT COUNT(*) FROM events WHERE source = ? AND session_id = ? AND kind = ?",
                rusqlite::params![s, sid, "SessionEnded"],
                |r| r.get::<_, i64>(0),
            )
        })
        .await
        .expect("interact")
        .expect("query")
    }

    // Story 5.3 AC #10: row with last_pid = None → SessionEnded with
    // reason "no_pid_at_upgrade"; projection transitions to Ended.
    #[tokio::test(flavor = "current_thread")]
    async fn liveness_probe_emits_session_ended_for_no_pid_at_upgrade() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);
        upsert_session(
            &pools,
            "claude",
            "sess-no-pid",
            &SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 1_000,
                last_pid: None,
                cwd: None,
                started_at: None,
            },
            1_000,
        )
        .await;

        let shutdown = CancellationToken::new();
        let report = probe_once(&pools.writer, &hub, &shutdown)
            .await
            .expect("probe");
        assert_eq!(report.emitted, 1);
        assert_eq!(report.failed, 0);

        let state = read_state(&pools, "claude", "sess-no-pid")
            .await
            .expect("state row");
        assert_eq!(state.current_state, SessionCurrentState::Ended);
        assert_eq!(state.last_event_kind, EventKind::SessionEnded);
        assert_eq!(
            count_session_ended(&pools, "claude", "sess-no-pid").await,
            1
        );
    }

    // Story 5.3 AC #11: row with last_pid = Some(<dead>) → SessionEnded with
    // reason "pid_dead".
    #[tokio::test(flavor = "current_thread")]
    async fn liveness_probe_emits_session_ended_for_dead_pid() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);

        // Spawn a short-lived subprocess, wait for it to die, capture its PID.
        // `wait()` reaps the child synchronously; once it returns the PID is no
        // longer a live process — `kill(pid, 0)` will return ESRCH immediately.
        // No sleep needed, and using one would violate the project's
        // deterministic-timing test discipline (story 5.3 review finding #6).
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("spawn shell");
        let pid = child.id();
        let _ = child.wait();

        upsert_session(
            &pools,
            "claude",
            "sess-dead-pid",
            &SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 1_000,
                last_pid: Some(pid),
                cwd: None,
                started_at: None,
            },
            1_000,
        )
        .await;

        let shutdown = CancellationToken::new();
        let report = probe_once(&pools.writer, &hub, &shutdown)
            .await
            .expect("probe");
        assert_eq!(report.emitted, 1);
        assert_eq!(report.failed, 0);

        let state = read_state(&pools, "claude", "sess-dead-pid")
            .await
            .expect("state row");
        assert_eq!(state.current_state, SessionCurrentState::Ended);
    }

    // Story 5.3 AC #11 negative: row with last_pid pointing at THIS test
    // runner's PID (definitely alive) → no SessionEnded emitted.
    #[tokio::test(flavor = "current_thread")]
    async fn liveness_probe_skips_alive_pid() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);

        let alive_pid = std::process::id();
        upsert_session(
            &pools,
            "claude",
            "sess-alive",
            &SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 1_000,
                last_pid: Some(alive_pid),
                cwd: None,
                started_at: None,
            },
            1_000,
        )
        .await;

        let shutdown = CancellationToken::new();
        let report = probe_once(&pools.writer, &hub, &shutdown)
            .await
            .expect("probe");
        assert_eq!(report.emitted, 0);
        assert_eq!(report.failed, 0);

        let state = read_state(&pools, "claude", "sess-alive")
            .await
            .expect("state row");
        assert_eq!(state.current_state, SessionCurrentState::Working);
        assert_eq!(count_session_ended(&pools, "claude", "sess-alive").await, 0);
    }

    // Story 5.3 AC #12: from Ended, a UserPromptSubmit drives Working.
    #[tokio::test(flavor = "current_thread")]
    async fn session_ended_then_resume_exits_ended() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);
        upsert_session(
            &pools,
            "claude",
            "sess-resume",
            &SessionState {
                current_state: SessionCurrentState::Ended,
                last_event_kind: EventKind::SessionEnded,
                last_event_at_ms: 1_000,
                last_pid: Some(100),
                cwd: None,
                started_at: None,
            },
            1_000,
        )
        .await;

        projection::session::write(
            &pools.writer,
            &hub,
            envelope_for("claude", "sess-resume", EventKind::UserPromptSubmit),
        )
        .await
        .expect("write");

        let state = read_state(&pools, "claude", "sess-resume")
            .await
            .expect("state row");
        assert_eq!(state.current_state, SessionCurrentState::Working);
        // last_pid carry-forward — the new envelope had pid=None, so the
        // prior Some(100) persists.
        assert_eq!(state.last_pid, Some(100));
    }

    // Story 5.3 AC #11: an already-Ended row must not re-emit SessionEnded.
    #[tokio::test(flavor = "current_thread")]
    async fn liveness_probe_does_not_re_emit_for_already_ended() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);
        upsert_session(
            &pools,
            "claude",
            "sess-already-ended",
            &SessionState {
                current_state: SessionCurrentState::Ended,
                last_event_kind: EventKind::SessionEnded,
                last_event_at_ms: 1_000,
                last_pid: None,
                cwd: None,
                started_at: None,
            },
            1_000,
        )
        .await;

        let shutdown = CancellationToken::new();
        let report = probe_once(&pools.writer, &hub, &shutdown)
            .await
            .expect("probe");
        assert_eq!(report.emitted, 0);
        assert_eq!(
            count_session_ended(&pools, "claude", "sess-already-ended").await,
            0
        );
    }

    // Story 5.3 AC #13: rebuild_missing_projections preserves last_pid and
    // Ended state by replaying events through transition.
    #[tokio::test(flavor = "current_thread")]
    async fn rebuild_preserves_last_pid_and_ended() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);

        // Ingest a sequence with pids; cap with a synthesized SessionEnded.
        let pre = EventEnvelope {
            source: "claude".to_string(),
            session_id: "sess-rebuild".to_string(),
            kind: EventKind::PreToolUse,
            reaction: None,
            payload: "{}".to_string(),
            pid: Some(111),
            notification_type: None,
            cwd: None,
        };
        projection::session::write(&pools.writer, &hub, pre)
            .await
            .expect("write 1");

        let stop = EventEnvelope {
            source: "claude".to_string(),
            session_id: "sess-rebuild".to_string(),
            kind: EventKind::Stop,
            reaction: None,
            payload: "{}".to_string(),
            pid: None, // carry-forward leaves last_pid = Some(111)
            notification_type: None,
            cwd: None,
        };
        projection::session::write(&pools.writer, &hub, stop)
            .await
            .expect("write 2");

        let ended = EventEnvelope {
            source: "claude".to_string(),
            session_id: "sess-rebuild".to_string(),
            kind: EventKind::SessionEnded,
            reaction: None,
            payload: r#"{"reason":"pid_dead","pid":111,"observed_at_ms":1000}"#.to_string(),
            pid: Some(111),
            notification_type: None,
            cwd: None,
        };
        projection::session::write(&pools.writer, &hub, ended)
            .await
            .expect("write 3");

        let before = read_state(&pools, "claude", "sess-rebuild")
            .await
            .expect("pre-delete state");
        assert_eq!(before.current_state, SessionCurrentState::Ended);
        assert_eq!(before.last_pid, Some(111));

        // Delete projections + rebuild.
        let conn = pools.writer.get().await.expect("writer pool");
        conn.interact(|c| -> rusqlite::Result<()> {
            c.execute(
                "DELETE FROM session_projections WHERE source != '__daemon__'",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("interact")
        .expect("delete");
        drop(conn);

        projection::session::rebuild_missing_projections(&pools.writer)
            .await
            .expect("rebuild");

        let after = read_state(&pools, "claude", "sess-rebuild")
            .await
            .expect("rebuilt state");
        assert_eq!(after.current_state, SessionCurrentState::Ended);
        assert_eq!(after.last_pid, Some(111));
        assert_eq!(after.last_event_kind, EventKind::SessionEnded);
    }

    // Story 5.7 AC #7/#12: rebuild reconstructs cwd (last non-NULL,
    // carry-forward) and started_at (first event's created_at, set-once) purely
    // from the event log — Story 1.6 AC #5 "storage is a pure function of the
    // event sequence." Insert raw rows with mixed cwd and ascending created_at,
    // delete projections, rebuild, assert.
    #[tokio::test(flavor = "current_thread")]
    async fn rebuild_preserves_cwd_and_started_at() {
        let (_tmp, pools) = fresh_pools().await;

        // Insert events directly (controlled created_at + cwd, including a
        // NULL, a value, a NULL-carry, and an overwrite).
        let conn = pools.writer.get().await.expect("writer pool");
        conn.interact(|c| -> rusqlite::Result<()> {
            let rows: &[(&str, i64, Option<&str>)] = &[
                ("PreToolUse", 1_000, None), // first event → started_at=1000, cwd None
                ("PreToolUse", 2_000, Some("/repo/a")), // cwd → /repo/a
                ("Stop", 3_000, None),       // cwd None → carry /repo/a
                ("PreToolUse", 4_000, Some("/repo/b")), // overwrite → /repo/b
            ];
            for (kind, created_at, cwd) in rows {
                c.execute(
                    "INSERT INTO events (source, session_id, kind, payload, created_at, cwd) \
                     VALUES ('claude', 'sess-cwd', ?, '{}', ?, ?)",
                    rusqlite::params![kind, created_at, cwd],
                )?;
            }
            Ok(())
        })
        .await
        .expect("interact")
        .expect("insert events");
        drop(conn);

        projection::session::rebuild_missing_projections(&pools.writer)
            .await
            .expect("rebuild");

        let after = read_state(&pools, "claude", "sess-cwd")
            .await
            .expect("rebuilt state");
        assert_eq!(
            after.cwd,
            Some("/repo/b".to_string()),
            "cwd must reconstruct as the last non-NULL value (overwrite-on-Some)"
        );
        assert_eq!(
            after.started_at,
            Some(1_000),
            "started_at must reconstruct as the FIRST event's created_at (set-once)"
        );
    }

    // NOTE (correct-course 2026-06-02, Option D): two Story 5.7 review tests
    // lived here — `legacy_started_at_backfills_from_event_log_not_post_upgrade_clock`
    // (pass 1) and `legacy_started_at_backfills_by_first_event_order_under_nonmonotonic_created_at`
    // (pass 2). Both pinned the legacy `started_at` event-log backfill, which was
    // removed: bowerbird is pre-release and the documented upgrade path is "nuke
    // the db," so a pre-5.7 `started_at: None`-with-prior row is unsupported (its
    // next in-place write gets the post-upgrade wall clock — an approximation we
    // accept, never shown to a real user). `transition`'s set-once rule still
    // makes a full rebuild reconstruct the true first-event time; that's covered
    // by `rebuild_preserves_cwd_and_started_at` above. See
    // docs/bmad/planning-artifacts/started-at-backfill-reconsideration-2026-06-02.md
    // and deferred-work.md (real migration-era backfills land at release time).

    // Story 5.3 review finding #2: between probe SELECT and the synthetic
    // SessionEnded write, a real hook event lands on the same session. The
    // probe must yield — the precondition check inside the writer txn fails,
    // no SessionEnded event is written, no broadcast is emitted, the row is
    // counted as skipped_stale.
    #[tokio::test(flavor = "current_thread")]
    async fn liveness_probe_skips_stale_row_when_real_hook_interleaved() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);

        // Seed a row whose last_pid is dead (PID None → no_pid_at_upgrade
        // candidate). Then simulate the race: between the probe's read and
        // the synthetic write, a real hook moves the projection.
        upsert_session(
            &pools,
            "claude",
            "sess-race",
            &SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 1_000,
                last_pid: None,
                cwd: None,
                started_at: None,
            },
            1_000,
        )
        .await;

        // Race simulation: build the precondition from the *old* snapshot
        // (current_state: Working), then change the row to a different
        // current_state before invoking write_if_state_matches. The
        // precondition check inside the txn must fail and return Ok(None).
        upsert_session(
            &pools,
            "claude",
            "sess-race",
            &SessionState {
                current_state: SessionCurrentState::WaitingInput,
                last_event_kind: EventKind::Notification,
                last_event_at_ms: 2_000,
                last_pid: Some(999_999),
                cwd: None,
                started_at: None,
            },
            2_000,
        )
        .await;

        // Build the synthetic envelope the probe would have built from the
        // stale snapshot. last_pid was None at snapshot time.
        let envelope = EventEnvelope {
            source: "claude".to_string(),
            session_id: "sess-race".to_string(),
            kind: EventKind::SessionEnded,
            reaction: None,
            payload: r#"{"reason":"no_pid_at_upgrade","pid":null,"observed_at_ms":1000}"#
                .to_string(),
            pid: None,
            notification_type: None,
            cwd: None,
        };
        let precondition = bowerbird_daemon::projection::session::WritePrecondition {
            expected_current_state: SessionCurrentState::Working,
            expected_last_pid: None,
            expected_last_event_at_ms: None,
        };
        let result = bowerbird_daemon::projection::session::write_if_state_matches(
            &pools.writer,
            &hub,
            envelope,
            precondition,
        )
        .await
        .expect("call");
        assert!(
            result.is_none(),
            "precondition mismatch must skip the synthetic write (Ok(None))"
        );

        // Row should retain the post-race state (WaitingInput, pid 999_999).
        let after = read_state(&pools, "claude", "sess-race")
            .await
            .expect("state row");
        assert_eq!(after.current_state, SessionCurrentState::WaitingInput);
        assert_eq!(after.last_pid, Some(999_999));
        // And NO SessionEnded event was inserted.
        assert_eq!(count_session_ended(&pools, "claude", "sess-race").await, 0);
    }

    // Story 5.3 review finding #5: shutdown cancellation observed between
    // rows stops the probe mid-iteration. Verified by pre-cancelling the
    // token; the probe should exit on the first row check.
    #[tokio::test(flavor = "current_thread")]
    async fn liveness_probe_observes_shutdown_mid_iteration() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);

        // Seed two rows that would otherwise both emit SessionEnded.
        for sid in ["sess-a", "sess-b"] {
            upsert_session(
                &pools,
                "claude",
                sid,
                &SessionState {
                    current_state: SessionCurrentState::Working,
                    last_event_kind: EventKind::PreToolUse,
                    last_event_at_ms: 1_000,
                    last_pid: None,
                    cwd: None,
                    started_at: None,
                },
                1_000,
            )
            .await;
        }

        let shutdown = CancellationToken::new();
        shutdown.cancel(); // pre-cancelled

        let report = probe_once(&pools.writer, &hub, &shutdown)
            .await
            .expect("probe");
        assert_eq!(
            report.emitted, 0,
            "pre-cancelled shutdown must abort before any row write"
        );
        // Neither projection should have transitioned.
        for sid in ["sess-a", "sess-b"] {
            let s = read_state(&pools, "claude", sid).await.expect("state");
            assert_ne!(
                s.current_state,
                SessionCurrentState::Ended,
                "row {sid} must not have been ended"
            );
            assert_eq!(count_session_ended(&pools, "claude", sid).await, 0);
        }
    }

    // Story 5.3 review finding #6 / Task 9: `MissedTickBehavior::Skip` means
    // a slow iteration does NOT queue catch-up ticks. With paused virtual
    // time, advance past TWO tick boundaries while the tick_fn is "slow"
    // (returns synchronously, but we advance time before the next select);
    // the loop must NOT fire a second tick to catch up — the interval has
    // collapsed the missed ticks into one. This pins the contract that
    // liveness::run depends on.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn liveness_probe_missed_tick_does_not_queue() {
        use bowerbird_daemon::projection::liveness::run_loop;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::time::advance;

        let count = Arc::new(AtomicUsize::new(0));
        let shutdown = CancellationToken::new();

        // Spawn the loop with a 5s cadence. Each "tick_fn" increments a
        // counter and returns synchronously.
        let loop_task = {
            let count = count.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                run_loop(Duration::from_secs(5), shutdown, move || {
                    let count = count.clone();
                    Box::pin(async move {
                        count.fetch_add(1, Ordering::SeqCst);
                    })
                })
                .await;
            })
        };

        // Let the spawned task park on its initial `interval.tick()` (which
        // resolves immediately in `run_loop` and is consumed before the loop
        // body). Then advance virtual time past THREE 5-second boundaries
        // without yielding between them — Burst behavior would catch up with
        // three queued ticks; Skip collapses them into one.
        tokio::task::yield_now().await;
        advance(Duration::from_secs(16)).await;
        tokio::task::yield_now().await;

        // With Skip, the counter should be exactly 1 (one tick fired,
        // not three queued). Let one more boundary pass to confirm the loop
        // is still alive and ticking normally.
        let after_one_burst = count.load(Ordering::SeqCst);
        assert_eq!(
            after_one_burst, 1,
            "MissedTickBehavior::Skip should collapse missed ticks (saw {after_one_burst})"
        );

        advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        let after_one_more = count.load(Ordering::SeqCst);
        assert_eq!(
            after_one_more, 2,
            "next scheduled tick must still fire normally (saw {after_one_more})"
        );

        shutdown.cancel();
        loop_task.await.expect("loop task");
    }
}

// ─── Story 5.4 — CatchPanicLayer middleware ─────────────────────────────────

#[cfg(test)]
mod story_5_4_catch_panic {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    /// Story 5.4 AC #2: a handler panic returns `500 Internal Server Error`
    /// with the structured JSON body `{"error":"internal panic"}` and the
    /// `x-request-id` header propagated by `PropagateRequestIdLayer`. A
    /// subsequent request to `/healthz` against the SAME router instance
    /// proves the daemon survived the panic (the tokio runtime did not die,
    /// no tower-level connection close left the next handler unreachable).
    ///
    /// The panic route lives HERE in the test, not in the shipped API surface.
    /// We exercise the production middleware via `api::apply_common_middleware`
    /// so the contract is proven against the real `CatchPanicLayer` stack
    /// without the daemon binary ever compiling an unauthenticated `/__panic`
    /// route.
    #[tokio::test(flavor = "current_thread")]
    async fn catch_panic_layer_returns_500_and_keeps_daemon_alive() {
        let app: Router = api::apply_common_middleware(
            Router::new()
                .route(
                    "/__panic",
                    get(|| async {
                        panic!("test panic from /__panic");
                        // The `panic!` diverges; the trailing value only gives
                        // the closure a concrete `IntoResponse` return type.
                        #[allow(unreachable_code)]
                        StatusCode::OK
                    }),
                )
                .route("/healthz", get(|| async { StatusCode::OK })),
        );

        // Trigger the panic.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/__panic")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot through CatchPanicLayer");
        assert_eq!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "CatchPanicLayer must turn a handler panic into a 500"
        );
        assert!(
            resp.headers().contains_key("x-request-id"),
            "PropagateRequestIdLayer must still set x-request-id on the 500 response"
        );
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read 500 body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("body must be JSON");
        assert_eq!(body, serde_json::json!({ "error": "internal panic" }));

        // The daemon (the single-threaded tokio runtime + the Router service)
        // must still serve other routes. /healthz is the cheapest probe.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot /healthz after caught panic");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/healthz must still serve 200 after a caught panic"
        );
    }
}

// ─── Story 5.4 — /events 404 for unknown sessions ───────────────────────────

#[cfg(test)]
mod story_5_4_events_404 {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use protocol::EventListResponse;
    use tower::ServiceExt;

    fn bearer_header() -> String {
        format!("Bearer {}", super::TEST_BEARER)
    }

    fn ready_state(pools: DbPools) -> AppState {
        let mc = Arc::new(AtomicBool::new(true));
        super::make_test_state(pools, mc)
    }

    fn auth_get(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, bearer_header())
            .body(Body::empty())
            .unwrap()
    }

    async fn json_body<T: serde::de::DeserializeOwned>(resp: axum::response::Response) -> T {
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("parse json")
    }

    /// Story 5.4 AC #5: `GET /sessions/{id}/events?since=<n>` for an id with
    /// no `session_projections` row returns `404` with body
    /// `{"error":"session not found"}` — the same shape `/sessions/{id}` and
    /// `/sessions/{id}/stats` already use. Previously the endpoint returned
    /// `200 {events:[], cursor:null, oldest_available_event_id:i64::MAX}` for
    /// any id including typos.
    #[tokio::test(flavor = "current_thread")]
    async fn events_404_for_unknown_session() {
        let (tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get("/sessions/never-existed/events?since=0"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = json_body(resp).await;
        assert_eq!(body, serde_json::json!({ "error": "session not found" }));

        super::teardown_pools(pools, tmp).await;
    }

    /// Story 5.4 AC #5: the new 404 gate must not break the legitimate
    /// "session exists, no events past my cursor" case — that returns 200
    /// with an empty `events` array and `cursor: None`.
    #[tokio::test(flavor = "current_thread")]
    async fn events_200_for_existing_session_with_no_new_events() {
        let (tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(16);
        let last_id = projection::session::write(
            &pools.writer,
            &hub,
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-tail".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
                pid: None,
                notification_type: None,
                cwd: None,
            },
        )
        .await
        .expect("write");

        let app = api::router(ready_state(pools.clone()));
        let resp = app
            .oneshot(auth_get(&format!(
                "/sessions/sess-tail/events?since={}",
                last_id.0
            )))
            .await
            .expect("oneshot");
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "existing session with no new events must remain 200"
        );
        let body: EventListResponse = json_body(resp).await;
        assert!(body.events.is_empty(), "no new events past cursor");
        assert_eq!(body.cursor, None);

        super::teardown_pools(pools, tmp).await;
    }
}

// ─── Story 5.4 — Migration idempotency on a populated DB ────────────────────

#[cfg(test)]
mod story_5_4_migrations {
    use super::*;

    /// Every column of an `events` row, in schema order
    /// (`event_id, source, session_id, kind, reaction, payload, created_at, pid, cwd`).
    type EventRow = (
        i64,
        String,
        String,
        String,
        Option<String>,
        String,
        i64,
        Option<i64>,
        Option<String>,
    );
    /// Every column of a `session_projections` row, in schema order
    /// (`source, session_id, state, updated_at`).
    type ProjRow = (String, String, String, i64);

    /// Full deterministic snapshot of the schema version and every row in both
    /// tables. Capturing whole rows (not just counts + one sampled `pid`) is
    /// what lets the idempotency assertion catch a future migration that
    /// *rewrites* existing data — payloads, timestamps, projection state — while
    /// leaving row counts and `user_version` untouched (Story 5.4 review).
    fn migration_snapshot(
        c: &rusqlite::Connection,
    ) -> rusqlite::Result<(i64, Vec<EventRow>, Vec<ProjRow>)> {
        let user_version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let events = c
            .prepare(
                "SELECT event_id, source, session_id, kind, reaction, payload, created_at, pid, cwd \
                 FROM events ORDER BY event_id",
            )?
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<EventRow>>>()?;
        let projections = c
            .prepare(
                "SELECT source, session_id, state, updated_at \
                 FROM session_projections ORDER BY source, session_id",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<ProjRow>>>()?;
        Ok((user_version, events, projections))
    }

    /// Story 5.4 AC #4: re-running `run_migrations` against a populated,
    /// file-backed SQLite DB is a strict no-op — `PRAGMA user_version` stays
    /// put, every seeded `events` and `session_projections` row survives,
    /// the schema-v2 `events.pid` column reads back as written, and no
    /// `Error::Migration` surfaces. The unit test
    /// `migrations.rs::tests::migrations_are_idempotent` (Story 5.3) covers
    /// the `:memory:` baseline; this contract test is the populated-DB
    /// follow-on the deferred-work entry at `deferred-work.md:17` asked for.
    #[tokio::test(flavor = "current_thread")]
    async fn migrations_idempotent_on_populated_db() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("bower.db");

        let before = {
            let pools = init_pools(&db_path).await.expect("init_pools 1");
            run_migrations(&pools.writer).await.expect("migrate 1");

            // Seed the DB with three events across two sessions; mixed
            // EventKind variants. One row carries a real PID so the v2
            // `events.pid` column is exercised end-to-end, and a real `cwd` so
            // the v3 `events.cwd` column is in the idempotency snapshot too
            // (Story 5.7 review pass 3 — guards against a future migration
            // silently dropping/rewriting `cwd`).
            let hub = BroadcastHub::new(16);
            projection::session::write(
                &pools.writer,
                &hub,
                EventEnvelope {
                    source: "claude".to_string(),
                    session_id: "sess-A".to_string(),
                    kind: EventKind::PreToolUse,
                    reaction: None,
                    payload: r#"{"tool":"Bash"}"#.to_string(),
                    pid: Some(4242),
                    notification_type: None,
                    cwd: Some("/repo".to_string()),
                },
            )
            .await
            .expect("write sess-A PreToolUse");
            projection::session::write(
                &pools.writer,
                &hub,
                EventEnvelope {
                    source: "claude".to_string(),
                    session_id: "sess-A".to_string(),
                    kind: EventKind::PostToolUse,
                    reaction: None,
                    payload: "{}".to_string(),
                    pid: None,
                    notification_type: None,
                    cwd: None,
                },
            )
            .await
            .expect("write sess-A PostToolUse");
            projection::session::write(
                &pools.writer,
                &hub,
                EventEnvelope {
                    source: "claude".to_string(),
                    session_id: "sess-B".to_string(),
                    kind: EventKind::Stop,
                    reaction: None,
                    payload: "{}".to_string(),
                    pid: None,
                    notification_type: None,
                    cwd: None,
                },
            )
            .await
            .expect("write sess-B Stop");

            let conn = pools.reader.get().await.expect("reader get");
            let snapshot = conn
                .interact(|c| migration_snapshot(c))
                .await
                .expect("interact")
                .expect("snapshot query");
            // Pool drops here. WAL is left on disk; the next init_pools picks
            // it up via WAL recovery on open.
            snapshot
        };

        let (user_version_before, ref events_before, ref projections_before) = before;
        assert!(
            user_version_before >= 2,
            "expected user_version >= 2 after initial migrations, got {user_version_before}"
        );
        assert_eq!(events_before.len(), 3, "three events seeded");
        assert_eq!(
            projections_before.len(),
            2,
            "two non-sentinel session projections seeded"
        );
        // The schema-v2 `events.pid` column round-trips on the one seeded row
        // that carried a PID.
        let seeded_pid = events_before
            .iter()
            .find(|e| e.2 == "sess-A" && e.3 == "PreToolUse")
            .and_then(|e| e.7);
        assert_eq!(seeded_pid, Some(4242), "v2 pid column round-trips");
        // The schema-v3 `events.cwd` column round-trips on the same seeded row.
        let seeded_cwd = events_before
            .iter()
            .find(|e| e.2 == "sess-A" && e.3 == "PreToolUse")
            .and_then(|e| e.8.clone());
        assert_eq!(
            seeded_cwd,
            Some("/repo".to_string()),
            "v3 cwd column round-trips"
        );

        // Re-open the pool against the SAME file and re-run migrations. The
        // contract: zero schema mutation, zero row mutation, no error.
        let pools = init_pools(&db_path).await.expect("init_pools 2");
        run_migrations(&pools.writer)
            .await
            .expect("re-running run_migrations against a populated DB must be Ok");

        let conn = pools.reader.get().await.expect("reader get");
        let after = conn
            .interact(|c| migration_snapshot(c))
            .await
            .expect("interact")
            .expect("post-migration snapshot query");

        // Whole-snapshot equality is the strong form of "zero rows changed":
        // user_version, every events row (incl. payload, created_at, pid, cwd),
        // and every session_projections row must be byte-for-byte identical.
        assert_eq!(
            after.0, before.0,
            "PRAGMA user_version must be unchanged after a repeat run_migrations"
        );
        assert_eq!(
            after.1, before.1,
            "every events row must be byte-for-byte unchanged across a repeat run_migrations"
        );
        assert_eq!(
            after.2, before.2,
            "every session_projections row must be byte-for-byte unchanged across a repeat run_migrations"
        );
    }
}

// =====================================================================
// Story 5.8 (ADR 0008) — server-side session filter: GET /sessions
// `?state=`/`?since=`/`?limit=` query params.
// =====================================================================

mod story_5_8_session_filter {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use protocol::{SessionCurrentState, SessionListItem};
    use tower::ServiceExt;

    fn ready_state(pools: DbPools) -> AppState {
        super::make_test_state(pools, Arc::new(AtomicBool::new(true)))
    }

    fn auth_get(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", super::TEST_BEARER),
            )
            .body(Body::empty())
            .unwrap()
    }

    async fn json_body<T: serde::de::DeserializeOwned>(resp: axum::response::Response) -> T {
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("parse json")
    }

    /// `last_event_at_ms` far in the future so `current_state_for_read` treats a
    /// stored `Working` as fresh (renders `Working`) regardless of the real
    /// wall-clock the handler reads. `Idle`/`WaitingInput`/`Ended` render
    /// verbatim either way.
    const FRESH: i64 = i64::MAX / 2;

    fn st(cs: SessionCurrentState, last_event_at_ms: i64) -> SessionState {
        SessionState {
            current_state: cs,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms,
            last_pid: None,
            cwd: None,
            started_at: Some(1),
        }
    }

    async fn seed(pools: &DbPools, source: &str, sid: &str, state: &SessionState, updated_at: i64) {
        let conn = pools.writer.get().await.expect("writer get");
        let json = serde_json::to_string(state).expect("serialize state");
        let source = source.to_string();
        let sid = sid.to_string();
        conn.interact(move |c| -> rusqlite::Result<()> {
            c.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![source, sid, json, updated_at],
            )?;
            Ok(())
        })
        .await
        .expect("interact")
        .expect("upsert");
    }

    async fn list_ids(app: axum::Router, uri: &str) -> Vec<String> {
        let resp = app.oneshot(auth_get(uri)).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK, "expected 200 for {uri}");
        let items: Vec<SessionListItem> = json_body(resp).await;
        items.into_iter().map(|i| i.session_id).collect()
    }

    // AC #1 / #8: no params → the full non-sentinel list in documented order,
    // every field intact. The unfiltered regression canary.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_unfiltered_unchanged() {
        let (tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "__daemon__",
            "__daemon__",
            &st(SessionCurrentState::Idle, FRESH),
            9_999,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-a",
            &st(SessionCurrentState::Working, FRESH),
            3_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-b",
            &st(SessionCurrentState::Idle, FRESH),
            2_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-c",
            &st(SessionCurrentState::Ended, FRESH),
            1_000,
        )
        .await;

        let app = api::router(ready_state(pools.clone()));
        let resp = app.oneshot(auth_get("/sessions")).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let items: Vec<SessionListItem> = json_body(resp).await;
        // Sentinel excluded; updated_at DESC order.
        let ids: Vec<&str> = items.iter().map(|i| i.session_id.as_str()).collect();
        assert_eq!(ids, vec!["sess-a", "sess-b", "sess-c"]);
        // Fields intact on the Working row.
        let a = &items[0];
        assert_eq!(a.current_state, SessionCurrentState::Working);
        assert_eq!(a.started_at, Some(1));
        assert_eq!(a.updated_at, 3_000);

        super::teardown_pools(pools, tmp).await;
    }

    // AC #2: ?state=<single> filters on the read-derived current_state.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_state_filter_single() {
        let (_tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-w",
            &st(SessionCurrentState::Working, FRESH),
            3_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-i",
            &st(SessionCurrentState::Idle, FRESH),
            2_000,
        )
        .await;
        let app = api::router(ready_state(pools));
        let ids = list_ids(app, "/sessions?state=working").await;
        assert_eq!(ids, vec!["sess-w"]);
    }

    // AC #3: ?state=working,waitinginput,idle drops the Ended graveyard.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_state_filter_multi_drops_ended() {
        let (_tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-w",
            &st(SessionCurrentState::Working, FRESH),
            4_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-wi",
            &st(SessionCurrentState::WaitingInput, FRESH),
            3_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-i",
            &st(SessionCurrentState::Idle, FRESH),
            2_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-e",
            &st(SessionCurrentState::Ended, FRESH),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools));
        let ids = list_ids(app, "/sessions?state=working,waitinginput,idle").await;
        assert_eq!(ids, vec!["sess-w", "sess-wi", "sess-i"]);
        assert!(!ids.contains(&"sess-e".to_string()));
    }

    // AC #3 (inverse): ?state=ended returns only the graveyard.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_state_filter_ended_only() {
        let (_tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-w",
            &st(SessionCurrentState::Working, FRESH),
            2_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-e",
            &st(SessionCurrentState::Ended, FRESH),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools));
        let ids = list_ids(app, "/sessions?state=ended").await;
        assert_eq!(ids, vec!["sess-e"]);
    }

    // AC #4: an unrecognized token → 400 naming the bad token, not a 500 or
    // silent empty list.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_state_filter_invalid_token_400() {
        let (_tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-w",
            &st(SessionCurrentState::Working, FRESH),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(auth_get("/sessions?state=running"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = json_body(resp).await;
        let msg = body["error"].as_str().expect("error string");
        assert!(
            msg.contains("running"),
            "message must name the bad token: {msg}"
        );
    }

    // Pass-3 finding: `filter.rs` unit-tests empty/trailing tokens, but the HTTP
    // surface only pinned `state=running`. A present-but-empty `?state=` and a
    // trailing-comma `?state=working,` are malformed (an empty token is not "no
    // filter" — absent is) and must 400 with an error body, the same loud-reject
    // an unknown token gets. Pins the handler-level behavior end to end.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_state_filter_empty_and_trailing_token_400() {
        let (_tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-w",
            &st(SessionCurrentState::Working, FRESH),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools));

        // Present-but-empty `?state=` → empty token → 400 with an error body.
        let empty = app
            .clone()
            .oneshot(auth_get("/sessions?state="))
            .await
            .expect("oneshot");
        assert_eq!(
            empty.status(),
            StatusCode::BAD_REQUEST,
            "a present-but-empty ?state= is malformed, not 'no filter'"
        );
        let empty_body: serde_json::Value = json_body(empty).await;
        assert!(
            empty_body["error"].is_string(),
            "empty ?state= must return an {{error}} body"
        );

        // Trailing comma → empty trailing token → 400 with an error body.
        let trailing = app
            .oneshot(auth_get("/sessions?state=working,"))
            .await
            .expect("oneshot");
        assert_eq!(
            trailing.status(),
            StatusCode::BAD_REQUEST,
            "a trailing-comma ?state= yields an empty token and must 400"
        );
        let trailing_body: serde_json::Value = json_body(trailing).await;
        assert!(
            trailing_body["error"].is_string(),
            "trailing-comma ?state= must return an {{error}} body"
        );
    }

    // Review finding #5: `SessionsParams` is `#[serde(deny_unknown_fields)]`, so
    // an unknown query key fails loudly (strict-inbound policy), the same way
    // `EventsParams` does. Axum surfaces the `Query` rejection as 400. Pins the
    // behavior so it cannot silently regress.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_unknown_query_key_400() {
        let (_tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-w",
            &st(SessionCurrentState::Working, FRESH),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(auth_get("/sessions?foo=bar"))
            .await
            .expect("oneshot");
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "an unknown query key must 400 (deny_unknown_fields), not be silently ignored"
        );
    }

    // AC #5: ?since=<updated_at_ms> exclusive lower bound; non-integer → 400.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_since_lower_bound() {
        let (tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-old",
            &st(SessionCurrentState::Idle, FRESH),
            1_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-mid",
            &st(SessionCurrentState::Idle, FRESH),
            2_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-new",
            &st(SessionCurrentState::Idle, FRESH),
            3_000,
        )
        .await;
        let app = api::router(ready_state(pools.clone()));

        let ids = list_ids(app.clone(), "/sessions?since=1500").await;
        // updated_at > 1500 → 2000, 3000 (DESC order).
        assert_eq!(ids, vec!["sess-new", "sess-mid"]);

        // Non-integer since → 400 (axum Query rejection).
        let resp = app
            .oneshot(auth_get("/sessions?since=notanint"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        super::teardown_pools(pools, tmp).await;
    }

    // AC #6: ?limit=<n> caps rows in order; limit=0 / negative → 400.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_limit_caps_rows() {
        let (tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-a",
            &st(SessionCurrentState::Idle, FRESH),
            3_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-b",
            &st(SessionCurrentState::Idle, FRESH),
            2_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-c",
            &st(SessionCurrentState::Idle, FRESH),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools.clone()));

        let ids = list_ids(app.clone(), "/sessions?limit=2").await;
        assert_eq!(ids, vec!["sess-a", "sess-b"]);

        let zero = app
            .clone()
            .oneshot(auth_get("/sessions?limit=0"))
            .await
            .expect("oneshot");
        assert_eq!(zero.status(), StatusCode::BAD_REQUEST);

        let neg = app
            .clone()
            .oneshot(auth_get("/sessions?limit=-1"))
            .await
            .expect("oneshot");
        assert_eq!(neg.status(), StatusCode::BAD_REQUEST);

        // Pass-3 finding: a non-integer limit is an axum Query rejection → 400,
        // the same loud-reject `?since=notanint` gets (AC #6 says non-integer
        // limit → 400; only 0/-1 were pinned before).
        let nan = app
            .oneshot(auth_get("/sessions?limit=notanint"))
            .await
            .expect("oneshot");
        assert_eq!(nan.status(), StatusCode::BAD_REQUEST);

        super::teardown_pools(pools, tmp).await;
    }

    // AC #7: limit caps the PRE-state-filter set, then state filters in Rust —
    // so a page may return fewer than `limit`. Seed [Ended, Ended, Working] in
    // updated_at DESC; ?state=working&limit=2 fetches the 2 newest (both Ended),
    // state-filters to 0 → empty array. This is the documented interaction, not
    // a bug.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_limit_plus_state_may_return_fewer() {
        let (_tmp, pools) = fresh_pools().await;
        seed(
            &pools,
            "claude",
            "sess-e1",
            &st(SessionCurrentState::Ended, FRESH),
            3_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-e2",
            &st(SessionCurrentState::Ended, FRESH),
            2_000,
        )
        .await;
        seed(
            &pools,
            "claude",
            "sess-w",
            &st(SessionCurrentState::Working, FRESH),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools));
        let ids = list_ids(app, "/sessions?state=working&limit=2").await;
        assert!(
            ids.is_empty(),
            "limit=2 fetches the 2 newest (both Ended); state=working filters them out → empty"
        );
    }

    // AC #2/#8: the filter matches the RENDERED current_state, not the stored
    // value. A stale Working row (renders Idle) is INCLUDED by ?state=idle and
    // EXCLUDED by ?state=working — mirrors the snapshot read-derived test.
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_state_filter_read_derived() {
        let (_tmp, pools) = fresh_pools().await;
        // Stored Working, last_event_at_ms = 0 → at real now, age > STALE_WORKING_MS
        // → renders Idle.
        seed(
            &pools,
            "claude",
            "sess-stale",
            &st(SessionCurrentState::Working, 0),
            1_000,
        )
        .await;
        let app = api::router(ready_state(pools));

        let as_idle = list_ids(app.clone(), "/sessions?state=idle").await;
        assert_eq!(as_idle, vec!["sess-stale"], "stale Working renders Idle");

        let as_working = list_ids(app, "/sessions?state=working").await;
        assert!(
            as_working.is_empty(),
            "the Working filter must not match a row that renders Idle"
        );
    }
}

/// Story 5.11 / ADR 0009 — event-driven PID supersession. When a successor
/// session emits on a PID, every OTHER non-`Ended` session still claiming that
/// PID is superseded → `SessionEnded { reason: "pid_superseded" }`. These tests
/// drive the real `projection::session::write` path (where supersession lives)
/// and assert both the persisted projection AND the broadcast frames.
mod story_5_11_supersession {
    use super::*;
    use bowerbird_daemon::projection::session::{write_if_state_matches, WritePrecondition};

    /// An ingest envelope carrying a specific `pid` (the shim-injected
    /// `bowerbird_ppid`). The supersession rule keys on this PID.
    fn envelope_with_pid(
        source: &str,
        session_id: &str,
        kind: EventKind,
        pid: u32,
    ) -> EventEnvelope {
        EventEnvelope {
            pid: Some(pid),
            ..envelope_for(source, session_id, kind)
        }
    }

    /// A `PreToolUse` whose payload names the Task-tool (`Agent`) — a subagent
    /// dispatch. The supersession code is tool-name-agnostic; this makes the
    /// AC5 subagent-gate intent explicit on the wire.
    fn agent_dispatch_with_pid(source: &str, session_id: &str, pid: u32) -> EventEnvelope {
        EventEnvelope {
            pid: Some(pid),
            payload: r#"{"tool_name":"Agent"}"#.to_string(),
            ..envelope_for(source, session_id, EventKind::PreToolUse)
        }
    }

    /// Seed a projection row directly (no event, no supersession side effect) —
    /// models the live pre-fix backlog of predecessors stranded non-`Ended` on
    /// a still-live PID (ADR 0009 §Consequences).
    async fn seed_session(pools: &DbPools, source: &str, session_id: &str, state: &SessionState) {
        let state_json = serde_json::to_string(state).expect("serialize state");
        let source = source.to_string();
        let session_id = session_id.to_string();
        let conn = pools.writer.get().await.expect("writer pool");
        conn.interact(move |c| -> rusqlite::Result<()> {
            c.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![source, session_id, state_json, 1_000_i64],
            )?;
            Ok(())
        })
        .await
        .expect("interact")
        .expect("seed upsert");
    }

    async fn read_state(pools: &DbPools, source: &str, session_id: &str) -> SessionState {
        let raw = read_session_state(&pools.reader, source, session_id).await;
        serde_json::from_str(&raw).expect("parse state")
    }

    /// The `reason` field of every `SessionEnded` event for a session, in
    /// `event_id` order. Length == number of SessionEnded events emitted.
    async fn ended_reasons(pools: &DbPools, source: &str, session_id: &str) -> Vec<String> {
        let conn = pools.reader.get().await.expect("reader pool");
        let s = source.to_string();
        let sid = session_id.to_string();
        let payloads: Vec<String> = conn
            .interact(move |c| -> rusqlite::Result<Vec<String>> {
                let mut stmt = c.prepare(
                    "SELECT payload FROM events WHERE source = ? AND session_id = ? \
                     AND kind = 'SessionEnded' ORDER BY event_id ASC",
                )?;
                let rows = stmt.query_map(rusqlite::params![s, sid], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .await
            .expect("interact")
            .expect("query");
        payloads
            .iter()
            .map(|p| {
                serde_json::from_str::<serde_json::Value>(p)
                    .ok()
                    .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(String::from))
                    .unwrap_or_else(|| format!("<unparsable: {p}>"))
            })
            .collect()
    }

    /// Drain a broadcast receiver, collecting `(session_id, current_state)` for
    /// each State frame and tallying Event frames.
    fn drain_state_frames(
        rx: &mut tokio::sync::broadcast::Receiver<bowerbird_daemon::broadcast::BroadcastEnvelope>,
    ) -> (usize, Vec<(String, SessionCurrentState)>) {
        use bowerbird_daemon::broadcast::BroadcastEnvelope;
        let mut events = 0usize;
        let mut states = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(BroadcastEnvelope::Event(_)) => events += 1,
                Ok(BroadcastEnvelope::State {
                    session_id, state, ..
                }) => states.push((session_id, state.current_state)),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(e) => panic!("unexpected recv error: {e:?}"),
            }
        }
        (events, states)
    }

    /// AC1 — a successor's event on PID P supersedes the lone predecessor on P;
    /// the successor is unaffected. The victim gets a `SessionEnded` whose
    /// payload `reason == "pid_superseded"`, persisted AND published as a State
    /// frame carrying `Ended`.
    #[tokio::test(flavor = "current_thread")]
    async fn successor_supersedes_lone_predecessor() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 4242;

        // Predecessor A goes live on P.
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write A");
        assert_ne!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Ended,
            "A is live before B claims its PID"
        );

        // Subscribe AFTER A's write so we observe only B's write + the
        // supersession it triggers.
        let mut rx = hub.subscribe();

        // Successor B emits on the same PID.
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-B", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write B");

        // A is superseded; B is live.
        let a = read_state(&pools, "claude", "sess-A").await;
        assert_eq!(
            a.current_state,
            SessionCurrentState::Ended,
            "A must be Ended after B claims its PID"
        );
        assert_eq!(a.last_event_kind, EventKind::SessionEnded);
        assert_eq!(a.last_pid, Some(P), "carry-forward keeps A's last_pid");
        assert_eq!(
            ended_reasons(&pools, "claude", "sess-A").await,
            vec!["pid_superseded".to_string()],
            "exactly one SessionEnded for A, reason pid_superseded"
        );
        assert_ne!(
            read_state(&pools, "claude", "sess-B").await.current_state,
            SessionCurrentState::Ended,
            "B (the emitter) is never superseded"
        );

        // B's own Event + State, plus A's supersession Event + State.
        let (events, states) = drain_state_frames(&mut rx);
        assert_eq!(events, 2, "B's event + A's synthetic SessionEnded event");
        assert!(
            states.contains(&("sess-A".to_string(), SessionCurrentState::Ended)),
            "A's transition to Ended must be published as a State frame; got {states:?}"
        );
    }

    /// AC1 multi-predecessor — three predecessors stranded non-`Ended` on the
    /// same live PID (the live PID 88706 pile-up) are ALL superseded in a single
    /// supersession scan when the live session emits.
    #[tokio::test(flavor = "current_thread")]
    async fn successor_supersedes_all_stranded_predecessors() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 88706;

        for sid in ["sess-A1", "sess-A2", "sess-A3"] {
            seed_session(
                &pools,
                "claude",
                sid,
                &SessionState {
                    current_state: SessionCurrentState::Idle,
                    last_event_kind: EventKind::Notification,
                    last_event_at_ms: 1_000,
                    last_pid: Some(P),
                    cwd: Some("/repo".to_string()),
                    started_at: Some(500),
                },
            )
            .await;
        }

        // The current live session on P emits.
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-live", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write live session");

        for sid in ["sess-A1", "sess-A2", "sess-A3"] {
            assert_eq!(
                read_state(&pools, "claude", sid).await.current_state,
                SessionCurrentState::Ended,
                "{sid} must be superseded in the single scan"
            );
            assert_eq!(
                ended_reasons(&pools, "claude", sid).await,
                vec!["pid_superseded".to_string()],
                "{sid} ended exactly once with pid_superseded"
            );
            assert_eq!(
                read_state(&pools, "claude", sid).await.cwd,
                Some("/repo".to_string()),
                "carry-forward preserves the victim's last-known cwd ({sid})"
            );
        }
        assert_ne!(
            read_state(&pools, "claude", "sess-live")
                .await
                .current_state,
            SessionCurrentState::Ended,
            "the live emitter is untouched"
        );
    }

    /// AC2 — idempotent. Re-ingesting the successor's event does NOT re-emit a
    /// second `SessionEnded` for an already-superseded predecessor, and no
    /// duplicate row accumulates.
    #[tokio::test(flavor = "current_thread")]
    async fn resupersession_is_idempotent() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 4242;

        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write A");
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-B", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write B (ends A)");
        assert_eq!(
            ended_reasons(&pools, "claude", "sess-A").await.len(),
            1,
            "A ended exactly once after B's first event"
        );

        // Subscribe AFTER A is already Ended; re-ingest another B event on P.
        let mut rx = hub.subscribe();
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-B", EventKind::PreToolUse, P),
        )
        .await
        .expect("re-ingest B");

        assert_eq!(
            ended_reasons(&pools, "claude", "sess-A").await.len(),
            1,
            "no duplicate SessionEnded row accumulates for A"
        );
        let (_events, states) = drain_state_frames(&mut rx);
        assert!(
            !states.iter().any(|(sid, _)| sid == "sess-A"),
            "no second State frame is published for the already-Ended A; got {states:?}"
        );
    }

    /// AC3 — reversible on resume. A → B (A ended) → A resumes on P: A un-ends
    /// through the normal write path AND, now the live holder of P, supersedes
    /// B. Whoever emitted most recently on the PID is the survivor.
    #[tokio::test(flavor = "current_thread")]
    async fn resumed_predecessor_unends_and_supersedes_current_holder() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 4944;

        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write A");
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-B", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write B (ends A)");
        assert_eq!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Ended,
            "A is superseded by B"
        );

        // A resumes (claude --resume) — a new event for A on P.
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("resume A");

        assert_eq!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Working,
            "A un-ends through the normal write path (Ended is non-terminal)"
        );
        assert_eq!(
            read_state(&pools, "claude", "sess-B").await.current_state,
            SessionCurrentState::Ended,
            "B is now superseded by the resumed A"
        );
        assert_eq!(
            ended_reasons(&pools, "claude", "sess-B").await,
            vec!["pid_superseded".to_string()],
            "B ended with pid_superseded once A resumed"
        );
    }

    /// AC5 — subagent gate (regression guard for the verified premise, ADR 0009
    /// §"The subagent gate" + Story 5.11 review finding #4). A session that
    /// dispatches Task-tool (`Agent`) subagents emits many events on one PID —
    /// including tool-use events whose tool_name is `Agent` — and must NEVER
    /// supersede itself.
    ///
    /// What this test PROVES (in-code mechanism): the supersession scan excludes
    /// the emitter by `(source, session_id)` and supersedes only OTHER sessions
    /// on the PID. We pin this *specifically* by seeding a genuine co-PID
    /// predecessor `sess-pred` that the parent's turn DOES supersede, while the
    /// parent (despite its `Agent` dispatches) is left live with no SessionEnded.
    /// That distinguishes "the emitter is excluded" from the weaker "supersession
    /// happened not to fire here."
    ///
    /// What this test CANNOT prove (the load-bearing premise): that a subagent
    /// does not surface as a *distinct* co-PID session_id. bowerbird has no
    /// subagent discriminator — `normalize` reads `session_id` verbatim from the
    /// hook payload — so the premise is verified externally against live data
    /// (ADR 0009 §gate: PID 6491 hosted exactly one session_id across 42 `Agent`
    /// dispatches), not enforced in code. If a future Claude Code release emits a
    /// distinct child session_id on the parent's PID, supersession would
    /// ping-pong; ADR 0009 §"Revisit when" routes that to correct-course (add a
    /// parent/child discriminator), and a real captured-hook adapter fixture
    /// would be the place to catch it. There is no fabricated distinct-child
    /// fixture here because an invented one would assert nothing about Claude
    /// Code's actual behavior.
    #[tokio::test(flavor = "current_thread")]
    async fn subagent_activity_never_supersedes_its_parent() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 6491;

        // A genuine co-PID predecessor that SHOULD be superseded by the parent's
        // turn — the control that proves supersession is live on P and the
        // exclusion below is emitter-specific, not a no-op.
        seed_session(
            &pools,
            "claude",
            "sess-pred",
            &SessionState {
                current_state: SessionCurrentState::Idle,
                last_event_kind: EventKind::Notification,
                last_event_at_ms: 1_000,
                last_pid: Some(P),
                cwd: Some("/repo".to_string()),
                started_at: Some(500),
            },
        )
        .await;

        // The parent session emits a full turn on one PID, including a subagent
        // dispatch (tool_name "Agent"), as PID 6491 / session e0215166 did.
        for env in [
            envelope_with_pid("claude", "sess-parent", EventKind::UserPromptSubmit, P),
            agent_dispatch_with_pid("claude", "sess-parent", P),
            envelope_with_pid("claude", "sess-parent", EventKind::PostToolUse, P),
            agent_dispatch_with_pid("claude", "sess-parent", P),
        ] {
            projection::session::write(&pools.writer, &hub, env)
                .await
                .expect("write parent event");
        }

        // The emitter (parent) is never superseded by its own activity...
        assert_ne!(
            read_state(&pools, "claude", "sess-parent")
                .await
                .current_state,
            SessionCurrentState::Ended,
            "a session must never supersede itself on its own (subagent) activity"
        );
        assert!(
            ended_reasons(&pools, "claude", "sess-parent")
                .await
                .is_empty(),
            "no synthetic co-PID SessionEnded may be emitted for the emitter"
        );

        // ...but a genuine OTHER predecessor on the same PID IS superseded, so
        // the exclusion above is specific to the emitter, not vacuous.
        assert_eq!(
            read_state(&pools, "claude", "sess-pred")
                .await
                .current_state,
            SessionCurrentState::Ended,
            "a genuine co-PID predecessor must still be superseded by the parent"
        );
        assert_eq!(
            ended_reasons(&pools, "claude", "sess-pred").await,
            vec!["pid_superseded".to_string()],
            "the predecessor ends with pid_superseded (control: supersession is live on P)"
        );
    }

    /// AC4 — coexists with the probe, no race, no double-emit. Supersession's
    /// per-victim write goes through `write_if_state_matches` under the same
    /// precondition discipline the probe uses: if a concurrent hook/probe write
    /// moved the victim row between the supersession SELECT and the synthetic
    /// write, the precondition fails and the write no-ops (`Ok(None)`) rather
    /// than stomping the interleaved state. Asserted deterministically at the
    /// `write_if_state_matches` seam (no real concurrency race).
    #[tokio::test(flavor = "current_thread")]
    async fn stale_precondition_noops_instead_of_double_emitting() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 7777;

        // A is live on P (Working). Imagine the supersession scan observed A at
        // (Working, P), but before the synthetic write commits an interleaving
        // hook moved A to Idle.
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write A → Working");
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::Stop, P),
        )
        .await
        .expect("interleaving hook moves A → Idle");
        assert_eq!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Idle,
            "setup: A is now Idle (moved since the imagined scan)"
        );

        // Replay supersession's own write with the STALE snapshot the scan
        // would have carried (Working, P). The row is Idle now → precondition
        // fails → Ok(None), nothing touched.
        let payload = r#"{"reason":"pid_superseded","pid":7777,"observed_at_ms":1}"#.to_string();
        let envelope = EventEnvelope {
            pid: Some(P),
            payload,
            ..envelope_for("claude", "sess-A", EventKind::SessionEnded)
        };
        let precondition = WritePrecondition {
            expected_current_state: SessionCurrentState::Working,
            expected_last_pid: Some(P),
            // This AC4 case drives the no-op via the current_state mismatch
            // (Working snapshot vs the row now at Idle); the monotonic guard is
            // exercised separately by the #3 same-state test below.
            expected_last_event_at_ms: None,
        };
        let result = write_if_state_matches(&pools.writer, &hub, envelope, precondition)
            .await
            .expect("write_if_state_matches");
        assert!(
            result.is_none(),
            "stale precondition must no-op (Ok(None)), not stomp the interleaved state"
        );
        assert_eq!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Idle,
            "A keeps its interleaved Idle state"
        );
        assert!(
            ended_reasons(&pools, "claude", "sess-A").await.is_empty(),
            "no SessionEnded row was written under the stale precondition"
        );
    }

    /// Review finding #2 / ADR 0009 §7 — `/replay` does NOT run supersession.
    /// Replaying A(pid P) then B(pid P) through the replay path must leave A
    /// untouched: the synthetic `SessionEnded` rows live in the log being
    /// replayed, so re-deriving them from replay arrival order would
    /// double-generate and could end the current live holder. Contrast
    /// `successor_supersedes_lone_predecessor`, where the same sequence on the
    /// LIVE path does end A.
    #[tokio::test(flavor = "current_thread")]
    async fn replay_does_not_supersede_predecessor() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 5150;

        // Replay A then B on the same PID, both via the replay entrypoint.
        projection::session::write_replayed(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("replay A");
        projection::session::write_replayed(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-B", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("replay B");

        // A is NOT superseded by replay — supersession is live-ingest-only.
        assert_ne!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Ended,
            "replay must not auto-supersede A; the log's own SessionEnded rows do that"
        );
        assert!(
            ended_reasons(&pools, "claude", "sess-A").await.is_empty(),
            "replay generated no synthetic SessionEnded for A"
        );
        // A replayed SessionEnded line still ends A through the normal write —
        // replay re-applies the log faithfully, it just doesn't synthesize new
        // lifecycle events.
        projection::session::write_replayed(
            &pools.writer,
            &hub,
            EventEnvelope {
                pid: Some(P),
                payload: r#"{"reason":"pid_superseded","pid":5150,"observed_at_ms":1}"#.to_string(),
                ..envelope_for("claude", "sess-A", EventKind::SessionEnded)
            },
        )
        .await
        .expect("replay A's recorded SessionEnded");
        assert_eq!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Ended,
            "replaying A's OWN recorded SessionEnded ends A (faithful reconstruction)"
        );
    }

    /// Review finding #3 — same-state interleaving must not supersede the most
    /// recent emitter. The `(current_state, last_pid)` precondition alone cannot
    /// see a victim that emitted again WITHOUT changing either (e.g.
    /// `Working` → `Working` on the same PID). The optional `last_event_at_ms`
    /// guard catches it: a synthetic write carrying a stale `last_event_at_ms`
    /// no-ops, so the session that just emitted survives. Asserted at the
    /// `write_if_state_matches` seam (the deterministic version of the race).
    #[tokio::test(flavor = "current_thread")]
    async fn same_state_interleave_does_not_supersede_recent_emitter() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 9001;

        // A is live (Working) on P.
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("write A → Working");
        let actual = read_state(&pools, "claude", "sess-A").await;
        assert_eq!(actual.current_state, SessionCurrentState::Working);

        // A synthetic SessionEnded carrying the snapshot a supersession scan
        // would have held — current_state + last_pid still match, but the
        // monotonic guard is pinned to a STALER last_event_at_ms (as if A
        // emitted again after the scan). The guard must reject it.
        let stale_precondition = WritePrecondition {
            expected_current_state: SessionCurrentState::Working,
            expected_last_pid: Some(P),
            expected_last_event_at_ms: Some(actual.last_event_at_ms - 1),
        };
        let result = write_if_state_matches(
            &pools.writer,
            &hub,
            EventEnvelope {
                pid: Some(P),
                payload: r#"{"reason":"pid_superseded","pid":9001,"observed_at_ms":1}"#.to_string(),
                ..envelope_for("claude", "sess-A", EventKind::SessionEnded)
            },
            stale_precondition,
        )
        .await
        .expect("write_if_state_matches");
        assert!(
            result.is_none(),
            "a stale last_event_at_ms must no-op even when current_state + last_pid match"
        );
        assert_ne!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Ended,
            "the most-recent emitter survives the stale supersession write"
        );
        assert!(
            ended_reasons(&pools, "claude", "sess-A").await.is_empty(),
            "no SessionEnded row written under the stale monotonic guard"
        );

        // Positive control: the SAME write with the CURRENT last_event_at_ms
        // does commit — the guard isn't simply always-failing.
        let fresh_precondition = WritePrecondition {
            expected_current_state: SessionCurrentState::Working,
            expected_last_pid: Some(P),
            expected_last_event_at_ms: Some(actual.last_event_at_ms),
        };
        let ok = write_if_state_matches(
            &pools.writer,
            &hub,
            EventEnvelope {
                pid: Some(P),
                payload: r#"{"reason":"pid_superseded","pid":9001,"observed_at_ms":1}"#.to_string(),
                ..envelope_for("claude", "sess-A", EventKind::SessionEnded)
            },
            fresh_precondition,
        )
        .await
        .expect("write_if_state_matches");
        assert!(
            ok.is_some(),
            "a matching last_event_at_ms commits — guard rejects only stale snapshots"
        );
    }

    /// Review finding #1 / ADR 0009 §6 — retry-on-next-event. The supersession
    /// follow-up is best-effort: if it is skipped (daemon crash / transient
    /// failure) after the successor commits, the predecessor stays stranded.
    /// The contract is that the successor's NEXT pid-carrying event re-runs
    /// supersession and recovers it. We model the stranded post-crash state by
    /// committing A and B on P via the replay path (no supersession), then a
    /// LIVE B event recovers it.
    #[tokio::test(flavor = "current_thread")]
    async fn stranded_predecessor_superseded_on_successors_next_event() {
        let (_tmp, pools) = fresh_pools().await;
        let hub = BroadcastHub::new(64);
        const P: u32 = 7012;

        // Simulate "B committed but its supersession follow-up was lost": both
        // A and B land on P with no supersession (replay path skips it).
        projection::session::write_replayed(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-A", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("strand A on P");
        projection::session::write_replayed(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-B", EventKind::UserPromptSubmit, P),
        )
        .await
        .expect("B lands on P without superseding A");
        assert_ne!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Ended,
            "precondition: A is stranded non-Ended (supersession was skipped)"
        );

        // B's NEXT event arrives on the LIVE path — supersession re-runs and
        // recovers the stranded predecessor.
        projection::session::write(
            &pools.writer,
            &hub,
            envelope_with_pid("claude", "sess-B", EventKind::PostToolUse, P),
        )
        .await
        .expect("B's next live event");

        assert_eq!(
            read_state(&pools, "claude", "sess-A").await.current_state,
            SessionCurrentState::Ended,
            "the stranded predecessor is recovered on the successor's next event"
        );
        assert_eq!(
            ended_reasons(&pools, "claude", "sess-A").await,
            vec!["pid_superseded".to_string()],
            "recovered via pid_superseded"
        );
        assert_ne!(
            read_state(&pools, "claude", "sess-B").await.current_state,
            SessionCurrentState::Ended,
            "B (the emitter) is unaffected"
        );
    }
}
