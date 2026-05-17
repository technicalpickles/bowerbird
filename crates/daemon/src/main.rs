use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use bowerbird_daemon::{
    api,
    config::Config,
    db::{init_pools, queries, run_migrations, DbPools},
    ensure_bowerbird_dir, init_tracing, install_panic_hook, projection,
    state::AppState,
};
use clap::Parser;
use protocol::{EventEnvelope, EventId, EventKind};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "bowerbird-daemon")]
struct Args {
    /// Verbosity: -v info, -vv debug, -vvv trace
    #[arg(short, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();

    let home = match home_dir() {
        Some(h) => h,
        None => {
            eprintln!("error: HOME environment variable is not set");
            std::process::exit(1);
        }
    };
    let bowerbird_dir = home.join(".bowerbird");
    if let Err(e) = ensure_bowerbird_dir(&bowerbird_dir) {
        eprintln!("error: failed to create {}: {}", bowerbird_dir.display(), e);
        std::process::exit(1);
    }

    install_panic_hook(bowerbird_dir.clone());
    init_tracing(args.verbose);

    let config = Config::with_bowerbird_dir(&bowerbird_dir);
    if let Err(e) = run(config).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

async fn run(config: Config) -> anyhow::Result<()> {
    let migrations_complete = Arc::new(AtomicBool::new(false));
    let shutdown = CancellationToken::new();

    let pools = init_pools(&config.db_path)
        .await
        .with_context(|| format!("failed to init pools at {}", config.db_path.display()))?;
    let pools = Arc::new(pools);

    run_migrations(&pools.writer)
        .await
        .context("schema migration failed")?;
    migrations_complete.store(true, Ordering::Release);
    tracing::info!("migrations complete");

    let recording_started_id = emit_sentinel(&pools, EventKind::RecordingStarted).await?;
    record_recording_session_started(&pools, recording_started_id).await?;
    tracing::info!(event_id = recording_started_id.0, "recording started");

    let state = AppState {
        db: pools.clone(),
        migrations_complete: migrations_complete.clone(),
        shutdown: shutdown.clone(),
    };
    let router = api::router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    let local_addr = listener.local_addr().context("listener.local_addr")?;
    tracing::info!(addr = %local_addr, "daemon listening");

    let shutdown_fut = shutdown_signal(shutdown.clone());
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_fut)
        .await
        .context("axum::serve")?;

    let recording_ended_id = emit_sentinel(&pools, EventKind::RecordingEnded).await?;
    record_recording_session_ended(&pools, recording_ended_id).await?;
    tracing::info!(event_id = recording_ended_id.0, "recording ended");

    wal_checkpoint_passive(&pools).await?;
    Ok(())
}

async fn shutdown_signal(token: CancellationToken) {
    use tokio::signal::unix::{signal, SignalKind};

    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                let _ = s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    let cancelled = token.cancelled();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = cancelled => {}
    }
    tracing::info!("shutdown signal received");
}

async fn emit_sentinel(pools: &DbPools, kind: EventKind) -> anyhow::Result<EventId> {
    let envelope = EventEnvelope {
        source: "daemon".to_string(),
        session_id: "daemon".to_string(),
        kind,
        reaction: None,
        payload: "{}".to_string(),
    };
    let id = projection::session::write(&pools.writer, envelope)
        .await
        .context("projection::session::write")?;
    Ok(id)
}

async fn record_recording_session_started(
    pools: &DbPools,
    started_id: EventId,
) -> anyhow::Result<()> {
    let conn = pools
        .writer
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("writer pool get failed: {e}"))?;
    let started = started_id.0;
    conn.interact(move |c| -> rusqlite::Result<()> {
        c.execute(
            queries::INSERT_RECORDING_SESSION_STARTED,
            rusqlite::params![started],
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("interact failed: {e}"))?
    .context("INSERT_RECORDING_SESSION_STARTED")?;
    Ok(())
}

async fn record_recording_session_ended(pools: &DbPools, ended_id: EventId) -> anyhow::Result<()> {
    let conn = pools
        .writer
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("writer pool get failed: {e}"))?;
    let ended = ended_id.0;
    conn.interact(move |c| -> rusqlite::Result<()> {
        c.execute(
            queries::UPDATE_RECORDING_SESSION_ENDED,
            rusqlite::params![ended],
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("interact failed: {e}"))?
    .context("UPDATE_RECORDING_SESSION_ENDED")?;
    Ok(())
}

async fn wal_checkpoint_passive(pools: &DbPools) -> anyhow::Result<()> {
    let conn = pools
        .writer
        .get()
        .await
        .map_err(|e| anyhow::anyhow!("writer pool get failed: {e}"))?;
    conn.interact(|c| -> rusqlite::Result<()> {
        c.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("interact failed: {e}"))?
    .context("wal_checkpoint(PASSIVE)")?;
    Ok(())
}
