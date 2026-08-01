# Cookbook

Recipes for common presenter problems. Each entry is a self-contained directory: a `README.md` explaining the pattern (what it is, how to run it, how to apply it) colocated with the runnable TypeScript reference code (`src/`, `package.json`, `tsconfig.json`). The code is the canonical implementation; the README is the explanation. There is no separate surface to keep in sync.

| Entry | The problem |
|-------|-------------|
| [`state-session-fanout/`](state-session-fanout/) | I need to track every session as it appears and route state to a per-session model. |
| [`rest-cursor-pagination/`](rest-cursor-pagination/) | I need to fetch a session's history via REST and handle event-log truncation gracefully. |
| [`dropped-frame-recovery/`](dropped-frame-recovery/) | My WebSocket dropped or the daemon restarted; how do I catch up without losing events? |

The patterns are orthogonal; read the one matching your use case.

## Why TypeScript on Node

Most presenter authors reach for Node first when consuming WebSocket + REST surfaces, so that's where the reference code lands. The substrate doesn't care what speaks WebSocket + JSON; any language with a JSON parser and a WebSocket client works the same way.

No SDK is shipped. The protocol is small enough that each entry self-contains its ~30 lines of interface declarations; duplication is the right cost for read-and-run reference code. (See project-context.md §Example presenters for the authoritative decision.) There are no runtime npm dependencies; each entry's dev-deps exist only for the optional `npm run typecheck`, and the runtime path is plain `node --experimental-strip-types`.

## Node version requirement

Node 22.6+ for the native `--experimental-strip-types` flag, which lets `.ts` files run directly without a compile step. Node 22 is LTS through 2027; future Node releases (23+) make the flag unnecessary.

If `node --version` reports anything older than v22.6.0, see [nodejs.org/en/download](https://nodejs.org/en/download/); `mise`, `nvm`, `fnm`, `volta`, and `asdf` are all good ways to manage Node versions.

## Quick run

```sh
bowerbird start
bowerbird replay
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"

node --experimental-strip-types docs/cookbook/state-session-fanout/src/index.ts
# Ctrl-C when you've seen enough state frames

node --experimental-strip-types docs/cookbook/rest-cursor-pagination/src/index.ts session-alpha

node --experimental-strip-types docs/cookbook/dropped-frame-recovery/src/index.ts &
sleep 1
bowerbird stop && bowerbird start && bowerbird replay
# the presenter prints {event:"recovered",recovered_count:N} after catch-up
kill %1
```

See each entry's `README.md` for expected output, how the pattern works, and adaptation hints.

## If it didn't work

- **`BOWERBIRD_TOKEN env var not set`**: step 3 of the quick run didn't happen in this shell. Re-run `export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"` in the same shell as the entry.
- **`cannot read ~/.bowerbird/server.json`**: the daemon isn't running. Run `bowerbird start` first.
- **`node: bad option: --experimental-strip-types`**: your Node is older than 22.6; see the version section above.

## Not a Cargo zone

These directories are a TypeScript project zone, NOT part of the Cargo workspace. The root `Cargo.toml`'s `[workspace] members = ["crates/*"]` deliberately does not include them, so `cargo build --workspace`, `cargo clippy --workspace`, and `cargo test --workspace` stay Rust-only. The TypeScript smoke is invoked by `tests/cli_examples.rs` (a workspace-root test crate) which spawns `node --experimental-strip-types` as a subprocess, and CI typechecks each entry (`tsc --noEmit`) on every PR. The authoritative decision is `docs/bmad/project-context.md` §Example presenters.

More recipes will follow as patterns emerge. Open an issue if you have a use case the existing three don't cover.
