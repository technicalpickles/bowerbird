# 0007. Daemon start-on-login supervision via a launchd LaunchAgent (macOS)

Date: 2026-06-03
Status: Accepted
Deciders: @pickles
Related: sprint-change-proposal-2026-06-01-dogfood-triage.md (Finding 1, §3 rationale, §4.3 change list — the dogfood reboot incident that motivates this); Story 5.9 (`docs/bmad/implementation-artifacts/5-9-daemon-start-on-login.md` — the implementation); ADR-0003 (`docs/decisions/0003-shim-p99-budget-on-macos-latest.md` — the shim hot-path budget that the rejected lazy-spawn alternative would violate); Story 2.5 (graceful shutdown exits 0 on SIGTERM — the precondition the `KeepAlive` choice depends on)
Implementation: `src/commands/launch_agent.rs` (new); `src/commands/install.rs`; `src/commands/uninstall.rs`; `src/commands/mod.rs`; `docs/bmad/planning-artifacts/architecture.md`; `docs/bmad/project-context.md`; `INSTALL.md`
Affects context.md sections: Durability and chaos

## Context

The daemon is the substrate's only durable component: while it is down, the shim cannot reach `~/.bowerbird/ingest.sock` and every event in the window is dropped. The shim never blocks Claude (it fails fast and exits), so a missing daemon costs *events*, not the user's coding session — but the events are gone with no recovery.

A live dogfooding session on 2026-06-01 (`docs/dogfooding-feedback.md`, session `ad3eaed4-af27-4bb0-9844-f0e237defbc1`) made this concrete: the workstation rebooted, `bowerbird-daemon` did not come back, and for ~90 seconds every Claude Code tool call logged a `hook error / Failed with non-blocking status code` while the shim failed to reach the dead socket. Recovery was a manual `bowerbird start`. This is **Finding 1** of the dogfood-triage proposal.

Process supervision was **deferred** post-V1 in `architecture.md` (§Decision Priority Analysis, §Infrastructure & Deployment) — *deferred, not cut*. It is not on `docs/no-list.md`. This ADR reverses the macOS half of that deferral: launchd start-on-login + crash-restart becomes V1 for macOS. Linux systemd integration stays deferred (the no-list posture is "no Windows; Linux packaging is community-driven," and the primary dogfooding target is macOS).

## Decision (one sentence)

On macOS, `bowerbird install` registers the daemon as a launchd LaunchAgent (`~/Library/LaunchAgents/<label>.plist`, `RunAtLoad=true`, `KeepAlive={SuccessfulExit=false}`) so a reboot/login starts it and a crash restarts it, and `bowerbird uninstall` removes that registration symmetrically; the shim is not touched, and Linux keeps today's `setsid`-detached spawn.

Specifics:

