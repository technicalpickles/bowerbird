//! User-facing `bowerbird` CLI. Story 3.1 wires `install` and `uninstall`;
//! Story 3.2 adds `start` / `stop` / `status`; Story 3.3 adds `auth token`.
//!
//! `anyhow::Context` is intentional here — `main.rs` is the single binary edge
//! where the project allows `anyhow` per architecture.md's anti-pattern list.
//! Inside `commands/*` we stay typed (`adapter_claude::InstallError` etc.).

mod commands;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "bowerbird",
    version,
    about = "bowerbird command-line interface"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the bowerbird hook entries into ~/.claude/settings.json and
    /// start the daemon if it is not already running.
    Install(commands::install::InstallArgs),
    /// Remove the bowerbird hook entries from ~/.claude/settings.json and
    /// stop the daemon if it is running.
    Uninstall(commands::uninstall::UninstallArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Install(args) => commands::install::run(args).context("bowerbird install"),
        Command::Uninstall(args) => commands::uninstall::run(args).context("bowerbird uninstall"),
    }
}
