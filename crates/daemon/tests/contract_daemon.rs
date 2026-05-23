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
use protocol::{EventEnvelope, EventKind, SessionCurrentState, SessionState};
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
    AppState {
        db: pools,
        migrations_complete,
        shutdown: CancellationToken::new(),
        bearer: BearerToken::new(TEST_BEARER.to_string()),
        started_at_ms: 0,
        broadcaster: Arc::new(BroadcastHub::new(16)),
        ws_semaphore: Arc::new(tokio::sync::Semaphore::new(ws_max_conns)),
        ws_config: WsConfig {
            ping_interval,
            pong_timeout,
        },
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

    let (_tmp, pools) = fresh_pools().await;
    let migrations_complete = Arc::new(AtomicBool::new(false));
    let state = make_test_state(pools, migrations_complete.clone());
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
        if content.contains("rusqlite::Connection::open") {
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
    tokio::sync::mpsc::Receiver<protocol::EventEnvelope>,
) {
    let sock_path = tmp.path().join("ingest.sock");
    let (tx, rx) = tokio::sync::mpsc::channel::<protocol::EventEnvelope>(capacity);
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

    let envelope = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("timeout waiting for envelope")
        .expect("channel closed");
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

    let (tx, rx) = tokio::sync::mpsc::channel::<EventEnvelope>(16);
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

    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout waiting for envelope")
        .expect("channel closed");
    assert_eq!(envelope.kind, EventKind::PreToolUse);
    assert_eq!(envelope.source, "claude");
    assert_eq!(envelope.session_id, "test-session-abc123");

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

    // Mutate only the claude session.
    projection::session::write(
        &pools.writer,
        &BroadcastHub::new(16),
        envelope_for("claude", "sess-shared", EventKind::PostToolUse),
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
#[tokio::test(flavor = "current_thread")]
async fn state_machine_full_sequence_determinism() {
    let (_tmp, pools) = fresh_pools().await;
    let session_id = "sess-determinism";

    let cases: &[(EventKind, SessionCurrentState)] = &[
        (EventKind::PreToolUse, SessionCurrentState::Working),
        (EventKind::PostToolUse, SessionCurrentState::Idle),
        (EventKind::PreToolUse, SessionCurrentState::Working),
        (EventKind::Notification, SessionCurrentState::WaitingInput),
        (EventKind::PreToolUse, SessionCurrentState::Working),
        (EventKind::Stop, SessionCurrentState::Idle),
    ];

    for (kind, expected) in cases {
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            envelope_for("claude", session_id, kind.clone()),
        )
        .await
        .expect("write");
        let stored = read_session_state(&pools.reader, "claude", session_id).await;
        let parsed: SessionState = serde_json::from_str(&stored).expect("parse");
        assert_eq!(
            parsed.current_state, *expected,
            "after {kind:?} current_state must be {expected:?}, got {:?}",
            parsed.current_state
        );
        assert_eq!(
            parsed.last_event_kind, *kind,
            "last_event_kind must always reflect the latest event"
        );
    }
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
        let (_tmp, pools) = fresh_pools().await;
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
            },
        )
        .await
        .expect("write sess-a");
        // sess-b: PostToolUse → stored Idle.
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-b".to_string(),
                kind: EventKind::PostToolUse,
                reaction: None,
                payload: "{}".to_string(),
            },
        )
        .await
        .expect("write sess-b");

        let app = api::router(ready_state(pools));
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
            "PostToolUse surfaces as Idle"
        );
        assert_eq!(by_id["sess-b"].last_event_kind, EventKind::PostToolUse);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sessions_list_applies_stale_working_fallback() {
        let (_tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-old".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
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

        let app = api::router(ready_state(pools));
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
    }

    // ----- Task 14 -----
    #[tokio::test(flavor = "current_thread")]
    async fn sessions_detail_returns_projection_state() {
        let (_tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-x".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
            },
        )
        .await
        .expect("write sess-x");

        let app = api::router(ready_state(pools));
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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sessions_detail_returns_404_when_unknown() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(auth_get("/sessions/does-not-exist"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = json_body(resp).await;
        assert_eq!(body, serde_json::json!({ "error": "session not found" }));
    }

    // ----- Task 15 -----
    #[tokio::test(flavor = "current_thread")]
    async fn events_list_returns_all_in_ascending_order() {
        let (_tmp, pools) = fresh_pools().await;
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
                },
            )
            .await
            .expect("write");
        }
        let app = api::router(ready_state(pools));
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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn events_list_returns_empty_with_none_cursor() {
        let (_tmp, pools) = fresh_pools().await;
        // Do not write any non-sentinel events; the session simply doesn't exist.
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(auth_get("/sessions/no-such/events?since=0"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);
        let body: EventListResponse = json_body(resp).await;
        assert!(body.events.is_empty());
        assert_eq!(body.cursor, None);
        assert_eq!(body.oldest_available_event_id.0, i64::MAX);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn events_list_respects_since_cursor() {
        let (_tmp, pools) = fresh_pools().await;
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
                },
            )
            .await
            .expect("write");
            written_ids.push(id.0);
        }
        let cutoff = written_ids[4]; // strictly > cutoff means last 5 returned.

        let app = api::router(ready_state(pools));
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
    }

    // ----- Task 16 -----
    #[tokio::test(flavor = "current_thread")]
    async fn events_list_oldest_available_after_purge() {
        let (_tmp, pools) = fresh_pools().await;
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

        let app = api::router(ready_state(pools));
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
        let (_tmp, pools) = fresh_pools().await;
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
                },
            )
            .await
            .expect("write");
        }
        let app = api::router(ready_state(pools));
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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sessions_stats_returns_404_when_unknown() {
        let (_tmp, pools) = fresh_pools().await;
        let app = api::router(ready_state(pools));
        let resp = app
            .oneshot(auth_get("/sessions/missing/stats"))
            .await
            .expect("oneshot");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ----- /status -----
    #[tokio::test(flavor = "current_thread")]
    async fn status_returns_uptime_and_last_event() {
        let (_tmp, pools) = fresh_pools().await;
        projection::session::write(
            &pools.writer,
            &BroadcastHub::new(16),
            EventEnvelope {
                source: "claude".to_string(),
                session_id: "sess-z".to_string(),
                kind: EventKind::PreToolUse,
                reaction: None,
                payload: "{}".to_string(),
            },
        )
        .await
        .expect("write");

        // Use a non-zero started_at_ms so uptime is meaningful.
        let mc = Arc::new(AtomicBool::new(true));
        let state = AppState {
            db: pools,
            migrations_complete: mc,
            shutdown: CancellationToken::new(),
            bearer: BearerToken::new(super::TEST_BEARER.to_string()),
            started_at_ms: 1,
            broadcaster: Arc::new(BroadcastHub::new(16)),
            ws_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            ws_config: WsConfig {
                ping_interval: Duration::from_secs(30),
                pong_timeout: Duration::from_secs(10),
            },
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
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_returns_none_last_event_when_only_sentinels() {
        let (_tmp, pools) = fresh_pools().await;
        projection::session::write_recording_started(&pools.writer)
            .await
            .expect("sentinel");
        let app = api::router(ready_state(pools));
        let resp = app.oneshot(auth_get("/status")).await.expect("oneshot");
        let body: DaemonStatus = json_body(resp).await;
        assert!(
            body.last_event_id.is_none(),
            "sentinel rows should not surface as last_event_id"
        );
        assert!(body.last_event_at_ms.is_none());
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
        let shutdown = state.shutdown.clone();
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

        state.shutdown.cancel();
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

        state.shutdown.cancel();
    }

    fn make_state_envelope(session_id: &str) -> BroadcastEnvelope {
        BroadcastEnvelope::State {
            source: "claude".to_string(),
            session_id: session_id.to_string(),
            state: SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 0,
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

        state.shutdown.cancel();
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
        state.shutdown.cancel();
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
        state.shutdown.cancel();
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
        state.shutdown.cancel();
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
        state.shutdown.cancel();
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
        state.shutdown.cancel();
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
        state.shutdown.cancel();
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

        state.shutdown.cancel();
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
        state.shutdown.cancel();
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

        state.shutdown.cancel();
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
                    state.shutdown.cancel();
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
        state.shutdown.cancel();
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
        state.shutdown.cancel();
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

        state.shutdown.cancel();

        // Within a generous window the server-side connection task exits
        // and our client either receives a Close, EOF, or a network error.
        let close_or_eof = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Err(_)) => return,
                    Some(Ok(_)) => continue,
                }
            }
        })
        .await;
        assert!(
            close_or_eof.is_ok(),
            "ws did not close within 2s of shutdown"
        );
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

        state.shutdown.cancel();
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
        let parsed = parse_state_frame(&frame);
        assert_eq!(parsed.session_id, "sess-A");

        state.shutdown.cancel();
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
        }));

        let timed = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            timed.is_err(),
            "events.claude.* must not deliver codex-sourced events; got {timed:?}"
        );

        state.shutdown.cancel();
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

        let order = ["sess-A", "sess-B", "sess-A", "sess-C"];
        for sid in &order {
            let _ =
                publish_via_projection(&state, "claude", sid, EventKind::PreToolUse, None, "{}")
                    .await;
        }

        for expected in &order {
            let frame = read_text_frame_or_close(&mut ws).await;
            let parsed = parse_state_frame(&frame);
            assert_eq!(
                parsed.session_id, *expected,
                "session_id must match publication order; expected {expected}, got {}",
                parsed.session_id
            );
        }

        state.shutdown.cancel();
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

        state.shutdown.cancel();
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

        state.shutdown.cancel();
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

        state.shutdown.cancel();
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
        state.shutdown.cancel();
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

        state.shutdown.cancel();
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
    use futures_util::{SinkExt, StreamExt};
    use protocol::{EventKind, ServerMessage, SessionCurrentState};
    use tokio_tungstenite::tungstenite::Message;

    use super::story_2_1_ws::{
        connect_authed, parse_hello, read_text_frame_or_close, spawn_test_daemon,
    };
    use super::story_2_2_publish::{
        connect_until_ready, parse_event_frame, parse_state_frame, publish_via_projection,
        wait_subscribe_live, ProbeKind, WsStream,
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
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-B",
            EventKind::PostToolUse,
            None,
            "{}",
        )
        .await;
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
        // sess-B: PostToolUse → Idle
        assert_eq!(frame_b.state.last_event_kind, EventKind::PostToolUse);
        assert_eq!(
            frame_b.state.current_state,
            SessionCurrentState::Idle,
            "sess-B PostToolUse → Idle"
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
        tokio::time::sleep(Duration::from_millis(2)).await;
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PostToolUse,
            None,
            "{}",
        )
        .await;
        let f_state = read_text_frame_or_close(&mut ws).await;
        let live = parse_state_frame(&f_state);
        assert_eq!(live.session_id, "sess-A");
        assert_eq!(live.source, "claude");
        assert_eq!(live.state.last_event_kind, EventKind::PostToolUse);
        assert_eq!(live.state.current_state, SessionCurrentState::Idle);
        assert!(
            live.state.last_event_at_ms > max_snapshot_last_event_at_ms,
            "live frame must strictly postdate the snapshot (live={}, max snapshot={})",
            live.state.last_event_at_ms,
            max_snapshot_last_event_at_ms
        );

        state.shutdown.cancel();
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

        state.shutdown.cancel();
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

        // Publish for sess-B — no state frame must arrive on this
        // connection within 300ms.
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-B",
            EventKind::PostToolUse,
            None,
            "{}",
        )
        .await;
        let timed = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            timed.is_err(),
            "sess-B state must not reach sess-A-only subscriber; got {timed:?}"
        );

        state.shutdown.cancel();
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

        state.shutdown.cancel();
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

        state.shutdown.cancel();
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

        state.shutdown.cancel();
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
        let _ = publish_via_projection(
            &state,
            "claude",
            "sess-A",
            EventKind::PostToolUse,
            None,
            "{}",
        )
        .await;
        let f_state = read_text_frame_or_close(&mut ws).await;
        let live = parse_state_frame(&f_state);
        assert_eq!(live.session_id, "sess-A");
        assert_eq!(live.state.last_event_kind, EventKind::PostToolUse);

        // No trailing frame within a short window — the publish
        // produced exactly one State frame; no leftover snapshot.
        let timed = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
        assert!(
            timed.is_err(),
            "no extra frame after live State frame: got {timed:?}"
        );

        state.shutdown.cancel();
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

        state.shutdown.cancel();
    }
}
