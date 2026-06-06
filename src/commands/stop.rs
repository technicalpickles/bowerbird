//! `bowerbird stop` — send SIGTERM to the running daemon via the singleton PID
//! file, fall back to SIGKILL after a 10s graceful drain window.

use clap::Args;

use super::daemon::{self, StopOutcome};

/// No flags in v1. The 10s SIGKILL-escalation timing is fixed to match
/// `bowerbird uninstall`'s budget; if `--force` or `--timeout` is ever needed,
/// add then.
#[derive(Args)]
pub struct StopArgs {}

pub fn run(_args: StopArgs) -> anyhow::Result<()> {
    stop_daemon()
}

/// macOS (Story 5.9 review pass-6 F1): stop the daemon through its lifecycle
/// OWNER. A plain PID-file SIGTERM relies on the daemon exiting 0 so
/// `KeepAlive={SuccessfulExit=false}` leaves it down — but the 10s SIGKILL
/// escalation for a *wedged* daemon is a non-zero exit, which launchd reads as a
/// crash and RESTARTS, so `bowerbird stop` would print "stopped" while the daemon
/// bounced right back. When a LaunchAgent is loaded, boot it out instead: launchd
/// terminates the daemon (SIGTERM, then SIGKILL on its own timeout) AND removes
/// the job from the domain, so `KeepAlive` has nothing left to restart — graceful
/// or wedged, it stays down. This is session-scoped: the plist stays on disk, so
/// the next login still starts it via `RunAtLoad` (the supervision contract is
/// intact), and `bowerbird start` re-bootstraps it on demand.
#[cfg(target_os = "macos")]
fn stop_daemon() -> anyhow::Result<()> {
    use super::launch_agent;
    use anyhow::Context;

    // Probe launchd by LABEL, not by plist presence (Story 5.9 review pass-7 F1):
    // `uninstall --no-stop` removes the plist while documenting that an
    // already-loaded in-session agent keeps being supervised by launchd until
    // logout or an explicit bootout (`uninstall.rs` --no-stop note). A plist-gated
    // check would skip launchd here, fall to the PID-file path, and a forced
    // SIGKILL escalation would be bounced right back by `KeepAlive` (the pass-6 F1
    // bug). `launch_agent_load_state()` / `bootout_launch_agent()` address the
    // agent by `gui/<uid>/<label>`, so they still work with the plist gone — the
    // modern `bootout` needs no file (only the legacy `unload` fallback reads it).
    let plist_path = launch_agent::plist_path()?;
    match launch_agent::launch_agent_load_state() {
        launch_agent::LoadState::Loaded => {
            launch_agent::bootout_launch_agent(&plist_path)
                .context("boot the bowerbird LaunchAgent out to stop the daemon")?;
            println!(
                "daemon stopped (launchd supervision paused until next login; \
                 run `bowerbird start` to resume now)"
            );
            return Ok(());
        }
        // Not loaded (a clean prior stop, or never bootstrapped): nothing for
        // launchd to stop. Fall through to the PID-file path, which still catches a
        // manual / pre-5.9 daemon.
        launch_agent::LoadState::NotLoaded => {}
        // Cannot verify launchd's state (unverifiable `launchctl print`). A
        // launchd-supervised daemon force-stopped via PID-file SIGKILL would be
        // bounced back by `KeepAlive` (pass-6 F1), so PREFER a bootout first: it
        // addresses the agent by label, no-ops cleanly if the agent is actually
        // absent, and tries modern then legacy `unload`. Then STILL fall through to
        // the PID-file stop to catch a manual / pre-5.9 daemon. If the bootout
        // cannot be proven, warn that a forced stop may be restarted by launchd
        // (pass-7 F1).
        launch_agent::LoadState::Unknown => {
            if let Err(e) = launch_agent::bootout_launch_agent(&plist_path) {
                eprintln!(
                    "warning: could not boot the LaunchAgent out ({e:#}); falling back to a \
                     PID-file stop — if launchd is supervising the daemon, a forced (SIGKILL) \
                     stop may be restarted by KeepAlive (run `bowerbird uninstall` to remove \
                     supervision)"
                );
            }
        }
    }

    stop_via_pid_file()
}

#[cfg(not(target_os = "macos"))]
fn stop_daemon() -> anyhow::Result<()> {
    stop_via_pid_file()
}

/// PID-file SIGTERM → SIGKILL stop. The lifecycle owner on Linux, and the macOS
/// fallback when no LaunchAgent is loaded (manual / pre-5.9 daemon).
fn stop_via_pid_file() -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;
    match daemon::stop_daemon_via_pid_file(&bowerbird_dir)? {
        StopOutcome::NotRunning => {
            // Exact wording is contracted with `tests/cli_lifecycle.rs`.
            println!("daemon not running (no pid file); nothing to stop");
        }
        StopOutcome::Stopped => {
            println!("daemon stopped");
        }
        StopOutcome::Escalated => {
            // Exit 0 even on escalation: the user wanted the daemon stopped;
            // the SIGKILL warning has already been printed to stderr by the
            // helper. Mirrors `bowerbird uninstall`'s behavior.
            println!("daemon stopped (after SIGKILL escalation)");
        }
    }
    Ok(())
}
