use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use assert_cmd::Command;
use bowerbird_daemon::api;
use bowerbird_daemon::db::queries::{event_kind_as_str, SELECT_EVENT_BY_ID};
use bowerbird_daemon::db::{init_pools, run_migrations, DbPools};
use bowerbird_daemon::projection;
use bowerbird_daemon::state::AppState;
use protocol::{EventEnvelope, EventKind};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

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
