//! User-facing `bowerbird` CLI. Story 3.1 wired `install` and `uninstall`;
//! Story 3.2 adds `start`, `status`, and `stop`; Story 3.3 adds `auth token`.
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
    /// Retrieve the daemon's bearer token from the system keychain (or fallback
    /// chain).
    Auth(commands::auth::AuthArgs),
    /// Export a session's event history as JSONL of protocol::Event records.
    /// Pipe to `bowerbird replay /dev/stdin` to round-trip through the
    /// daemon's pub/sub path on a different machine or after a fresh
    /// `bowerbird install`.
    Export(commands::export::ExportArgs),
    /// Install the bowerbird hook entries into ~/.claude/settings.json and
    /// start the daemon if it is not already running.
    Install(commands::install::InstallArgs),
    /// Replay a JSONL file of recorded events through the daemon's pub/sub
    /// path. Omit the file argument to use the bundled demo fixture.
    Replay(commands::replay::ReplayArgs),
    /// Spawn the bowerbird daemon detached from this shell. Idempotent: if the
    /// daemon is already running, prints the existing pid and exits 0.
    Start(commands::start::StartArgs),
    /// Show whether the bowerbird daemon is running, its version, uptime,
    /// and how many WebSocket presenters are currently connected.
    Status(commands::status::StatusArgs),
    /// Send SIGTERM to the running bowerbird daemon, fall back to SIGKILL
    /// after a 10s graceful drain window.
    Stop(commands::stop::StopArgs),
    /// Remove the bowerbird hook entries from ~/.claude/settings.json and
    /// stop the daemon if it is running.
    Uninstall(commands::uninstall::UninstallArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Auth(args) => commands::auth::run(args).context("bowerbird auth"),
        Command::Export(args) => commands::export::run(args).context("bowerbird export"),
        Command::Install(args) => commands::install::run(args).context("bowerbird install"),
        Command::Replay(args) => commands::replay::run(args).context("bowerbird replay"),
        Command::Start(args) => commands::start::run(args).context("bowerbird start"),
        Command::Status(args) => commands::status::run(args).context("bowerbird status"),
        Command::Stop(args) => commands::stop::run(args).context("bowerbird stop"),
        Command::Uninstall(args) => commands::uninstall::run(args).context("bowerbird uninstall"),
    }
}
