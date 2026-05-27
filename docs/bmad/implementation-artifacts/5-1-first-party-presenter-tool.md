# Story 5.1: First-party presenter tool (sibling repository)

Status: in-progress

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As the bowerbird maintainer,
I want a real presenter tool I can use daily against live Claude Code sessions,
so that dogfooding has a useful surface to observe — not just JSON in a terminal — and the friction I find informs the rest of Epic 5.

Source: `docs/bmad/planning-artifacts/epics.md` §"Story 5.1: First-party presenter tool (sibling repository)" (lines 900–927). Full epic rationale in `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-26.md`. This is the dogfooding-and-presenter cornerstone of Epic 5: bench-gate hardening (5.2), release-pipeline E2E (5.3), install UX polish (5.4), cookbook consolidation (5.5), first-time-reader docs (5.6), and the v0.1.0 tag (5.8) all read the friction signal that this story generates.

**Resolved design decisions** (the epic AC explicitly defers these to story creation):

- **Sibling repo name:** `bowerbird-deck`. Generic "small dashboard" framing — doesn't lock the UI form, so a future macOS menu-bar variant can land in the same repo without a rename.
- **UI form:** Terminal TUI on Node 22.6+ with `--experimental-strip-types`. Cross-platform, low-friction, reuses the Story 4.2 reference-example shape (no bundler, no SDK). The TUI runs in a persistent terminal window the maintainer keeps visible for ambient awareness.
- **Demonstrated cookbook pattern:** `docs/cookbook/state-session-fanout.md`. The presenter's primary signal is `state.session.<id>.current_state` across active sessions — the same shape `examples/multi-session-router/src/index.ts` already exercises. AC #5 names this as the README's cookbook-pattern reference.
- **Remote location:** the dev agent should propose the GitHub org/repo URL at the start of work (default: `github.com/technicalpickles/bowerbird-deck`) and commit it to the story before publishing. The architecture.md backlink edit requires a real URL.

## Acceptance Criteria

1. **Given** the sibling repository `bowerbird-deck` exists (a brand-new repo, separate from `bowerbird/` — NOT inside `crates/` or `examples/`) **When** the maintainer runs the presenter against a locally running `bowerbird` daemon connected to a live Claude Code session **Then** the presenter surfaces, in a terminal TUI:
   - One line (or row/card) per active session, showing `session_id`, `current_state` (one of `idle` / `working` / `waiting-on-input` / `Unknown` — the `SessionCurrentState` enum from `crates/protocol/src/state.rs`), and the most recent tool-use reaction (`Reaction` enum from `crates/protocol/src/reaction.rs`, displayed by name; `Unknown` rendered as a generic placeholder).
   - A live update on every `state.session.<id>` envelope received from the WebSocket subsystem.
   - A graceful "no active sessions" state when the daemon is reachable but has nothing to show.
   - A clearly-signaled disconnected state with auto-reconnect when the daemon is unreachable or the WS handshake fails.

2. **Given** the presenter is installed on the maintainer's main machine **When** the maintainer codes with Claude Code for at least 5 working days **Then** the presenter is the maintainer's *actual* signal source for "is Claude doing something" — used in preference to alt-tabbing to the terminal. Evidence lands in the story's Dev Agent Record › Completion Notes as a one-paragraph dogfooding log naming the calendar dates and any cases where the maintainer fell back to the terminal (with reason).

3. **Given** the presenter is in a sibling repository, not in `crates/` or `examples/` **When** a reader of the bowerbird repository looks at `docs/bmad/planning-artifacts/architecture.md` §Frontend Architecture (currently lines 494–498) **Then** they find a backlink to `https://github.com/<owner>/bowerbird-deck` (real URL — replace `<owner>` with the GitHub org/user the repo lives under), with the existing one-sentence justification preserved ("Per Axiom 1 (the substrate observes; it does not interpret), interpretation belongs in a presenter, and a presenter is structurally a *consumer* of bowerbird — not a component of it"). The placeholder text "See Epic 5 Story 5.1 for the V1 first-party presenter." is replaced with the real link.

