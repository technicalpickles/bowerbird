# Quickstart

Start the daemon, replay a bundled fixture, run a reference example, see live JSON state. About five minutes. No Claude Code session required.

The bundled fixture exists so you can exercise the pub/sub path without configuring Claude Code first. Once you're comfortable with the substrate, [`bowerbird install`](../README.md#install) wires it into Claude Code's real hooks.

## Prerequisites

- **`bowerbird --version` works.** If not, install via the [prebuilt tarball](../README.md#install) or [`cargo install --git`](../README.md#2-from-source-via-cargo-install). The detailed walkthrough is in [`INSTALL.md`](../INSTALL.md).
- **Node 22.6 or newer** for the `--experimental-strip-types` flag (lets `.ts` files run directly with no build step). Check with `node --version`; upgrade via [nodejs.org/en/download](https://nodejs.org/en/download/) or a version manager like `mise`, `nvm`, `fnm`, `volta`, or `asdf`. This mirrors the floor [`examples/README.md`](../examples/README.md) names.
- **A source clone.** The prebuilt tarball ships binaries only; the TypeScript reference examples live in the repo. Grab them with `git clone https://github.com/technicalpickles/bowerbird && cd bowerbird` if you don't already have a checkout.

## Five steps

```sh
bowerbird start                                                    # 1. start daemon
bowerbird replay                                                   # 2. populate pub/sub from bundled fixture
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"      # 3. get bearer token
node --experimental-strip-types examples/multi-session-router/src/index.ts   # 4. run a reference example
# Ctrl-C when you've seen enough.  Then:
bowerbird stop                                                     # 5. clean up
```

## What you should see

After step 4, the terminal starts printing one JSON object per line as the bundled fixture's events get fanned out through the substrate:

```
{"event":"state","source":"claude","session_id":"session-alpha","current_state":"Idle","last_event_kind":"PostToolUse"}
{"event":"state","source":"claude","session_id":"session-beta","current_state":"Working","last_event_kind":"PreToolUse"}
```

You should now see `{event:"state",source:...,session_id:...}` JSON objects scrolling on stdout. If that line is missing, jump to the next section.

## If it didn't work

- **`bowerbird: command not found`** — The binary isn't on your `$PATH`. Re-check the install steps in [`INSTALL.md`](../INSTALL.md) and confirm `/usr/local/bin` (or wherever you put it) is on your `$PATH` (`echo $PATH`).
- **`BOWERBIRD_TOKEN env var not set`** — Step 3 didn't run, or the shell that runs step 4 isn't the same one that exported the env var. Re-run `export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"` in the same shell as step 4.
- **`node: bad option: --experimental-strip-types`** — Your Node is older than 22.6. Run `node --version` to confirm; upgrade per the Prerequisites section.

## Next steps

- [`docs/presenter-authoring.md`](presenter-authoring.md) — understand the pieces: WebSocket frame handling, REST snapshots, dropped-frame recovery.
- [`docs/protocol.md`](protocol.md) — look up wire details when you need them.
- [`docs/cookbook/`](cookbook/) — recipes for specific patterns, paired with the reference examples under [`examples/`](../examples/).
