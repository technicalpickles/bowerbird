# Story 5.9: Daemon start-on-login supervision

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As the bowerbird maintainer,
I want `bowerbird install` to register the daemon to start on login with crash-restart (and `bowerbird uninstall` to remove that registration symmetrically),
so that a reboot doesn't silently drop every event until I manually restart the daemon.

## Context & Provenance

Surfaced by **Cluster A** of the 2026-06-01 dogfood triage (`docs/dogfooding-feedback.md`; session `ad3eaed4-af27-4bb0-9844-f0e237defbc1`). The workstation rebooted, `bowerbird-daemon` did not come back, and for ~90 seconds every Claude Code tool call stacked a `hook error / Failed with non-blocking status code: No stderr output` while the shim failed to reach a missing `~/.bowerbird/ingest.sock`. Tool calls ran fine (the shim never blocks), but every event in the window was dropped, and recovery was manual.

This is **Finding 1** of the dogfood-triage proposal (`sprint-change-proposal-2026-06-01-dogfood-triage.md` §4.3). It is **not on the no-list** — process supervision was *deferred* post-V1 (architecture.md §Process supervision), not cut. ADR 0007 reverses the macOS half of that deferral. (Finding 2, the causeless hook-error wall, is the sibling Story 5.10 — out of scope here.)

Gates the **v0.1.0 tag** (Story 5.14).

## Acceptance Criteria

> ACs are derived from proposal §4.3 + §3 rationale and the existing install/lifecycle contract in `INSTALL.md` and `architecture.md` §Infrastructure & Deployment. "macOS-only" everywhere means `#[cfg(target_os = "macos")]`; on Linux the existing `setsid`-detached spawn behavior is unchanged (see AC 8).

1. **ADR 0007 lands first.** `docs/decisions/0007-daemon-start-on-login.md` exists (Status: Accepted, Date 2026-06-03, Deciders @pickles) and records: the launchd-LaunchAgent-vs-shim-lazy-spawn decision, the shim-stays-thin rationale, the `KeepAlive` semantics chosen (see AC 4), the macOS-only scope (Linux systemd stays deferred), and `Affects context.md sections: Durability and chaos`. It cites the proposal, this story, and ADR 0003 (`shim` hot-path discipline that lazy-spawn would violate).

2. **`bowerbird install` (macOS) writes a LaunchAgent plist.** On macOS, install writes `~/Library/LaunchAgents/<label>.plist` (atomically: write `.tmp` → rename, mode `0644`) describing the daemon. The plist is well-formed XML and contains at minimum: `Label` = `<label>`, `ProgramArguments` = `[<absolute-path-to-bowerbird-daemon>]`, `RunAtLoad` = `true`, `KeepAlive` per AC 4, and `StandardOutPath`/`StandardErrorPath` under the data dir (e.g. `~/.bowerbird/daemon.out.log` / `daemon.err.log`). The `<label>` is a single committed constant (reverse-DNS, e.g. `com.technicalpickles.bowerbird.daemon` — finalize in ADR 0007) reused by install, uninstall, and start. `bowerbird stop` is intentionally NOT modified by this story: it stays PID-file SIGTERM only (a clean exit-0 + `KeepAlive={SuccessfulExit=false}` keeps the daemon down), so it never addresses launchd by label.

3. **`ProgramArguments` carries an absolute daemon path.** Because launchd's default PATH does not include `/usr/local/bin`, the plist must name an absolute path, NOT the PATH-relative `bowerbird-daemon` that `resolve_daemon_bin()` returns today. Resolution order: `BOWERBIRD_DAEMON_BIN` (if set and absolute) → a sibling of `std::env::current_exe()` (the CLI ships alongside the daemon) → a `PATH` search resolved to absolute. If no absolute path can be resolved, install fails with a clear error naming the daemon binary (it does NOT write a plist pointing at a bare name that launchd cannot exec).

4. **`KeepAlive` gives crash-restart without fighting `bowerbird stop`.** The plist uses `KeepAlive = { SuccessfulExit = false }` (restart only on non-zero exit), NOT `KeepAlive = true`. Rationale: Story 2.5 graceful shutdown exits 0 on SIGTERM, so a clean `bowerbird stop` lets launchd leave the daemon down; a crash (non-zero exit) triggers launchd restart. `RunAtLoad = true` covers the reboot/login case. ADR 0007 ratifies this choice.

5. **Install bootstraps the agent (and that start replaces the manual spawn).** On macOS, after writing the plist, install loads it via `launchctl bootstrap gui/<uid> <plist-path>` (modern API; `launchctl load` is the legacy fallback), which starts the daemon via `RunAtLoad`. Install does NOT also `setsid`-spawn the daemon on macOS — launchd owns the lifecycle, and the daemon's singleton PID lock would reject a double-start anyway. Bootstrapping an already-loaded agent is idempotent (an "already bootstrapped"/`EEXIST`-class result is treated as success, not an error).

6. **`--no-start` skips the bootstrap but still writes the plist.** Passing `--no-start` writes the plist (so the registration is in place) but does NOT invoke `launchctl bootstrap` and does NOT start the daemon — mirroring today's `--no-start` semantics and giving CI/tests a launchctl-free path.