4. **Given** the presenter consumes the WebSocket and REST API **When** any aspect of consumption is awkward (auth flow, snapshot-on-connect, dropped-frame handling, reconnect behavior, topic-filter grammar, type-system seams on Node's WebSocket constructor — i.e. the patterns Story 4.2 retro-flagged) **Then** the awkwardness is captured via the **severity-driven split**:
   - **Hotfix story** (`5.X-hotfix-<topic>` file in `docs/bmad/implementation-artifacts/` + key added to the Epic 5 block in `sprint-status.yaml`) when the friction *blocks the dogfooding window* — i.e. it makes `bowerbird-deck` unusable for the maintainer's daily work until resolved. Hotfix stories get a full CS/DS/CR cycle and land between the next two planned Epic 5 stories.
   - **Deferred-work entry** (`docs/bmad/implementation-artifacts/deferred-work.md`, canonical format) for everything else — annoying but workable friction that doesn't block daily use.
   - In both cases, cross-link from the `bowerbird-deck` line of code that hit the friction (a `// see bowerbird/docs/.../5.X-hotfix-<topic>.md` or `// see bowerbird/docs/.../deferred-work.md#<anchor>` comment).
   - The presenter MUST NOT silently work around a substrate awkwardness — friction is the signal we are after.

5. **Given** the `bowerbird-deck` codebase **When** the maintainer reaches a "this is the V1 presenter" milestone (subjective: the presenter is useful enough that the maintainer prefers it to the terminal for daily work) **Then** a `README.md` in the sibling repo names:
   - The required `bowerbird` version (initially "main; pinned to v0.1.0 once Story 5.8 tags it").
   - How to install the presenter (`git clone`, `npm install`, or just `git clone` if zero deps).
   - How to run the presenter (`npm start` or `node --experimental-strip-types src/index.ts`).
   - The cookbook pattern from `bowerbird/docs/cookbook/` the presenter most directly demonstrates: `state-session-fanout.md`. Link to the canonical path under `bowerbird/docs/cookbook/state-session-fanout.md` (or its v0.1.0 tag equivalent once that ships).

6. **Given** Story 5.1 is the first story in Epic 5 and the dogfooding-validation-phase marker in `docs/bmad/implementation-artifacts/sprint-status.yaml` reads `backlog` **When** Story 5.1 reaches `done` **Then** `dogfooding-validation-phase` transitions to `in-progress` in the same merge, signaling that ad-hoc `5.X-hotfix-<topic>` stories may now be created inline per the sprint-change-proposal-2026-05-26 process. The phase transitions to `done` only when Story 5.8 tags v0.1.0.

## Tasks / Subtasks

- [ ] **Task 1: Decide GitHub remote and create `bowerbird-deck` sibling repo** (AC: #1, #3)
  - [x] Confirm the GitHub org/user that will own `bowerbird-deck` (default: `github.com/technicalpickles/bowerbird-deck`). — Confirmed `technicalpickles` (matches the gh-auth account).
  - [ ] Create the repo via `gh repo create <owner>/bowerbird-deck --public --description "First-party presenter for bowerbird — terminal TUI for live Claude Code session state"`. — **Deferred to maintainer** (per dev-session choice "local-only today; I'll create the GitHub repo myself later"). Local origin remote is already set to `git@github.com:technicalpickles/bowerbird-deck.git`; once the GH repo exists, `git push -u origin main` from `~/pickleton/repos/bowerbird-deck/worktrees/main/` lands the initial commit.
  - [x] Clone it as a *sibling* directory next to `bowerbird/` (NOT inside `bowerbird/`). Path: `<pt root>/repos/bowerbird-deck/` if pt-tracked; otherwise wherever the maintainer keeps sibling repos. — Created at `~/pickleton/repos/bowerbird-deck/` using the pt-tracked layout (`bare.git/` + `worktrees/main/`).
  - [x] Initial commit: `LICENSE` (matching bowerbird's), a stub `README.md` (filled in by Task 7), `.gitignore` from `examples/multi-session-router/.gitignore` as a starting point. — Initial commit `f9a2e9e`: `LICENSE` (dual MIT/Apache mirror of bowerbird's), `LICENSE-MIT`, `LICENSE-APACHE`, `.gitignore`, plus the full Task 2/3/4/7 bootstrap (`package.json`, `tsconfig.json`, `src/index.ts`, `README.md`).

- [x] **Task 2: Bootstrap the Node project shape** (AC: #1)
  - [x] Copy the `package.json` shape from `bowerbird/examples/multi-session-router/package.json` — same `engines.node >= 22.6.0`, same `type: module`, same `--experimental-strip-types` flag in `scripts.start`.
  - [x] Copy the TypeScript interface declarations for the protocol surface (`ServerMessage`, `Event`, `SessionCurrentState`, `Reaction`, etc.) from `bowerbird/examples/multi-session-router/src/index.ts`. These were authored by hand against `crates/protocol/src/*.rs` in Story 4.2; same pattern applies here (no `@bowerbird/presenter` SDK yet — see `project-context.md` §Example presenters open question). — Hand-authored against `crates/protocol/src/{state,event,reaction,ws,rest}.rs`; covers `SessionCurrentState`, `StateFrame`, `EventBody`, `EventFrame`, `DroppedFrame`, `HelloFrame`, `CloseFrame`, `SessionListItem`, `ServerInfo`. Permissive `ServerMessage` union with `{op: string; [k]: unknown}` catch-all mirrors the substrate's `ServerMessage::Unknown`.
  - [x] Carry forward the Story 4.2 lessons from `bowerbird/docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md` §"What was hard":
    - [x] The `@ts-expect-error` shape on `new WebSocket(url, { headers: { Authorization: \`Bearer ${token}\` } })` (DOM lib doesn't know about Node's headers options bag).
    - [x] Module-scope reference to the active WS so SIGINT/SIGTERM handlers can call `ws.close()` instead of hanging in a quiet `await new Promise`.

- [x] **Task 3: Implement the minimum viable presenter** (AC: #1)
  - [x] **State subscription:** subscribe to `state.session.*` on connect; render one row per session keyed by `session_id`. Rows show `session_id` (truncated to 12 chars), `current_state` (color-coded if a TUI library is in play), and last `Reaction` (rendered by enum name). — Color codes: green=Idle, yellow=Working, cyan=WaitingInput, gray=Unknown. Map keyed by `(source, session_id)` (natural key per substrate-not-actor invariant), NOT just `session_id`.
  - [x] **Event subscription (secondary):** subscribe to `events.*` or scope tighter (`events.claude.*`) for the "recent tool-use activity" requirement in AC #1. Keep a small in-memory ring (16 events) per session; surface the most recent tool name + reaction in the same row, or in a per-session detail pane if the TUI library supports one. — Simpler than a ring: per-session row carries `last_reaction` + `last_tool` fields, updated on each `PreToolUse` event. Rendered in the "tool (reaction)" column. **Friction worth watching during dogfooding window:** state snapshot gives historical state but events subscription has no history → `last_tool` / `last_reaction` columns stay null for pre-existing sessions until a fresh `PreToolUse` fires. Could be a deferred-work item ("fetch recent events per session on connect") if it lands as real friction.
  - [x] **TUI library choice:** the minimal path is `process.stdout.write` with ANSI escapes (no deps; same shape as `examples/event-log-viewer/`). If the maintainer wants splits/panes, `blessed`, `ink`, or `terminal-kit` are reasonable; document the choice in a one-line comment at the top of `src/index.ts`. — Chose **zero-deps ANSI escapes**. Header comment at `src/index.ts:13` names the choice. If splits/detail panes become a real need during dogfooding, `blessed` or `ink` can land in a later iteration without changing the connection / state-table shape.
  - [x] **Snapshot-on-connect:** the WS server emits a `Snapshot` frame after subscribe (Story 2.3); render its sessions before any live event arrives, so the TUI is populated from instant 0. — Confirmed live against the running daemon: 31 sessions arrived as a burst of `state` frames immediately after `subscribe`. No special handling needed — the snapshot is just the first batch of state frames in the stream.
  - [x] **Dropped-frame handling:** on a `DroppedFrame` envelope from the WS subsystem (Story 2.4 / Epic 2 retro AI-4 path), refetch the full state via `GET /sessions` and reconcile. The `reconnect-recovery` example at `bowerbird/examples/reconnect-recovery/src/index.ts` is the canonical recipe. — Implemented in `reconcileViaRest()`: on `DroppedFrame`, close the socket, reconcile via `GET /sessions` (replacing the local map while preserving last_reaction/last_tool from events), then the connection loop reopens. Not yet exercised under load — the dogfooding window may surface real Dropped behavior.
  - [x] **Reconnect loop:** on WS close (any reason: daemon restart, network blip, timeout), exponential backoff up to 30s ceiling, refetch `GET /sessions`, reopen WS, resubscribe. Surface the disconnected state in the TUI (e.g. dim the rows + a status line at the bottom). — Backoff: 250ms → 30s cap, doubles per failure. Disconnected rendering: `\x1b[2m` dim the rows + status line shows `○ disconnected` (red). Re-reads `~/.bowerbird/server.json` each iteration since the daemon's port is ephemeral (Story 3.2). Pattern carried from `examples/reconnect-recovery/`.

- [x] **Task 4: Auth + config** (AC: #1)
  - [x] Read the bearer token from `~/.bowerbird/server.json` (the daemon's published location since Story 3.3). Same pattern as `examples/multi-session-router/src/index.ts`. — **Story-spec drift caught at implementation time:** the bearer token is NOT in `server.json` per `crates/protocol/src/rest.rs:77-94` and `crates/daemon/src/api/token.rs` (Story 3.3 landed the token in the system keychain + `BOWERBIRD_TOKEN` env var fallback; `server.json` carries only `bind_addr`). Implementation matches the **three reference examples** (`multi-session-router`, `reconnect-recovery`, `event-log-viewer`): read `BOWERBIRD_TOKEN` env var with an error message pointing at `bowerbird auth token`. This is documentation/spec drift, not substrate friction — flagged here for the eventual Story 5.6 docs pass (first-time-reader docs) to fold the correction into the story-template references.
  - [x] Read the daemon's bind address + port from the same file. Don't hard-code `127.0.0.1:8080` — let the daemon's `config.toml` be the source of truth. — Done: `loadServerInfo()` reads `bind_addr` from `~/.bowerbird/server.json` on every connection-loop iteration so an ephemeral-port restart is transparent.
  - [x] If `~/.bowerbird/server.json` doesn't exist, exit with a clear error pointing at `bowerbird start`. — Error wrapping in `loadServerInfo()`: `cannot read <path>: <msg>. Is the daemon running? Try \`bowerbird start\`.`

- [ ] **Task 5: Verify against bundled fixture, then live Claude Code** (AC: #1, #2)
  - [ ] **Smoke step (no Claude Code needed):** start bowerbird via `bowerbird start`; run `bowerbird replay` (no args; uses the Story 4.1 bundled fixture spanning two sessions); start the presenter. Confirm the TUI shows two session rows transitioning through states as replay fires. This is the same hermetic pattern Story 4.2's `cli_examples.rs` already proves end-to-end against three reference examples.
  - [ ] **Live step:** install bowerbird on the maintainer's main machine via `bowerbird install`; start a real Claude Code session; confirm the presenter's TUI tracks it through `idle → working → waiting-on-input → idle`. Record any latency surprises or wire-shape surprises as Task 6 friction items.
  - [ ] **Dogfooding step:** use the presenter as the maintainer's actual signal source for 5 working days. Log calendar dates in the Dev Agent Record › Completion Notes section. AC #2 explicitly requires this.

- [ ] **Task 6: Capture friction via the severity-driven split** (AC: #4)
  - [ ] For each substrate awkwardness encountered during Task 5 (especially during the 5-day dogfooding window), apply the severity split:
    - **Blocks dogfooding** (the friction makes `bowerbird-deck` unusable for the maintainer's daily work until resolved) → file a `5.X-hotfix-<topic>` story file under `docs/bmad/implementation-artifacts/` AND add the key to the Epic 5 block in `sprint-status.yaml`. Hotfix stories get a full CS/DS/CR cycle.
    - **Annoying but workable** (everything else — the maintainer can keep using the presenter while the issue waits) → add a new entry to `docs/bmad/implementation-artifacts/deferred-work.md` with the canonical format (problem + reproduction + proposed-fix + backlink).
  - [ ] DO NOT silently work around a substrate awkwardness in `bowerbird-deck` code. The friction signal IS the deliverable.
  - [ ] Cross-link from the `bowerbird-deck` line of code that hit the friction (a `// see bowerbird/docs/.../5.X-hotfix-<topic>.md` or `// see bowerbird/docs/.../deferred-work.md#<anchor>` comment is sufficient).

- [x] **Task 7: Write `bowerbird-deck/README.md`** (AC: #5)
  - [x] Name the required bowerbird version (initially `main` pinned to a specific commit SHA; switch to `v0.1.0` after Story 5.8 tags it). — Pinned to `32c6d8c` (current bowerbird `main` HEAD at 2026-05-27); README notes the v0.1.0 follow-up.
  - [x] Install instructions: `gh repo clone <owner>/bowerbird-deck && cd bowerbird-deck && npm install` (or note "zero deps; just clone" if the implementation avoids npm-managed deps). — Runtime is zero-deps; README documents `npm install` as optional (covers `npm run typecheck` only).
  - [x] Run instructions: `npm start` resolving to `node --experimental-strip-types src/index.ts`. Reproduce the Node 22.6+ floor here, same as Story 4.2's examples. — Done. Includes the `export BOWERBIRD_TOKEN=$(bowerbird auth token)` step so first-time readers don't trip on the env-var requirement.
  - [x] Cookbook pattern reference: link to `https://github.com/<owner>/bowerbird/blob/main/docs/cookbook/state-session-fanout.md` (or its v0.1.0 tag equivalent post-5.8). One paragraph naming why this pattern (the presenter is fundamentally a state-fanout consumer; recent-tool-use is a secondary detail). — Done. README also links to the three reference examples as "if you want the recipe without the TUI noise."
  - [x] One-paragraph "Status" header: "First-party V1 presenter; tracks `bowerbird` `main` / `v0.1.0`. Friction discovered while building this lives in the parent repo, not here." — Done at the top of README.md.

- [ ] **Task 8: Update bowerbird's `architecture.md` §Frontend Architecture with the real backlink** (AC: #3)
  - [ ] Edit `docs/bmad/planning-artifacts/architecture.md` at the `### Frontend Architecture` section (currently lines 494–498).
  - [ ] Replace the placeholder text "See Epic 5 Story 5.1 for the V1 first-party presenter." with: "See [bowerbird-deck](https://github.com/\<owner\>/bowerbird-deck) — the V1 first-party presenter (Story 5.1)."
  - [ ] Preserve the existing one-sentence Axiom 1 justification verbatim ("Per Axiom 1 (the substrate observes; it does not interpret), interpretation belongs in a presenter, and a presenter is structurally a *consumer* of bowerbird — not a component of it.").
  - [ ] **Timing:** this edit lands in the *same* bowerbird PR that closes Story 5.1 — i.e. after the 5-day dogfooding window finishes (Task 5) and the dogfooding-log lands in the Completion Notes. The presenter must be real enough to back-link to before bowerbird's architecture.md acknowledges it by URL. This is the only `bowerbird/` repo change Story 5.1 makes besides this story file itself.

- [ ] **Task 9: Transition sprint-status markers** (AC: #6)
  - [ ] When the story file moves to `review`, update `docs/bmad/implementation-artifacts/sprint-status.yaml`:
    - `5-1-first-party-presenter-tool: review`
    - `dogfooding-validation-phase: in-progress` (per the sprint-change-proposal-2026-05-26 spec; happens when Story 5.1 ships, not when it starts).
  - [ ] When the story file moves to `done`, the dogfooding-validation-phase remains `in-progress` until Story 5.8 tags v0.1.0.

## Dev Notes

### What "done" means for this story

This story is half a code change and half a calendar event. The code half (Tasks 1–4, 7, 8) is straightforward: spin up a sibling repo, write a small Node TUI, edit one section of bowerbird's architecture.md. The calendar half (Task 5 dogfooding window + Task 6 friction capture) is the load-bearing one — 5 working days of actual use against live Claude Code is non-negotiable per AC #2, because the friction signal is what informs Epic 5's remaining stories.

Don't try to "finish" the presenter before starting Task 5. The pattern is: get a minimum useful TUI shipping in `bowerbird-deck`, install it, *use it*, fix or file friction as it appears, and let the dogfooding window be the polishing pass.

### Axiom 1 is the constraint that puts this in a sibling repo

From `docs/bmad/project-context.md` §Project axioms:

> **Axiom 1: The substrate observes; it does not interpret.** Anything that turns raw data into application-level concepts (personas, voices, sprites, priorities, urgency, sentiment) is a *presenter* concern.

`bowerbird-deck` is a presenter. State-rendering is interpretation. A TUI that color-codes `working` red versus `idle` green is taking a stance the substrate explicitly refuses to take. That stance belongs in a separate repo so it cannot accidentally creep into `crates/`.

If during implementation the maintainer is tempted to add a helper to `crates/protocol/` or `crates/daemon/` "just for the presenter" — stop. The right answer is either (a) compute the helper in `bowerbird-deck`, or (b) file a substrate change as a separate ADR with its own justification independent of the presenter's needs.

### TypeScript-on-Node patterns that already work

From `docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md` §"TypeScript-on-Node was the right call":

- **Node 22.6+ floor.** `--experimental-strip-types` ships in 22.6; lower versions need `tsx` or `ts-node` and the build complexity isn't worth it.
- **No bundler, no `npm install` for runtime deps if avoidable.** The three reference examples manage with zero runtime deps; `bowerbird-deck` should attempt the same. If a TUI library makes the difference between "useful" and "abandoned" the cost is acceptable, but try the zero-deps path first.
- **Hand-write the protocol type declarations.** ~30 lines of TypeScript interfaces transcribed from `crates/protocol/src/*.rs`. The "Reference SDK question" in `project-context.md` is still open; until it resolves, hand-writing is the right cost.

### Friction-capture is the deliverable, not a side-effect

AC #4 is the most important AC in this story. Epic 5's remaining stories (5.2 bench gates, 5.3 release pipeline, 5.4 install UX, 5.5 cookbook consolidation, 5.6 docs, 5.7 projection correctness, 5.8 v0.1.0 tag) all need to know what *actually* hurts when bowerbird meets real use. That signal exists only if `bowerbird-deck` faithfully surfaces friction instead of hiding it under workarounds.

Examples of friction to expect (from Story 4.2's retro):

- Node's `WebSocket` constructor takes a headers options bag that DOM types disagree with — `@ts-expect-error` comment per call site.
- Signal handlers hanging in `await new Promise` without a module-scope active-WS reference.
- Snapshot-on-connect timing — is the snapshot before or after the first live event?
- `Reaction::Unknown` rendering — what does the TUI show? (The substrate's job is to ship `Unknown`; the presenter's job is to render *something* that doesn't look like a bug.)

When in doubt, file the friction. A 20-line `5.X-hotfix-<topic>` story or a 5-line deferred-work entry is cheaper to retract than a substrate awkwardness silently absorbed into the presenter.

### Project Structure Notes

- `bowerbird-deck` is a *sibling repo*, NOT a directory inside `bowerbird/`. It does not appear in `bowerbird/crates/`, `bowerbird/examples/`, `bowerbird/adapters/`, or anywhere else under this repo. The only `bowerbird/` artifact this story produces is the architecture.md backlink (Task 8) and this story file itself.
- The presenter consumes ONLY the public WebSocket + REST surface. It does not link `crates/protocol/`. It transcribes the protocol's TypeScript-equivalent declarations by hand, exactly as the three reference examples already do.
- The maintainer's `~/.bowerbird/server.json` is the source of truth for daemon address and bearer token. Don't read `~/.bowerbird/config.toml` directly — that's the daemon's private config.

### Testing Standards

This story is unusual: the canonical "test" is the 5-day dogfooding window (AC #2). Code-level testing is light by design.

- **`bowerbird-deck` should ship at least one smoke test** that follows the Story 4.2 pattern: spawn a hermetic bowerbird daemon under `BOWERBIRD_DATA_DIR=<tmp>`, run `bowerbird replay` with the bundled fixture, start the presenter as a subprocess, assert the TUI's stdout contains the expected session-row text for both `session-alpha` and `session-beta`. This is the same shape as `bowerbird/tests/cli_examples.rs`.
- **`bowerbird-deck` should NOT add Rust tests** to the bowerbird repo. Friction with the substrate's contract is captured per AC #4 (hotfix story or deferred-work entry), not as a regression test in the substrate's test suite — until the friction is resolved by a substrate change, at which point the substrate's test suite gains a test in that *substrate-side* PR, not in the `bowerbird-deck` PR.
- **The bowerbird repo's existing test suite must still pass** after the Task 8 architecture.md edit. Run `cargo test --workspace -- --test-threads=1` after the edit; the `tests/cli_docs_drift.rs` `architecture_md_docs_tree_matches_shipped_surface` test is the most likely failure mode if the edit phrasing drifts from expectations.

### References

- `docs/bmad/planning-artifacts/epics.md:900-927` — Story 5.1 epic definition.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-26.md` — full Epic 5 rationale (dogfooding → presenter → hardening sequencing).
- `docs/bmad/planning-artifacts/architecture.md:494-498` — Frontend Architecture section; Task 8 edit target.
- `docs/bmad/planning-artifacts/architecture.md:449-498` — REST + WebSocket + Frontend Architecture context block.
- `docs/bmad/project-context.md` §Project axioms (lines 40–59) — Axiom 1 (substrate observes; presenter interprets).
- `docs/bmad/project-context.md` §Example presenters (around lines 196–203) — open Reference SDK question; until resolved, hand-write protocol type declarations.
- `docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md` §"TypeScript-on-Node was the right call" + §"What was hard" — patterns and gotchas the presenter will hit.
- `bowerbird/examples/multi-session-router/src/index.ts` — closest existing reference for `state.session.*` fanout; copy package.json + ts-strip-types shape.
- `bowerbird/examples/reconnect-recovery/src/index.ts` — dropped-frame + reconnect recipe.
- `bowerbird/docs/cookbook/state-session-fanout.md` — the cookbook pattern AC #5 references.
- `bowerbird/docs/presenter-authoring.md` — the audience-switch document the presenter author reads first.
- `bowerbird/docs/protocol.md` — wire reference for `ServerMessage`, topic grammar, REST routes.

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context) via Claude Code's `/bmad-dev-story` skill (BMM module).

### Debug Log References

- Local smoke test (2026-05-27): presenter launched against the running daemon (pid 91535, uptime 24h45m, 31 active sessions), received `hello` frame + 31-session snapshot via `state.session.*` burst. All three `SessionCurrentState` variants (`Idle`, `Working`, `WaitingInput`) observed in the snapshot. Non-TTY mode emits a JSON snapshot per render; TTY mode (the real use case) draws the ANSI table.
- `npm run typecheck` clean against `@types/node@22` + `typescript@5.6` with `strict: true`.

### Completion Notes List

**Session 1 (2026-05-27): code work + initial smoke test.**

Tasks 1–4, 7 are complete (with the noted maintainer-action carve-outs on Task 1). The story stays `in-progress` because Tasks 5 (5-day dogfooding window), 6 (friction capture), 8 (architecture.md backlink edit), and 9 (sprint-status transition) all depend on the calendar window in AC #2.

**Maintainer follow-up to unblock the dogfooding window:**

1. `gh repo create technicalpickles/bowerbird-deck --public --description "First-party presenter for bowerbird — terminal TUI for live Claude Code session state"`
2. From `~/pickleton/repos/bowerbird-deck/worktrees/main/`: `git push -u origin main` (the origin URL is already set on the bare repo).
3. *(Optional)* `pt track git@github.com:technicalpickles/bowerbird-deck.git --name bowerbird-deck` if pt's worktree views should pick it up — the directory already matches the `bare.git/` + `worktrees/main/` layout pt expects, so this should be a no-op rebind; verify with `pt list` and `pt worktrees bowerbird-deck`. (Skipping this is fine — the repo is functionally a git repo regardless.)
4. Start the dogfooding window: `export BOWERBIRD_TOKEN=$(bowerbird auth token); npm start` in a persistent terminal window for ambient awareness. AC #2 wants 5 *working days*.

**During the 5-day dogfooding window:**

- Log calendar dates in this section as the window progresses (e.g. "2026-05-28 (day 1) — used the presenter all day; alt-tabbed to terminal twice when X happened").
- Capture friction per AC #4 split: hotfix story for daily-work blockers (file under `docs/bmad/implementation-artifacts/5.X-hotfix-<topic>.md` + add the key to `sprint-status.yaml` Epic 5 block); deferred-work entry for everything else (`docs/bmad/implementation-artifacts/deferred-work.md`). Cross-link from the relevant `bowerbird-deck` source line with a `// see bowerbird/docs/...` comment.

**Friction items pre-flagged from this session (track these specifically; either confirm during dogfooding or close them out):**

- *Story-spec drift, Task 4:* "Read the bearer token from `~/.bowerbird/server.json`" is wrong — Story 3.3 put the token in keychain + `BOWERBIRD_TOKEN` env var. The story spec should match Story 3.3 reality. Action: Story 5.6 (first-time-reader docs pass) is the right place to scrub `docs/bmad/planning-artifacts/` and `epics.md` for similar drift.
- *Substrate behavior, presenter side:* state snapshot has no event history → `last_tool` / `last_reaction` columns stay null for pre-existing sessions until a fresh `PreToolUse` event fires. Workaround on the presenter side would be `GET /sessions/<id>/events?since=0` per session at connect, but per AC #4 we should NOT silently work around — instead, watch for this in daily use; if it actively hurts, file a hotfix or deferred-work entry. If it never bothers the maintainer, it's noise that died on its own.

**After the dogfooding window closes:**

- Edit `bowerbird/docs/bmad/planning-artifacts/architecture.md` at the `### Frontend Architecture` section (lines 494–498) per Task 8.
- Flip `5-1-first-party-presenter-tool` to `review` AND `dogfooding-validation-phase` to `in-progress` per Task 9 / AC #6 in the same merge.
- Re-invoke `/bmad-dev-story` (or just continue the workflow manually) to walk Steps 9–10 of the dev-story workflow.

### File List

**`bowerbird-deck` (sibling repo, NOT in this repo):**

- `~/pickleton/repos/bowerbird-deck/bare.git/` — bare git with origin set to `git@github.com:technicalpickles/bowerbird-deck.git` (awaiting maintainer `gh repo create` + `git push`).
- `~/pickleton/repos/bowerbird-deck/worktrees/main/.gitignore`
- `~/pickleton/repos/bowerbird-deck/worktrees/main/LICENSE` (dual MIT/Apache mirror of bowerbird's)
- `~/pickleton/repos/bowerbird-deck/worktrees/main/LICENSE-MIT`
- `~/pickleton/repos/bowerbird-deck/worktrees/main/LICENSE-APACHE`
- `~/pickleton/repos/bowerbird-deck/worktrees/main/README.md`
- `~/pickleton/repos/bowerbird-deck/worktrees/main/package.json`
- `~/pickleton/repos/bowerbird-deck/worktrees/main/tsconfig.json`
- `~/pickleton/repos/bowerbird-deck/worktrees/main/src/index.ts`
- `~/pickleton/repos/bowerbird-deck/worktrees/main/package-lock.json` (devDependencies install: typescript + @types/node)

**`bowerbird` (this repo, this PR):**

- `docs/bmad/implementation-artifacts/sprint-status.yaml` — Story 5.1 `ready-for-dev → in-progress`.
- `docs/bmad/implementation-artifacts/5-1-first-party-presenter-tool.md` — this file (status `ready-for-dev → in-progress`, Tasks 1–4 & 7 marked complete with per-subtask notes, Dev Agent Record populated, dogfooding-window handoff staged).
- *(Pending Task 8, after dogfooding closes)* `docs/bmad/planning-artifacts/architecture.md` — Frontend Architecture backlink update.
