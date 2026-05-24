use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;

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
    let pid_path = bowerbird_dir.join("bowerbird.pid");

    let pid = match read_pid(&pid_path)? {
        Some(p) => p,
        None => {
            println!("daemon not running (no pid file); nothing to stop");
            return Ok(());
        }
    };

    if !pid_alive(pid) {
        println!("daemon not running (stale pid {pid}); nothing to stop");
        return Ok(());
    }

    println!("sending SIGTERM to bowerbird-daemon (pid {pid})");
    send_signal(pid, libc::SIGTERM).context("send SIGTERM")?;

    // Wait up to twice the daemon's default drain timeout (5s). The drain
    // budget is exposed as `Config::shutdown_drain_timeout` but the CLI does
    // not link the daemon crate; doubling the documented default gives the
    // graceful path room without making `uninstall` feel hung.
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(pid) {
        if Instant::now() >= deadline {
            eprintln!(
                "daemon did not exit within 10s; escalating to SIGKILL (settings.json was \
                 already cleaned up)"
            );
            let _ = send_signal(pid, libc::SIGKILL);
            // SIGKILL delivery is asynchronous: the kernel reaps within
            // microseconds but `kill(pid, 0)` may briefly still report the
            // process as alive between signal delivery and reaping. Drain
            // for up to 1s so the post-loop liveness check does not bail
            // with a scary "still alive after SIGKILL" message that is
            // really just the kernel catching up.
            let kill_deadline = Instant::now() + Duration::from_secs(1);
            while pid_alive(pid) && Instant::now() < kill_deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if pid_alive(pid) {
        anyhow::bail!("daemon pid {pid} still alive after SIGKILL");
    }
    println!("daemon stopped");
    Ok(())
}

fn read_pid(pid_path: &Path) -> anyhow::Result<Option<i32>> {
    match std::fs::read_to_string(pid_path) {
        Ok(s) => match s.trim().parse::<i32>() {
            Ok(p) if p > 0 => Ok(Some(p)),
            _ => Ok(None),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context(format!("read {}", pid_path.display()))),
    }
}

fn pid_alive(pid: i32) -> bool {
    let r = unsafe { libc::kill(pid, 0) };
    if r == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // EPERM = exists but cannot signal; ESRCH = no such process.
    errno == libc::EPERM
}

fn send_signal(pid: i32, sig: libc::c_int) -> std::io::Result<()> {
    let r = unsafe { libc::kill(pid, sig) };
    if r == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
