use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use super::daemon;
#[cfg(not(target_os = "macos"))]
use super::daemon::StopOutcome;

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

    teardown_supervision(args.no_stop)?;
    Ok(())
}

/// macOS (Story 5.9 / ADR 0007): boot the LaunchAgent out (launchd terminates
/// the daemon) and remove the plist. `--no-stop` skips the bootout but still
/// removes the plist (AC 7), mirroring install's `--no-start`.
///
/// A `bootout` failure is now FATAL (review pass 2, F3): `bootout_launch_agent`
/// already normalizes the already-unloaded case to success, so a remaining error
/// is a *real* failure (or an unverifiable launchd state) that would leave a
/// loaded agent supervising the current session. Downgrading it to a warning and
/// then printing "removed login registration" was a lie — we propagate instead,
/// and we do NOT remove the plist (so a retry still has the registration to act
/// on). The manual-daemon PID-file fallback stays non-fatal (it mirrors the
/// daemon-stop posture and is not about launchd ownership).
#[cfg(target_os = "macos")]
fn teardown_supervision(no_stop: bool) -> anyhow::Result<()> {
    use super::launch_agent;
    use std::path::PathBuf;

    let plist_path = launch_agent::plist_path()?;

    if !no_stop {
        // Resolve the data dir + ingest socket from the LaunchAgent's REGISTERED
        // env (read now, while the plist still exists — it is removed below), not
        // the current CLI env (Story 5.9 review pass-4 #3). If install registered
        // `BOWERBIRD_DATA_DIR=/A` or `BOWERBIRD_INGEST_SOCK=/A/custom.sock`, a
        // later uninstall run without those env vars would otherwise check the
        // wrong PID file / socket and miss a surviving manual / pre-5.9 daemon.
        // Fall back to the CLI env only when the plist carries no such entry.
        let registered = launch_agent::registered_plist_env(&plist_path);
        let reg = |k: &str| {
            registered
                .iter()
                .find(|(rk, _)| rk == k)
                .map(|(_, v)| v.clone())
        };
        let data_dir = match reg("BOWERBIRD_DATA_DIR") {
            Some(d) => PathBuf::from(d),
            None => super::resolve_bowerbird_dir()?,
        };
        let ingest_sock = reg("BOWERBIRD_INGEST_SOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|| super::effective_ingest_sock(&data_dir));

        // Boot the launchd-owned daemon out — fatal on a real/unverifiable
        // failure so we never claim success while the agent lingers (F3 pass-2).
        launch_agent::bootout_launch_agent(&plist_path)
            .context("boot the bowerbird LaunchAgent out of launchd")?;
        // Also catch a manually-started / pre-5.9 PID-file daemon launchd never
        // owned, so uninstall actually leaves no daemon running. Non-fatal: this
        // mirrors the historical daemon-stop posture and a leftover manual daemon
        // is not a launchd-registration residue.
        if let Err(e) = daemon::stop_daemon_via_pid_file(&data_dir) {
            eprintln!("warning: {e:#}");
        }

        // Re-probe the effective socket after ANY stop outcome (Story 5.9 review
        // pass-4 #4). A stale/wrong PID file, PID reuse, or a concurrent restart
        // can make `stop_daemon_via_pid_file` report `Stopped`/`Escalated` (it
        // killed *something*) while the daemon socket is still accepting — the
        // previous code only probed when the outcome was NOT `Stopped`/`Escalated`
        // (pass-3 F5), so it could still claim clean removal while a manual daemon
        // kept running. The probe is cheap and only warns when a daemon is
        // actually still live; non-fatal, matching the manual-daemon stop posture
        // (settings.json is already un-merged).
        if super::daemon_is_up(&ingest_sock) {
            eprintln!(
                "warning: a daemon is still accepting on {} after uninstall; the LaunchAgent \
                 registration was removed but a manual / pre-5.9 daemon is still running and \
                 no bowerbird PID file could stop it — stop that process manually",
                ingest_sock.display()
            );
        }
    }

    launch_agent::remove_launch_agent_plist(&plist_path)
        .with_context(|| format!("remove launch agent plist {}", plist_path.display()))?;
    println!(
        "removed bowerbird-daemon login registration ({})",
        plist_path.display()
    );
    Ok(())
}

/// Non-macOS: keep today's PID-file SIGTERM stop. `--no-stop` leaves the daemon
/// running.
#[cfg(not(target_os = "macos"))]
fn teardown_supervision(no_stop: bool) -> anyhow::Result<()> {
    if no_stop {
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

#[cfg(not(target_os = "macos"))]
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
