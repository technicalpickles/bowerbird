//! Reader for `~/.bowerbird/config.toml`, the user-supplied fallback for the
//! bearer token (Story 3.3).
//!
//! Lives at `crates/daemon/src/config_file.rs` (top-level `src/`, NOT under
//! `api/`) because resolving the token is a startup concern, not an HTTP
//! handler concern. The API surface only sees the *result* — `AppState.bearer`
//! is set once at startup and never reloaded (NFR14).
//!
//! # Serde policy: `deny_unknown_fields`
//!
//! This file is an **inbound** surface — the daemon parsing user input. The
//! project's asymmetric serde policy (project-context.md "Wire format
//! conventions") says inbound surfaces are strict so typos like
//! `Token = "..."` (capital T) or `tokn = "..."` are rejected loudly instead
//! of silently producing `token = None`. This is the opposite of `ServerInfo`,
//! which is daemon-emitted and stays permissive so older CLIs don't break on
//! a future field. **Do not remove `deny_unknown_fields` here without first
//! understanding the asymmetric policy.**
//!
//! # Mode policy: warn, don't refuse
//!
//! The recommended mode is `0600`. A wider mode emits a WARN line via
//! `tracing` (see `api::token::read_config_token`) but the value is STILL
//! returned to the caller. Refusing to load on wrong mode would lock users
//! out of the daemon in shared-machine or unusual-umask scenarios where they
//! cannot fix the permissions. Operator-friendly beats pedantic.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Parsed shape of `~/.bowerbird/config.toml`. `deny_unknown_fields` because
/// this is an inbound surface — see module doc.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default)]
    pub token: Option<String>,
}

/// Why a [`read`] call did not return a token. The variants are exposed so
/// callers (the token resolver in `api::token`) can render them into the
/// `TokenError` enumeration with provenance.
#[derive(Debug)]
pub enum ReadFailure {
    /// File does not exist (absence is a valid configuration).
    NotFound,
    /// File exists but could not be opened or read.
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// File contents are not valid TOML or violate `deny_unknown_fields`.
    Toml {
        path: PathBuf,
        error: toml::de::Error,
    },
    /// Parsed successfully but the `token` field is missing or empty.
    NoTokenField,
}

impl fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadFailure::NotFound => write!(f, "not present"),
            ReadFailure::Io { path, error } => {
                write!(f, "read error at {}: {error}", path.display())
            }
            ReadFailure::Toml { path, error } => {
                write!(f, "parse error at {}: {error}", path.display())
            }
            ReadFailure::NoTokenField => write!(f, "no `token` field"),
        }
    }
}

impl std::error::Error for ReadFailure {}

/// Read and parse the config file at `path`. Returns `Err(ReadFailure::NotFound)`
/// when the file is missing (NOT a hard error — absence is a valid state).
pub fn read(path: &Path) -> Result<ConfigFile, ReadFailure> {
    let bytes = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadFailure::NotFound),
        Err(e) => {
            return Err(ReadFailure::Io {
                path: path.to_path_buf(),
                error: e,
            });
        }
    };
    toml::from_str::<ConfigFile>(&bytes).map_err(|e| ReadFailure::Toml {
        path: path.to_path_buf(),
        error: e,
    })
}

/// Return the file's permission bits when they are NOT `0o600`. `None` means
/// either the file doesn't exist, the metadata read failed, or the mode is
/// already 0600. Callers (the token resolver) log a WARN on a wider mode but
/// still load the file.
#[cfg(unix)]
pub fn check_mode(path: &Path) -> Option<u32> {
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
pub fn check_mode(_path: &Path) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn write_with_mode(path: &Path, contents: &str, mode: u32) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(path)
            .expect("open");
        f.write_all(contents.as_bytes()).expect("write");
    }

    #[test]
    fn read_returns_notfound_when_absent() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("config.toml");
        match read(&missing) {
            Err(ReadFailure::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_returns_token_when_field_present() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("config.toml");
        write_with_mode(&p, "token = \"abc123\"\n", 0o600);
        let cfg = read(&p).expect("read");
        assert_eq!(cfg.token.as_deref(), Some("abc123"));
    }

    #[cfg(unix)]
    #[test]
    fn read_returns_none_token_when_field_missing() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("config.toml");
        write_with_mode(&p, "# nothing here\n", 0o600);
        let cfg = read(&p).expect("read");
        assert!(cfg.token.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn read_rejects_unknown_field() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("config.toml");
        write_with_mode(&p, "Token = \"capital-t-is-a-typo\"\n", 0o600);
        match read(&p) {
            Err(ReadFailure::Toml { .. }) => {}
            other => panic!("expected Toml parse error, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn check_mode_none_when_0600() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("config.toml");
        write_with_mode(&p, "token = \"x\"\n", 0o600);
        assert_eq!(check_mode(&p), None);
    }

    #[cfg(unix)]
    #[test]
    fn check_mode_returns_actual_when_wider() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("config.toml");
        write_with_mode(&p, "token = \"x\"\n", 0o644);
        assert_eq!(check_mode(&p), Some(0o644));
    }
}
