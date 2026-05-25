# bowerbird

bowerbird is a local-only substrate that captures Claude Code activity over
Unix-socket hook events, normalizes them via the `adapter-claude` crate,
persists them in WAL-mode SQLite, and broadcasts them to subscribed tools over
an authenticated WebSocket. The three reference examples under `examples/`
(post-V1) will demonstrate the canonical patterns; for V1, the protocol and
the daemon are the contract.

Status: V1 in development. See
[`docs/bmad/planning-artifacts/epics.md`](docs/bmad/planning-artifacts/epics.md)
for the live scope and progress.

## Quickstart

```sh
# 1. Install (downloads prebuilt binaries; copies to /usr/local/bin/)
curl -fsSL https://github.com/technicalpickles/bowerbird/releases/latest/download/bowerbird-aarch64-apple-darwin.tar.gz | tar -xz
sudo install bowerbird-*-aarch64-apple-darwin/bin/* /usr/local/bin/

# 2. Wire bowerbird into Claude Code's hooks and start the daemon
bowerbird install

# 3. Use Claude Code as normal; activity appears in ~/.bowerbird/bower.db
bowerbird auth token | tr -d '\n' | pbcopy   # copy bearer token for tool config
bowerbird status                              # render full status block
```

The Quickstart targets macOS arm64 — substitute the appropriate tarball name
for your platform from the Install section. The `releases/latest/download/...`
URL always resolves to the most recent non-prerelease tag.

## Install

Three install paths, in order of preference:

### 1. Prebuilt binary (recommended)

Each tagged release attaches three tarballs to its GitHub Release:

| Target | Tarball |
|---|---|
| macOS arm64 (Apple Silicon) | `bowerbird-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS x86_64 (Intel) | `bowerbird-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 (glibc 2.35+) | `bowerbird-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |

Download the tarball matching your platform, extract it, and place the three
binaries (`bowerbird`, `bowerbird-shim`, `bowerbird-daemon`) on your `$PATH`:

```sh
tar -xzvf bowerbird-vX.Y.Z-<target>.tar.gz
cd bowerbird-vX.Y.Z-<target>
sudo install -m 0755 bin/* /usr/local/bin/
```

**macOS Gatekeeper workaround.** The prebuilt binaries are not signed (Apple
Developer ID certificates are deferred post-V1). On first run, macOS will
block the binaries with a Gatekeeper warning. Clear the quarantine attribute:

```sh
xattr -d com.apple.quarantine /usr/local/bin/bowerbird /usr/local/bin/bowerbird-shim /usr/local/bin/bowerbird-daemon 2>/dev/null || true
```

**musl Linux is deferred post-V1** (NFR9). On musl-based distributions
(Alpine, Void, etc.) install from source instead:

```sh
cargo install --git https://github.com/technicalpickles/bowerbird --tag vX.Y.Z
```

Windows is an explicit V1 scope cut (see `docs/no-list.md` once Story 4.3
lands).

See [`INSTALL.md`](INSTALL.md) (also bundled in each release tarball) for the
post-extract walkthrough: verification, `bowerbird install`, uninstall
semantics, and the `tool-reactions.toml` placement step.

### 2. From source via `cargo install`

Requires a stable Rust toolchain (NFR10 — no nightly features used). MSRV is
1.82; the workspace pins channel `1.94.1` in `rust-toolchain.toml` for CI
reproducibility. `Cargo.lock` is committed; `--locked` builds reproduce the
exact dependency graph.

```sh
# Install from a tagged release directly:
cargo install --git https://github.com/technicalpickles/bowerbird --tag vX.Y.Z --locked

# Or from a local clone:
git clone https://github.com/technicalpickles/bowerbird
cd bowerbird
cargo install --path . --locked
```

Both forms drop the `bowerbird` CLI binary at `~/.cargo/bin/bowerbird`. To
also install the shim and daemon binaries, build them separately:

```sh
cargo install --path crates/shim --locked
cargo install --path crates/daemon --locked
```

### 3. Crates.io

Deferred post-V1. The `bowerbird` namespace on crates.io may already be
squatted; reclaiming or republishing requires owning the name. For V1, install
via prebuilt binary or `cargo install --git`. A future story will publish the
workspace crates once the namespace is secured.

## `bowerbird install` walkthrough

`bowerbird install` wires bowerbird into your Claude Code configuration and
starts the daemon. Specifically:

(a) **Files modified.** `~/.claude/settings.json` is read, parsed, merged
with the bowerbird hook entries, and atomically rewritten. The
`BOWERBIRD_CLAUDE_SETTINGS` env var overrides the path (useful for
development against a non-default Claude Code config).

(b) **Atomic write contract.** Read → parse → merge → write `.tmp` → fsync →
rename. An interrupted install (process killed between write and rename)
leaves the original `settings.json` intact. The contract test at
`crates/adapter-claude/tests/contract_install.rs` covers this.

(c) **Hook kinds registered.** Four kinds — `PreToolUse`, `PostToolUse`,
`Stop`, `Notification`. Each gets a hook entry pointing at
`bowerbird-shim --hook-kind <KIND>`. The invocation is **PATH-relative**
(no path component): re-downloading to a different `$PATH` location does
NOT require re-running `bowerbird install`.

(d) **Data directory.** `~/.bowerbird/` is created mode `0700` with:

| Path | Mode | Purpose |
|---|---|---|
| `ingest.sock` | `0600` | Unix-socket hook ingest endpoint |
| `bower.db` (+ `.wal`, `.shm`) | SQLite-managed | Event + state store |
| `bowerbird.pid` | `0644` | Singleton-daemon PID lockfile |
| `server.json` | `0600` | Daemon's bound HTTP address |
| `config.toml` | `0600` (recommended) | Optional user-created daemon config |

(e) **Daemon auto-start.** `bowerbird install` spawns the daemon detached as
part of the install flow. The spawn is idempotent: if a daemon is already
running per the singleton PID-lock, the install skips it. Pass `--no-start`
to opt out (useful for scripted setups).

(f) **Uninstall semantics.** `bowerbird uninstall` reverses (a) — the
bowerbird hook entries are removed from `settings.json` — and stops the
daemon (SIGTERM with 10s graceful drain, SIGKILL fallback). It does NOT
delete `~/.bowerbird/`. Your event history is your data; re-installing
should not lose it. Explicit data-directory cleanup is `rm -rf
~/.bowerbird/` and is a deliberate manual step.

(g) **Keychain entry.** First daemon start creates a Keychain entry
(macOS: Keychain Access; Linux: Secret Service) with
`service=bowerbird-daemon, user=bearer-token` and a generated UUID4 token.
macOS users see a one-time Keychain prompt; subsequent reads from the same
binary path do not re-prompt. Retrieve the token with `bowerbird auth
token` for tool configuration.

## Architecture

See [`docs/bmad/planning-artifacts/architecture.md`](docs/bmad/planning-artifacts/architecture.md)
for the full system shape: crate boundaries, async runtime configuration,
WebSocket subsystem config knobs, and the daemon's startup sequence.

## Protocol

See [`docs/protocol-changelog.md`](docs/protocol-changelog.md) for the
immutable change history (additive forward-compat policy for outbound
messages; `deny_unknown_fields` strict on inbound). The consolidated
`docs/protocol.md` reference is in flight under Story 4.3.

## Contributing

V1 is solo-developed by pickles. Open issues at
<https://github.com/technicalpickles/bowerbird/issues>. The Story-Automator
under `docs/bmad/story-automator/` is how stories move from Epic →
ready-for-dev → done.

## License

Dual-licensed under MIT OR Apache-2.0 at your option. See
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
