# event-log-viewer

## What this example demonstrates

REST cursor-pagination + gap-detection via `oldest_available_event_id`. Loops `GET /sessions/<id>/events?since=<cursor>` until `cursor === null`, renders each event as tab-separated `<event_id>\t<kind>\t<tool>\t<reaction>`. Demonstrates Story 1.7's cursor contract and the gap-detection mechanical fact: the daemon emits `oldest_available_event_id`; the presenter decides whether the gap matters.

No WebSocket — this example is the canonical "fetch history via REST, render, exit" pattern. The WS path is owned by `multi-session-router` (live state fan-out) and `reconnect-recovery` (resilience).

Illustrates project-context.md Axiom 4: mechanical facts (the cursor, the oldest-event-id) live in the protocol; semantics (what a gap means for your tool) live in the presenter.

## Run it

```sh
bowerbird start
bowerbird replay
node --experimental-strip-types examples/event-log-viewer/src/index.ts session-alpha
```

Without a session-id argument, defaults to `session-alpha` (the first session in the bundled fixture).

Requires Node 22.6+ for `--experimental-strip-types`.

## Expected output

Tab-separated lines, one per event in the bundled fixture's `session-alpha`:

```
1	PreToolUse	Read	Continue
3	PostToolUse	Read	Continue
5	PreToolUse	Edit	Continue
7	PostToolUse	Edit	Continue
9	Notification	-	-
11	Stop	-	-
```

Pipe through `column -t -s$'\t'` for a pretty table:

```sh
node --experimental-strip-types examples/event-log-viewer/src/index.ts session-alpha | column -t -s$'\t'
```

## Anatomy

The cookbook anchor `// cookbook-begin:rest-cursor-pagination` … `// cookbook-end:rest-cursor-pagination` wraps the canonical fetch-loop: cursor initialization, loop body, cursor update, gap-detection branch.

The `extractToolName(payload)` helper parses the JSON-string `payload` field defensively — Axiom 1 says payloads ride verbatim, so a future binary-payload adapter would render as `-` rather than crash the viewer.

## Troubleshooting

**Every reaction renders as `Unknown`.** The daemon falls back to `Reaction::Unknown` when `~/.bowerbird/adapters/claude/tool-reactions.toml` is missing. Copy it from the install tarball or your repo checkout into `~/.bowerbird/adapters/claude/` — see `INSTALL.md` for the placement step.

**`session ... not found`.** Run `bowerbird export <session-id>` (no arg lists all sessions) to see what's actually in your event log. The bundled-fixture sessions are `session-alpha` and `session-beta`.

## Authentication

```sh
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
```
