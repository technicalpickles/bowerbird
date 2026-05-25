//! `bowerbird auth token` and the CLI-side token resolver.
//!
//! The CLI does NOT depend on `crates/daemon` because depending on it would
//! drag tokio + axum into the CLI binary, violating Story 3.1's
//! CLI-stays-light invariant. So the four-step resolution chain
//! (env → keychain → config.toml → fail) is duplicated here. The daemon's
//! authoritative copy lives at `crates/daemon/src/api/token.rs`; the two
//! must agree on the keychain service+user strings (see [`SERVICE`] +
//! [`USER`] below) or `bowerbird auth token` will silently read a different
//! entry than the daemon writes.
//!
//! The duplication is the v1 trade-off; a future story may extract a shared
//! `crates/auth-sync` crate if a third consumer materializes. For now,
//! `tests/cli_auth.rs::auth_token_from_daemon_matches_auth_token_from_cli`
//! is the regression guard.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

/// Keychain service name. **Must match `bowerbird_daemon::api::token::SERVICE`
/// verbatim.** Change both together. See module doc.
pub(crate) const SERVICE: &str = "bowerbird-daemon";
/// Keychain user/account name. **Must match
/// `bowerbird_daemon::api::token::USER` verbatim.** Change both together.
pub(crate) const USER: &str = "bearer-token";

#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    cmd: AuthCmd,
}

#[derive(Subcommand)]
enum AuthCmd {
    /// Print the current bearer token to stdout, suitable for piping into
    /// Authorization headers.
    Token(TokenArgs),
}

#[derive(Args)]
pub struct TokenArgs {}

/// Source that won the resolution race, mirroring
/// `bowerbird_daemon::api::token::TokenSource`. The `generated` discriminator
/// is dropped because the CLI never generates — it only reads. A missing
/// keychain entry causes the resolver to fall through to the next step
/// (config.toml or fail); a "first run" surface would race with the daemon
/// which is the authoritative writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    Keychain,
    ConfigFile,
}

impl std::fmt::Display for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenSource::Env => write!(f, "$BOWERBIRD_TOKEN"),
            TokenSource::Keychain => write!(f, "system keychain"),
            TokenSource::ConfigFile => write!(f, "~/.bowerbird/config.toml"),
        }
    }
}

/// Per-path record of one attempted resolution step, parallel to the
/// daemon's `TriedPath`. Aggregated into a [`TokenError`].
#[derive(Debug)]
pub enum TriedPath {
    EnvUnset,
    Keychain(String),
    ConfigFile(String),
}

impl std::fmt::Display for TriedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriedPath::EnvUnset => write!(f, "BOWERBIRD_TOKEN env (unset)"),
            TriedPath::Keychain(d) => write!(f, "keychain ({d})"),
            TriedPath::ConfigFile(d) => write!(f, "~/.bowerbird/config.toml ({d})"),
        }
    }
}

/// All-paths-exhausted error. Display enumerates every attempt so the user
/// sees exactly what was tried and what to do next (NFR13 contract).
#[derive(Debug)]
pub struct TokenError {
    pub tried: Vec<TriedPath>,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "failed to resolve bearer token; tried:")?;
        for path in &self.tried {
            writeln!(f, "  - {path}")?;
        }
        write!(
            f,
            "set BOWERBIRD_TOKEN or write `token = \"<uuid>\"` into \
             ~/.bowerbird/config.toml (mode 0600)"
        )
    }
}

impl std::error::Error for TokenError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    token: Option<String>,
}

