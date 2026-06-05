use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use super::daemon;
#[cfg(not(target_os = "macos"))]
use super::daemon::StartOutcome;

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

    if outcome.legacy_upgrade_detected {
        println!(
            "note: detected pre-Story-5.2 hooks; re-running install to subscribe UserPromptSubmit"
        );
    }

    // Story 5.4 AC #1 — seed the adapter's tool-reactions TOML from the
    // bundled bytes. Skips silently when the user already has a copy.
    let bowerbird_dir = super::resolve_bowerbird_dir()?;
    match adapter_claude::seed_tool_reactions(&bowerbird_dir)
        .with_context(|| format!("seed tool-reactions.toml under {}", bowerbird_dir.display()))?
    {
        adapter_claude::SeedOutcome::Wrote => {
            println!(
                "seeded {}/adapters/claude/tool-reactions.toml from bundled defaults",
                bowerbird_dir.display()
            );
        }
        adapter_claude::SeedOutcome::AlreadyPresent => {
            // Hint goes to STDERR, not stdout: a script capturing `bowerbird
            // install` stdout (e.g. to parse the "seeded ..." line) must not
            // see this skip note interleaved. AC #1 also calls for a WARN-level
            // log here; the CLI binary has no tracing subscriber wired yet, so
            // the stderr line IS the operator signal for now. Wiring structured
            // logging into the CLI is tracked in deferred-work.md.
            eprintln!(
                "note: {}/adapters/claude/tool-reactions.toml already exists; leaving user copy in place",
                bowerbird_dir.display()
            );
        }
    }

    supervise_or_start(args.no_start)?;
    Ok(())
}