7. **`bowerbird uninstall` (macOS) removes the agent symmetrically.** On macOS, uninstall boots the agent out (`launchctl bootout gui/<uid>/<label>`, legacy fallback `launchctl unload`) and removes the plist file. A `bootout` of an already-unloaded agent is a clean no-op (not an error). Uninstall does NOT delete `~/.bowerbird/` (event history is the user's data — unchanged contract). `--no-stop` skips the `bootout` + daemon stop but still removes the plist, mirroring AC 6.

8. **Linux is unchanged; behavior is documented.** On non-macOS (Linux), install/uninstall behave exactly as before this story (the `setsid`-detached spawn and PID-file stop), no plist is written, and install prints one note to **stderr** that start-on-login supervision is macOS-only for V1 (systemd integration stays deferred — architecture.md §Deferred Decisions). No Linux test regresses.

9. **`install` → `uninstall` round-trip leaves no LaunchAgent residue (macOS).** After `install` then `uninstall`, the plist file is gone and (when bootstrapped) the agent is booted out. The settings.json merge/un-merge and `tool-reactions.toml` seed behavior from Stories 3.1/5.4 are unchanged and still pass.

10. **Docs updated to reflect supervision is now V1 (macOS).**
    - `docs/bmad/planning-artifacts/architecture.md`: §Decision Priority Analysis L366 ("launchd … deferred post-V1"), the Deferred-Decisions list L376, and §Infrastructure & Deployment "Process supervision (V1)" L502-505 are corrected — macOS launchd start-on-login is now V1; Linux systemd stays deferred. Backlink ADR 0007.
    - `docs/bmad/project-context.md`: §Durability and chaos gains a note that the daemon is supervised by a launchd LaunchAgent on macOS (start-on-login + crash-restart), per ADR 0007's `Affects context.md sections` field.
    - `INSTALL.md`: §3 (Run `bowerbird install`) documents the LaunchAgent registration (start-on-login, crash-restart, the plist path) and §5 (Uninstall) documents its removal.
    - No `docs/protocol-changelog.md` entry — this story touches no wire protocol (the changelog gate keys on `crates/protocol/src/*.rs`, which this story does not modify).

11. **Quality gates green.** `cargo test --workspace -- --test-threads=1`, `cargo fmt --check`, and `cargo clippy --all-targets --workspace -- -D warnings` all pass. The CLI-must-stay-light invariant holds: `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum) v'` is `0` (launchctl is invoked via `std::process::Command`, plist is hand-rendered XML — no new heavy deps). **Waiver (review pass 1, F5):** the workspace test gate is green **except** the documented pre-existing daemon flake `story_2_4_dropped::lag_invalidates_snapshot_coverage_resubscribe_resnapshots`, which fails identically in isolation on this host (`docs/research/test-isolation-bowerbird-findings.md` §Symptom B) and is independent of this story — Story 5.9 changes only `src/commands/*` + docs/tests and touches zero daemon code. `fmt`, `clippy --all-targets --workspace`, and the `cargo tree` lightness check are unconditionally green.

## Tasks / Subtasks

- [x] **Task 1 — Author ADR 0007** (AC: 1)
  - [x] Write `docs/decisions/0007-daemon-start-on-login.md` following the ADR format in `docs/decisions/0006-session-cwd-on-the-wire.md` (header block: Date, Status: Accepted, Deciders, Related, Implementation, `Affects context.md sections: Durability and chaos`; then Context / Decision (one sentence) / Alternatives considered / Consequences / Revisit when).
  - [x] Record alternatives: (a) launchd LaunchAgent [chosen]; (b) shim lazy-spawn [rejected — puts a subprocess fork on the shim hot path, violating ADR 0003 / project-context.md "No subprocess on the hot path"]; (c) leave manual [rejected — the dogfood finding]; (d) `KeepAlive=true` [rejected — fights `bowerbird stop`].
  - [x] State the macOS-only scope and that Linux systemd stays deferred.

- [x] **Task 2 — Daemon-path + label + plist rendering helpers** (AC: 2, 3, 4)
  - [x] Add a committed `LAUNCH_AGENT_LABEL` constant (reverse-DNS) and a `launch_agents_dir()` resolver honoring a `BOWERBIRD_LAUNCH_AGENTS_DIR` env override (test isolation, mirrors `BOWERBIRD_CLAUDE_SETTINGS`), defaulting to `$HOME/Library/LaunchAgents`.
  - [x] Add an absolute-daemon-path resolver per AC 3 (`BOWERBIRD_DAEMON_BIN` → `current_exe()` sibling → `PATH`-to-absolute; error if none).
  - [x] Add a pure `render_launch_agent_plist(label, daemon_path, data_dir) -> String` that builds the plist XML (unit-testable cross-platform, no syscalls). Include `RunAtLoad=true`, `KeepAlive={SuccessfulExit=false}`, std out/err paths.
  - [x] Decide the home for these helpers: a new `src/commands/launch_agent.rs` module (keeps `install.rs`/`uninstall.rs` thin and gives `daemon.rs`-style shared helpers a home). Wire `pub mod launch_agent;` into `commands/mod.rs`.

- [x] **Task 3 — Atomic plist write + launchctl bootstrap (macOS)** (AC: 2, 5, 6)
  - [x] `write_launch_agent_plist()` writes atomically (`.tmp` → rename, mode 0644), creating `launch_agents_dir()` if absent.
  - [x] `bootstrap_launch_agent()` runs `launchctl bootstrap gui/<uid> <plist>` (legacy `launchctl load -w` fallback); treat already-loaded as success.
  - [x] In `install::run` (macOS branch): write the plist; if `!no_start`, bootstrap it INSTEAD of `start_daemon_if_needed()`'s `setsid` spawn. Print a clear "registered bowerbird-daemon to start on login" line.

- [x] **Task 4 — launchctl bootout + plist removal (macOS)** (AC: 7, 9)
  - [x] `bootout_launch_agent()` runs `launchctl bootout gui/<uid>/<label>` (legacy `launchctl unload` fallback); treat already-unloaded as success.
  - [x] `remove_launch_agent_plist()` removes the file (missing file = clean no-op).
  - [x] In `uninstall::run` (macOS branch): if `!no_stop`, bootout; always remove the plist. Keep the existing settings.json un-merge unchanged.

- [x] **Task 5 — Linux/non-macOS path unchanged + note** (AC: 8)
  - [x] Guard all plist/launchctl logic behind `#[cfg(target_os = "macos")]`. On other targets, `install`/`uninstall` keep today's `setsid`-spawn / PID-file-stop behavior.
  - [x] On non-macOS, install prints one stderr note: supervision is macOS-only for V1.

- [x] **Task 6 — Tests** (AC: 2, 6, 7, 9, 11)
  - [x] Unit-test `render_launch_agent_plist` (cross-platform): asserts well-formed XML, presence of `Label`, the absolute `ProgramArguments` path, `RunAtLoad`, and `KeepAlive`/`SuccessfulExit=false`.
  - [x] Unit-test the absolute-daemon-path resolver (env override wins; current_exe sibling fallback).
  - [x] `tests/cli_install.rs`: `install --no-start` with `BOWERBIRD_LAUNCH_AGENTS_DIR` + `HOME` pointed at a TempDir writes the plist there (assert file exists + content); `uninstall --no-stop` removes it; round-trip leaves no residue. `--no-start`/`--no-stop` ensure NO real `launchctl` runs in CI.
  - [x] macOS-gated test (`#[cfg(target_os = "macos")]`): plist write path uses the override dir and does not touch the developer's real `~/Library/LaunchAgents`.
  - [x] Run the full gate (AC 11) including the `cargo tree` no-tokio/axum check.

- [x] **Task 7 — Docs** (AC: 10)
  - [x] Update `architecture.md` L366, L376, L502-505; backlink ADR 0007.
  - [x] Update `project-context.md` §Durability and chaos (ADR 0007 `Affects` touch).
  - [x] Update `INSTALL.md` §3 and §5.

- [x] **Task 8 — Bookkeeping**
  - [x] Replace the Story 5.9 **stub** ACs in `epics.md` (L1210-1216) with the real ACs (no renumber — the tail was already renumbered by the dogfood-triage proposal).
  - [x] `sprint-status.yaml`: `5-9-daemon-start-on-login: backlog → ready-for-dev` (done at create-story time), then dev transitions; collapse to exactly one active `last_updated:` key (history stays in the commented breadcrumb block — the Story 5.8 convention).

### Review Findings

- [x] [Review][Patch] Propagate daemon runtime environment into the LaunchAgent plist [src/commands/launch_agent.rs:145] — `render_launch_agent_plist()` only writes `ProgramArguments` plus stdout/stderr log paths. `install::supervise_or_start()` resolves `data_dir` and uses it for `StandardOutPath`/`StandardErrorPath`, but the launched daemon resolves its own data directory from `BOWERBIRD_DATA_DIR` (falling back to `$HOME/.bowerbird`) in `crates/daemon/src/main.rs:95-117`, and also honors `BOWERBIRD_INGEST_SOCK` plus `BOWERBIRD_TOKEN` at startup. Result: a macOS install run with a non-default `BOWERBIRD_DATA_DIR` can register a launchd job whose logs land in the requested data dir while the daemon DB/socket/token resolution happens against the default environment. Add an `EnvironmentVariables` plist section for the supported runtime overrides, at minimum the resolved `BOWERBIRD_DATA_DIR` (and deliberately decide/test whether `BOWERBIRD_INGEST_SOCK` and `BOWERBIRD_TOKEN` should be captured or excluded).
- [x] [Review][Patch] Handle existing/manual macOS daemons before handing lifecycle to launchd [src/commands/install.rs:88] — the macOS install path writes the plist and immediately calls `launchctl bootstrap` without checking whether a pre-5.9/manual/`bowerbird start` daemon is already running. If one is, the launchd-started daemon can fail the singleton lock and exit non-zero; with `KeepAlive={SuccessfulExit=false}`, that can become a restart loop while the old detached daemon keeps owning the PID/socket. The same lifecycle gap appears in `src/commands/start.rs:25-44` (`bowerbird start` still always uses `start_daemon_detached`) and `src/commands/uninstall.rs:54-71` (normal macOS uninstall only bootouts launchd and removes the plist, so a manually-started PID-file daemon can survive uninstall). Fix by making macOS install migrate an existing daemon into launchd ownership (stop/fail clearly before bootstrap), making `bowerbird start` use launchd when the LaunchAgent exists/is loaded, and adding a PID-file stop fallback during macOS uninstall when `--no-stop` is not set.
- [x] [Review][Patch] Narrow launchctl idempotency so real failures and stale loaded jobs are not reported as success [src/commands/launch_agent.rs:329] — `already_loaded()` treats any exit code `5` as "already loaded", and `bootstrap_launch_agent()` returns success before verifying the loaded job matches the just-written plist. That can mask unrelated `Bootstrap failed: 5` errors, and it can leave launchd supervising an old ProgramArguments/env configuration after reinstall. Match explicit "already loaded" stderr signatures and/or verify with `launchctl print gui/<uid>/<label>`; when the plist changed, bootout/re-bootstrap or surface a clear "loaded job not updated" error. Add unit coverage around the idempotency parser or introduce a launchctl seam for macOS CLI tests.
- [x] [Review][Patch] Validate the daemon path is launchable before writing/bootstrapping the plist [src/commands/launch_agent.rs:62] — `resolve_daemon_bin_absolute()` accepts any absolute `BOWERBIRD_DAEMON_BIN` verbatim, and the sibling/PATH branches only check `is_file()`, not executable permission. This can register a LaunchAgent that launchd cannot exec, turning install into a "successful" dead registration. Validate that the selected path exists and is executable before writing/bootstrapping; if `--no-start` intentionally permits pre-registration before the binary exists, document that exception and keep the stricter check on the bootstrap path. Update tests to use a temp executable rather than a placeholder absolute path.
- [x] [Review][Patch] Correct the quality-gate record so AC 11 is not claimed satisfied while the workspace test gate is red [docs/bmad/implementation-artifacts/5-9-daemon-start-on-login.md:195] — AC 11 says `cargo test --workspace -- --test-threads=1`, `cargo fmt --check`, and clippy all pass, but the Dev Agent Record says the full workspace gate failed one daemon test. `docs/bmad/implementation-artifacts/sprint-status.yaml:96` also says "11 ACs satisfied" while recording that failure. Either make the workspace gate green, formally quarantine/waive the known pre-existing daemon flake in the AC/story record, or change the story/sprint wording so it no longer claims AC 11 was satisfied.

#### Review-pass-1 resolution (2026-06-03)

All five findings resolved on the CLI (`src/commands/*`) + docs; **no `crates/` change**, so the protocol-changelog gate stays green and the daemon code is untouched.

- **F1 (env propagation).** `render_launch_agent_plist()` now takes an `env: &[(&str, &str)]` slice and emits an `EnvironmentVariables` dict. `install::supervise_or_start()` (macOS) always embeds the resolved **absolute** `BOWERBIRD_DATA_DIR` (canonicalized, so the launchd daemon resolves DB/socket where the CLI pointed the logs) and embeds `BOWERBIRD_INGEST_SOCK` when it is set in the install env. `BOWERBIRD_TOKEN` is **deliberately excluded** — the plist is mode 0644 and a bearer token in a world-readable file is a secret leak; under launchd the daemon resolves the token from the keychain/config (recorded in ADR 0007 Consequences). New unit tests assert the env dict is present, carries the data dir, omits the token, and is omitted entirely when empty.
- **F2 (lifecycle ownership).** Before bootstrap, macOS install now disarms the two states that would crash-loop a launchd daemon under `KeepAlive={SuccessfulExit=false}`: if the agent is already loaded (reinstall) it `bootout`s first so the new plist's `ProgramArguments`/`EnvironmentVariables` take effect, then re-bootstraps; if a manual/pre-5.9 daemon owns the socket it stops it via the PID file (failing loudly rather than bootstrapping into a singleton-lock loop). `bowerbird start` (macOS) now drives launchd when the LaunchAgent is registered (`bootstrap` if unloaded, `kickstart` if loaded-but-down, "already running" if up) and only falls back to the `setsid` spawn when no plist exists. macOS `uninstall` adds a PID-file stop fallback (non-fatal) after `bootout` so a manually-started daemon does not survive uninstall. New integration test covers the bootstrap-path validation; the launchctl round-trips remain the manual macOS dogfood step.
- **F3 (idempotency).** `already_loaded` no longer treats a bare exit-5 as success; classification is now explicit-stderr-signal matching extracted into pure, cross-platform-tested helpers (`stderr_signals_already_loaded` / `signals_already_unloaded`), and `bootstrap_launch_agent`/`bootout_launch_agent` confirm the end state by positive `launchctl print` verification (`launch_agent_loaded()`). Reinstall freshness is handled by F2's bootout-then-rebootstrap. New unit tests pin the narrowed classifier (bare `Bootstrap failed: 5` is now a real failure).
- **F4 (launchable path).** New `is_executable_file()` + `ensure_daemon_launchable()`; the sibling/PATH resolution branches now require an executable (not just `is_file`), and the bootstrap path validates the daemon is an executable file before writing/bootstrapping (no dead registration). `--no-start` keeps the documented pre-registration exception (env override trusted verbatim, no exec check). New unit test (temp executable) + integration test (install without `--no-start` + non-executable `BOWERBIRD_DAEMON_BIN` fails clearly and writes no plist, never invoking launchctl).
- **F5 (gate record).** See the corrected AC 11 note below and the Debug Log — the full workspace gate is green **except** the documented pre-existing daemon flake `lag_invalidates_snapshot_coverage_resubscribe_resnapshots`, which fails identically in isolation on this host (`docs/research/test-isolation-bowerbird-findings.md` §Symptom B) and is independent of this story (Story 5.9 touches zero daemon code). It is formally waived here rather than claimed green.
- **Non-macOS hardening (incidental).** The whole `launch_agent` module is gated `#[cfg(any(target_os = "macos", test))]` — every caller is already macOS-gated, so on the Linux lane the module's helpers no longer risk dead-code warnings while the cross-platform unit tests still run.

#### Code review pass 2 findings (2026-06-03)

- [x] [Review][Patch] Re-check and stop any existing daemon after launchd bootout, using the effective ingest socket and PID outcome [src/commands/install.rs:154] — `supervise_or_start()` only probes for a manual/pre-5.9 daemon in the `else` branch when `launch_agent_loaded()` is false. If an agent is loaded and a manual daemon also owns the singleton lock, install bootouts the agent and immediately bootstraps, recreating the crash-loop F2 was meant to prevent. The probe also always checks `data_dir.join("ingest.sock")` even when `BOWERBIRD_INGEST_SOCK` is captured into the plist; a daemon using the custom socket is invisible to the handoff code. Finally, when `daemon_is_up()` is true, the result of `stop_daemon_via_pid_file()` is ignored; `StopOutcome::NotRunning` with a live socket should fail clearly rather than bootstrap into a daemon that cannot be stopped by the PID file. Fix by deriving one effective ingest socket path, running the existing-daemon handoff after any bootout regardless of loaded state, and treating a live socket plus `NotRunning` as a blocker.
- [x] [Review][Patch] Make `bowerbird start` reconcile an existing manual daemon before bootstrapping or claiming launchd ownership [src/commands/start.rs:44] — when a plist exists but the LaunchAgent is unloaded, `start_daemon()` calls `bootstrap_launch_agent()` without checking whether a manual daemon is already accepting on the ingest socket. That can fail the daemon singleton lock and produce the same launchd restart loop as install. The loaded+socket-up branch also prints "daemon already running under launchd" without proving launchd owns the live PID; a manual daemon can satisfy the socket probe while the loaded agent is down or stale. Fix by checking the effective daemon/socket state before bootstrap/kickstart and by avoiding the launchd-owned message unless launchd ownership is actually verified or the wording is neutral.
- [x] [Review][Patch] Do not report uninstall success after a real launchd bootout/verification failure [src/commands/uninstall.rs:62] — `teardown_supervision()` downgrades every `bootout_launch_agent()` error to a warning, then removes the plist and prints "removed bowerbird-daemon login registration". `bootout_launch_agent()` already normalizes already-unloaded cases as success, so remaining errors are real failures that can leave a loaded LaunchAgent supervising the current session. The same path is weakened by `agent_loaded()` mapping any `launchctl print` spawn/permission/transient error to `false` in `src/commands/launch_agent.rs:393`, which lets `bootout_launch_agent()` treat an unverifiable state as unloaded. Return a real error for bootout failures (or at minimum do not claim success), and make launchd state verification fallible so "cannot verify" is not collapsed into "not loaded."
- [x] [Review][Patch] Use one canonical absolute data directory for both plist logs and daemon environment [src/commands/install.rs:101] — install computes `data_dir_abs` for `BOWERBIRD_DATA_DIR` but passes the original `data_dir` to `render_launch_agent_plist()`, so `StandardOutPath` and `StandardErrorPath` can be relative or symlink-divergent while the daemon runs against the canonical env path. The `canonicalize(...).unwrap_or_else(|_| data_dir.clone())` fallback can also embed a relative `BOWERBIRD_DATA_DIR`, which the daemon rejects at startup. Fix by converting the data dir to an absolute path once, failing if that is impossible, then passing that same absolute path to the renderer and the `EnvironmentVariables` entry.
- [x] [Review][Patch] Make atomic LaunchAgent writes safe under concurrent installs [src/commands/launch_agent.rs:262] — `write_launch_agent_plist()` always uses `plist_path.with_extension("plist.tmp")`. Two concurrent `bowerbird install` runs can overwrite each other's temp file or race the rename, producing a plist from the wrong invocation or a spurious failure. Use a unique temp path in the same directory (for example pid/thread/random suffix) and keep the final rename atomic.
- [x] [Review][Patch] Add a launchctl seam or focused macOS tests for the new lifecycle branches [tests/cli_install.rs:324] — the current integration tests mostly use `--no-start`/`--no-stop`, and the bootstrap-path test fails before launchctl. That leaves the highest-risk behavior unexercised: loaded-agent + manual-daemon handoff, plist-exists-but-unloaded + manual-daemon `start`, bootout failure handling, kickstart, and launchctl print verification. Add a small `LaunchctlRunner`/command seam or equivalent focused tests so these branches can be tested without invoking real launchctl in CI.
- [x] [Review][Patch] Align the story/ADR label-reuse wording with the intentional PID-file-only `bowerbird stop` behavior [docs/bmad/implementation-artifacts/5-9-daemon-start-on-login.md:27] — AC 2 says the LaunchAgent label is reused by install, uninstall, start, and stop, and ADR 0007 repeats that wording. The same ADR later says `bowerbird stop` is intentionally not modified, and `src/commands/stop.rs` remains PID-file-only. That behavior is coherent with `KeepAlive={SuccessfulExit=false}`, but the spec wording makes a literal AC audit fail. Update the AC/ADR wording to say the label is reused by install/uninstall/start, while stop remains PID-file SIGTERM and does not need the label.

#### Review-pass-2 resolution (2026-06-03)

All seven findings resolved on the CLI (`src/commands/*`) + tests + docs; **no `crates/` change**, so the protocol-changelog gate stays green and daemon code is untouched.

- **F1 (install existing-daemon handoff).** `supervise_or_start()` now runs the existing-daemon probe **unconditionally after any bootout** (not only in the not-loaded branch), so a manual daemon launched by a now-booted-out agent can't survive into the bootstrap and crash-loop the singleton lock. The probe uses the **effective ingest socket** via the new `commands::effective_ingest_sock()` (honors `BOWERBIRD_INGEST_SOCK`, the same value embedded in the plist) so a daemon on a custom socket is visible. A live socket + `StopOutcome::NotRunning` now **bails** ("a daemon is accepting on … but no bowerbird PID file points at a stoppable process") instead of bootstrapping a daemon launchd can't manage.
- **F2 (`bowerbird start` reconcile).** `start` probes the effective socket first: if a daemon is already accepting, it prints **neutral** "daemon already running" (no "under launchd" — a manual daemon can satisfy the probe while the loaded agent is stale) and returns without bootstrapping/kickstarting over it. Only a **down** socket leads to kickstart (loaded) / bootstrap (registered).
- **F3 (uninstall failure honesty + fallible verification).** `launch_agent_loaded()` / `agent_loaded()` are now `anyhow::Result<bool>`: `Err` = "cannot verify" (launchctl spawn/permission failure), `Ok(false)` = positively-verified absent — no longer collapsing "cannot verify" into "not loaded". `bootout_launch_agent` propagates verification failures (`agent_loaded(uid)?`). `uninstall::teardown_supervision` now treats a real `bootout` failure as **fatal** (propagates, does NOT remove the plist or print "removed login registration"), since `bootout_launch_agent` already normalizes the already-unloaded case; the manual-daemon PID-file stop stays non-fatal.
- **F4 (one canonical data dir).** `supervise_or_start()` does `create_dir_all` then `canonicalize` **once**, failing loudly if no absolute path resolves (the old `unwrap_or(data_dir)` could embed a relative `BOWERBIRD_DATA_DIR` the daemon rejects). That single `data_dir_abs` is passed to **both** the plist renderer (log paths) and the `BOWERBIRD_DATA_DIR` env entry, so logs and DB/socket can't diverge.
- **F5 (concurrent-install-safe atomic write).** `write_launch_agent_plist` uses a per-writer temp name (`<plist>.<pid>.<seq>.tmp`, `unique_tmp_path`) in the plist's own directory, with best-effort temp cleanup on a failed `set_permissions`/`rename`; the final rename stays atomic. New unit test pins distinctness + same-dir placement.
- **F6 (launchctl test seam).** New fake-`launchctl`-on-PATH seam in `tests/cli_install.rs` (`FAKE_LAUNCHCTL` script + `with_fake_launchctl` helper) drives the real `bowerbird` binary through the macOS lifecycle without real launchd. Three macOS-gated tests: reinstall-over-loaded-agent (asserts bootout precedes bootstrap), uninstall-bootout-failure (asserts non-zero exit + plist retained), and start-kickstart (loaded-but-down agent is kickstarted, not re-bootstrapped).
- **F7 (label-reuse wording).** AC 2 + ADR 0007 corrected: the label is reused by install/uninstall/**start**; `bowerbird stop` stays PID-file SIGTERM only and never addresses launchd by label.

Gate after pass-2 fixes: `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings`, and `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum) v' == 0` all green. The **`bowerbird` CLI package** — this story's entire code surface — is **138 passed / 0 failed** across all 15 test targets (`cargo test -p bowerbird`), including the new F5 unit test and the three F6 lifecycle integration tests. The full `cargo test --workspace -- --test-threads=1` run hits the **documented intermittent daemon-`contract_daemon` hang** on this host (Story 5.3 known issue / `docs/research/test-isolation-bowerbird-findings.md`) — independent of this story (zero daemon code touched); when it does complete, the only failure is the pre-existing `lag_invalidates_snapshot_coverage_resubscribe_resnapshots` daemon flake (fails identically in isolation: 0 passed / 1 failed / 184 filtered out, §Symptom B). Both daemon issues are formally waived per AC 11 (F5).

## Dev Notes

### What this story changes vs. what it must preserve

**Files that change (all UPDATE except the new ADR / launch_agent module):**
- `src/commands/install.rs` — macOS branch writes + bootstraps the LaunchAgent; `--no-start` writes-but-doesn't-bootstrap.
- `src/commands/uninstall.rs` — macOS branch boots out + removes the plist; `--no-stop` removes-but-doesn't-bootout.
- `src/commands/launch_agent.rs` — **NEW** module (label const, dir/path resolvers, plist render, atomic write, bootstrap/bootout wrappers).
- `src/commands/mod.rs` — add `pub mod launch_agent;` (and possibly a shared `current_exe`-sibling resolver).
- `docs/decisions/0007-daemon-start-on-login.md` — **NEW** ADR.
- `architecture.md`, `project-context.md`, `INSTALL.md`, `epics.md`, `sprint-status.yaml` — doc/bookkeeping.

**Current install behavior that must keep working (read `src/commands/install.rs`):**
1. settings.json merge (`adapter_claude::install`) and its created/already-present/legacy-upgrade messaging — UNCHANGED.
2. `tool-reactions.toml` seed (Story 5.4) with the stdout-clean / stderr-hint discipline — UNCHANGED.
3. `--no-start` short-circuit — EXTENDED (now also gates the launchctl bootstrap, not just the spawn).
4. On Linux, the `setsid`-detached spawn (`daemon::start_daemon_detached`) — UNCHANGED.

**Current uninstall behavior that must keep working (read `src/commands/uninstall.rs`):**
1. settings.json un-merge — UNCHANGED.
2. Daemon-stop failures are non-fatal (warn to stderr, exit 0) — KEEP this posture for `bootout` failures too.
3. `~/.bowerbird/` is never deleted — UNCHANGED.

### The load-bearing interaction: KeepAlive vs. `bowerbird stop`

This is the easiest way to ship something broken. `bowerbird stop` (`src/commands/stop.rs` → `daemon::stop_daemon_via_pid_file`) sends SIGTERM and expects the daemon to stay down. If the plist used `KeepAlive=true`, launchd would **immediately restart** the daemon after every `stop`, silently breaking the stop command and the uninstall stop path.

The fix (AC 4): `KeepAlive = { SuccessfulExit = false }`. The daemon's Story 2.5 graceful shutdown exits **0** on SIGTERM, so launchd sees a clean exit and leaves it down; a crash exits non-zero and launchd restarts it. `RunAtLoad = true` independently covers reboot/login. Verify the daemon actually exits 0 on SIGTERM before relying on this (it does per Story 2.5 / the `Stopped` outcome in `daemon.rs`, but confirm). This interaction belongs in ADR 0007's Consequences and is why `bowerbird stop` is NOT modified by this story (clean exit + SuccessfulExit=false means stop still works).

> Out-of-scope but worth a one-line deferred-work note if you hit it: `bowerbird stop` while a LaunchAgent is loaded only *stops* the daemon; it does not *disable* RunAtLoad, so a later login restarts it. That's the intended supervision behavior, not a bug. If a "pause supervision without uninstalling" need emerges, that's a follow-up story, not this one.

### launchctl: modern vs. legacy API

- Modern (macOS 10.11+): `launchctl bootstrap gui/<uid> <plist>` to load, `launchctl bootout gui/<uid>/<label>` to unload. `<uid>` is `libc::getuid()` (the CLI already links `libc`).
- Legacy: `launchctl load -w <plist>` / `launchctl unload -w <plist>`. Keep as a fallback if `bootstrap`/`bootout` errors in a way that suggests the modern API is unavailable, but prefer modern.
- Idempotency: bootstrapping a loaded agent or booting out an unloaded one returns a non-zero exit / specific message — treat these "already in target state" cases as success. Match on the exit status / stderr, don't blindly `?`.
- Invoke via `std::process::Command::new("launchctl")` — no new deps. Capture output so a real failure surfaces a useful message; do not let launchctl write to the user's terminal uncontrolled.

### Daemon binary path resolution (AC 3) — the subtle one

`resolve_daemon_bin()` (in `commands/mod.rs`) returns the PATH-relative string `"bowerbird-daemon"` today. That works for `setsid` spawn (inherits the shell's PATH) but **fails under launchd** (minimal PATH, `/usr/local/bin` typically absent). The plist must embed an absolute path. Add a resolver:
1. `BOWERBIRD_DAEMON_BIN` if set and absolute (tests / non-standard installs).
2. Sibling of `std::env::current_exe()` — the CLI and daemon ship together (INSTALL.md installs all three to the same dir).
3. `PATH` search canonicalized to absolute.
4. Error (no plist written) if none resolves — better than a plist launchd silently fails to exec.

### Test isolation (mirror the existing patterns)

- `tests/cli_install.rs` and `tests/cli_lifecycle.rs` are the templates: `assert_cmd::Command::cargo_bin`, `env_remove` the `BOWERBIRD_*` vars, then set `HOME`/`BOWERBIRD_DATA_DIR` to a `TempDir`. Add a `BOWERBIRD_LAUNCH_AGENTS_DIR` override so the plist lands in the TempDir, never the developer's real `~/Library/LaunchAgents`.
- **Do NOT invoke real `launchctl` in CI tests.** Use `--no-start`/`--no-stop` so install/uninstall write/remove the plist file but skip bootstrap/bootout. The actual launchctl round-trip is a manual macOS dogfood step in the validation phase (analogous to how `cli_install.rs` bypasses the real daemon spawn via `--no-start` and leaves the spawn to `contract_daemon.rs`).
- Keep `render_launch_agent_plist` a pure function so its unit test runs on Linux CI too.
- **Smoke-test gotcha (from Story 5.8):** running a workspace-built daemon against the maintainer's live `~/.bowerbird/bower.db` can migrate it and lock out the installed daemon. If you dogfood the real `launchctl` path, point `BOWERBIRD_DATA_DIR` at a temp dir, or be ready to `cargo install --path crates/daemon --force` + `bowerbird start` (one-time Keychain Allow). Pre-announce any command that triggers a macOS Keychain/Touch ID prompt.

### Previous-story intelligence (5.8, just shipped)

- 5.8's review took **4 passes**, mostly doc/bookkeeping drift. Get the docs (AC 10) and bookkeeping (Task 8) right the first time: keep `sprint-status.yaml` at exactly one active `last_updated:` key, history in the commented breadcrumb block.
- stdout vs stderr discipline is enforced by tests: operator *notes/hints* go to **stderr** (so scripted stdout stays clean); the primary success line goes to stdout. The Linux "macOS-only" note (AC 8) and any skip hints are stderr.
- The changelog gate (`protocol_changelog_gate`) only fires on `crates/protocol/src/*.rs` touches. This story touches none, so no `protocol-changelog.md` entry — confirm the gate stays green precisely because protocol is untouched.

### Architecture / project-context compliance

- **Shim is NOT touched.** The whole point of choosing launchd over lazy-spawn is to keep the shim a pure thin client (project-context.md §Shim hot-path discipline: "No subprocess on the hot path"). Any change under `crates/shim/` in this story is a red flag.
- **CLI stays light** (architecture.md §CLI framework): no `tokio`/`axum`/`reqwest`. launchctl via `std::process::Command`, plist as a hand-built string. AC 11's `cargo tree` check enforces this.
- **macOS-only matches the no-list posture** (no Windows; Linux packaging is community-driven). This story does not add Linux supervision — it documents that systemd stays deferred.
- **`anyhow` only at the binary edge** — `commands/*` helpers should return typed-ish errors or `anyhow::Result` consistent with the existing `commands/daemon.rs` style (that module already uses `anyhow::Result` + `Context`, so matching it is fine for the CLI binary).

### Project Structure Notes

- New module `src/commands/launch_agent.rs` parallels `src/commands/daemon.rs` (shared lifecycle helpers). No conflict with the unified structure.
- Alignment check: subcommand surface is unchanged (no new clap subcommand) — install/uninstall gain behavior, not new commands. architecture.md §CLI framework's alphabetical subcommand list stays accurate.
- The only structural variance is the architecture.md reversal (launchd was "deferred"); ADR 0007 is the supersession record, so this is a sanctioned change, not drift.

### References

- [Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-06-01-dogfood-triage.md#4.3] — Finding 1 scope, launchd-not-lazy-spawn rationale, install-writes-plist / uninstall-removes-it, macOS-only, shim-not-changed.
- [Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-06-01-dogfood-triage.md#Section 1] — the reboot dogfood incident (Cluster A, session `ad3eaed4`).
- [Source: docs/bmad/planning-artifacts/epics.md#Story 5.9] — the stub this story replaces (L1210-1216); `Affects context.md sections: Durability and chaos`.
- [Source: docs/dogfooding-feedback.md] — 2026-06-01 Finding 1 entry (the trigger).
- [Source: src/commands/install.rs] — current install contract to preserve (settings merge, tool-reactions seed, `--no-start`).
- [Source: src/commands/uninstall.rs] — current uninstall contract (settings un-merge, non-fatal stop, never-delete-data-dir, `--no-stop`).
- [Source: src/commands/daemon.rs] — `setsid` spawn + PID-file stop helpers; the module to parallel; `pid_alive`/`StopOutcome` semantics; `libc` already linked.
- [Source: src/commands/mod.rs] — `resolve_daemon_bin()` (PATH-relative, needs an absolute variant for the plist), `resolve_bowerbird_dir()`, env-override pattern.
- [Source: tests/cli_install.rs] — install/uninstall round-trip test template + `HOME`/TempDir isolation + stdout-vs-stderr assertions.
- [Source: tests/cli_lifecycle.rs] — `--test-threads=1`, `BOWERBIRD_DATA_DIR`/`BOWERBIRD_DAEMON_BIN` isolation, keychain-disable pattern.
- [Source: INSTALL.md#3] — the install contract prose to extend (LaunchAgent registration) and §5 (uninstall removal).
- [Source: docs/bmad/planning-artifacts/architecture.md#Decision Priority Analysis] — L366/L376 "launchd deferred post-V1" (to correct).
- [Source: docs/bmad/planning-artifacts/architecture.md#Infrastructure & Deployment] — L502-505 "Process supervision (V1)" + the CLI-no-tokio invariant + `cargo tree` verification command.
- [Source: docs/decisions/0006-session-cwd-on-the-wire.md] — ADR format/structure to mirror for ADR 0007.
- [Source: docs/decisions/0003-shim-p99-budget-on-macos-latest.md] — the shim hot-path budget that lazy-spawn would violate (ADR 0007 cites it).
- [Source: docs/no-list.md] — no Windows / Linux-packaging-is-community posture that "macOS-only supervision" matches.
- [Source: docs/bmad/project-context.md#Durability and chaos] — the section ADR 0007 affects (add the supervision note).
- [Source: docs/bmad/project-context.md#Shim hot-path discipline] — "No subprocess on the hot path" (why not lazy-spawn).

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context) via the bmad-dev-story workflow.

### Debug Log References

- Full gate (`cargo test --workspace -- --test-threads=1`), after review-pass-1 fixes: **500 passed, 1 failed**. The only failure is `story_2_4_dropped::lag_invalidates_snapshot_coverage_resubscribe_resnapshots` in `crates/daemon/tests/contract_daemon.rs` — a **pre-existing, documented** wall-clock-fragile daemon test (`docs/research/test-isolation-bowerbird-findings.md` §"Symptom B", full investigation in `docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md`). Reconfirmed pass-1: it fails identically run in isolation 3× on this host (184 filtered out, that one fails) and Story 5.9 touches **zero** daemon code (diff is CLI `src/commands/*` + docs/tests only), so it is independent of this story. Formally waived in AC 11 (F5) rather than claimed green.
- `cargo fmt --check` green; `cargo clippy --all-targets --workspace -- -D warnings` green; `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum) v'` = `0` (CLI-stays-light invariant holds — launchctl via `std::process::Command`, plist hand-rendered). The `launch_agent` module is now `#[cfg(any(target_os = "macos", test))]` so the Linux lane compiles its cross-platform unit tests without dead-code warnings on the macOS-only helpers (verified `cargo clippy --all-targets` is clean on macOS; the Linux dep build-scripts can't run on this host, but the module-gate removes the dead-code class structurally).

### Completion Notes List

- **macOS supervision via launchd LaunchAgent (ADR 0007).** `bowerbird install` (macOS) writes `~/Library/LaunchAgents/com.technicalpickles.bowerbird.daemon.plist` atomically (`.tmp`→rename, mode 0644) with an **absolute** `ProgramArguments` daemon path, `RunAtLoad=true`, and the load-bearing `KeepAlive={SuccessfulExit=false}` (crash-restart without fighting `bowerbird stop`'s graceful exit-0 — confirmed against `crates/daemon/src/main.rs`: handled SIGTERM → `run()` returns Ok → fall off `main` → exit 0; error/panic → `exit(1)`/`exit(130)`). Install bootstraps the agent (`launchctl bootstrap gui/<uid>`) instead of the `setsid` spawn.
- **Symmetric uninstall.** `bowerbird uninstall` (macOS) boots the agent out (`launchctl bootout`) and removes the plist; bootout is non-fatal (mirrors the daemon-stop posture) so removal still proceeds; round-trip leaves no residue.
- **Idempotency (narrowed in review pass 1, F3).** `bootstrap`/`bootout` decide "already in the target state" by **positive `launchctl print` verification** (`launch_agent_loaded()`), not a bare exit code: a bare `Bootstrap failed: 5` is now a real failure, while explicit "already loaded"/"already in progress"/`failed: 37` stderr signatures (and ESRCH=3 / "no such"/"could not find" for unload) short-circuit. Classification lives in pure, cross-platform-tested helpers; legacy `launchctl load -w` / `unload` remains the fallback.
- **`--no-start` / `--no-stop`.** Write/remove the plist but skip the launchctl bootstrap/bootout — the launchctl-free path that keeps real `launchctl` out of CI. All new integration tests use these flags.
- **Linux unchanged.** All plist/launchctl logic is `#[cfg(target_os = "macos")]`; on other targets install/uninstall keep the `setsid`-spawn / PID-file-stop behavior and install prints one **stderr** note that supervision is macOS-only for V1 (systemd deferred).
- **Shim untouched** (the whole point of launchd-over-lazy-spawn) and no `crates/protocol/src` touch (no protocol-changelog entry; the changelog gate stays green precisely because protocol is untouched).
- **Test-isolation note for reviewers:** the shared `bowerbird_bin()` helper in `tests/cli_install.rs` now sets an absolute `BOWERBIRD_DAEMON_BIN` placeholder and `env_remove`s `BOWERBIRD_LAUNCH_AGENTS_DIR`, so the pre-existing settings/seed tests stay HOME-isolated (plist lands under the per-test `$HOME/Library/LaunchAgents`) and don't gain a hard dependency on `bowerbird-daemon` being built. The two `launch_agent` unit tests that mutate `BOWERBIRD_DAEMON_BIN` rely on the workspace `--test-threads=1` run requirement.

### File List

- `docs/decisions/0007-daemon-start-on-login.md` — NEW. ADR 0007 (launchd LaunchAgent decision, KeepAlive semantics, macOS-only scope, alternatives).
- `src/commands/launch_agent.rs` — NEW. Label constant, dir/plist-path resolvers, absolute-daemon-path resolver (requires executable for sibling/PATH), pure plist renderer + XML escape + `EnvironmentVariables` (F1), `is_executable_file`/`ensure_daemon_launchable` (F4 pass-1), atomic plist write **with per-writer unique temp + cleanup (F5 pass-2)**, plist removal, macOS-gated launchctl bootstrap/bootout + `kickstart`; **`launch_agent_loaded`/`agent_loaded` now `Result<bool>` — fallible print verification so "cannot verify" ≠ "not loaded" (F3 pass-2)**; pure cross-platform idempotency classifiers; unit tests (incl. `unique_tmp_path` distinctness).
- `src/commands/mod.rs` — `pub mod launch_agent;` gated `#[cfg(any(target_os = "macos", test))]`; **macOS-gated `effective_ingest_sock()` helper (honors `BOWERBIRD_INGEST_SOCK`) shared by install/start (F1/F2 pass-2)**.
- `src/commands/install.rs` — macOS `supervise_or_start` embeds `EnvironmentVariables` (token excluded — F1 pass-1); **canonicalizes the data dir once (create+canonicalize, fail-loud) and passes that single abs path to BOTH the plist renderer and the env entry (F4 pass-2)**; validates the daemon is launchable before bootstrap; **runs the existing-daemon handoff UNCONDITIONALLY after any bootout, probing the effective socket, and bails on live-socket + `StopOutcome::NotRunning` (F1 pass-2)**; `--no-start` writes-but-doesn't-bootstrap; non-macOS keeps `setsid` spawn + macOS-only stderr note.
- `src/commands/uninstall.rs` — macOS `teardown_supervision`: **bootout failure is now FATAL (propagates, plist retained, no false "removed" claim — F3 pass-2)**; non-fatal PID-file stop fallback for a manual daemon retained; non-macOS keeps PID-file stop.
- `src/commands/start.rs` — macOS `start_daemon` probes the **effective socket first**: a live daemon → neutral "daemon already running" + return (no bootstrap over it, no unverified launchd-ownership claim — F2 pass-2); only a down socket leads to `kickstart` (loaded) / `bootstrap` (registered); `setsid` fallback when no plist exists; non-macOS unchanged.
- `tests/cli_install.rs` — pass-1 macOS tests retained; **pass-2 adds a fake-`launchctl`-on-PATH seam (`FAKE_LAUNCHCTL` + `with_fake_launchctl`) and 3 macOS-gated lifecycle tests (F6): reinstall-over-loaded-agent (bootout-before-bootstrap ordering), uninstall-bootout-failure (non-zero exit + plist retained), start-kickstart (loaded-but-down → kickstart, not re-bootstrap)**.
- `docs/bmad/planning-artifacts/architecture.md` — §Decision Priority Analysis, Deferred-Decisions list, and §Infrastructure & Deployment "Process supervision (V1)" corrected for macOS launchd-now-V1; ADR 0007 backlinked.
- `docs/bmad/project-context.md` — §Durability and chaos gains the launchd-supervision note (ADR 0007 `Affects` touch).
- `INSTALL.md` — §3 (e)/(f) and §5 document the LaunchAgent registration + removal.
- `docs/bmad/planning-artifacts/epics.md` — Story 5.9 stub ACs replaced with the real ACs (no renumber).
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — `5-9` → in-progress → review → in-progress; single active `last_updated:` key.
- `docs/bmad/implementation-artifacts/5-9-daemon-start-on-login.md` — this story file (status, tasks, Review Findings, Dev Agent Record).

### Change Log

- 2026-06-03 — Story created via `bmad-create-story` from the §4.3 stub. ADR 0007 was NOT pre-landed (proposal §5 step 2 was skipped between 5.8 and here), so authoring it is Task 1 (mirrors how Story 5.8's ADR 0008 was authored during the story). Key design calls seeded for the dev: `KeepAlive={SuccessfulExit=false}` to avoid fighting `bowerbird stop`, absolute daemon-path resolution for launchd's minimal PATH, `BOWERBIRD_LAUNCH_AGENTS_DIR` + `--no-start`/`--no-stop` for launchctl-free CI tests, and the architecture.md "deferred post-V1" reversal. Status: backlog → ready-for-dev.
- 2026-06-03 — Implemented (dev-story): authored ADR 0007; added `src/commands/launch_agent.rs` (label/resolvers/pure plist renderer/atomic write/removal/launchctl bootstrap+bootout); wired macOS supervision into `install`/`uninstall` with `--no-start`/`--no-stop` honored and all launchd logic `#[cfg(target_os = "macos")]`-gated; non-macOS keeps `setsid`/PID-file behavior + a macOS-only stderr note; tests (2 unit + 4 integration); docs (architecture.md, project-context.md, INSTALL.md) and bookkeeping (epics.md, sprint-status.yaml). Gate green except the pre-existing `lag_invalidates_snapshot_coverage_resubscribe_resnapshots` daemon flake (documented; no daemon code touched). Status: in-progress → review.
- 2026-06-03 — Code review pass 1 (`bmad-code-review`): documented five unresolved patch findings in the Review Findings section — launchd plist does not propagate daemon runtime env, macOS install/start/uninstall lifecycle can leave or create unsupervised daemons instead of launchd-owned lifecycle, launchctl idempotency is too broad and can mask failures/stale jobs, daemon path validation can register an unlaunchable binary, and AC 11 / sprint status claim all gates satisfied despite the red full-workspace gate. Removed the duplicate story-created changelog entry while documenting the review. Status: review → in-progress.
- 2026-06-03 — Review pass 1 resolved (all five findings; see §Review-pass-1 resolution). F1: `render_launch_agent_plist` gained an `EnvironmentVariables` slice; install embeds resolved abs `BOWERBIRD_DATA_DIR` + optional `BOWERBIRD_INGEST_SOCK`, excludes the token (0644-secret-leak) — ADR 0007 Consequences updated. F2: install/start/uninstall are launchd-aware (bootout-then-rebootstrap on reinstall, stop a manual daemon before bootstrap, `bowerbird start` uses launchd when registered, uninstall PID-file stop fallback). F3: positive `launchctl print` verification + narrowed pure stderr classifiers (bare exit-5 no longer masks). F4: `ensure_daemon_launchable`/`is_executable_file` gate the bootstrap path; `--no-start` keeps pre-registration. F5: AC 11 + Debug Log corrected — gate is 500 passed / 1 failed, the single failure being the documented pre-existing `lag_invalidates_snapshot_coverage_resubscribe_resnapshots` daemon flake (independent; formally waived). Incidental: `launch_agent` module gated `cfg(any(macos, test))` to keep the Linux lane dead-code-clean. New tests: +4 unit (env-omit, executable validator, narrowed load/unload classifiers) + 1 integration (F4 unlaunchable). fmt + clippy --all-targets --workspace + cargo-tree lightness all green. Still `crates/`-free (no protocol-changelog entry). Status: in-progress → review (ready for a fresh code-review pass).
- 2026-06-03 — Code review pass 2 (`bmad-code-review`): documented seven unresolved patch findings in the Review Findings section — install/start can still bootstrap over a manual daemon in loaded-agent/custom-socket/no-stoppable-PID cases, uninstall can claim success after a real bootout or launchd verification failure, plist log paths can diverge from the canonical data dir, fixed `.plist.tmp` weakens atomicity under concurrent installs, the launchctl lifecycle branches need a test seam/coverage, and the AC/ADR label-reuse wording contradicts the intentional PID-file-only `bowerbird stop` behavior. Status: review → in-progress.
- 2026-06-03 — Review pass 2 resolved (all seven findings; see §Review-pass-2 resolution). F1: install runs the existing-daemon handoff unconditionally after any bootout, probing the effective socket (`commands::effective_ingest_sock`), and bails on live-socket + `StopOutcome::NotRunning`. F2: `bowerbird start` probes the effective socket first and prints neutral "daemon already running" instead of bootstrapping over a live/manual daemon or claiming unverified launchd ownership. F3: `launch_agent_loaded`/`agent_loaded` are now `Result<bool>` (fallible verification, "cannot verify" ≠ "not loaded"); `uninstall` treats a real bootout failure as fatal (plist retained, no false success). F4: data dir canonicalized once (create+canonicalize, fail-loud) and the single abs path feeds both the plist log paths and `BOWERBIRD_DATA_DIR` env. F5: atomic plist write uses a per-writer unique temp (`<plist>.<pid>.<seq>.tmp`) with cleanup. F6: new fake-`launchctl`-on-PATH seam + 3 macOS-gated lifecycle tests (reinstall handoff, uninstall bootout-failure, start kickstart). F7: AC 2 + ADR 0007 label-reuse wording corrected to install/uninstall/start (stop stays PID-file-only). Still `crates/`-free (no protocol-changelog entry). fmt + clippy --all-targets --workspace + cargo-tree lightness green; the `bowerbird` CLI package (this story's surface) is 138 passed / 0 failed across all 15 targets. The full `--test-threads=1` workspace run hits the documented intermittent daemon-`contract_daemon` hang on this host (Story 5.3 known issue, independent — zero daemon code touched); on completion the only failure is the waived `lag_invalidates_snapshot_coverage_resubscribe_resnapshots` flake. Status: in-progress → review (ready for a fresh code-review pass).
