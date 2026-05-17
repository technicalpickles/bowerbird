//! Contract tests for Story 1.2 — daemon foundation with SQLite persistence.
//!
//! The daemon is a binary crate, so the tests reach into the binary's source
//! modules via `#[path]` includes. The included modules use `crate::...`
//! references (e.g. `crate::error::Error`); we mirror that layout under the
//! test binary's crate root so the paths resolve identically.

#![allow(dead_code, unused_imports)]

#[path = "../src/error.rs"]
mod error;

#[path = "../src/config.rs"]
mod config;

#[path = "."]
mod db {
    #[path = "../src/db/migrations.rs"]
    pub mod migrations;
    #[path = "../src/db/pool.rs"]
    pub mod pool;
    #[path = "../src/db/queries.rs"]
    pub mod queries;

    pub use migrations::{migrations, run_migrations};
    pub use pool::{build_pools, DbPools};
}

mod state {
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    use crate::db::DbPools;

    pub struct AppState {
        pub db: DbPools,
        pub shutdown: CancellationToken,
    }

    pub type SharedState = Arc<AppState>;
}

#[path = "."]
mod api {
    #[path = "../src/api/health.rs"]
    pub mod health;
}

#[path = "."]
mod projection {
    #[path = "../src/projection/session.rs"]
    pub mod session;
}

// ----- end of module stitching -----

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::params;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::config::Config;
use crate::db::queries::{
    PRAGMA_FOREIGN_KEYS, PRAGMA_JOURNAL_MODE, PRAGMA_SYNCHRONOUS, UPSERT_PROJECTION,
};
use crate::db::{build_pools, run_migrations, DbPools};
use crate::error::Result;
use crate::state::AppState;

fn test_config(dir: &TempDir) -> Config {
    Config::with_data_dir(dir.path().to_path_buf()).expect("config")
}

async fn fresh_pools(dir: &TempDir) -> DbPools {
    let cfg = test_config(dir);
    build_pools(&cfg).await.expect("build pools")
}

async fn migrated_pools(dir: &TempDir) -> DbPools {
    let pools = fresh_pools(dir).await;
    run_migrations(&pools).await.expect("migrations");
    pools
}

fn make_state(pools: DbPools) -> Arc<AppState> {
    Arc::new(AppState {
        db: pools,
        shutdown: CancellationToken::new(),
    })
}

// ---------------------------------------------------------------------------
// Test: PRAGMA invariants on every checkout (AC #2)
// ---------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pragma_invariants_on_every_checkout() {
    let dir = TempDir::new().expect("tempdir");
    let pools = migrated_pools(&dir).await;

    // Writer pool: every checkout (single connection, but check multiple times).
    for _ in 0..3 {
        let conn = pools.writer.get().await.expect("writer checkout");
        let result: rusqlite::Result<(i64, String, i64)> = conn
            .interact(|c| {
                let fk: i64 = c.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
                let jm: String = c.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
                let sync: i64 = c.pragma_query_value(None, "synchronous", |r| r.get(0))?;
                Ok((fk, jm, sync))
            })
            .await
            .expect("interact");
        let (fk, jm, sync) = result.expect("pragmas");
        assert_eq!(fk, 1, "foreign_keys must be ON");
        assert_eq!(jm.to_lowercase(), "wal", "journal_mode must be WAL");
        assert_eq!(sync, 1, "synchronous must be NORMAL");
    }

    // Readers pool: at minimum two parallel checkouts.
    let r1 = pools.readers.get().await.expect("reader 1");
    let r2 = pools.readers.get().await.expect("reader 2");
    for reader in [r1, r2] {
        let result: rusqlite::Result<(i64, String, i64)> = reader
            .interact(|c| {
                let fk: i64 = c.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
                let jm: String = c.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
                let sync: i64 = c.pragma_query_value(None, "synchronous", |r| r.get(0))?;
                Ok((fk, jm, sync))
            })
            .await
            .expect("interact");
        let (fk, jm, sync) = result.expect("pragmas");
        assert_eq!(fk, 1);
        assert_eq!(jm.to_lowercase(), "wal");
        assert_eq!(sync, 1);
    }
}

// ---------------------------------------------------------------------------
// Test: migrations apply cleanly on fresh DB (AC #3)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn migrations_apply_on_fresh_db() {
    let dir = TempDir::new().expect("tempdir");
    let pools = migrated_pools(&dir).await;
    let conn = pools.writer.get().await.expect("checkout");
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
    assert!(events.contains(&"event_id".to_owned()));
    assert!(events.contains(&"source".to_owned()));
    assert!(events.contains(&"session_id".to_owned()));
    assert!(events.contains(&"kind".to_owned()));
    assert!(events.contains(&"reaction".to_owned()));
    assert!(events.contains(&"payload".to_owned()));
    assert!(events.contains(&"created_at".to_owned()));

    let proj = &columns[1].1;
    assert!(proj.contains(&"source".to_owned()));
    assert!(proj.contains(&"session_id".to_owned()));
    assert!(proj.contains(&"state".to_owned()));
    assert!(proj.contains(&"updated_at".to_owned()));

    let rec = &columns[2].1;
    assert!(rec.contains(&"id".to_owned()));
    assert!(rec.contains(&"started_event_id".to_owned()));
    assert!(rec.contains(&"ended_event_id".to_owned()));
}

