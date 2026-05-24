//! Atomic writer for `~/.bowerbird/server.json`.
//!
//! The daemon publishes its bound HTTP address here once the kernel has
//! assigned an ephemeral port (default `bind_addr` is `127.0.0.1:0`). The CLI
//! reads it to find the daemon's HTTP surface for `/healthz` and `/status`
//! probes.
//!
//! **Liveness contract.** This file is a *hint*, not a liveness proof. After
//! an unclean exit (SIGKILL, OOM, panic past the panic hook) the file is left
//! on disk with a possibly-wrong `bind_addr`. The singleton PID file
//! (`crate::singleton`) plus an ingest-socket connect probe is what proves the
//! daemon is alive; `server.json` only tells the CLI where to send HTTP if
//! something *is* alive.
//!
//! **Mode 0600.** Story 3.3 will extend `ServerInfo` with a `token` field. We
//! pay the cost of setting the mode now so 3.3 inherits a safe baseline rather
//! than racing with a mode-change step (TOCTOU window for the token).

use std::io::Write;
use std::path::{Path, PathBuf};

use protocol::ServerInfo;

const FILENAME: &str = "server.json";

/// Atomically write `server.json` inside `data_dir`.
///
/// Sequence: serialize → open `server.json.tmp.<pid>` with mode 0600 → write +
/// fsync → rename onto the final path. `rename(2)` is atomic on the same
/// filesystem; the fsync makes the bytes durable before the rename commits.
pub fn write(data_dir: &Path, info: &ServerInfo) -> std::io::Result<PathBuf> {
    let final_path = data_dir.join(FILENAME);
    let tmp_path = data_dir.join(format!("{FILENAME}.tmp.{}", std::process::id()));

    let bytes = serde_json::to_vec_pretty(info)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    write_tmp_then_rename(&tmp_path, &final_path, &bytes)?;
    Ok(final_path)
}

#[cfg(unix)]
fn write_tmp_then_rename(tmp_path: &Path, final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    // O_TRUNC on the tmp open: a stale tmp from a prior crash is overwritten
    // rather than leaking content.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp_path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(tmp_path, final_path)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_tmp_then_rename(tmp_path: &Path, final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(tmp_path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(tmp_path, final_path)?;
    Ok(())
}

/// Best-effort delete of `server.json`. A failure (file already gone, permission
/// denied) is *not* propagated — the next daemon startup will overwrite a
/// leftover file via [`write`]. Returns `true` if the file was actually
/// removed, `false` if it was already absent.
pub fn remove(data_dir: &Path) -> std::io::Result<bool> {
    let path = data_dir.join(FILENAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_creates_server_json_with_payload() {
        let tmp = TempDir::new().unwrap();
        let info = ServerInfo {
            bind_addr: "127.0.0.1:54321".to_string(),
        };
        let path = write(tmp.path(), &info).expect("write");
        assert_eq!(path, tmp.path().join(FILENAME));
        let parsed: ServerInfo =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("valid json");
        assert_eq!(parsed.bind_addr, "127.0.0.1:54321");
    }

    #[cfg(unix)]
    #[test]
    fn write_sets_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let info = ServerInfo {
            bind_addr: "127.0.0.1:1".to_string(),
        };
        let path = write(tmp.path(), &info).expect("write");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode must be 0600, got {mode:o}");
    }

    #[test]
    fn write_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let first = ServerInfo {
            bind_addr: "127.0.0.1:1".to_string(),
        };
        let second = ServerInfo {
            bind_addr: "127.0.0.1:2".to_string(),
        };
        write(tmp.path(), &first).expect("first write");
        write(tmp.path(), &second).expect("second write");
        let parsed: ServerInfo =
            serde_json::from_slice(&std::fs::read(tmp.path().join(FILENAME)).unwrap()).unwrap();
        assert_eq!(parsed.bind_addr, "127.0.0.1:2");
    }

    #[test]
    fn remove_when_present_returns_true() {
        let tmp = TempDir::new().unwrap();
        let info = ServerInfo {
            bind_addr: "127.0.0.1:1".to_string(),
        };
        write(tmp.path(), &info).expect("write");
        assert!(remove(tmp.path()).expect("remove"));
        assert!(!tmp.path().join(FILENAME).exists());
    }

    #[test]
    fn remove_when_absent_returns_false() {
        let tmp = TempDir::new().unwrap();
        assert!(!remove(tmp.path()).expect("remove on absent"));
    }
}
