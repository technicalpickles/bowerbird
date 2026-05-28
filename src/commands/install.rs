use std::path::PathBuf;

use anyhow::Context;
use clap::Args;

use super::daemon::{self, StartOutcome};

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

    if args.no_start {
        return Ok(());
    }

    start_daemon_if_needed()?;
    Ok(())
}

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