/// Resolve the bearer token from the same chain the daemon uses
/// (env → keychain → config.toml → fail). Returns the token as a
/// `SecretString` so `Debug` cannot accidentally leak it; callers
/// `expose_secret()` exactly once at the `println!` site.
pub fn resolve_token_for_cli() -> Result<(SecretString, TokenSource), TokenError> {
    let mut tried = Vec::with_capacity(3);

    // Step 1: env var
    match std::env::var("BOWERBIRD_TOKEN") {
        Ok(v) if !v.is_empty() => return Ok((SecretString::from(v), TokenSource::Env)),
        _ => tried.push(TriedPath::EnvUnset),
    }

    // Step 2: keychain (gated by BOWERBIRD_KEYRING_BACKEND)
    let backend = install_keyring_backend();
    match backend {
        KeyringBackend::Disabled => tried.push(TriedPath::Keychain(
            "skipped via BOWERBIRD_KEYRING_BACKEND".to_string(),
        )),
        KeyringBackend::Real => match read_keychain() {
            Ok(Some(t)) => return Ok((SecretString::from(t), TokenSource::Keychain)),
            Ok(None) => tried.push(TriedPath::Keychain("no entry".to_string())),
            Err(e) => tried.push(TriedPath::Keychain(e)),
        },
    }

    // Step 3: config.toml
    match read_config_token() {
        Ok(t) => return Ok((SecretString::from(t), TokenSource::ConfigFile)),
        Err(reason) => tried.push(TriedPath::ConfigFile(reason)),
    }

    Err(TokenError { tried })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyringBackend {
    Real,
    Disabled,
}

fn install_keyring_backend() -> KeyringBackend {
    use std::sync::OnceLock;
    static INIT: OnceLock<KeyringBackend> = OnceLock::new();
    *INIT.get_or_init(|| match std::env::var("BOWERBIRD_KEYRING_BACKEND") {
        Ok(v) if v == "disable" => KeyringBackend::Disabled,
        Ok(v) if v == "mock" => {
            // The CLI accepts `mock` unconditionally (it has no production-
            // hardening pressure — the CLI is never running as a long-lived
            // daemon). Install the in-process mock builder so cli_auth tests
            // can drive it in the same process they spawn the CLI from.
            keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
            KeyringBackend::Real
        }
        _ => KeyringBackend::Real,
    })
}

fn read_keychain() -> Result<Option<String>, String> {
    let entry = match keyring::Entry::new(SERVICE, USER) {
        Ok(e) => e,
        Err(e) => return Err(e.to_string()),
    };
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn read_config_token() -> Result<String, String> {
    let path = data_dir().join("config.toml");
    if !path.exists() {
        return Err("not present".to_string());
    }

    // Mode check is separate from the read — a wider mode produces a
    // stderr warning but the value is still returned (operator-friendly,
    // mirrors the daemon's policy).
    if let Some(actual) = check_mode(&path) {
        eprintln!(
            "bowerbird: warning: {} mode is {:o} (recommend 0600)",
            path.display(),
            actual,
        );
    }

    let bytes = std::fs::read_to_string(&path)
        .map_err(|e| format!("read error at {}: {e}", path.display()))?;
    let cfg: ConfigFile =
        toml::from_str(&bytes).map_err(|e| format!("parse error at {}: {e}", path.display()))?;
    match cfg.token {
        Some(t) if !t.is_empty() => Ok(t),
        _ => Err("no `token` field".to_string()),
    }
}

fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("BOWERBIRD_DATA_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(h) = std::env::var_os("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h).join(".bowerbird");
        }
    }
    PathBuf::from("~/.bowerbird")
}

#[cfg(unix)]
fn check_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).ok()?;
    let mode = meta.permissions().mode() & 0o777;
    if mode == 0o600 {
        None
    } else {
        Some(mode)
    }
}

#[cfg(not(unix))]
fn check_mode(_path: &Path) -> Option<u32> {
    None
}

/// Entry point for the `bowerbird auth` subcommand.
pub fn run(args: AuthArgs) -> anyhow::Result<()> {
    match args.cmd {
        AuthCmd::Token(t) => run_token(t),
    }
}

fn run_token(_args: TokenArgs) -> anyhow::Result<()> {
    match resolve_token_for_cli() {
        Ok((token, source)) => {
            // STDOUT: exactly the token + \n. NO banner, NO prefix, NO trailing
            // text — must be pipe-safe for `curl -H "Authorization: Bearer
            // $(bowerbird auth token)"`.
            println!("{}", token.expose_secret());
            // STDERR: one-line provenance hint, but ONLY when stderr is a TTY
            // (mirrors `git config --get` style: data on stdout, hints on
            // stderr, machine-readable by default).
            if std::io::stderr().is_terminal() {
                eprintln!("bowerbird: loaded token from {source}");
            }
            Ok(())
        }
        Err(e) => {
            // NFR13: print the enumerated tried-paths summary to stderr and
            // exit non-zero. anyhow::bail surfaces via main.rs's `?`; main.rs
            // already exits 1 on Err.
            anyhow::bail!("{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_source_display_is_user_facing() {
        assert_eq!(TokenSource::Env.to_string(), "$BOWERBIRD_TOKEN");
        assert_eq!(TokenSource::Keychain.to_string(), "system keychain");
        assert_eq!(
            TokenSource::ConfigFile.to_string(),
            "~/.bowerbird/config.toml"
        );
    }

    #[test]
    fn token_error_lists_every_attempted_path() {
        let err = TokenError {
            tried: vec![
                TriedPath::EnvUnset,
                TriedPath::Keychain("dbus unreachable".to_string()),
                TriedPath::ConfigFile("not present".to_string()),
            ],
        };
        let s = err.to_string();
        assert!(s.contains("BOWERBIRD_TOKEN"));
        assert!(s.contains("keychain"));
        assert!(s.contains("config.toml"));
        assert!(s.contains("set BOWERBIRD_TOKEN"));
    }
}