/// macOS (Story 5.9 / ADR 0007): register the daemon as a launchd LaunchAgent
/// (start-on-login + crash-restart) and bootstrap it unless `--no-start`.
/// launchd owns the lifecycle, so we do NOT also `setsid`-spawn the daemon —
/// the singleton PID lock would reject a double-start anyway.
#[cfg(target_os = "macos")]
fn supervise_or_start(no_start: bool) -> anyhow::Result<()> {
    use super::launch_agent;

    let plist_path = launch_agent::plist_path()?;
    let daemon_path = launch_agent::resolve_daemon_bin_absolute()?;
    let data_dir = super::resolve_bowerbird_dir()?;

    // One canonical absolute data dir for BOTH the plist log paths AND the
    // daemon's `BOWERBIRD_DATA_DIR` env (F4). Previously the env got a
    // canonicalized path while the log paths got the raw (possibly relative or
    // symlink-divergent) value, so logs and the DB/socket could land in
    // different directories; and the old `canonicalize(...).unwrap_or(data_dir)`
    // fallback could embed a *relative* `BOWERBIRD_DATA_DIR`, which the daemon
    // rejects at startup. `canonicalize` needs the path to exist, so create it
    // first (idempotent — the daemon needs the dir anyway) and fail loudly if an
    // absolute path still cannot be resolved.
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;
    let data_dir_abs = std::fs::canonicalize(&data_dir)
        .with_context(|| format!("resolve an absolute data dir from {}", data_dir.display()))?;

    // launchd jobs start from a minimal environment and do NOT inherit the
    // shell env present at install time, so embed the runtime overrides the
    // daemon reads (F1). The bearer token is deliberately NOT embedded: the
    // plist is mode 0644 and a token in a world-readable file is a secret leak;
    // under launchd the daemon resolves the token from the keychain/config
    // instead (ADR 0007).
    let data_dir_env = data_dir_abs.to_string_lossy().into_owned();
    let ingest_sock_env = std::env::var("BOWERBIRD_INGEST_SOCK")
        .ok()
        .filter(|s| !s.is_empty());
    let mut env: Vec<(&str, &str)> = vec![("BOWERBIRD_DATA_DIR", &data_dir_env)];
    if let Some(sock) = &ingest_sock_env {
        env.push(("BOWERBIRD_INGEST_SOCK", sock));
    }

    let xml = launch_agent::render_launch_agent_plist(
        launch_agent::LAUNCH_AGENT_LABEL,
        &daemon_path,
        &data_dir_abs,
        &env,
    );

    // `--no-start` writes the plist (registration is in place) but skips the
    // launchctl bootstrap — the launchctl-free path for CI/tests (AC 6). It is
    // also the documented exception that permits pre-registering the plist
    // before the daemon binary exists, so the executable check (F4) below is
    // gated to the bootstrap path only.
    if no_start {
        launch_agent::write_launch_agent_plist(&plist_path, &xml)
            .with_context(|| format!("write launch agent plist {}", plist_path.display()))?;
        println!(
            "registered bowerbird-daemon to start on login ({}); --no-start, not bootstrapped",
            plist_path.display()
        );
        return Ok(());
    }

    // Bootstrap path: refuse to register a daemon launchd cannot exec (F4) — a
    // dead registration would crash-loop under KeepAlive instead of failing
    // install loudly.
    launch_agent::ensure_daemon_launchable(&daemon_path)?;

    launch_agent::write_launch_agent_plist(&plist_path, &xml)
        .with_context(|| format!("write launch agent plist {}", plist_path.display()))?;
    println!(
        "registered bowerbird-daemon to start on login ({})",
        plist_path.display()
    );

    // Hand the lifecycle to launchd cleanly. Before bootstrap we must disarm
    // anything that would make the launchd-started daemon hit the singleton PID
    // lock, exit non-zero, and crash-loop under KeepAlive={SuccessfulExit=false}.
    //
    // (a) An already-loaded agent (reinstall) is booted out so the new plist's
    //     ProgramArguments/EnvironmentVariables take effect on re-bootstrap
    //     (this also closes the F3 stale-plist gap).
    if launch_agent::launch_agent_loaded()? {
        launch_agent::bootout_launch_agent(&plist_path)
            .context("bootout previously-loaded launch agent before re-bootstrap")?;
    }

    // (b) A manual / pre-5.9 daemon may still own the singleton lock — and it
    //     may have been launched by the loaded agent we just booted out, so this
    //     check runs UNCONDITIONALLY after any bootout, not only in the
    //     not-loaded branch (F1). Probe the *effective* socket (honoring
    //     BOWERBIRD_INGEST_SOCK, which we also embed in the plist) so a daemon on
    //     a custom socket isn't invisible. If the socket is live but the PID file
    //     can't stop it, refuse rather than bootstrap into an unmanageable
    //     crash-loop.
    let ingest_sock = super::effective_ingest_sock(&data_dir_abs);
    if super::daemon_is_up(&ingest_sock) {
        println!("stopping the running unsupervised daemon so launchd can take over");
        let outcome = daemon::stop_daemon_via_pid_file(&data_dir).map_err(|e| {
            anyhow::anyhow!(
                "could not stop the running daemon before handing off to launchd ({e:#}); \
                 refusing to bootstrap (a launchd start would fail the singleton lock and \
                 crash-loop) — run `bowerbird stop` and re-run install"
            )
        })?;
        if outcome == daemon::StopOutcome::NotRunning && super::daemon_is_up(&ingest_sock) {
            anyhow::bail!(
                "a daemon is accepting on {} but no bowerbird PID file points at a stoppable \
                 process; refusing to bootstrap launchd over a daemon it cannot manage \
                 (stop that daemon, then re-run install)",
                ingest_sock.display()
            );
        }
    }

    launch_agent::bootstrap_launch_agent(&plist_path)
        .with_context(|| format!("bootstrap launch agent {}", plist_path.display()))?;
    println!("started bowerbird-daemon via launchd");
    Ok(())
}

/// Non-macOS: supervision is macOS-only for V1 (Linux systemd stays deferred,
/// architecture.md §Deferred Decisions). Keep today's `setsid`-detached spawn
/// and print one stderr note about the scope (AC 8). The note goes to stderr so
/// scripted stdout stays clean (Story 5.4 discipline).
#[cfg(not(target_os = "macos"))]
fn supervise_or_start(no_start: bool) -> anyhow::Result<()> {
    eprintln!(
        "note: start-on-login supervision is macOS-only for V1; on this platform \
         `bowerbird install` spawns the daemon detached (systemd integration deferred)"
    );

    if no_start {
        return Ok(());
    }

    start_daemon_if_needed()
}

#[cfg(not(target_os = "macos"))]
fn start_daemon_if_needed() -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;
    match daemon::start_daemon_detached(&bowerbird_dir)? {
        StartOutcome::AlreadyRunning => {
            println!("daemon already running; skipping start");
        }
        StartOutcome::Spawned { pid } => {
            println!("started bowerbird-daemon (pid {pid})");
        }
    }
    Ok(())
}
