# Test Harness & CI Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `scripts/test.sh` safe to run on a machine with the real bowerbird LaunchAgent loaded, fix the `bowerbird stop` early-return that leaks pid-file daemons, stop the intermittent ENOTCONN bench crash from redding main, and land two small ci.yml wins (bench-job cargo cache, per-PR cross-target check).

**Architecture:** Four independent fixes on one branch. The stop bug gets a product-level env seam (`BOWERBIRD_LAUNCH_AGENT_LABEL`) plus a behavior fix (sweep the pid-file path after a bootout instead of early-returning); the test suites get the seam wired into every command helper, with a `scripts/test.sh` export as backstop. The bench harness tolerates a benign peer-close race. The CI changes are pure ci.yml edits verified by the PR's own runs.

**Tech Stack:** Rust (cargo workspace), POSIX sh test fixtures, GitHub Actions.

**Tracking:** taskwarrior `2e9cfda3` (stop bug, both halves), `88099d39` (ENOTCONN), `092e0a44` (bench cache), `21fa8e4f` (cross-target check). Cite these UUIDs in commit messages.

## Global Constraints

- Run tests ONLY via `scripts/test.sh` (exclusive lock + timeout). Never raw `cargo test`, never two runs concurrently in this worktree. Scoped runs: `scripts/test.sh --test cli_lifecycle -- --exact <name>`.
- CAUTION until Task `launch-agent-label-seam` is complete: running the full cli_auth / cli_lifecycle / cli_examples / cli_export / cli_replay suites on this machine KILLS the maintainer's live daemon (that is the bug being fixed). Early tasks scope test runs to the specific new tests, which all use the fake-launchctl PATH seam and never touch real launchd.
- Never `std::env::set_var` in tests (clippy.toml bans it). Inject env per-child via `Command::env` only.
- Every test parallel-safe: TempDir data dirs, ephemeral ports, no sleeps-as-sync, generous hang-detector timeouts (not tight latency assertions).
- No emdash characters in any added line: `git diff main | grep '^+.*—'` must be empty before every commit.
- `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` must pass (CI runs both).
- Branch: `fix/test-harness-ci-hardening` off `main`. Conventional-commit subjects (`fix(...)`, `test(...)`, `ci(...)`). Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task fake-launchctl-shared-module

Extract the fake-launchctl fixture out of `tests/cli_install.rs` into a shared support module so cli_lifecycle (Task stop-bootout-fallthrough) can use it. Pure refactor, no behavior change.

**Files:**
- Create: `tests/support/fake_launchctl.rs`
- Modify: `tests/cli_install.rs` (delete the inline `FAKE_LAUNCHCTL` const, `write_executable`, `with_fake_launchctl` at roughly lines 508-569; add the module include)

**Interfaces:**
- Produces: `fake_launchctl::FAKE_LAUNCHCTL: &str`, `fake_launchctl::write_executable(path: &Path, body: &str)`, `fake_launchctl::with_fake_launchctl(cmd: &mut assert_cmd::Command, fake_bin_dir: &Path, log: &Path)`. Consumed by cli_install now and cli_lifecycle in Task stop-bootout-fallthrough.

- [ ] **Step 1: Create the shared module**

`tests/support/fake_launchctl.rs` (cargo does not compile `tests/` subdirectories as test binaries, so this is include-only). Move the three items verbatim from `tests/cli_install.rs:520-569`, dropping their `#[cfg(target_os = "macos")]` attributes (the including `mod` is cfg-gated instead), making them `pub`, and keeping every existing comment:

```rust
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
```

- [ ] **Step 2: Repoint cli_install.rs**

In `tests/cli_install.rs`, delete the moved items (`FAKE_LAUNCHCTL` const, `fn write_executable`, `fn with_fake_launchctl`; KEEP the section comment block at lines 508-518 and KEEP `spawn_flock_holder`). Near the top of the file (after the existing `use` lines) add:

