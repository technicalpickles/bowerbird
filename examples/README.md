# bowerbird examples

Three TypeScript reference tools demonstrating the canonical patterns every long-running bowerbird presenter needs. Read the one matching your use case; the patterns are orthogonal.

## Three reference examples

| Example | Pattern | Cookbook anchor |
|---|---|---|
| [`multi-session-router/`](multi-session-router/) | Subscribe to `state.session.*`, route state frames to a per-session map, treat first appearance as "new session appeared." | `state-session-fanout` |
| [`event-log-viewer/`](event-log-viewer/) | REST cursor-pagination via `GET /sessions/<id>/events?since=<cursor>`, with gap-detection from `oldest_available_event_id`. No WebSocket. | `rest-cursor-pagination` |
| [`reconnect-recovery/`](reconnect-recovery/) | Long-running WebSocket consumer with `Close`/`Dropped` → REST catch-up → re-subscribe resilience flow. | `dropped-frame-recovery` |

Each example is self-contained under its own directory with `src/index.ts`, `package.json`, `tsconfig.json`, and `README.md`. The `reconnect-recovery` example additionally carries a `tests/recover.test.ts` exercising the recovery function via Node's built-in `--test` runner.

## Why TypeScript on Node

Most presenter authors reach for Node first when consuming WebSocket + REST surfaces; that's where the documentation lands. The substrate doesn't care what speaks WebSocket + JSON, so the examples ship as TypeScript on Node 22.6+ rather than as Rust workspace members.

No SDK is shipped. The protocol is small enough that each example self-contains its ~30 lines of interface declarations — duplication is the right cost for read-and-run reference code. (See project-context.md §Example presenters for the authoritative decision.)

## Node version requirement

Node 22.6+ for the native `--experimental-strip-types` flag, which lets `.ts` files run directly without a compile step. Node 22 is LTS through 2027; future Node releases (23+) make the flag unnecessary.

If `node --version` reports anything older than v22.6.0, see [nodejs.org/en/download](https://nodejs.org/en/download/) — `mise`, `nvm`, `fnm`, `volta`, and `asdf` are all good ways to manage Node versions.

## Quick run

```sh
bowerbird start
bowerbird replay
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"

node --experimental-strip-types examples/multi-session-router/src/index.ts
# Ctrl-C when you've seen enough state frames

node --experimental-strip-types examples/event-log-viewer/src/index.ts session-alpha

node --experimental-strip-types examples/reconnect-recovery/src/index.ts &
sleep 1
bowerbird stop && bowerbird start && bowerbird replay
# the example prints {event:"recovered",recovered_count:N} after catch-up
kill %1
```

See each example's `README.md` for sample output, anatomy, and adaptation hints.

## Cookbook anchors

Each `src/index.ts` carries a `// cookbook-begin:<name>` / `// cookbook-end:<name>` marker pair around its canonical pattern. The anchors are pure comments — they have no runtime effect, no preprocessing, and the example runs identically with or without them.

Story 4.3's documentation suite will define the inlining mechanism (mdBook `{{#include}}` directives, or a hand-rolled build step) that consumes these markers to generate cookbook entries. For now they are a forward-compat hook; Story 4.2 ships only the markers and a doc-drift guardrail (`tests/cli_examples_drift.rs::each_example_source_carries_cookbook_anchors`) asserting they remain present.

## Architecture reconciliation note

This directory is a TypeScript project zone, NOT a Cargo workspace zone. The root `Cargo.toml`'s `[workspace] members = ["crates/*"]` deliberately does NOT include `"examples/*"`. The decision sources are:

- **`docs/bmad/project-context.md` §Example presenters** is the authoritative call: "TypeScript, runs on Node. Lives in `examples/`. No build step beyond `tsc`."
- **`docs/bmad/planning-artifacts/architecture.md`** was updated by Story 4.2 to match (the prior architecture.md draft had a Rust shape; Story 4.2 surgically replaced the §Project Structure and §Examples boundary blocks).
- **`cargo build --workspace`, `cargo clippy --workspace`, `cargo test --workspace`** remain semantically clean — they cover Rust only. The TypeScript smoke is invoked by `tests/cli_examples.rs` (a workspace-root test crate) which spawns `node --experimental-strip-types` as a subprocess.

This mirrors Story 4.1's architecture.md reconciliation pattern.

## No runtime dependencies

Each example uses only Node built-ins: `WebSocket` (stable in Node 22 LTS via undici), `fetch`, `fs`, `path`, `os`, `child_process`, `process`, `http` (for the recovery test's fake daemon). No `ws` package, no `node-fetch`. The only optional dev-dep is `typescript` for the `npm run typecheck` script; CI does not install dev-deps for the smoke run.

## Troubleshooting

- **`BOWERBIRD_TOKEN env var not set`** — Retrieve your token: `bowerbird auth token`.
- **`cannot read ~/.bowerbird/server.json`** — The daemon isn't running. Start it: `bowerbird start`.
- **`node: bad option: --experimental-strip-types`** — Your Node is older than 22.6. Upgrade via your version manager.
- **Reactions render as `Unknown`** — The adapter falls back when `~/.bowerbird/adapters/claude/tool-reactions.toml` is missing; copy it from your install tarball. See [`INSTALL.md`](../INSTALL.md).
