//! Process-scoped singleton enforcement for the daemon.
//!
//! Guarantees that only one `bowerbird-daemon` operates against a given data
//! directory at a time. Uses BSD `flock(2)` with `LOCK_EX | LOCK_NB` on an
//! FD held for the daemon's lifetime — the kernel releases the lock when the
//! FD closes, so clean exit, panic, OOM kill, or SIGKILL all self-clean. The
//! PID file content is informational only (surfaced in the conflict error
//! message); the FD is the lock primitive.
//!
//! Acquire BEFORE `init_pools`: the race this guards is two daemons running
//! migrations against the same `bower.db` concurrently. Folds the
//! `deferred-work.md` entry under Story 1.2's code review (Singleton
//! enforcement / file lock / PID file).

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::sys::signal;
use nix::unistd::Pid;

const PID_FILENAME: &str = "bowerbird.pid";

/// Guard returned by [`acquire`]. The flock is held for the lifetime of this
/// value. Dropping it releases the lock (and closes the FD); the PID file is
/// left on disk for informational use — the next acquire overwrites it.
#[derive(Debug)]
pub struct SingletonLock {
    // The `Flock<File>` owns both the FD and the kernel lock. On drop it
    // explicitly LOCK_UNs and closes the FD; on panic the FD-close alone
    // suffices because BSD flocks die with the OFD.
    _lock: Flock<File>,
    _pid_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum SingletonError {
    #[error("failed to open or create {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "another bowerbird daemon is already running (pid={holder_pid}); \
         data directory {data_dir} can only be owned by one process at a time"
    )]
    AlreadyHeld {
        data_dir: PathBuf,
        /// PID read from the existing `bowerbird.pid` at conflict time. May
        /// be `0` when the file was empty or unparseable — the lock is still
        /// held by SOMEONE, but the identifier was not recoverable.
        holder_pid: i32,
    },
    #[error("failed to write pid to {path}: {source}")]
    WritePid {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Acquire the singleton lock for `data_dir`. The caller is responsible for
/// ensuring `data_dir` exists (`ensure_bowerbird_dir` in this crate is the
/// canonical entry point). The PID file at `<data_dir>/bowerbird.pid` is
/// created if missing.
///
/// On `EWOULDBLOCK` the function reads the existing PID file. If the named
/// holder is dead (`kill(pid, 0) == ESRCH`) the lock is retried once — a
/// stale PID file from a prior crash is overwritten on the retry's success.
/// If the retry still fails the lock is held by a live process that did not
/// match the recorded PID (concurrent-startup race or hand-edited PID file)
/// and we error out without overwriting the holder's state.
pub fn acquire(data_dir: &Path) -> Result<SingletonLock, SingletonError> {
    let pid_path = data_dir.join(PID_FILENAME);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&pid_path)
        .map_err(|source| SingletonError::Open {
            path: pid_path.clone(),
            source,
        })?;

    let lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(l) => l,
        Err((file, _errno)) => {
            // Lock conflict. Read the holder PID for the error message —
            // best-effort; the file may be empty if the prior process died
            // between `flock` and the PID write.
            let holder = read_pid_file(&pid_path).unwrap_or(0);
            if holder > 0 && !pid_alive(holder) {
                // Stale PID. The kernel released the lock when the prior
                // process's FD closed; the file content just hasn't been
                // refreshed. Retry once.
                match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                    Ok(l) => l,
                    Err((_file, _errno)) => {
                        return Err(SingletonError::AlreadyHeld {
                            data_dir: data_dir.to_path_buf(),
                            holder_pid: holder,
                        });
                    }
                }
            } else {
                drop(file); // close the FD; we never held the lock
                return Err(SingletonError::AlreadyHeld {
                    data_dir: data_dir.to_path_buf(),
                    holder_pid: holder,
                });
            }
        }
    };

    write_pid_to_locked_path(&pid_path, std::process::id())?;

    Ok(SingletonLock {
        _lock: lock,
        _pid_path: pid_path,
    })
}

fn read_pid_file(path: &Path) -> Option<i32> {
    let contents = std::fs::read_to_string(path).ok()?;
    contents.trim().parse::<i32>().ok()
}

