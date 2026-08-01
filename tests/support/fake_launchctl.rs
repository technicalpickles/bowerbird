//! Fake `launchctl` PATH seam, shared by the CLI integration suites.
//!
//! Moved out of `tests/cli_install.rs` (Story 5.9 review F6) so any suite that
//! shells the real `bowerbird` binary can exercise launchd branches without
//! touching real launchd. The fake records each invocation to
//! `$FAKE_LAUNCHCTL_LOG` and exits with per-subcommand codes from
//! `FAKE_LAUNCHCTL_*` env vars (default: `print` exits 1 = "not loaded";
//! everything else exits 0). CI-safe on the macOS runner.
//!
//! Included per test binary via `#[path = "support/fake_launchctl.rs"]`, so
//! each binary compiles its own copy; items unused by a given binary are
//! expected.
#![allow(dead_code)]

use assert_cmd::Command;
use std::fs;
use std::path::Path;

/// Fake `launchctl` script body (POSIX sh). `$1` is the subcommand.
///
/// `print` models the real `launchctl print` for an absent service: a non-zero
/// exit carries a "Could not find ..." stderr by default so the CLI classifies
/// it as genuinely-absent (Ok(false)). `FAKE_LAUNCHCTL_PRINT_STDERR` overrides
/// the message so a test can simulate an *unverifiable* failure (non-absent
/// stderr), which the CLI must surface as "cannot verify" rather than "not
/// loaded" (Story 5.9 review pass-3 F1).
pub const FAKE_LAUNCHCTL: &str = r#"#!/bin/sh
echo "$@" >> "$FAKE_LAUNCHCTL_LOG"
case "$1" in
  print)
     code="${FAKE_LAUNCHCTL_PRINT_EXIT:-1}"
     if [ "$code" -ne 0 ]; then
       echo "${FAKE_LAUNCHCTL_PRINT_STDERR:-Could not find service in domain for gui}" >&2
     elif [ -n "$FAKE_LAUNCHCTL_PRINT_PID" ]; then
       # Story 5.9 review pass-7 F5: a loaded job that launchd is actually running
       # carries a `pid = N` line; the CLI cross-checks it against the singleton
       # holder. Absent this var, `print` reports loaded-but-not-running.
       printf '\tpid = %s\n' "$FAKE_LAUNCHCTL_PRINT_PID"
     fi
     exit "$code" ;;
  bootstrap) exit "${FAKE_LAUNCHCTL_BOOTSTRAP_EXIT:-0}" ;;
  bootout)
     code="${FAKE_LAUNCHCTL_BOOTOUT_EXIT:-0}"
     [ "$code" -ne 0 ] && echo "Boot-out failed: 1: Operation not permitted" >&2
     exit "$code" ;;
  kickstart) exit "${FAKE_LAUNCHCTL_KICKSTART_EXIT:-0}" ;;
  load)      exit "${FAKE_LAUNCHCTL_LOAD_EXIT:-0}" ;;
  unload)    exit "${FAKE_LAUNCHCTL_UNLOAD_EXIT:-0}" ;;
  *)         exit 0 ;;
esac
"#;

pub fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, body).expect("write executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
}

/// Prepend `fake_bin_dir` (holding the fake `launchctl`) to the spawned
/// process's PATH and point `FAKE_LAUNCHCTL_LOG` at `log`.
pub fn with_fake_launchctl(cmd: &mut Command, fake_bin_dir: &Path, log: &Path) {
    let orig = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{}", fake_bin_dir.display(), orig));
    cmd.env("FAKE_LAUNCHCTL_LOG", log);
}
