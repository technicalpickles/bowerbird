pub mod auth;
pub mod daemon;
pub mod export;
pub mod install;
pub mod replay;
pub mod start;
pub mod status;
pub mod stop;
pub mod uninstall;

use std::path::{Path, PathBuf};

/// Resolve the path to Claude Code's settings.json.
///
/// Order: explicit flag → `BOWERBIRD_CLAUDE_SETTINGS` env override (for tests)
/// → default `$HOME/.claude/settings.json`.
pub(crate) fn resolve_claude_settings(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Some(p) = std::env::var_os("BOWERBIRD_CLAUDE_SETTINGS") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let home = home_dir()?;
    Ok(home.join(".claude").join("settings.json"))
}

/// Resolve the bowerbird data directory. Mirrors `bowerbird-daemon`'s
/// resolution: `BOWERBIRD_DATA_DIR` env wins, else `$HOME/.bowerbird`.
pub(crate) fn resolve_bowerbird_dir() -> anyhow::Result<PathBuf> {
    if let Some(p) = std::env::var_os("BOWERBIRD_DATA_DIR") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let home = home_dir()?;
    Ok(home.join(".bowerbird"))
}

pub(crate) fn home_dir() -> anyhow::Result<PathBuf> {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Ok(PathBuf::from(h)),
        _ => Err(anyhow::anyhow!("HOME is unset or empty")),
    }
}

/// Resolve the `bowerbird-daemon` binary path. `BOWERBIRD_DAEMON_BIN` env
/// override is honored for tests / non-standard installs. Otherwise we trust
/// `PATH` resolution — the CLI ships alongside the daemon and both end up on
/// the user's `PATH` via the same install mechanism.
pub(crate) fn resolve_daemon_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("BOWERBIRD_DAEMON_BIN") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    PathBuf::from("bowerbird-daemon")
}

/// Quick probe: is the daemon's ingest socket connectable? `true` means
/// "process is up and accepting connections"; `false` covers both
/// "daemon not running" and "socket file is stale" (e.g. left behind by a
/// crash) — both cases warrant a fresh spawn.
pub(crate) fn daemon_is_up(ingest_sock: &Path) -> bool {
    use std::os::unix::net::UnixStream;
    UnixStream::connect(ingest_sock).is_ok()
}
