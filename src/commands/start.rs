//! `bowerbird start` — spawn the daemon detached if it is not already running
//! and wait for `/healthz` to return 200 within 2 seconds (NFR3).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;

use super::daemon::{self, wait_for_server_json, HealthzOutcome, StartOutcome};

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
    start_daemon(&bowerbird_dir)
}

/// macOS: when a LaunchAgent is registered (Story 5.9), `bowerbird start` drives
/// it through launchd instead of spawning an unsupervised `setsid` daemon that
/// would fight launchd's lifecycle (Story 5.9 review F2). If no plist exists
/// (dev box, or `install` was never run) we fall back to the detached spawn so
/// `bowerbird start` still works standalone.
#[cfg(target_os = "macos")]
fn start_daemon(bowerbird_dir: &std::path::Path) -> anyhow::Result<()> {
    use super::launch_agent;

    let plist_path = launch_agent::plist_path()?;
    if !plist_path.exists() {
        return start_detached(bowerbird_dir);
    }

    // Probe the *effective* socket (honoring BOWERBIRD_INGEST_SOCK, matching the
    // path install embeds in the plist) so a daemon on a custom socket is not
    // missed (F2).
    let ingest_sock = super::effective_ingest_sock(bowerbird_dir);
    let loaded = launch_agent::launch_agent_loaded()?;

    // If a daemon is already accepting on the socket, do NOT bootstrap/kickstart
    // over it: a manual daemon can satisfy the socket probe while the loaded
    // agent is down/stale, and bootstrapping a competing process would fail the
    // singleton lock and crash-loop under KeepAlive. We also can't prove launchd
    // owns this PID, so the message stays neutral rather than claiming launchd
    // ownership (F2).
    if super::daemon_is_up(&ingest_sock) {
        let pid = daemon::read_pid(&bowerbird_dir.join("bowerbird.pid"))
            .ok()
            .flatten();
        match pid {
            Some(p) => println!("daemon already running (pid {p})"),
            None => println!("daemon already running"),
        }
        return Ok(());
    }

    // Socket is down, so no daemon is competing for the singleton lock. Drive
    // launchd: kickstart a loaded-but-down agent (a clean `bowerbird stop`
    // leaves it down under KeepAlive={SuccessfulExit=false}), or bootstrap one
    // that is merely registered.
    if loaded {
        launch_agent::kickstart_launch_agent().context("kickstart the registered launch agent")?;
    } else {
        launch_agent::bootstrap_launch_agent(&plist_path)
            .context("bootstrap the registered launch agent")?;
    }
    println!("started bowerbird-daemon via launchd");
    wait_for_ready(bowerbird_dir, "daemon started via launchd")
}

#[cfg(not(target_os = "macos"))]
fn start_daemon(bowerbird_dir: &std::path::Path) -> anyhow::Result<()> {
    start_detached(bowerbird_dir)
}

/// The detached `setsid` spawn path — the lifecycle on Linux, and the macOS
/// fallback when no LaunchAgent is registered.
fn start_detached(bowerbird_dir: &std::path::Path) -> anyhow::Result<()> {
    match daemon::start_daemon_detached(bowerbird_dir)? {
        StartOutcome::AlreadyRunning => {
            let pid = daemon::read_pid(&bowerbird_dir.join("bowerbird.pid"))
                .ok()
                .flatten();
            match pid {
                Some(p) => {
                    println!("daemon already running (pid {p}); use 'bowerbird stop' to stop it")
                }
                None => println!("daemon already running; use 'bowerbird stop' to stop it"),
            }
            Ok(())
        }
        StartOutcome::Spawned { pid } => {
            println!("started bowerbird-daemon (pid {pid})");
            wait_for_ready(bowerbird_dir, &format!("daemon spawned (pid {pid})"))
        }
    }
}

/// Poll for the daemon's `server.json` then its `/healthz`, up to the readiness
/// budget. `who` describes how the daemon was started, for the timeout errors.
fn wait_for_ready(bowerbird_dir: &std::path::Path, who: &str) -> anyhow::Result<()> {
    let deadline = Instant::now() + READINESS_BUDGET;

    let info = match wait_for_server_json(bowerbird_dir, deadline, SERVER_JSON_POLL_INTERVAL) {
        Some(i) => i,
        None => {
            // Do NOT kill the daemon on timeout — the user may want to
            // investigate it post-mortem. Crash logs live in
            // ~/.bowerbird/crash-*.log per the panic-hook contract.
            anyhow::bail!(
                "{who} but server.json did not appear within {}ms; \
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
        "{who} but failed to become healthy within {}ms; \
         see ~/.bowerbird/ for crash logs",
        READINESS_BUDGET.as_millis()
    );
}
