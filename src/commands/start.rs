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
/// would fight launchd's lifecycle (Story 5.9 review pass-2 F2). If no plist
/// exists (dev box, or `install` was never run) we fall back to the detached
/// spawn so `bowerbird start` still works standalone.
#[cfg(target_os = "macos")]
fn start_daemon(bowerbird_dir: &std::path::Path) -> anyhow::Result<()> {
    use super::launch_agent;
    use std::path::PathBuf;

    let plist_path = launch_agent::plist_path()?;
    if !plist_path.exists() {
        return start_detached(bowerbird_dir);
    }

    // launchd starts the daemon with the plist's `EnvironmentVariables`, which
    // win for the launchd-spawned process and may differ from the current CLI
    // env. Probing/waiting against the CLI env would time out while the daemon
    // comes up where launchd actually put it, so resolve the data dir + ingest
    // socket from the *registered* plist env (Story 5.9 review pass-3 F2).
    let registered = launch_agent::registered_plist_env(&plist_path);
    let reg = |k: &str| {
        registered
            .iter()
            .find(|(rk, _)| rk == k)
            .map(|(_, v)| v.clone())
    };
    // When the plist carries no `BOWERBIRD_DATA_DIR` (a legacy / malformed /
    // no-env registration), launchd does NOT inherit the current shell env, so
    // it starts the daemon in launchd's own default — `$HOME/.bowerbird`, the
    // daemon's own fallback — NOT the current CLI data dir. Falling back to the
    // CLI dir here would make the probe/readiness wait look in a directory the
    // launchd daemon never uses, producing a false readiness timeout (Story 5.9
    // review pass-4 #2). Use the launchd default explicitly; never the current
    // CLI env for a launchd-managed process.
    let effective_dir = match reg("BOWERBIRD_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => super::home_dir()?.join(".bowerbird"),
    };
    // The launchd daemon uses the plist's `BOWERBIRD_INGEST_SOCK` when present,
    // else `<effective_dir>/ingest.sock` — NOT the current CLI env (which the
    // launchd process never sees).
    let ingest_sock = reg("BOWERBIRD_INGEST_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| effective_dir.join("ingest.sock"));

    // Probe the registered socket BEFORE asking launchd anything (Story 5.9
    // review pass-4 #1). If a daemon is already accepting, no launchd action is
    // needed — and an unverifiable `launchctl print` (which `launch_agent_loaded`
    // surfaces as `Err`, pass-3 F1) must NOT make `bowerbird start` fail when the
    // daemon is already up and no launchd query is even required. A manual daemon
    // can satisfy the socket probe while the loaded agent is down/stale, and
    // bootstrapping a competing process would fail the singleton lock and
    // crash-loop under KeepAlive; we also can't prove launchd owns this PID, so
    // the message stays neutral rather than claiming launchd ownership (pass-2 F2).
    if super::daemon_is_up(&ingest_sock) {
        let pid = daemon::read_pid(&effective_dir.join("bowerbird.pid"))
            .ok()
            .flatten();
        match pid {
            Some(p) => println!("daemon already running (pid {p})"),
            None => println!("daemon already running"),
        }
        return Ok(());
    }

    // The socket is down — but that does NOT prove no daemon competes for the
    // singleton lock. The daemon takes the singleton (flock + PID file) BEFORE it
    // binds the socket, so a manual / pre-5.9 daemon wedged before bind, or one
    // bound to a previous socket path, can hold the singleton while the registered
    // socket is unconnectable. launchd starting (or `kickstart -k` restarting)
    // over such a holder would fail the singleton acquisition and crash-loop under
    // KeepAlive={SuccessfulExit=false}. Stop a live PID-file holder first, and
    // fail clearly if it survives, so the launchd start has a free singleton
    // (Story 5.9 review pass-5 F2).
    if daemon::pid_holder_alive(&effective_dir) {
        daemon::stop_daemon_via_pid_file(&effective_dir)
            .context("stop the unsupervised daemon holding the singleton before launchd start")?;
        if daemon::pid_holder_alive(&effective_dir) {
            anyhow::bail!(
                "a daemon is still holding the singleton for {} after attempting to stop it; \
                 launchd cannot start over it (it would fail the singleton lock and crash-loop) \
                 — stop that daemon, then re-run `bowerbird start`",
                effective_dir.display()
            );
        }
    }

    // Now — and only now — query launchd to decide how to start it: kickstart a
    // loaded-but-down agent (a clean `bowerbird stop` leaves it down under
    // KeepAlive={SuccessfulExit=false}), or bootstrap one that is merely
    // registered. Deferring the launchd query to here is what lets an
    // unverifiable `print` no longer block the already-up path above.
    if launch_agent::launch_agent_loaded()? {
        launch_agent::kickstart_launch_agent().context("kickstart the registered launch agent")?;
    } else {
        launch_agent::bootstrap_launch_agent(&plist_path)
            .context("bootstrap the registered launch agent")?;
    }
    println!("started bowerbird-daemon via launchd");
    wait_for_ready(&effective_dir, "daemon started via launchd")
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
