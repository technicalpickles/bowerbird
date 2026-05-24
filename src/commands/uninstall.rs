use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use super::daemon::{self, StopOutcome};

#[derive(Args)]
pub struct UninstallArgs {
    /// Override the Claude Code settings.json path (defaults to
    /// ~/.claude/settings.json; honor `BOWERBIRD_CLAUDE_SETTINGS` env too).
    #[arg(long)]
    settings: Option<PathBuf>,
    /// Skip stopping the daemon after cleaning settings.json. Useful when the
    /// user wants to leave a daemon running while removing only the hook.
    #[arg(long)]
    no_stop: bool,
}

pub fn run(args: UninstallArgs) -> anyhow::Result<()> {
    let settings_path = super::resolve_claude_settings(args.settings)?;
    let outcome = adapter_claude::uninstall(&settings_path)
        .with_context(|| format!("uninstall hook from {}", settings_path.display()))?;

    if !outcome.existed {
        println!(
            "{} did not exist; no settings.json change needed",
            settings_path.display()
        );
    } else if outcome.hook_kinds_removed.is_empty() {
        println!(
            "no bowerbird hook entries found in {}; settings.json unchanged",
            settings_path.display()
        );
    } else {
        println!(
            "removed bowerbird hook entries from {} for: {}",
            settings_path.display(),
            outcome.hook_kinds_removed.join(", ")
        );
    }

    if args.no_stop {
        return Ok(());
    }

    // Daemon-stop failures are non-fatal: settings.json is already updated and
    // refusing to exit 0 would block the user from a clean reinstall. We do
    // print a warning so the operator can investigate.
    if let Err(e) = stop_daemon_if_running() {
        eprintln!("warning: {e:#}");
    }
    Ok(())
}

fn stop_daemon_if_running() -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;
    match daemon::stop_daemon_via_pid_file(&bowerbird_dir)? {
        StopOutcome::NotRunning => {
            println!("daemon not running (no pid file); nothing to stop");
        }
        StopOutcome::Stopped | StopOutcome::Escalated => {
            // The SIGKILL-escalation warning was already printed to stderr by
            // the helper. Wording matches the Story 3.1 message that
            // `tests/cli_install.rs` asserts against — do NOT change this
            // string without updating that test in lockstep.
            println!("daemon stopped");
        }
    }
    Ok(())
}
