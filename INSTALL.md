# Installing bowerbird from a prebuilt tarball

This file is bundled inside each release tarball. The repo copy and the
tarball copy are byte-identical at release time. If you cloned the repo
instead, the `README.md` Install section covers `cargo install --path .`.

## 1. Place the binaries on your `$PATH`

After extracting the tarball, you have three binaries under `bin/`:

- `bowerbird` — the user-facing CLI (the only binary you invoke by name)
- `bowerbird-shim` — the Claude Code hot-path hook entry point (invoked by
  Claude Code, not by you)
- `bowerbird-daemon` — the long-running background service (spawned by
  `bowerbird install` and `bowerbird start`, not by you)

### macOS

```sh
# Clear the quarantine attribute on the extracted binaries (one-time per download)
xattr -d com.apple.quarantine bin/bowerbird bin/bowerbird-shim bin/bowerbird-daemon 2>/dev/null || true

# Install to /usr/local/bin (or any other $PATH directory)
sudo install -m 0755 bin/bowerbird bin/bowerbird-shim bin/bowerbird-daemon /usr/local/bin/
```

The `xattr -d` step removes the Gatekeeper quarantine flag macOS attaches to
files downloaded from the internet. Without it, macOS blocks the first
execution with a "bowerbird is from an unidentified developer" warning. The
flag is per-download, so future re-downloads need the `xattr` step too. Code
signing and notarization are deferred post-V1.

### Linux

```sh
sudo install -m 0755 bin/bowerbird bin/bowerbird-shim bin/bowerbird-daemon /usr/local/bin/
```

The Linux binaries are dynamically linked against glibc 2.35 (matches Ubuntu
22.04, Ubuntu 24.04, Debian 12+, RHEL 9.0+). musl-based distributions
(Alpine, Void) need to install from source — see the README's Install
section.

## 2. Verify the install

```sh
bowerbird --version
bowerbird-shim --version
bowerbird-daemon --version
```

All three should print a version string matching the tarball you downloaded.
If `bowerbird` is not found, confirm `/usr/local/bin` is on your `$PATH`
(`echo $PATH`).

## 3. Run `bowerbird install`

```sh
bowerbird install
```

This wires bowerbird into Claude Code and starts the daemon. The detailed
contract:

**(a) Files modified.** `~/.claude/settings.json` is read, parsed, merged
with the bowerbird hook entries, and atomically rewritten. The
`BOWERBIRD_CLAUDE_SETTINGS` env var overrides the path (useful for
development against a non-default Claude Code config).

**(b) Atomic write contract.** Read → parse → merge → write `.tmp` → fsync
→ rename. An interrupted install (process killed between write and rename)
leaves the original `settings.json` intact. The contract test
`settings_atomic_rename_under_interrupt` in
`crates/adapter-claude/tests/contract_install.rs` pins this invariant.

**(c) Hook kinds registered.** Four kinds — `PreToolUse`, `PostToolUse`,
`Stop`, `Notification`. Each gets a hook entry pointing at
`bowerbird-shim --hook-kind <KIND>`. The invocation is PATH-relative (no
path component): re-downloading to a different `$PATH` location does NOT
require re-running `bowerbird install`. The regression test
`installed_command_uses_path_relative_binary_name_no_slash_in_first_token`
pins this.

**(d) Data directory.** `~/.bowerbird/` created mode `0700` with
`ingest.sock` (mode `0600`), `bower.db` (+ `.wal`, `.shm`),
`bowerbird.pid`, and `server.json` (mode `0600`). An optional
user-created `config.toml` (mode `0600` recommended) is honored if
present.

**(e) Daemon start-on-login (macOS) / auto-start (Linux).** On **macOS**,
`bowerbird install` registers a launchd LaunchAgent at
`~/Library/LaunchAgents/com.technicalpickles.bowerbird.daemon.plist` and
bootstraps it. The agent uses `RunAtLoad` (so the daemon comes back after a
reboot or login — no more silently-dropped events until a manual restart)
and `KeepAlive = { SuccessfulExit = false }` (so a crash restarts the daemon,
but a clean `bowerbird stop` stays down). launchd owns the lifecycle, so
install does not separately spawn the daemon. If a daemon is already running
when you re-run install (a manual `bowerbird start`, or a pre-5.9 install),
install hands it to launchd cleanly — it reloads the agent or stops the
unsupervised daemon before bootstrapping, so you never end up with two daemons
or a crash-restart loop. Once the agent is registered, `bowerbird start` drives
launchd (it starts or kickstarts the LaunchAgent) rather than spawning its own
process. On **Linux**, install spawns
`bowerbird-daemon` detached via `setsid` (the spawn is idempotent: if a daemon
is already running per the singleton PID-lock, install skips it), and prints a
note that start-on-login supervision is macOS-only for V1 (systemd integration
is deferred). Pass `--no-start` to write the LaunchAgent plist (macOS) without
bootstrapping it, or to skip the spawn (Linux) — useful for scripted setups.

