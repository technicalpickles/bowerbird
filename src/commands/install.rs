use std::path::PathBuf;
use std::process::Stdio;

use anyhow::Context;
use clap::Args;

#[derive(Args)]
pub struct InstallArgs {
    /// Override the Claude Code settings.json path (defaults to
    /// ~/.claude/settings.json; honor `BOWERBIRD_CLAUDE_SETTINGS` env too).
    #[arg(long)]
    settings: Option<PathBuf>,
    /// Skip starting the daemon after merging settings.json. Useful for
    /// scripted setups where the user manages daemon lifecycle separately.
    #[arg(long)]
    no_start: bool,
}

pub fn run(args: InstallArgs) -> anyhow::Result<()> {
    let settings_path = super::resolve_claude_settings(args.settings)?;
    let outcome = adapter_claude::install(&settings_path)
        .with_context(|| format!("install hook into {}", settings_path.display()))?;

    if outcome.created {
        println!(
            "created {} and installed bowerbird hook entries",
            settings_path.display()
        );
    } else if outcome.hook_kinds_added.is_empty() {
        println!(
            "bowerbird hook entries already present in {}; no settings.json change",
            settings_path.display()
        );
    } else {
        println!(
            "added bowerbird hook entries to {} for: {}",
            settings_path.display(),
            outcome.hook_kinds_added.join(", ")
        );
    }

    if args.no_start {
        return Ok(());
    }

    start_daemon_if_needed()?;
    Ok(())
}

fn start_daemon_if_needed() -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;
    let ingest_sock = bowerbird_dir.join("ingest.sock");

    if super::daemon_is_up(&ingest_sock) {
        println!("daemon already running; skipping start");
        return Ok(());
    }

    let bin = super::resolve_daemon_bin();
    let child = spawn_detached(&bin).with_context(|| {
        format!(
            "spawn daemon binary `{}` (set BOWERBIRD_DAEMON_BIN or ensure bowerbird-daemon is on PATH)",
            bin.display()
        )
    })?;
    println!("started bowerbird-daemon (pid {})", child);
    Ok(())
}

#[cfg(unix)]
fn spawn_detached(bin: &std::path::Path) -> anyhow::Result<u32> {
    use std::os::unix::process::CommandExt;

    // Detach the child so it survives the install process exiting:
    //  - `setsid` makes it a new session leader (no controlling terminal).
    //  - stdio is rerouted to /dev/null so it does not write to our terminal.
    // The daemon's own panic hook + tracing already handles its log output to
    // ~/.bowerbird and stderr.
    let child = unsafe {
        std::process::Command::new(bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(|| {
                // Become a session leader, detached from any controlling tty.
                // setsid returns -1 on failure (e.g. already a process group
                // leader); treat that as recoverable since the parent process
                // group still owns the child until we exit.
                let _ = nix_setsid();
                Ok(())
            })
            .spawn()?
    };
    Ok(child.id())
}

#[cfg(unix)]
fn nix_setsid() -> std::io::Result<()> {
    // Avoid pulling `nix` into the CLI's dep tree (the daemon already uses it
    // but the CLI is intentionally lightweight). libc's setsid is a single
    // syscall with a stable signature.
    extern "C" {
        fn setsid() -> libc::pid_t;
    }
    let r = unsafe { setsid() };
    if r == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn spawn_detached(bin: &std::path::Path) -> anyhow::Result<u32> {
    // Non-unix is a documented scope cut (project-context.md "no Windows
    // support"). A future port would use CREATE_NEW_PROCESS_GROUP here.
    let child = std::process::Command::new(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(child.id())
}
