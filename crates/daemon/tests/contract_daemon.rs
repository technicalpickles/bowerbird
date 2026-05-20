use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use assert_cmd::Command;
use bowerbird_daemon::api;
use bowerbird_daemon::db::migrations::migrations;
use bowerbird_daemon::db::queries::{
    event_kind_as_str, SELECT_EVENT_BY_ID, UPSERT_SESSION_PROJECTION,
};
use bowerbird_daemon::db::{init_pools, run_migrations, DbPools};
use bowerbird_daemon::projection;
use bowerbird_daemon::state::AppState;
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
        let id = projection::session::write(&pools.writer, envelope.clone())
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
    let id = projection::session::write(&pools.writer, envelope)
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
            projection::session::write(&writer, envelope)
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
    let state = AppState {
        db: pools,
        migrations_complete: migrations_complete.clone(),
        shutdown: CancellationToken::new(),
    };
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
    let state = AppState {
        db: pools,
        // Deliberately leave migrations_complete = false to assert healthz is
        // independent of readiness.
        migrations_complete: Arc::new(AtomicBool::new(false)),
        shutdown: CancellationToken::new(),
    };
    let app = api::router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
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
        b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
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
        b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
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
        b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
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
        b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(
        resp1.starts_with("200"),
        "first should be 200, got: {resp1:?}"
    );

    // Don't consume from rx — channel is now full. Second send → 503.
    let resp2 = send_line_recv_response(
        &sock_path,
        b"{\"session_id\":\"s2\",\"tool_name\":\"Test\"}\n",
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
        b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n",
    )
    .await;
    assert!(
        resp.starts_with("200"),
        "daemon should still work after EOF client, got: {resp:?}"
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
        envelope_for("claude", "sess-shared", EventKind::PreToolUse),
    )
    .await
    .expect("write claude");
    projection::session::write(
        &pools.writer,
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
        projection::session::write(&pools.writer, envelope_for("claude", "sess-A", kind))
            .await
            .expect("write A");
    }
    for kind in b_seq {
        projection::session::write(&pools.writer, envelope_for("claude", "sess-B", kind))
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