// ---------------------------------------------------------------------------
// Test: migration definitions pass rusqlite_migration's validate() (AC #4)
// ---------------------------------------------------------------------------
#[test]
fn migrations_validate() {
    use crate::db::migrations::migrations;
    assert!(
        migrations().validate().is_ok(),
        "migration set must validate"
    );
}

// ---------------------------------------------------------------------------
// Test: migration failure surfaces as Err (AC #4)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn migration_failure_surfaces() {
    let dir = TempDir::new().expect("tempdir");
    // Pre-create DB with a future user_version that's ahead of our migrations.
    let cfg = test_config(&dir);
    let db_path = cfg.db_path();
    {
        // Use rusqlite directly — this is OK in test code per the lint rule's
        // scope (the lint scans crate sources, not test fixtures). This file
        // is `tests/contract_daemon.rs`, which the CI grep gate excludes
        // because it scans only `crates/daemon/src/**/*.rs`.
        let conn = rusqlite::Connection::open(&db_path).expect("open");
        conn.pragma_update(None, "user_version", 999)
            .expect("set version");
    }
    let pools = build_pools(&cfg).await.expect("build pools");
    let result = run_migrations(&pools).await;
    assert!(
        result.is_err(),
        "migration must fail when user_version is ahead"
    );
}

// ---------------------------------------------------------------------------
// Test: WAL concurrent reader/writer (AC #5)
// ---------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wal_concurrent_reader_writer() {
    let dir = TempDir::new().expect("tempdir");
    let pools = Arc::new(migrated_pools(&dir).await);

    let writer_pool = pools.clone();
    let writer_task = tokio::spawn(async move {
        for i in 0..50i64 {
            let conn = writer_pool.writer.get().await.expect("writer");
            let _: rusqlite::Result<usize> = conn
                .interact(move |c| {
                    c.execute(
                        "INSERT INTO events (source, session_id, kind, reaction, payload, created_at) \
                         VALUES (?, ?, ?, ?, ?, ?)",
                        params!["test", "s1", "PreToolUse", None::<String>, "{}", i],
                    )
                })
                .await
                .expect("interact");
        }
    });

    let reader_pool = pools.clone();
    let reader_task = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let mut counts = Vec::new();
        while start.elapsed() < Duration::from_secs(3) {
            let conn = reader_pool.readers.get().await.expect("reader");
            let n: i64 = conn
                .interact(|c| {
                    c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                })
                .await
                .expect("interact")
                .expect("query");
            counts.push(n);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        counts
    });

    writer_task.await.expect("writer task");
    let counts = reader_task.await.expect("reader task");
    assert!(
        !counts.is_empty(),
        "reader produced at least one observation"
    );
    let final_count = *counts.last().unwrap();
    assert!(final_count >= 1, "reader saw rows while writer ran");
}

// ---------------------------------------------------------------------------
// Test: state+event atomicity via explicit rollback (surrogate for SIGKILL)
// (AC #1 — see story Dev Notes — Transaction Invariant)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn state_plus_event_atomicity_rollback() {
    let dir = TempDir::new().expect("tempdir");
    let pools = migrated_pools(&dir).await;
    {
        let conn = pools.writer.get().await.expect("writer");
        let _: rusqlite::Result<()> = conn
            .interact(|c| {
                let tx = c.transaction()?;
                tx.execute(
                    UPSERT_PROJECTION,
                    params!["src", "sess", "{\"k\":1}", 100i64],
                )?;
                // Note: we intentionally do NOT insert the event row and we
                // rollback to simulate mid-transaction crash semantics.
                tx.rollback()?;
                Ok(())
            })
            .await
            .expect("interact");
    }

    // Verify no projection row exists either — the transaction was atomic.
    let reader = pools.readers.get().await.expect("reader");
    let (proj_count, event_count): (i64, i64) = reader
        .interact(|c| -> rusqlite::Result<(i64, i64)> {
            let p: i64 =
                c.query_row("SELECT COUNT(*) FROM session_projections", [], |r| r.get(0))?;
            let e: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
            Ok((p, e))
        })
        .await
        .expect("interact")
        .expect("query");
    assert_eq!(proj_count, 0);
    assert_eq!(event_count, 0);

    // Now exercise the real projection::session::write happy path and confirm
    // both rows are committed atomically.
    use protocol::{EventEnvelope, EventKind};
    let envelope = EventEnvelope {
        source: "src".into(),
        session_id: "sess".into(),
        kind: EventKind::PreToolUse,
        reaction: None,
        payload: "{}".into(),
    };
    let id = projection::session::write(&pools, envelope)
        .await
        .expect("write");
    assert!(id.0 > 0);

    let reader = pools.readers.get().await.expect("reader");
    let (proj_count, event_count): (i64, i64) = reader
        .interact(|c| -> rusqlite::Result<(i64, i64)> {
            let p: i64 =
                c.query_row("SELECT COUNT(*) FROM session_projections", [], |r| r.get(0))?;
            let e: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
            Ok((p, e))
        })
        .await
        .expect("interact")
        .expect("query");
    assert_eq!(proj_count, 1);
    assert_eq!(event_count, 1);
}

