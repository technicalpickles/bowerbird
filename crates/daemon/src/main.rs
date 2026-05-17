use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use bowerbird_daemon::{
    api,
    config::Config,
    db::{init_pools, run_migrations, DbPools},
    ensure_bowerbird_dir, init_tracing, install_panic_hook, projection, set_crash_dir,
    state::AppState,
};
use clap::Parser;
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

    // Arm the panic hook BEFORE any code that can panic during startup, so
    // early panics (clap parsing-tail, HOME resolution, dir creation) still
    // produce a crash log. Initial destination is the OS temp dir; it gets
    // upgraded to `~/.bowerbird/` once the real directory is known.
    install_panic_hook(std::env::temp_dir());
    init_tracing(args.verbose);

    let home = match home_dir() {
        Some(h) if !h.as_os_str().is_empty() && h.is_absolute() => h,
        _ => {
            eprintln!("error: HOME is unset, empty, or not absolute");
            std::process::exit(1);
        }
    };
    let bowerbird_dir = home.join(".bowerbird");
    if let Err(e) = ensure_bowerbird_dir(&bowerbird_dir) {
        eprintln!("error: failed to create {}: {}", bowerbird_dir.display(), e);
        std::process::exit(1);
    }
    set_crash_dir(bowerbird_dir.clone());

    let config = Config::with_bowerbird_dir(&bowerbird_dir);
    if let Err(e) = run(config).await {
        tracing::error!(error = format!("{e:#}"), "daemon exited with error");
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

    run_migrations(&pools.writer)
        .await
        .context("schema migration failed")?;
    migrations_complete.store(true, Ordering::Release);
    tracing::info!("migrations complete");

    let started = projection::session::write_recording_started(&pools.writer)
        .await
        .context("write_recording_started")?;
    tracing::info!(
        event_id = started.event_id.0,
        recording_session_id = started.recording_session_id,
        "recording started"
    );

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
    // WARN, not INFO: the bound address is operationally important and must be
    // visible at the default verbosity (`error`). See P20 in story 1.2 review.
    tracing::warn!(addr = %local_addr, "daemon listening");

    let shutdown_fut = shutdown_signal(shutdown.clone());
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_fut)
        .await;

    // Drain marker for load balancers / probes — flip ready BEFORE writing the
    // sentinel so a probe between now and process exit observes 503.
    migrations_complete.store(false, Ordering::Release);

    // Always run cleanup, even if axum::serve returned an error. Skipping the
    // sentinel + checkpoint on a serve error would break the gap-detection
    // invariant the recording_sessions schema exists to support.
    if let Err(e) =
        projection::session::write_recording_ended(&pools.writer, started.recording_session_id)
            .await
    {
        tracing::error!(error = ?e, "write_recording_ended failed");
    } else {
        tracing::info!("recording ended");
    }
    if let Err(e) = wal_checkpoint_passive(&pools).await {
        tracing::error!(error = ?e, "wal_checkpoint(PASSIVE) failed");
    }

    serve_result.context("axum::serve")?;
    Ok(())
}

async fn shutdown_signal(token: CancellationToken) {
    tokio::select! {
        _ = next_signal() => {
            tracing::warn!("shutdown signal received");
            token.cancel();
            // Arm a force-exit watcher for the next signal so a hung graceful
            // shutdown can be terminated with a second Ctrl-C / SIGTERM.
            tokio::spawn(force_exit_on_next_signal());
        }
        _ = token.cancelled() => {
            tracing::warn!("shutdown requested via cancellation token");
        }
    }
}

async fn force_exit_on_next_signal() {
    next_signal().await;
    eprintln!("second shutdown signal received; forcing exit");
    std::process::exit(130);
}

async fn next_signal() {
    use tokio::signal::unix::{signal, Signal, SignalKind};

    fn register(kind: SignalKind, name: &str) -> Option<Signal> {
        match signal(kind) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::error!("failed to register {name} handler: {e}");
                None
            }
        }
    }

    let mut sigint = register(SignalKind::interrupt(), "SIGINT");
    let mut sigterm = register(SignalKind::terminate(), "SIGTERM");
    let mut sighup = register(SignalKind::hangup(), "SIGHUP");
    let mut sigquit = register(SignalKind::quit(), "SIGQUIT");

    async fn recv_or_pending(s: &mut Option<Signal>) {
        match s {
            Some(s) => {
                let _ = s.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    }

    tokio::select! {
        _ = recv_or_pending(&mut sigint) => {}
        _ = recv_or_pending(&mut sigterm) => {}
        _ = recv_or_pending(&mut sighup) => {}
        _ = recv_or_pending(&mut sigquit) => {}
    }
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
