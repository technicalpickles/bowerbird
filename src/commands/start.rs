//! `bowerbird start` — spawn the daemon detached if it is not already running
//! and wait for `/healthz` to return 200 within 2 seconds (NFR3).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;

use super::daemon::{
    self, wait_for_server_json, HealthzOutcome, StartOutcome,
};

/// No flags in v1. `--detach` is implicit (the daemon is always detached;
/// foreground is a `cargo run -p bowerbird-daemon` workflow for development,
/// not a CLI surface).
#[derive(Args)]
pub struct StartArgs {}

/// AC #1: cold-start readiness window. Together with the 250ms per-attempt
/// budget below this gives ~8 polls before timeout.
const READINESS_BUDGET: Duration = Duration::from_millis(2_000);
const HEALTHZ_PER_ATTEMPT: Duration = Duration::from_millis(250);
/// Polling cadence while waiting for `server.json` to appear.
const SERVER_JSON_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub fn run(_args: StartArgs) -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;

    match daemon::start_daemon_detached(&bowerbird_dir)? {
        StartOutcome::AlreadyRunning => {
            let pid = super::daemon::read_pid(&bowerbird_dir.join("bowerbird.pid"))
                .ok()
                .flatten();
            match pid {
                Some(p) => println!("daemon already running (pid {p}); use 'bowerbird stop' to stop it"),
                None => println!("daemon already running; use 'bowerbird stop' to stop it"),
            }
            return Ok(());
        }
        StartOutcome::Spawned { pid } => {
            println!("started bowerbird-daemon (pid {pid})");
            wait_for_ready(&bowerbird_dir, pid)?;
        }
    }
    Ok(())
}

fn wait_for_ready(bowerbird_dir: &std::path::Path, pid: u32) -> anyhow::Result<()> {
    let deadline = Instant::now() + READINESS_BUDGET;

    let info = match wait_for_server_json(bowerbird_dir, deadline, SERVER_JSON_POLL_INTERVAL) {
        Some(i) => i,
        None => {
            // Do NOT kill the spawned daemon on timeout — the user may want to
            // investigate it post-mortem. Crash logs live in
            // ~/.bowerbird/crash-*.log per the panic-hook contract.
            anyhow::bail!(
                "daemon spawned (pid {pid}) but server.json did not appear within {}ms; \
                 see ~/.bowerbird/ for crash logs",
                READINESS_BUDGET.as_millis()
            );
        }
    };

    let addr: SocketAddr = info
        .bind_addr
        .parse()
        .with_context(|| format!("parse server.json bind_addr {:?}", info.bind_addr))?;

    while Instant::now() < deadline {
        match daemon::http_get_healthz(addr, HEALTHZ_PER_ATTEMPT) {
            HealthzOutcome::Ok => {
                println!("daemon ready at http://{addr}");
                return Ok(());
            }
            HealthzOutcome::Unhealthy | HealthzOutcome::Unreachable => {
                // Sub-attempt sleep so we don't busy-spin. The connect-timeout
                // already consumed up to per_attempt; this is the cool-down
                // before retrying.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    anyhow::bail!(
        "daemon spawned (pid {pid}) but failed to become healthy within {}ms; \
         see ~/.bowerbird/ for crash logs",
        READINESS_BUDGET.as_millis()
    );
}