- **Label.** A single committed reverse-DNS constant `com.technicalpickles.bowerbird.daemon` (matching the [bowerbird-deck](https://github.com/technicalpickles/bowerbird-deck) owner namespace), reused by install, uninstall, and start. `bowerbird stop` intentionally stays PID-file SIGTERM only (it does not address launchd by label) — see the load-bearing `KeepAlive` interaction in Consequences.
- **Plist contents.** `Label`, `ProgramArguments = [<absolute-path-to-bowerbird-daemon>]`, `RunAtLoad = true`, `KeepAlive = { SuccessfulExit = false }`, and `StandardOutPath`/`StandardErrorPath` under the data dir (`~/.bowerbird/daemon.out.log` / `daemon.err.log`). Written atomically (`.tmp` → rename, mode `0644`).
- **Absolute daemon path.** The plist embeds an absolute path resolved as `BOWERBIRD_DAEMON_BIN` (if set and absolute) → a sibling of `std::env::current_exe()` → a `PATH` search canonicalized to absolute. If none resolves, install fails with a clear error rather than writing a plist launchd cannot exec. (`resolve_daemon_bin()`'s PATH-relative `"bowerbird-daemon"` works for `setsid` spawn, which inherits the shell PATH, but launchd's minimal PATH lacks `/usr/local/bin`.)
- **Bootstrap.** Install loads the agent with `launchctl bootstrap gui/<uid> <plist>` (modern API; `launchctl load -w` legacy fallback) instead of the `setsid` spawn — launchd owns the lifecycle, and the daemon's singleton PID lock would reject a double-start anyway. `--no-start` writes the plist but skips bootstrap (launchctl-free CI path).
- **Bootout.** Uninstall removes the agent with `launchctl bootout gui/<uid>/<label>` (legacy `launchctl unload` fallback) and deletes the plist. `--no-stop` removes the plist but skips bootout. `~/.bowerbird/` is never deleted (unchanged contract).
- **Idempotency.** Bootstrapping an already-loaded agent and booting out an already-unloaded one are treated as success, not error (match on exit status / stderr, do not blindly `?`).

## Alternatives considered

- **launchd LaunchAgent [chosen].** The OS-native supervisor on macOS. `RunAtLoad` covers reboot/login; `KeepAlive={SuccessfulExit=false}` covers crash-restart; the CLI invokes `launchctl` via `std::process::Command` and hand-renders the plist XML, so no new heavy deps and the CLI-stays-light invariant holds.
- **Shim lazy-spawn (shim starts the daemon when it finds the socket dead).** Rejected. This puts a subprocess fork on the shim hot path, directly violating ADR-0003 and `project-context.md` §"Shim hot-path discipline" ("No subprocess on the hot path"). The shim's whole value is that it never blocks Claude; forking a daemon (plus the race of N concurrent shims all trying to spawn) trades the substrate's one hard performance contract for a convenience the OS already provides.
- **Leave supervision manual (status quo).** Rejected — this is exactly the dogfood finding. A reboot silently drops events until the maintainer notices and runs `bowerbird start`.
- **`KeepAlive = true`.** Rejected. Unconditional keep-alive makes launchd **immediately restart** the daemon after every `bowerbird stop` (and the uninstall stop path), silently breaking those commands. Because Story 2.5's graceful shutdown exits **0** on SIGTERM, `{SuccessfulExit = false}` lets a clean stop stay down while a crash (non-zero exit) still restarts — the behavior we actually want.
- **Linux systemd in the same story.** Rejected for scope. macOS is the primary dogfooding target; systemd user-units have their own lifecycle quirks and warrant a deliberate pass. Linux keeps the `setsid`-detached spawn and install prints a one-line stderr note that supervision is macOS-only for V1.

## Consequences

- **The load-bearing interaction (`KeepAlive` vs. `bowerbird stop`).** `bowerbird stop` sends SIGTERM and expects the daemon to stay down. With `{SuccessfulExit = false}`, launchd sees the clean exit-0 and leaves it down; `bowerbird stop` is therefore NOT modified by this story. A crash (exit non-zero) is the only thing that triggers a launchd restart. Confirmed against `crates/daemon/src/main.rs`: a handled SIGTERM lets `run()` return `Ok`, `main` falls off the end → exit 0; error/panic paths `std::process::exit(1)` (or 130 on a second signal).
- **`bowerbird stop` does not disable `RunAtLoad`.** Stopping a loaded agent stops the running daemon but a later login restarts it — that is the intended supervision behavior, not a bug. A "pause supervision without uninstalling" need, if it emerges, is a follow-up story (tracked in `deferred-work.md`), not this one.
- **Shim is untouched.** Choosing launchd over lazy-spawn is precisely what keeps the shim a pure thin passthrough. Any `crates/shim/` change in the implementing story is a red flag.
- **CLI stays light.** `launchctl` via `std::process::Command`, plist as a hand-built string — no `tokio`/`axum`/`reqwest`. The per-story `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum) v' == 0` check enforces this.
- **No wire-protocol change.** This story touches no `crates/protocol/src/*.rs`, so the `protocol_changelog_gate` stays green precisely because protocol is untouched — no `docs/protocol-changelog.md` entry.
- **Testability.** A `BOWERBIRD_LAUNCH_AGENTS_DIR` env override (mirroring `BOWERBIRD_CLAUDE_SETTINGS`) points the plist at a TempDir in CI, and `--no-start`/`--no-stop` keep real `launchctl` out of CI. The plist renderer is a pure function so its unit test runs on Linux CI too. The real `launchctl` round-trip is a manual macOS dogfood step.
- **architecture.md reversal is sanctioned, not drift.** This ADR is the supersession record for the "launchd deferred post-V1" lines; the implementing story updates those lines and backlinks here.

### Review-pass-1 refinements (2026-06-03)

Code review of the Story 5.9 implementation surfaced four behavioral gaps; the resolutions below are part of this decision:

- **Plist `EnvironmentVariables`, and the token is never embedded.** launchd jobs start from a minimal environment and do NOT inherit the shell env present at `bowerbird install` time. The plist therefore embeds the runtime overrides the daemon reads at startup — the resolved **absolute** `BOWERBIRD_DATA_DIR` (always, so the launchd-started daemon resolves its DB/socket where the CLI pointed the logs) and `BOWERBIRD_INGEST_SOCK` when set. `BOWERBIRD_TOKEN` is **deliberately excluded**: the plist is mode `0644` and a bearer token in a world-readable file is a secret leak, so under launchd the daemon resolves the token from the keychain/config chain (Story 3.3) instead.
- **launchd owns the lifecycle; install/start/uninstall disarm competitors first.** Bootstrapping over a daemon launchd does not own would fail the singleton PID lock, and `KeepAlive={SuccessfulExit=false}` would turn that into a crash-restart loop. So macOS install, before bootstrap, `bootout`s an already-loaded agent and re-bootstraps the freshly-written plist (also picking up changed `ProgramArguments`/env on reinstall), or stops a manual/pre-5.9 PID-file daemon (failing loudly rather than looping). `bowerbird start` drives launchd when the agent is registered rather than spawning a competing `setsid` daemon, and `bowerbird uninstall` removes the LaunchAgent registration, then attempts a PID-file stop of any manual / pre-5.9 daemon as a fallback — warning non-fatally if such a daemon is still accepting on the ingest socket afterward (e.g. no PID file points at it) rather than claiming it stopped everything.
- **Idempotency by positive verification, not exit-code guessing.** "Already loaded"/"already unloaded" is confirmed via `launchctl print gui/<uid>/<label>`, so an unrelated `Bootstrap failed: 5` is a real error rather than a swallowed success.
- **Refuse to register an unlaunchable daemon.** The bootstrap path validates the resolved daemon path is an executable file before writing/bootstrapping; `--no-start` keeps the pre-registration exception (the plist may be written before the binary is in place).

## Revisit when

- Linux supervision is needed for V1+ — a systemd user-unit pass is its own ADR + story (the env-override + pure-renderer shape here generalizes, but the unit semantics and `systemctl --user` lifecycle differ enough to design deliberately).
- A "pause supervision without uninstalling" need emerges — today `bowerbird stop` leaves `RunAtLoad` intact, so login restarts the daemon. A `bowerbird supervise --disable` (or similar) would be the follow-up.
- The non-loopback TCP bind decision lands (its own deferred item) — a daemon reachable off-host changes the trust model and may interact with how/whether it should auto-start.