```rust
#[cfg(target_os = "macos")]
#[path = "support/fake_launchctl.rs"]
mod fake_launchctl;
#[cfg(target_os = "macos")]
use fake_launchctl::{with_fake_launchctl, write_executable, FAKE_LAUNCHCTL};
```

Call sites stay unchanged (same item names in scope).

- [ ] **Step 3: Verify the refactor compiles and cli_install passes**

cli_install is SAFE to run on this machine (its tests use `--no-start`/`--no-stop`/fake launchctl and `BOWERBIRD_LAUNCH_AGENTS_DIR`; the section comment in the file confirms no real launchctl).

Run: `scripts/test.sh --test cli_install`
Expected: all cli_install tests PASS.

Run: `cargo clippy --all-targets --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add tests/support/fake_launchctl.rs tests/cli_install.rs
git commit -m "test: extract fake-launchctl seam into tests/support for reuse

Prep for the stop-bug fix (taskwarrior 2e9cfda3): cli_lifecycle needs the
same PATH seam cli_install already had inline.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task launch-agent-label-seam

Half 1 of taskwarrior `2e9cfda3`: make the launchd label test-overridable via `BOWERBIRD_LAUNCH_AGENT_LABEL`, wire it into every test suite that shells `bowerbird start`/`stop`, and export a backstop default in `scripts/test.sh` so a future suite that forgets the env still cannot touch the real agent.

**Files:**
- Modify: `src/commands/launch_agent.rs` (add `launch_agent_label()` + `_with_override` + unit test; replace `LAUNCH_AGENT_LABEL` uses at lines 45, 405, 456, 545, 564)
- Modify: `src/commands/install.rs:173`, `src/commands/start.rs:131,148`, `src/commands/uninstall.rs:132`
- Modify: `tests/cli_lifecycle.rs`, `tests/cli_auth.rs`, `tests/cli_examples.rs`, `tests/cli_export.rs`, `tests/cli_replay.rs` (helpers set the override)
- Modify: `tests/cli_install.rs` (helper pins the REAL label so plist-filename assertions survive the test.sh backstop)
- Modify: `scripts/test.sh` (export backstop default)

**Interfaces:**
- Produces: `launch_agent::launch_agent_label() -> String` (env override or the compiled-in const). Every launchd-addressing call site resolves through it. `LAUNCH_AGENT_LABEL` const stays `pub` (unit tests and the default use it).
- Produces (test-side): each suite's command helper sets `BOWERBIRD_LAUNCH_AGENT_LABEL`. cli_lifecycle exposes `const TEST_LAUNCH_AGENT_LABEL: &str = "com.technicalpickles.bowerbird.test-isolation";` which Task stop-bootout-fallthrough asserts against.

- [ ] **Step 1: Write the failing unit test**

In `src/commands/launch_agent.rs` tests mod:

```rust
    // taskwarrior 2e9cfda3: the label must be test-overridable or every
    // integration suite that shells `bowerbird stop` probes (and boots out)
    // the developer's REAL LaunchAgent. Same snapshot-injection seam shape as
    // `resolve_daemon_bin_absolute_with_override` / `token::TokenEnv`.
    #[test]
    fn launch_agent_label_override_wins_only_when_nonempty() {
        assert_eq!(
            launch_agent_label_with_override(Some("com.example.test.label".into())),
            "com.example.test.label"
        );
        assert_eq!(
            launch_agent_label_with_override(Some("".into())),
            LAUNCH_AGENT_LABEL
        );
        assert_eq!(launch_agent_label_with_override(None), LAUNCH_AGENT_LABEL);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `scripts/test.sh -p bowerbird --lib -- launch_agent_label_override`
(If the CLI crate is not named `bowerbird` for `-p`, check `Cargo.toml`; the root package. Adjust to the actual package name.)
Expected: FAIL to compile ("cannot find function `launch_agent_label_with_override`").

- [ ] **Step 3: Implement the resolver**

In `src/commands/launch_agent.rs`, directly below the `LAUNCH_AGENT_LABEL` const:

```rust
/// Resolve the effective LaunchAgent label: `BOWERBIRD_LAUNCH_AGENT_LABEL`
/// env override (test isolation, mirrors `BOWERBIRD_LAUNCH_AGENTS_DIR`) or
/// [`LAUNCH_AGENT_LABEL`]. Every launchd-addressing call site (plist path,
/// bootout/kickstart/print targets, the rendered plist label) resolves
/// through here, so a test running with an isolated label can never probe or
/// boot out the developer's real agent (taskwarrior 2e9cfda3: cli_auth's
/// isolated-HOME stop found the real rc3 agent Loaded, booted it out, and
/// leaked its own TempDir daemon).
#[cfg(target_os = "macos")]
pub fn launch_agent_label() -> String {
    launch_agent_label_with_override(std::env::var_os("BOWERBIRD_LAUNCH_AGENT_LABEL"))
}

/// [`launch_agent_label`] with the env override passed explicitly, so unit
/// tests never mutate process env (same seam shape as
/// `resolve_daemon_bin_absolute_with_override`). Cross-platform for Linux CI.
#[cfg(any(target_os = "macos", test))]
fn launch_agent_label_with_override(override_label: Option<std::ffi::OsString>) -> String {
    match override_label {
        Some(l) if !l.is_empty() => l.to_string_lossy().into_owned(),
        _ => LAUNCH_AGENT_LABEL.to_string(),
    }
}
```

Replace the const at the five launchd-addressing sites in `launch_agent.rs` (do NOT touch the `#[cfg(test)]` mod, which keeps using the const for rendering):

- line 45 `plist_path`: `Ok(launch_agents_dir()?.join(format!("{}.plist", launch_agent_label())))`
- line 405 `bootout_launch_agent`: `let target = format!("gui/{uid}/{}", launch_agent_label());`
- line 456 `kickstart_launch_agent`: same replacement
- line 545 `launch_agent_running_pid`: same replacement
- line 564 `agent_loaded`: same replacement

And the three callers outside the module (each currently passes/interpolates `launch_agent::LAUNCH_AGENT_LABEL`):

- `src/commands/install.rs:173` (the `render_launch_agent_plist` label argument): `&launch_agent::launch_agent_label(),`
- `src/commands/start.rs:131` and `:148` (message interpolation): `launch_agent::launch_agent_label()`
- `src/commands/uninstall.rs:132` (message interpolation): `launch_agent::launch_agent_label()`

Check each surrounding expression compiles (`format!` args take a `String` fine; a `&str` parameter needs `&launch_agent_label()`).

- [ ] **Step 4: Run unit tests, clippy, fmt**

Run: `scripts/test.sh -p bowerbird --lib`
Expected: PASS including the new test.

Run: `cargo clippy --all-targets --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 5: Wire the override into the test-suite helpers**

`tests/cli_lifecycle.rs`: add next to `LIFECYCLE_TEST_TOKEN`:

```rust
/// taskwarrior 2e9cfda3: an isolated label so `stop`/`start` launchd probes
/// address a service that never exists, instead of the developer's real
/// agent. Real `launchctl print` on this label exits 113 ("Could not find"),
/// which the CLI classifies NotLoaded, falling to the pid-file path.
const TEST_LAUNCH_AGENT_LABEL: &str = "com.technicalpickles.bowerbird.test-isolation";
```

and in `bowerbird_bin()` (line ~35, beside the existing token/keyring env):

```rust
    cmd.env("BOWERBIRD_LAUNCH_AGENT_LABEL", TEST_LAUNCH_AGENT_LABEL);
```

`tests/cli_auth.rs`: in `bowerbird_auth_command` (line ~32) AND the `stop_daemon` helper (line ~301) AND any other place a `Command` for the bowerbird binary is built (grep the file for `cargo_bin`), add the same env line with the literal label string (or a file-local const mirroring cli_lifecycle's).

`tests/cli_examples.rs` (`bowerbird_bin`, line ~32), `tests/cli_export.rs` (line ~17), `tests/cli_replay.rs` (line ~19): same env line in each `bowerbird_bin()` helper. Grep each file for `cargo_bin` to catch helpers that bypass `bowerbird_bin()` (cli_examples has a second construction site around line 216).

`tests/cli_install.rs`: in `bowerbird_bin()` (line ~25) PIN the real label so its plist-filename assertions hold even under the scripts/test.sh backstop (safe: cli_install never invokes real launchctl):

```rust
    // Pin the real label: these tests assert plist filenames and never run
    // real launchctl (--no-start/--no-stop/fake-launchctl PATH seam), so the
    // scripts/test.sh isolation backstop must not rename the plist under them.
    cmd.env(
        "BOWERBIRD_LAUNCH_AGENT_LABEL",
        "com.technicalpickles.bowerbird.daemon",
    );
```

- [ ] **Step 6: Add the scripts/test.sh backstop**

In `scripts/test.sh`, just before the `echo "test.sh: cargo test ..."` line (~line 298):

```sh
# taskwarrior 2e9cfda3: a suite whose helper forgets the label override must
# still never probe/bootout the developer's real LaunchAgent. Suites set the
# var per-Command themselves; this export only covers future suites that
# forget. Respect a caller-provided value.
export BOWERBIRD_LAUNCH_AGENT_LABEL="${BOWERBIRD_LAUNCH_AGENT_LABEL:-com.technicalpickles.bowerbird.test-isolation}"
```

- [ ] **Step 7: Verify the previously-dangerous suites are now safe, then run them**

Precondition check on this machine: `bowerbird status` shows running (the launchd agent is loaded). Record `pgrep -fl bowerbird-daemon` output.

Run: `scripts/test.sh --test cli_lifecycle --test cli_auth --test cli_install`
Expected: PASS. Then verify NO damage: `bowerbird status` still running, `pgrep -fl bowerbird-daemon` shows the same single launchd-supervised pid, no extras (no leaked TempDir daemons).

- [ ] **Step 8: Commit**

```bash
git add src/commands/launch_agent.rs src/commands/install.rs src/commands/start.rs src/commands/uninstall.rs tests/cli_lifecycle.rs tests/cli_auth.rs tests/cli_examples.rs tests/cli_export.rs tests/cli_replay.rs tests/cli_install.rs scripts/test.sh
git commit -m "fix: make the LaunchAgent label test-overridable (2e9cfda3 half 1)

BOWERBIRD_LAUNCH_AGENT_LABEL env seam, resolved at every launchd-addressing
call site; every CLI integration suite sets an isolated label, and
scripts/test.sh exports a backstop default. Tests can no longer probe or
boot out the developer's real daemon.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task stop-bootout-fallthrough

Half 2 of taskwarrior `2e9cfda3`: after a successful bootout of a Loaded agent, `bowerbird stop` must still sweep the pid-file path (quietly when it finds nothing) instead of early-returning. The booted-out launchd job is not necessarily the daemon that owns the resolved data dir.

**Files:**
- Modify: `src/commands/stop.rs` (the `LoadState::Loaded` arm at lines 45-53; `stop_via_pid_file` signature at lines 78-106)
- Test: `tests/cli_lifecycle.rs` (two new macOS tests using the shared fake-launchctl module)

**Interfaces:**
- Consumes: `fake_launchctl::{FAKE_LAUNCHCTL, write_executable, with_fake_launchctl}` from Task fake-launchctl-shared-module; `TEST_LAUNCH_AGENT_LABEL` from Task launch-agent-label-seam.
- Produces: `fn stop_via_pid_file(announce_not_running: bool)` (private to stop.rs).

- [ ] **Step 1: Include the fake-launchctl module in cli_lifecycle and write the failing test**

Top of `tests/cli_lifecycle.rs` (after existing `use` lines):

```rust
#[cfg(target_os = "macos")]
#[path = "support/fake_launchctl.rs"]
mod fake_launchctl;
```

New tests at the end of the file:

```rust
/// taskwarrior 2e9cfda3 (product half): a Loaded launchd probe must not END
/// the stop. The booted-out agent is not necessarily the daemon owning
/// BOWERBIRD_DATA_DIR; without the pid-file sweep, `stop` strands the data
/// dir's own daemon. This is exactly how cli_auth's isolated stops killed
/// the developer's real rc3 agent while leaking their TempDir daemons
/// (2026-08-01 incident, story 5-13 Debug Log).
#[cfg(target_os = "macos")]
#[test]
fn stop_after_bootout_still_sweeps_the_pid_file_daemon() {
    let tmp = TempDir::new().expect("tempdir");

    // Start a real daemon manually. The isolated TEST_LAUNCH_AGENT_LABEL makes
    // start's launchd probe NotLoaded, so this is a plain manual spawn.
    bowerbird_cmd_in(&tmp).arg("start").assert().success();
    assert!(
        wait_for_daemon_up(&tmp, Instant::now() + Duration::from_secs(30)),
        "daemon must come up before the stop-under-test"
    );
    let pid = read_pid_file(&tmp).expect("pid file after start");

    // Stop with a fake launchctl reporting the agent Loaded (print exit 0)
    // and booting out successfully: the machine-with-real-agent-loaded shape,
    // with launchd effects stubbed out.
    let bin_dir = tmp.path().join("fakebin");
    std::fs::create_dir_all(&bin_dir).expect("fakebin dir");
    fake_launchctl::write_executable(
        &bin_dir.join("launchctl"),
        fake_launchctl::FAKE_LAUNCHCTL,
    );
    let log = tmp.path().join("launchctl.log");
    let mut cmd = bowerbird_cmd_in(&tmp);
    fake_launchctl::with_fake_launchctl(&mut cmd, &bin_dir, &log);
    cmd.env("FAKE_LAUNCHCTL_PRINT_EXIT", "0");
    let assertion = cmd.arg("stop").assert().success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("daemon stopped (launchd supervision paused"),
        "the bootout must still be announced; stdout={stdout}"
    );

    // The load-bearing assertion: the pid-file daemon dies too. Clean up
    // before asserting so a red run does not leak the daemon.
    let dead = wait_for_pid_dead(pid, Instant::now() + Duration::from_secs(30));
    if !dead {
        force_stop(&tmp);
    }
    assert!(
        dead,
        "stop must sweep the pid-file daemon after bootout, not leak it (pid {pid})"
    );
}

/// taskwarrior 2e9cfda3 (seam half, observed end to end): the launchd probe
/// must address the OVERRIDDEN label, never the real one.
#[cfg(target_os = "macos")]
#[test]
fn stop_probes_launchd_by_the_overridden_label() {
    let tmp = TempDir::new().expect("tempdir");
    let bin_dir = tmp.path().join("fakebin");
    std::fs::create_dir_all(&bin_dir).expect("fakebin dir");
    fake_launchctl::write_executable(
        &bin_dir.join("launchctl"),
        fake_launchctl::FAKE_LAUNCHCTL,
    );
    let log = tmp.path().join("launchctl.log");

    // No daemon, default fake print exit 1 (absent): NotLoaded, pid-file path,
    // clean noop. The interesting artifact is the probe target in the log.
    let mut cmd = bowerbird_cmd_in(&tmp);
    fake_launchctl::with_fake_launchctl(&mut cmd, &bin_dir, &log);
    cmd.arg("stop").assert().success();

    let logged = std::fs::read_to_string(&log).expect("fake launchctl log");
    assert!(
        logged.contains(TEST_LAUNCH_AGENT_LABEL),
        "probe must use the overridden label; log={logged}"
    );
    assert!(
        !logged.contains("com.technicalpickles.bowerbird.daemon"),
        "the real label must never be addressed under an override; log={logged}"
    );
}
```

- [ ] **Step 2: Run both to verify the sweep test fails red**

Run: `scripts/test.sh --test cli_lifecycle -- --exact stop_after_bootout_still_sweeps_the_pid_file_daemon --exact stop_probes_launchd_by_the_overridden_label`
Expected: `stop_probes_launchd_by_the_overridden_label` PASSES (seam already landed). `stop_after_bootout_still_sweeps_the_pid_file_daemon` FAILS on the `dead` assertion (current code early-returns after bootout; the daemon survives). Safe on this machine: all launchctl calls hit the fake.

- [ ] **Step 3: Implement the fall-through**

In `src/commands/stop.rs`, change the `Loaded` arm (keep the existing comments, extend them):

```rust
        launch_agent::LoadState::Loaded => {
            launch_agent::bootout_launch_agent(&plist_path)
                .context("boot the bowerbird LaunchAgent out to stop the daemon")?;
            println!(
                "daemon stopped (launchd supervision paused until next login; \
                 run `bowerbird start` to resume now)"
            );
            // Do NOT return here (taskwarrior 2e9cfda3): the booted-out
            // launchd job is not necessarily the daemon that owns the
            // resolved data dir. With BOWERBIRD_DATA_DIR pointing elsewhere
            // (tests, custom setups), a manual daemon can hold that dir's
            // singleton while the LaunchAgent supervised another; the old
            // early return stranded it. Sweep the pid-file path too, quietly
            // when it finds nothing (the bootout already covered the common
            // same-daemon case, and a contradictory "daemon not running"
            // line after "daemon stopped" would just confuse). When the
            // booted-out daemon IS the pid-file daemon and is still
            // draining, the sweep's SIGTERM is a no-op and the wait makes
            // `stop` return only after the process is actually gone.
            return stop_via_pid_file(false);
        }
```

Change `stop_via_pid_file` to take the flag; the three existing callers pass `true` (the `NotLoaded`/`Unknown` fall-through at line 78, and the non-macOS `stop_daemon` at line 83):

```rust
/// PID-file SIGTERM then SIGKILL stop. The lifecycle owner on Linux, and the
/// macOS fallback/sweep. `announce_not_running` is false only for the
/// post-bootout sweep, where "nothing further to stop" is the expected quiet
/// outcome rather than user-facing news.
fn stop_via_pid_file(announce_not_running: bool) -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;
    match daemon::stop_daemon_via_pid_file(&bowerbird_dir)? {
        StopOutcome::NotRunning => {
            if announce_not_running {
                // Exact wording is contracted with `tests/cli_lifecycle.rs`.
                println!("daemon not running (no pid file); nothing to stop");
            }
        }
        StopOutcome::Stopped => {
            println!("daemon stopped");
        }
        StopOutcome::Escalated => {
            // Exit 0 even on escalation: the user wanted the daemon stopped;
            // the SIGKILL warning has already been printed to stderr by the
            // helper. Mirrors `bowerbird uninstall`'s behavior.
            println!("daemon stopped (after SIGKILL escalation)");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run the new tests plus the whole lifecycle/auth suites**

Run: `scripts/test.sh --test cli_lifecycle --test cli_auth`
Expected: PASS, including the exact-stdout stop tests (their path is NotLoaded, `announce_not_running == true`, wording unchanged). Machine check afterward: `bowerbird status` running, no leaked `bowerbird-daemon` processes.

Run: `cargo clippy --all-targets --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/commands/stop.rs tests/cli_lifecycle.rs
git commit -m "fix(stop): sweep the pid-file daemon after a launchd bootout (2e9cfda3 half 2)

The Loaded arm early-returned after bootout, stranding a daemon that owns
the resolved data dir when it is not the launchd job. Sweep quietly.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task bench-enotconn-tolerance

taskwarrior `88099d39`: the daemon bench harness panics with ENOTCONN (macOS errno 57) when the daemon closes the ingest socket before the bench's `shutdown(Write)`. Second bite turned main run 30712457763 red. Tolerate the benign shutdown race; keep every other failure fatal (a bench crash must stay breakage).

**Files:**
- Modify: `crates/daemon/benches/hook_to_presenter.rs:183-195` (`send_ingest_line`)

**Interfaces:**
- Produces: nothing new; `send_ingest_line` keeps its signature. All four shapes (solo/fanout3/burst/steady) go through it.

- [ ] **Step 1: Implement the tolerance**

Replace the bare `s.shutdown(std::net::Shutdown::Write)?;` line:

```rust
    // The daemon reads the full line, replies "200 ...", and can close its
    // end before this shutdown lands; macOS then fails shutdown(2) with
    // ENOTCONN (errno 57), and a torn-down peer can surface EPIPE. That early
    // close is not a failed ingest: the reply is already buffered and the
    // read below still verifies the 200. Anything else propagates, and the
    // gate wrapper keeps treating a bench crash as breakage, not noise
    // (taskwarrior 88099d39; crashed main runs 30703564941 and 30712457763).
    if let Err(e) = s.shutdown(std::net::Shutdown::Write) {
        match e.kind() {
            std::io::ErrorKind::NotConnected | std::io::ErrorKind::BrokenPipe => {}
            _ => return Err(e),
        }
    }
```

No blanket retry, no tolerance on the connect/write/read/200-check paths.

- [ ] **Step 2: Run the bench once locally to confirm no regression**

The race is intermittent, so this verifies "still works", not "race fixed" (the fix is review-verified by construction: the only swallowed errors are the two benign kinds on the one benign syscall).

Run (short shapes; release build takes a few minutes):
`DAEMON_BENCH_SAMPLES=50 DAEMON_BENCH_BURST_COUNT=20 DAEMON_BENCH_STEADY_SECS=5 cargo bench -p bowerbird-daemon --bench hook_to_presenter`
Expected: completes, prints all four shape lines, exit 0. (This is a bench, not a test; it does not go through scripts/test.sh. Do not run it while a scripts/test.sh run is in flight.)

Run: `cargo clippy --all-targets --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/daemon/benches/hook_to_presenter.rs
git commit -m "fix(bench): tolerate ENOTCONN/EPIPE from ingest shutdown race (88099d39)

The daemon can reply 200 and close before send_ingest_line's
shutdown(Write); macOS surfaces ENOTCONN and the whole gate crashes.
Swallow only that benign shutdown failure; the 200 check still gates.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task bench-gate-cargo-cache

taskwarrior `092e0a44`: the two bench-gate jobs rebuild from scratch every run (daemon macOS: 2m03s for a ~5.7s bench). Add `Swatinem/rust-cache@v2` to both. Its default key already incorporates the job and lockfile, so the two jobs keep separate caches for their different profiles.

**Files:**
- Modify: `.github/workflows/ci.yml` (shim-bench-gate steps at line ~96, daemon-bench-gate steps at line ~136)

- [ ] **Step 1: Add the cache step to both bench jobs**

In BOTH `shim-bench-gate` and `daemon-bench-gate`, immediately after `- uses: actions/checkout@v4`:

```yaml
      # taskwarrior 092e0a44: without a cache every run rebuilds the full dep
      # graph from scratch for a bench measured in seconds. rust-cache's
      # default key includes the job id and Cargo.lock hash, so the two bench
      # jobs (different profiles) and the test job never share entries. The
      # bench binaries themselves still rebuild; only deps are cached, so the
      # measured numbers are unaffected.
      - uses: Swatinem/rust-cache@v2
```

- [ ] **Step 2: Sanity-check the workflow file**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo OK`
Expected: `OK`. (Real verification is the PR's own CI runs in Task endgame-verification: first run seeds, so check the SECOND run's bench-job wall time drops.)

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: cache cargo deps in the two bench-gate jobs (092e0a44)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task ci-cross-target-check

taskwarrior `21fa8e4f`: release-only cross-compile breakage (rc1's x86_64-apple-darwin E0463) is invisible to CI because ci.yml only builds host targets. Add a `cargo check --target x86_64-apple-darwin` to the macOS `ci` row.

**Files:**
- Modify: `.github/workflows/ci.yml` (`ci` job steps, after the clippy step at line ~24)

- [ ] **Step 1: Add the two steps**

In the `ci` job, after the clippy step:

```yaml
      # taskwarrior 21fa8e4f / Story 5.12 rc1: x86_64-apple-darwin only ever
      # built at tag time and failed with E0463 (target not added to the
      # pinned toolchain). Check it per-PR so cross-compile breakage cannot
      # hide until release. `rustup target add` run in the repo dir applies to
      # the rust-toolchain.toml pin, same as release.yml's load-bearing step.
      - name: Add x86_64-apple-darwin to the pinned toolchain
        if: matrix.os == 'macos-latest'
        run: rustup target add x86_64-apple-darwin
      - name: Cross-target check (x86_64-apple-darwin)
        if: matrix.os == 'macos-latest'
        run: cargo check --workspace --target x86_64-apple-darwin
```

(`--workspace` default targets = libs + bins, matching what release.yml actually builds; `--all-targets` would also compile tests/benches the release never ships, for real extra minutes.)

- [ ] **Step 2: Sanity-check + commit**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo OK`
Expected: `OK`.

```bash
git add .github/workflows/ci.yml
git commit -m "ci: check x86_64-apple-darwin per-PR on the macOS row (21fa8e4f)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task endgame-verification

Full-suite verification, the real-machine repro check from taskwarrior `2e9cfda3`, and the PR.

- [ ] **Step 1: Full suite via scripts/test.sh**

Precondition: `bowerbird status` shows the daemon running (launchd agent loaded). Record `pgrep -fl bowerbird-daemon`.

Run: `scripts/test.sh`
Expected: all workspace tests PASS (647+ at last count).

- [ ] **Step 2: The 2e9cfda3 acceptance check**

Immediately after the suite:

```bash
bowerbird status
pgrep -fl bowerbird-daemon
```

Expected: status still `running`; exactly the same single daemon pid as before the suite; zero leaked TempDir daemons (no extra `bowerbird-daemon` processes with ppid 1). This is the exact repro from the task record, now green.

- [ ] **Step 3: Pre-PR hygiene**

```bash
cargo fmt --check
cargo clippy --all-targets --workspace -- -D warnings
git diff main | grep '^+.*—' ; echo "emdash exit: $? (want 1)"
```

Expected: fmt/clippy clean; the grep finds nothing (exit 1).

- [ ] **Step 4: Push and open the PR**

Use the `git:pull-request` skill (and `writing-voice` for the body; no emdashes in the PR body). Head: `fix/test-harness-ci-hardening` into `main`. Body covers: the 2026-08-01 daemon-kill incident and both halves of the fix, the ENOTCONN gate crash (runs 30703564941, 30712457763), the two ci.yml wins, and taskwarrior UUIDs 2e9cfda3 / 88099d39 / 092e0a44 / 21fa8e4f.

Watch the PR's CI: all jobs green expected; on the bench jobs the FIRST run seeds the cache (no speedup yet). Optionally re-run one bench job to confirm the cache pays off.

- [ ] **Step 5: After merge**

```bash
task 2e9cfda3 done
task 88099d39 done
task 092e0a44 done
task 21fa8e4f done
```

Then annotate nothing further; the PR is the record. Next up per the parked handoff: `/bmad-create-story 5-14 first-time-reader-docs-pass`.
