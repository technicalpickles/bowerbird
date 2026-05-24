use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use bowerbird_daemon::{
    api::{
        self,
        token::{self, TokenSource},
    },
    broadcast::BroadcastHub,
    config::Config,
    db::{init_pools, run_migrations, DbPools},
    ensure_bowerbird_dir, ingest, init_tracing, install_panic_hook, projection, set_crash_dir,
    singleton,
    state::{wait_for_ws_connection_drain, AppState, WsConfig},
    time::current_unix_millis,
    write_error_report,
};
use clap::Parser;
use std::future::IntoFuture;
use tokio_util::sync::CancellationToken;

use adapter_claude::ClaudeAdapter;

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

    let bowerbird_dir = match resolve_bowerbird_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = ensure_bowerbird_dir(&bowerbird_dir) {
        eprintln!("error: failed to create {}: {}", bowerbird_dir.display(), e);
        std::process::exit(1);
    }
    set_crash_dir(bowerbird_dir.clone());

    // Singleton enforcement BEFORE any DB or socket work — the race this guards
    // is two daemons running migrations against the same `bower.db`. The lock
    // is held in `_singleton_lock` for the rest of main; dropping it (on a
    // clean return) releases the flock and closes the FD. Unclean exits (OOM,
    // SIGKILL, panic past the panic hook) release via kernel FD reclaim.
    let _singleton_lock = match singleton::acquire(&bowerbird_dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    let mut config = Config::with_bowerbird_dir(&bowerbird_dir);
    // Honor BOWERBIRD_INGEST_SOCK as a path override so tests can run an
    // isolated daemon without colliding with `~/.bowerbird/ingest.sock` (mirrors
    // the shim's resolve_sock_path pattern).
    if let Some(p) = std::env::var_os("BOWERBIRD_INGEST_SOCK") {
        if !p.is_empty() {
            config.ingest_sock_path = PathBuf::from(p);
        }
    }
    if let Err(e) = run(config).await {
        let msg = format!("{e:#}");
        tracing::error!(error = msg, "daemon exited with error");
        // AC #8: write a crash log for unhandled-error exits too, not just
        // panics. Best-effort — failure here must not block the exit path.
        let _ = write_error_report(&msg);
        std::process::exit(1);
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Resolve the bowerbird data directory. `BOWERBIRD_DATA_DIR` wins if set and
/// non-empty — same shape as the `BOWERBIRD_INGEST_SOCK` override below.
/// Tests use this to run isolated daemon instances against a TempDir without
/// colliding on `~/.bowerbird/`; Story 3.2's lifecycle CLI will reuse the same
/// override for its `--data-dir` flag.
fn resolve_bowerbird_dir() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("BOWERBIRD_DATA_DIR") {
        if !p.is_empty() {
            let path = PathBuf::from(p);
            if !path.is_absolute() {
                return Err(format!(
                    "BOWERBIRD_DATA_DIR must be an absolute path; got {}",
                    path.display()
                ));
            }
            return Ok(path);
        }
    }
    let home = match home_dir() {
        Some(h) if !h.as_os_str().is_empty() && h.is_absolute() => h,
        _ => return Err("HOME is unset, empty, or not absolute".to_string()),
    };
    Ok(home.join(".bowerbird"))
}

async fn run(config: Config) -> anyhow::Result<()> {
    let migrations_complete = Arc::new(AtomicBool::new(false));
    let shutdown_requested = CancellationToken::new();
    let ws_close_requested = CancellationToken::new();

    // Load the bearer token before anything else REST-related is constructed.
    // A `Generated` source warrants a WARN line so operators running without
    // `$BOWERBIRD_TOKEN` set realize the ephemeral token has been minted and
    // is visible only in this log stream (the token value itself is never
    // logged — `secrecy::SecretString` redacts `Debug`/`Display`).
    let (bearer, token_source) = token::load_or_generate();
    match token_source {
        TokenSource::Env => {
            tracing::info!("bearer token loaded from $BOWERBIRD_TOKEN");
        }
        TokenSource::Generated => {
            tracing::warn!(
                "daemon generated ephemeral bearer token; set $BOWERBIRD_TOKEN to control it (token value not logged)"
            );
        }
    }

    let started_at_ms =
        current_unix_millis().context("failed to read startup wall-clock for AppState")?;

    let pools = init_pools(&config.db_path)
        .await
        .with_context(|| format!("failed to init pools at {}", config.db_path.display()))?;

    run_migrations(&pools.writer)
        .await
        .context("schema migration failed")?;
    migrations_complete.store(true, Ordering::Release);
    tracing::info!("migrations complete");

    // Rebuild any session_projections rows missing for sessions that exist in
    // `events`. Runs before the ingest listener is spawned so no concurrent
    // writes can interleave. Best-effort: a rebuild failure does not block
    // startup — the daemon must come up for sessions whose projections are
    // intact (Story 1.6 Task 6).
    match projection::session::rebuild_missing_projections(&pools.writer).await {
        Ok(n) if n > 0 => tracing::info!(count = n, "rebuilt missing session projections"),
        Ok(_) => {}
        Err(e) => tracing::error!(error = ?e, "rebuild_missing_projections failed; continuing"),
    }

    let started = projection::session::write_recording_started(&pools.writer)
        .await
        .context("write_recording_started")?;
    tracing::info!(
        event_id = started.event_id.0,
        recording_session_id = started.recording_session_id,
        "recording started"
    );

    let adapter = ClaudeAdapter::new(config.tool_reactions_path.clone());
    for issue in adapter.validate_config() {
        tracing::warn!(
            path = %config.tool_reactions_path.display(),
            "tool-reactions config: {issue}"
        );
    }
    let adapter = Arc::new(adapter);

    let (ingest_tx, ingest_rx) =
        tokio::sync::mpsc::channel::<protocol::EventEnvelope>(config.ingest_channel_capacity);
    let ingest_listener = ingest::listener::bind(&config.ingest_sock_path).with_context(|| {
        format!(
            "failed to bind ingest socket {}",
            config.ingest_sock_path.display()
        )
    })?;
    // Construct the broadcaster BEFORE the ingest writer spawn so it can be
    // threaded into the writer task. The same `Arc` lands in `AppState` below
    // via `broadcaster.clone()`.
    let broadcaster = Arc::new(BroadcastHub::new(config.ws_broadcast_capacity));
    let ingest_writer_task = tokio::spawn(ingest::writer::run(
        ingest_rx,
        pools.writer.clone(),
        broadcaster.clone(),
        shutdown_requested.clone(),
    ));
    let ingest_listener_task = tokio::spawn(ingest::listener::run_bound(
        ingest_listener,
        config.ingest_sock_path.clone(),
        ingest_tx,
        shutdown_requested.clone(),
        adapter,
    ));

    let ws_semaphore = Arc::new(tokio::sync::Semaphore::new(config.ws_max_connections));
    let ws_config = WsConfig {
        ping_interval: config.ws_ping_interval,
        pong_timeout: config.ws_pong_timeout,
        coalesce_window: config.ws_broadcast_coalesce_window,
    };

    let state = AppState {
        db: pools.clone(),
        migrations_complete: migrations_complete.clone(),
        shutdown_requested: shutdown_requested.clone(),
        ws_close_requested: ws_close_requested.clone(),
        bearer,
        started_at_ms,
        broadcaster,
        ws_semaphore: ws_semaphore.clone(),
        ws_config,
    };
    let router = api::router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    let local_addr = listener.local_addr().context("listener.local_addr")?;
    // WARN, not INFO: the bound address is operationally important and must be
    // visible at the default verbosity (`error`). See P20 in story 1.2 review.
    tracing::warn!(addr = %local_addr, "daemon listening");

    let serve = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(shutdown_requested.clone()))
        .into_future();
    tokio::pin!(serve);
    let mut serve_result = None;

    tokio::select! {
        result = &mut serve => {
            shutdown_requested.cancel();
            serve_result = Some(result);
        }
        _ = shutdown_requested.cancelled() => {}
    }

    // Ensure ingest shuts down before the final lifecycle marker is written:
    // queued accepted events must either be persisted before RecordingEnded or
    // rejected by a closed queue, not silently aborted by runtime teardown.
    shutdown_requested.cancel();
    match ingest_listener_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!(error = ?e, "ingest listener exited with error"),
        Err(e) => tracing::error!(error = ?e, "ingest listener task join failed"),
    }
    if let Err(e) = ingest_writer_task.await {
        tracing::error!(error = ?e, "ingest writer task join failed");
    }

    ws_close_requested.cancel();
    let ws_drain_timed_out = wait_for_ws_connection_drain(
        ws_semaphore.clone(),
        config.ws_max_connections,
        config.shutdown_drain_timeout,
    )
    .await
    .is_err();

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
    pools.reader.close();
    pools.writer.close();

    let serve_result = match serve_result {
        Some(result) => result,
        None if !ws_drain_timed_out => serve.await,
        None => return Ok(()),
    };
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
