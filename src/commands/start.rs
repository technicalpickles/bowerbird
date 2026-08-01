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

    // Reject an ingest socket path that cannot fit a Unix-domain socket address
    // before asking launchd to (re)start the daemon (Story 5.9 review pass-7 F4):
    // the daemon would fail to bind it after launchd starts the job and crash-loop
    // under KeepAlive={SuccessfulExit=false}, while `start` only reported a bare
    // readiness timeout.
    launch_agent::ensure_ingest_sock_len(&ingest_sock)?;

    // Probe the registered socket BEFORE asking launchd to take any ACTION
    // (Story 5.9 review pass-4 #1): an unverifiable `launchctl print` must never
    // fail `start` when the daemon is already up. But a live socket alone does NOT
    // mean the daemon is *supervised* — a manual / pre-5.9 daemon can accept on the
    // socket while launchd has no agent loaded, leaving the install unsupervised
    // even though AC 5 says `start` replaces the manual spawn when a LaunchAgent is
    // registered. So when the socket is live, decide by launchd's load state
    // (pass-6 F3):
    if super::daemon_is_up(&ingest_sock) {
        match launch_agent::launch_agent_load_state() {
            // A loaded label proves the job is REGISTERED, not that launchd's
            // process is the one accepting on the socket (Story 5.9 review pass-7
            // F5): the agent can stay loaded after a clean daemon exit while a
            // manual / pre-5.9 daemon binds the registered socket, leaving that
            // daemon without crash-restart even though we'd report "under launchd".
            // Proof of supervision = launchd is RUNNING the job AND its pid is the
            // singleton holder. Verify both; otherwise migrate (fall through).
            launch_agent::LoadState::Loaded => match launch_agent::launch_agent_running_pid() {
                Ok(Some(lpid))
                    if daemon::singleton_state(&effective_dir)
                        == daemon::SingletonState::Held(lpid) =>
                {
                    println!("daemon already running under launchd (pid {lpid})");
                    return Ok(());
                }
                // Loaded but launchd is not running the job, or its pid does not
                // match the singleton holder: a manual daemon owns the socket.
                // Migrate it into launchd ownership (fall through to the
                // stop-then-(re)start path below).
                Ok(_) => {
                    println!(
                        "a daemon is accepting on {} but launchd is not running the registered \
                         job (or its pid does not match the singleton holder); migrating it to \
                         launchd supervision",
                        ingest_sock.display()
                    );
                }
                // Cannot read launchd's pid (unverifiable `launchctl print`). Do NOT
                // kill a working daemon on a guess; leave it running and warn that
                // supervision could not be confirmed (preserves the pass-4 #1 /
                // pass-6 F3 unverifiable-print success path).
                Err(_) => {
                    eprintln!(
                        "warning: a daemon is already accepting on {} but launchd's pid for the \
                         registered LaunchAgent could not be verified; leaving the running daemon \
                         in place — it may not be under launchd supervision (check `launchctl \
                         print gui/$(id -u)/{}`)",
                        ingest_sock.display(),
                        launch_agent::launch_agent_label()
                    );
                    println!("daemon already running");
                    return Ok(());
                }
            },
            // Can't prove launchd's state (unverifiable `launchctl print`). Do NOT
            // kill a working daemon to migrate it on a guess; leave it running and
            // warn that supervision could not be confirmed (pass-6 F3 keeps the
            // pass-4 #1 success path for the unverifiable case).
            launch_agent::LoadState::Unknown => {
                eprintln!(
                    "warning: a daemon is already accepting on {} but launchd's state for the \
                     registered LaunchAgent could not be verified; leaving the running daemon in \
                     place — it may not be under launchd supervision (check `launchctl print \
                     gui/$(id -u)/{}`)",
                    ingest_sock.display(),
                    launch_agent::launch_agent_label()
                );
                println!("daemon already running");
                return Ok(());
            }
            // A manual daemon is accepting but launchd does NOT supervise it.
            // Migrate it into launchd ownership: fall through to stop it, then
            // bootstrap (AC 5).
            launch_agent::LoadState::NotLoaded => {
                println!(
                    "a daemon is running but not under launchd; migrating it to launchd supervision"
                );
            }
        }
    }

    // The socket is down OR a manual daemon must be migrated. Either way a FREE
    // singleton is required before the launchd start. The daemon takes the
    // singleton (a flock on `bowerbird.pid`) BEFORE it binds the socket, so a
    // holder can own the lock with the socket down (wedged before bind, or bound to
    // a previous socket path). launchd starting (or `kickstart -k` restarting) over
    // such a holder would fail the singleton acquisition and crash-loop under
    // KeepAlive={SuccessfulExit=false}. Detect the holder by the FLOCK, not pid-file
    // content (pass-6 F4): `stop_daemon_via_pid_file` SIGTERMs a live holder and
    // FAILS clearly when the singleton is held but the pid is unidentifiable, so we
    // never signal a reused pid nor bootstrap over an unmanageable holder
    // (Story 5.9 review pass-5 F2 / pass-6 F4).
    if daemon::singleton_state(&effective_dir) != daemon::SingletonState::Free {
        daemon::stop_daemon_via_pid_file(&effective_dir)
            .context("stop the unsupervised daemon holding the singleton before launchd start")?;
        if daemon::singleton_state(&effective_dir) != daemon::SingletonState::Free {
            anyhow::bail!(
                "a daemon is still holding the singleton for {} after attempting to stop it; \
                 launchd cannot start over it (it would fail the singleton lock and crash-loop) \
                 — stop that daemon, then re-run `bowerbird start`",
                effective_dir.display()
            );
        }
    }
    // A migrated manual daemon (socket was live, no stoppable singleton holder)
    // could still be accepting; bootstrapping launchd over it would crash-loop, so
    // fail clearly if the socket survived the stop (pass-6 F3).
    if super::daemon_is_up(&ingest_sock) {
        anyhow::bail!(
            "a daemon is still accepting on {} but no bowerbird singleton holder could be stopped; \
             refusing to start launchd over a daemon it cannot manage — stop that daemon, then \
             re-run `bowerbird start`",
            ingest_sock.display()
        );
    }

    // Revalidate the registered daemon executable before handing the agent to
    // launchd (pass-6 F6): `install --no-start` may register a plist before the
    // binary exists, and launchd would otherwise retry a missing/non-executable
    // job forever behind a bare readiness timeout. Surface the same clear error
    // `install` uses on the bootstrap path. A stub plist with no parseable
    // ProgramArguments (None) skips the check.
    if let Some(program) = launch_agent::registered_plist_program(&plist_path) {
        launch_agent::ensure_daemon_launchable(&program)
            .context("the registered LaunchAgent daemon binary is not launchable")?;
    }

    // `server.json` is only a hint and can be stale after an unclean exit
    // (`crates/daemon/src/server_file.rs`) — the singleton + ingest-socket probe is
    // the real liveness proof. Remove any stale copy before starting so the
    // readiness wait below binds to the freshly-started daemon's file, not an old
    // port (which could falsely time out, or report readiness against an unrelated
    // service) (pass-6 F5).
    let _ = std::fs::remove_file(daemon::server_json_path(&effective_dir));

    // Now — and only now — query launchd to decide how to start it: kickstart a
    // loaded-but-down agent (a clean `bowerbird stop` leaves it down under
    // KeepAlive={SuccessfulExit=false}), or bootstrap one that is merely
    // registered. TRI-STATE (pass-6 F2): an unverifiable `print` must NOT bail
    // before `bootstrap_launch_agent`'s own modern-then-legacy `load -w` fallback,
    // so Unknown bootstraps (which on an old launchctl falls back to `load -w`).
    match launch_agent::launch_agent_load_state() {
        launch_agent::LoadState::Loaded => {
            launch_agent::kickstart_launch_agent()
                .context("kickstart the registered launch agent")?;
        }
        launch_agent::LoadState::NotLoaded | launch_agent::LoadState::Unknown => {
            launch_agent::bootstrap_launch_agent(&plist_path)
                .context("bootstrap the registered launch agent")?;
        }
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