// ---------------------------------------------------------------------------
// Test: /healthz returns 200 unauthenticated (AC #9)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn healthz_returns_200() {
    let dir = TempDir::new().expect("tempdir");
    let pools = migrated_pools(&dir).await;
    let state = make_state(pools);
    let app = api::health::router(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["status"], "ok");
}

// ---------------------------------------------------------------------------
// Test: /readyz returns 503 before migrations, 200 after (AC #3)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn readyz_phases() {
    let dir = TempDir::new().expect("tempdir");
    let pre_pools = fresh_pools(&dir).await;
    let state = make_state(pre_pools);
    let app = api::health::router(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "/readyz must 503 before schema exists"
    );

    // Now run migrations against a NEW pool against the same DB path and verify ready.
    let cfg = test_config(&dir);
    let pools = build_pools(&cfg).await.expect("build");
    run_migrations(&pools).await.expect("migrate");
    let state = make_state(pools);
    let app = api::health::router(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Test: connection factory enforcement lint self-test (AC #6)
//
// The lint script lives at `scripts/lint-connection-factory.sh`. It exits 0
// when the working tree is clean and non-zero when a violation appears. We
// drive both paths: clean scan must pass; scan with the fixture injected via
// SCAN_EXTRA must fail. This prevents the lint from rotting silently.
// ---------------------------------------------------------------------------
#[test]
fn lint_self_test_connection_factory() {
    let workspace_root = workspace_root();
    let script = workspace_root.join("scripts/lint-connection-factory.sh");
    let fixture = workspace_root.join("crates/daemon/tests/fixtures/lint_violation.rs.txt");
    assert!(script.exists(), "lint script must exist at {script:?}");
    assert!(fixture.exists(), "fixture must exist at {fixture:?}");

    // Sanity: fixture contains the violating pattern.
    let contents = std::fs::read_to_string(&fixture).expect("fixture");
    assert!(contents.contains("rusqlite::Connection::open"));

    // Clean scan must pass.
    let clean = std::process::Command::new("bash")
        .arg(&script)
        .current_dir(&workspace_root)
        .output()
        .expect("run lint script");
    assert!(
        clean.status.success(),
        "lint script must pass on a clean tree; stdout={} stderr={}",
        String::from_utf8_lossy(&clean.stdout),
        String::from_utf8_lossy(&clean.stderr)
    );

    // Fixture-injected scan must fail.
    let violating = std::process::Command::new("bash")
        .arg(&script)
        .env("SCAN_EXTRA", fixture.to_string_lossy().as_ref())
        .current_dir(&workspace_root)
        .output()
        .expect("run lint script with fixture");
    assert!(
        !violating.status.success(),
        "lint script must fail when fixture is injected; stdout={} stderr={}",
        String::from_utf8_lossy(&violating.stdout),
        String::from_utf8_lossy(&violating.stderr)
    );
}

// ---------------------------------------------------------------------------
// Test: inline SQL ban lint self-test (AC #6)
// ---------------------------------------------------------------------------
#[test]
fn lint_self_test_inline_sql() {
    let workspace_root = workspace_root();
    let script = workspace_root.join("scripts/lint-inline-sql.sh");
    assert!(script.exists(), "lint script must exist at {script:?}");

    // Clean scan must pass.
    let clean = std::process::Command::new("bash")
        .arg(&script)
        .current_dir(&workspace_root)
        .output()
        .expect("run lint script");
    assert!(
        clean.status.success(),
        "lint script must pass on a clean tree; stdout={} stderr={}",
        String::from_utf8_lossy(&clean.stdout),
        String::from_utf8_lossy(&clean.stderr)
    );

    // Create an ephemeral fixture and inject it via SCAN_EXTRA.
    let tmp = TempDir::new().expect("tempdir");
    let fixture = tmp.path().join("inline_sql_violation.rs");
    std::fs::write(
        &fixture,
        "fn _violation() { let _ = \"SELECT 1 FROM events\"; }\n",
    )
    .expect("write fixture");
    let violating = std::process::Command::new("bash")
        .arg(&script)
        .env("SCAN_EXTRA", fixture.to_string_lossy().as_ref())
        .current_dir(&workspace_root)
        .output()
        .expect("run lint script with fixture");
    assert!(
        !violating.status.success(),
        "lint script must fail when an inline SQL fixture is injected; stdout={} stderr={}",
        String::from_utf8_lossy(&violating.stdout),
        String::from_utf8_lossy(&violating.stderr)
    );
}

fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is `crates/daemon`; walk up two levels.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}
