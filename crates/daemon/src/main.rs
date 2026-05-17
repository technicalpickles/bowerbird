// Several items in these modules are exercised by contract tests and reserved
// for upcoming stories (`projection::session::write` is invoked from the
// ingest path in Story 1.3, `queries::INSERT_EVENT`/`UPSERT_PROJECTION` back
// that path, and `Error` variants cover failure modes the daemon will surface
// once those paths land). `#[allow(dead_code)]` is intentional during 1.2 to
// keep the public surface intact without scaffolding callers that violate
// YAGNI.
#![allow(dead_code)]

mod api;
mod config;
mod crash;
mod db;
mod error;
mod logging;
mod projection;
mod state;

use std::sync::Arc;

use clap::Parser;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::state::AppState;

#[derive(Debug, Parser)]
#[command(name = "bowerbird-daemon", version, about)]
struct Cli {
    /// Increase log verbosity. `-v` enables `info`, `-vv` enables `debug`.
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    crash::install_panic_hook();
    logging::init(cli.verbose);

    if let Err(e) = run(cli).await {
        eprintln!("daemon failed: {e}");
        std::process::exit(1);
    }
}

async fn run(_cli: Cli) -> error::Result<()> {
    let cfg = Config::from_env()?;
    tokio::fs::create_dir_all(&cfg.data_dir).await?;

    let pools = match db::build_pools(&cfg).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pool build failed: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = db::run_migrations(&pools).await {
        eprintln!("migration failed: {e}");
        std::process::exit(1);
    }

    let shutdown = CancellationToken::new();
    let state = Arc::new(AppState {
        db: pools,
        shutdown: shutdown.clone(),
    });

    let app = api::router(state.clone());
    let listener = TcpListener::bind(cfg.tcp_addr)
        .await
        .map_err(error::Error::Bind)?;
    let bound = listener.local_addr().map_err(error::Error::Bind)?;
    tracing::info!("daemon started; bound {bound}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state.shutdown.clone()))
        .await?;

    state.db.close().await;
    tracing::info!("shutdown complete");
    Ok(())
}

async fn shutdown_signal(token: CancellationToken) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = token.cancelled() => {}
    }
    token.cancel();
}
