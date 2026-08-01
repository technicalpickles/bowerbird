# Quickstart

Install bowerbird, start the daemon, replay a bundled fixture, run a reference example, watch live JSON state. About five minutes end to end, install included. No Claude Code session required.

The bundled fixture exists so you can exercise the pub/sub path without configuring Claude Code first. Once you have seen it work, [`bowerbird install`](../README.md#install) wires it into Claude Code's real hooks.

## Before you start

- **Node 22.6 or newer.** The reference examples run as plain `.ts` files via Node's `--experimental-strip-types` flag, no build step. Check with `node --version`; if you are below 22.6, upgrade at [nodejs.org/en/download](https://nodejs.org/en/download/).
- **A clone of this repo.** The prebuilt tarball ships binaries only; the reference examples are TypeScript files in the repo:

  ```sh
  git clone https://github.com/technicalpickles/bowerbird
  cd bowerbird
  ```

  Run every step below from that directory.

## Step 0: install bowerbird

On macOS arm64 (Apple Silicon), download the current release candidate, clear the quarantine attribute, and put the three binaries on your `$PATH`:

```sh
curl -fsSL https://github.com/technicalpickles/bowerbird/releases/download/v0.1.0-rc3/bowerbird-v0.1.0-rc3-aarch64-apple-darwin.tar.gz | tar -xz
xattr -d com.apple.quarantine bowerbird-v0.1.0-rc3-aarch64-apple-darwin/bin/* 2>/dev/null || true
sudo install -m 0755 bowerbird-v0.1.0-rc3-aarch64-apple-darwin/bin/* /usr/local/bin/
bowerbird --version
```

The last command prints a version string; that is this step's success signal. On another platform, substitute your tarball name from the [install table](../README.md#install); [`INSTALL.md`](../INSTALL.md) is the detailed walkthrough.

## Step 1: start the daemon

```sh
bowerbird start
```

You see two lines (your pid and port will differ):

```
started bowerbird-daemon (pid 12345)
daemon ready at http://127.0.0.1:49152
```

## Step 2: replay the bundled fixture

```sh
bowerbird replay
```

This feeds 12 recorded hook events through the daemon, as if two Claude Code sessions were live:

```
using bundled fixture (12 events across 2 sessions)
replayed 12 events from bundled-fixture
```

## Step 3: export the bearer token

```sh
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
```

The token lands in the variable, so the only visible output is a hint line such as `bowerbird: loaded token from system keychain` (macOS may show a one-time Keychain prompt; click Allow). Confirm the variable is set:

```sh
echo $BOWERBIRD_TOKEN
```

That prints a UUID.

## Step 4: run a reference example

In the same shell that ran step 3:

```sh
node --experimental-strip-types docs/cookbook/state-session-fanout/src/index.ts
```

The terminal prints a connect line, then one JSON object per line as the fixture's sessions get fanned out through the substrate:

```
connected to daemon 0.1.0 (protocol 1.0)
new session: claude/session-alpha
{"event":"state","source":"claude","session_id":"session-alpha","current_state":"Idle","last_event_kind":"Stop"}
new session: claude/session-beta
{"event":"state","source":"claude","session_id":"session-beta","current_state":"Idle","last_event_kind":"Stop"}
```

You should now see `{event:"state",source:...,session_id:...}` JSON objects scrolling on stdout. If that line is missing, jump to the troubleshooting section at the bottom. Depending on timing you may also see both sessions move to `"current_state":"Ended"`: the fixture's recorded processes are long gone, and the daemon notices. Ctrl-C when you have seen enough.

## Step 5: stop the daemon

```sh
bowerbird stop
```

```
sending SIGTERM to bowerbird-daemon (pid 12345)
daemon stopped
```

## Next steps

You have seen the substrate work end to end. Where to go depends on what you want next:

- [`docs/presenter-authoring.md`](presenter-authoring.md): build your own tool. WebSocket frame handling, REST snapshots, dropped-frame recovery.
- [`docs/protocol.md`](protocol.md): look up wire details when you need them.
- [`docs/cookbook/`](cookbook/): self-contained recipes for specific patterns, each colocating its prose README with the runnable reference code.

## If it didn't work

- **`bowerbird: command not found`**: the binary isn't on your `$PATH`. Re-check step 0 and confirm `/usr/local/bin` (or wherever you put it) is on your `$PATH` (`echo $PATH`).
- **`BOWERBIRD_TOKEN env var not set`**: step 3 didn't run, or the shell that runs step 4 isn't the same one that exported the env var. Re-run `export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"` in the same shell as step 4.
- **`node: bad option: --experimental-strip-types`**: your Node is older than 22.6. Run `node --version` to confirm; upgrade per the Before-you-start section.