fn pid_alive(pid: i32) -> bool {
    // `kill(pid, 0)` returns Ok if the process exists, ESRCH if not. EPERM
    // means "exists, but we lack permission to signal it" — still alive for
    // our purposes (and unlikely for a same-user daemon scenario, but worth
    // handling correctly).
    match signal::kill(Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

fn write_pid_to_locked_path(path: &Path, pid: u32) -> Result<(), SingletonError> {
    // Open a fresh handle to the path with O_TRUNC. We already hold the BSD
    // flock on a separate OFD (the one inside `SingletonLock`), so no other
    // process can race us. The new FD is closed on function return.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|source| SingletonError::WritePid {
            path: path.to_path_buf(),
            source,
        })?;
    writeln!(f, "{pid}").map_err(|source| SingletonError::WritePid {
        path: path.to_path_buf(),
        source,
    })?;
    f.sync_all().map_err(|source| SingletonError::WritePid {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn acquire_succeeds_on_fresh_data_dir() {
        let tmp = TempDir::new().unwrap();
        let _lock = acquire(tmp.path()).expect("first acquire");
        assert!(tmp.path().join(PID_FILENAME).exists());
    }

    #[test]
    fn acquire_writes_current_pid_into_pid_file() {
        let tmp = TempDir::new().unwrap();
        let _lock = acquire(tmp.path()).expect("acquire");
        let contents = std::fs::read_to_string(tmp.path().join(PID_FILENAME)).unwrap();
        let written: u32 = contents.trim().parse().expect("pid is numeric");
        assert_eq!(written, std::process::id());
    }

    #[test]
    fn second_acquire_in_same_process_fails_with_already_held() {
        let tmp = TempDir::new().unwrap();
        let _first = acquire(tmp.path()).expect("first acquire");
        let err = acquire(tmp.path()).expect_err("second acquire must fail");
        match err {
            SingletonError::AlreadyHeld { holder_pid, .. } => {
                assert_eq!(holder_pid, std::process::id() as i32);
            }
            other => panic!("expected AlreadyHeld, got {other:?}"),
        }
    }

    #[test]
    fn drop_releases_lock_for_subsequent_acquire() {
        let tmp = TempDir::new().unwrap();
        let first = acquire(tmp.path()).expect("first acquire");
        drop(first);
        let _second = acquire(tmp.path()).expect("second acquire after drop");
    }

    #[test]
    fn stale_pid_file_with_dead_process_allows_reacquire() {
        let tmp = TempDir::new().unwrap();
        // Spawn-and-reap a child to obtain a known-dead PID. Writing a hardcoded
        // PID like 99999 risks colliding with a real process on busy systems.
        let child = Command::new("true").spawn().expect("spawn true");
        let dead_pid = child.id();
        let out = child.wait_with_output().expect("reap");
        assert!(out.status.success());
        // Write the stale PID without holding any flock.
        std::fs::write(tmp.path().join(PID_FILENAME), format!("{dead_pid}\n")).unwrap();
        let _lock = acquire(tmp.path()).expect("acquire over stale pid");
        let contents = std::fs::read_to_string(tmp.path().join(PID_FILENAME)).unwrap();
        let written: u32 = contents.trim().parse().unwrap();
        assert_eq!(
            written,
            std::process::id(),
            "stale pid must be overwritten with current pid"
        );
    }

    #[test]
    fn unparseable_pid_file_yields_zero_holder_in_error_message() {
        let tmp = TempDir::new().unwrap();
        let _first = acquire(tmp.path()).expect("first acquire");
        // Corrupt the pid file content while the lock is still held. The
        // second acquire must still error with AlreadyHeld; the holder_pid
        // collapses to 0 since the file is now garbage.
        std::fs::write(tmp.path().join(PID_FILENAME), b"not-a-pid").unwrap();
        let err = acquire(tmp.path()).expect_err("second acquire must fail");
        match err {
            SingletonError::AlreadyHeld { holder_pid, .. } => {
                assert_eq!(holder_pid, 0);
            }
            other => panic!("expected AlreadyHeld, got {other:?}"),
        }
    }
}
