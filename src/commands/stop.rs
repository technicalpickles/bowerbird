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
    let bowerbird_dir = super::resolve_bowerbird_dir()?;
    match daemon::stop_daemon_via_pid_file(&bowerbird_dir)? {
        StopOutcome::NotRunning => {
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