**(f) Uninstall.** `bowerbird uninstall` reverses (a). On **macOS** it boots
the LaunchAgent out (`launchctl bootout`, which stops the daemon) and removes
the plist; as a fallback it also attempts to stop a manually-started / pre-5.9
daemon via its PID file. If a manual daemon is still accepting on the ingest
socket after that (for example because no PID file points at it), uninstall
prints a non-fatal warning so you can stop that process yourself — the
LaunchAgent registration is still removed either way. On **Linux** it stops the
daemon (SIGTERM with 10s graceful drain, SIGKILL fallback). It does NOT delete
`~/.bowerbird/`. Your event history is
your data; re-installing should not lose it. To wipe history, `rm -rf
~/.bowerbird/` is a deliberate manual step.

**(g) Keychain entry.** First daemon start creates a Keychain entry
(macOS: Keychain Access; Linux: Secret Service) with
`service=bowerbird-daemon, user=bearer-token` and a generated UUID4 token.
macOS users see a one-time Keychain prompt; subsequent reads from the
same binary path do not re-prompt. `bowerbird auth token` retrieves the
value for tool configuration. The keychain backend convention is
established by Story 3.3; production builds disable the `mock-keyring`
feature so even an env-var injection cannot bypass the real keychain.

### `tool-reactions.toml` placement

`bowerbird install` automatically seeds `~/.bowerbird/adapters/claude/tool-reactions.toml`
from the copy bundled into the `bowerbird` binary (the same bytes that ship
in the tarball at `adapters/claude/tool-reactions.toml`). If you have
previously hand-edited the file the install command leaves it untouched and
prints a `note:` to that effect, so re-running `bowerbird install` after a
version bump never clobbers your local additions.

If the daemon ever runs without this file, the adapter falls back to
`Reaction::Unknown` for any tool not present in the TOML — reactions still
flow, they just become unhelpfully generic.

## 4. Confirm Claude Code is hooked

Start a fresh Claude Code session and run any tool (read a file, write a
file, list a directory). Then check that the daemon recorded the activity:

```sh
ls -la ~/.bowerbird/bower.db ~/.bowerbird/bower.db-wal ~/.bowerbird/bower.db-shm
bowerbird status                              # human-readable status block
bowerbird auth token | tr -d '\n' | pbcopy   # bearer token for tools (macOS)
```

If `bower.db` exists and is non-empty, the hook plumbing is working. If
`bowerbird status` cannot reach the daemon, verify the daemon is running:

```sh
cat ~/.bowerbird/bowerbird.pid             # PID of the running daemon
ps -p $(cat ~/.bowerbird/bowerbird.pid)    # confirm process is alive
```

## 5. Uninstall

```sh
bowerbird uninstall
```

This:

1. Reverses the merge into `~/.claude/settings.json` (atomically; an
   interrupted uninstall leaves the file intact).
2. Stops the daemon. On **macOS**, boots the LaunchAgent out (`launchctl
   bootout`, which terminates the daemon and ends start-on-login supervision)
   and removes
   `~/Library/LaunchAgents/com.technicalpickles.bowerbird.daemon.plist`. On
   **Linux**, sends SIGTERM with a 10s graceful drain and SIGKILL fallback.
   (`--no-stop` keeps the daemon running. On macOS it removes the plist so
   future logins will not reload the agent, but it does NOT boot out a job
   already loaded this login session: launchd keeps supervising that daemon
   until you log out or run `launchctl bootout
   gui/$(id -u)/com.technicalpickles.bowerbird.daemon`.)
3. Leaves `~/.bowerbird/` in place. Your event history survives uninstall.

To remove the binaries themselves:

```sh
sudo rm /usr/local/bin/bowerbird /usr/local/bin/bowerbird-shim /usr/local/bin/bowerbird-daemon
```

To wipe history and config:

```sh
rm -rf ~/.bowerbird/
```

The keychain entry is left in place by `bowerbird uninstall` — remove it via
your OS keychain tool (Keychain Access on macOS, `secret-tool clear` on
Linux) if you want to fully clean up.

## Further reading

- [`README.md`](README.md) — project overview, Quickstart, architecture and
  protocol pointers.
- [`docs/protocol.md`](docs/protocol.md) — REST + WebSocket + ingest-socket wire reference.
- [`docs/quickstart.md`](docs/quickstart.md) — five-minute walkthrough using the bundled replay fixture (no Claude Code session required).
- `docs/bmad/planning-artifacts/architecture.md` — system architecture.
