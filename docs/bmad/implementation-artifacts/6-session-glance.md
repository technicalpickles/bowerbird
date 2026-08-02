# Story 6-session-glance: One-shot session glance CLI

Status: review

<!-- Story key is the slug `6-session-glance`. Do NOT rename to `6.1` or
     `6-1-session-glance`: epics.md:1411 ("Story keys are slug-first, not
     ordinal") is an explicit anti-ordinal instruction aimed at exactly this
     file and at the matching sprint-status key. Epic 5's numeric ids rotted
     four times through dogfood-driven insertions; this epic expects
     insertions. -->

## Story

As the maintainer (and any tool builder copying the entry),
I want a one-shot CLI print of every live session's state, grouped by repository with per-session ages,
so that a missed notification has a glanceable fallback and pane-cycling stops being the discovery mechanism (FR40, FR44).

**First story of Epic 6, and the epic's load-bearing reference implementation.** Two later stories bind to it:
`6-tmux-ambient` shells out to this CLI (the only hard code dependency in the epic's DAG, epics.md:1413), and every
Phase 3 surface's WHERE naming reads "matches glance's repo-keying behavior" instead of reinterpreting FR44
(epics.md:1427). That makes two artifacts of this story durable contracts rather than implementation detail: the
canonical repo-from-`cwd` derivation, and the machine-friendly output mode.

**Presenter-layer only.** Zero substrate changes: no `crates/` production code, no `crates/protocol/src` touch (the
changelog gate will not and should not fire), no schema migration, no new daemon capability. The entry consumes REST
surfaces that already shipped in Stories 5.7 (`cwd`, `started_at`) and 5.8 (`GET /sessions?state=`). The only Rust this
story writes is test code (`tests/cli_docs_drift.rs` refactor, `tests/cli_examples.rs` smoke, `tests/cli_examples_drift.rs`
entry registration).

## Acceptance Criteria

Source: [epics.md:1433-1464](../planning-artifacts/epics.md). Epic-level conventions at
[epics.md:1409-1431](../planning-artifacts/epics.md) bind every AC below and are restated in Dev Notes where they
change what the dev has to do.

1. **Given** a running daemon with sessions in mixed states across multiple repositories
   **When** I run the `session-glance` entry
   **Then** it prints every non-Ended session grouped by repository (derived presenter-side from `cwd`), each with its
   current state and age (derived from `started_at`), then exits: one-shot, no watch loop
   **And** it consumes REST `GET /sessions?state=` (the Story 5.8 filters' first consumer), not an unfiltered dump.

2. **Given** downstream consumers (`6-tmux-ambient` invokes this CLI)
   **When** the entry is run with a machine-friendly output mode (e.g. a count/format flag)
   **Then** it emits a stable, documented output contract (this CLI's output is a mini-API; the contract is stated in
   the README, not implied).

3. **Given** this entry is the FR44 reference implementation
   **When** later stories implement WHERE
   **Then** their repo derivation conforms to this entry's behavior (documented as the canonical repo-from-`cwd`
   derivation).

4. **Given** `tests/cli_docs_drift.rs` hardcodes a three-entry README list and message string
   **When** this story lands the fourth entry
   **Then** the guard is refactored to derive the entry list/count from the `docs/cookbook/*/` glob, and, per A13, the
   refactored guard is observed failing against a deliberately broken README before it is kept.

5. **Given** the cookbook integration checklist (epic conventions above)
   **When** the entry lands
   **Then** every checklist item is satisfied and CI is green.

6. **Given** the dogfood gate protocol
   **When** the story is ready to land
   **Then** the gate evidence is recorded: 3-5 working days of unprompted use, provoked adversity (daemon stopped
   mid-day: the entry fails with a clear message, not a stack trace), and a harvest note.

## Tasks / Subtasks

Task headers are stable kebab-case slugs. Cite the slug in commits, never an ordinal (same insertion-stability reason
the story key is a slug).

- [x] **`scaffold-entry` (AC: 1, 5)**
  - [x] Create `docs/cookbook/session-glance/` with the canonical entry shape, copying the scaffolding from an
        existing entry rather than inventing it: `src/index.ts`, `README.md`, `package.json`, `package-lock.json`,
        `tsconfig.json`. Model on `docs/cookbook/rest-cursor-pagination/` (the other REST-only, fetch-render-exit entry;
        `state-session-fanout` is WS-shaped and is the wrong template here).
  - [x] `package.json`: `"name": "bowerbird-cookbook-session-glance"`, `"private": true`, `"type": "module"`,
        `"engines": { "node": ">=22.6.0" }`, `scripts.start` + `scripts.typecheck`, devDeps `typescript ^5.6.0` and
        `@types/node ^22.0.0`. The `engines.node` string is machine-checked by
        `tests/cli_examples_drift.rs::each_entry_package_json_declares_node_22_6_engine` (accepts only a `>=`-prefixed
        floor of 22.6+).
  - [x] `package-lock.json` must be committed. CI's `typecheck-examples` job runs `npm ci` per entry
        (`.github/workflows/ci.yml:87`) and `npm ci` hard-fails without a lockfile. Generate it with `npm install` in
        the entry dir, then verify with a clean `npm ci && npm run typecheck`.
  - [x] `tsconfig.json` copied verbatim from a sibling entry (target ES2023, module ESNext, moduleResolution Bundler,
        strict, noEmit, skipLibCheck, `types: ["node"]`, `include: ["src/**/*.ts"]`).
  - [x] No runtime npm dependencies. The entry self-contains its ~30 lines of interface declarations, per
        `docs/cookbook/README.md` §Why TypeScript on Node ("No SDK is shipped").

- [x] **`consume-rest-filter` (AC: 1)**
  - [x] Fetch `GET http://{bind_addr}/sessions?state=idle,working,waitinginput,unknown` with
        `Authorization: Bearer ${BOWERBIRD_TOKEN}`. See Dev Notes "The `?state=` filter has no negation" before
        writing the query string: "non-Ended" must be spelled as a positive CSV of the four non-Ended tokens.
  - [x] Reuse the sibling entries' `loadServerInfo()` + `resolveToken()` shape verbatim (see
        `docs/cookbook/rest-cursor-pagination/src/index.ts`) so the daemon-discovery and token-resolution behavior is
        identical across the cookbook. Do not invent a new discovery path.
  - [x] Decode the response as a bare JSON array of `SessionListItem` (NOT an envelope object). Declare the interface
        with the exact field set in Dev Notes "REST shape, verified against source".
  - [x] One-shot: fetch once, render, exit. No watch loop, no polling, no `setInterval`. Exit 0 on success.

- [x] **`canonical-repo-derivation` (AC: 1, 3)**
  - [x] Implement the repo-from-`cwd` derivation as a single named, exported function in `src/index.ts` (proposed:
        `deriveRepo(cwd: string | null): string`). It must be a named export so later stories can cite it by function
        name, per the house cross-reference rule (project-context.md §Cookbook discipline: "Reference by function name,
        not line number").
  - [x] Write the rule as a doc comment directly above that function, stating explicitly that it is the canonical
        FR44 repo-from-`cwd` derivation and that later Epic 6 surfaces conform to it rather than reinterpreting FR44.
        The doc comment plus the README section (below) are the durable record; the AC is not satisfied by the
        implementation implying the rule.
  - [x] Implement per Dev Notes "Canonical repo-from-`cwd` derivation": nearest ancestor of `cwd` containing a `.git`
        entry (file OR directory), rendered as that ancestor's basename; fall back to `basename(cwd)` when no `.git`
        ancestor is found or the path is not readable; fall back to a single named bucket when `cwd` is `null`.
        Never throw out of this function: an unreadable path is a bucket, not a crash.
  - [x] Unit-test the derivation's pure branches. `docs/cookbook/dropped-frame-recovery/tests/recover.test.ts` is the
        in-tree precedent for a cookbook entry carrying its own `tests/` sidecar; follow that shape if you add one.

- [x] **`render-grouped-output` (AC: 1)**
  - [x] Group the fetched sessions by derived repo, and within each group render one line per session carrying at
        minimum: session identity, `current_state`, and age.
  - [x] Age is derived presenter-side from `started_at` (epoch-ms, nullable). Render a named placeholder when
        `started_at` is `null` rather than printing `NaN` or a 1970 timestamp.
  - [x] `current_state` arrives PascalCase on the wire (`Idle` / `Working` / `WaitingInput` / `Ended` / `Unknown`)
        even though the `?state=` filter tokens are lowercase. Do not assume the two spellings match; see Dev Notes.
  - [x] Empty result (daemon up, zero non-Ended sessions) prints a clear "no live sessions" line and still exits 0.
        Do not print an empty output that reads as a failure.

- [x] **`machine-output-contract` (AC: 2)**
  - [x] Add a machine-friendly output mode. Recommended shape (see Dev Notes "Machine mode: what `6-tmux-ambient`
        actually needs" before changing it): `--format=json` emitting NDJSON, one object per session with a fixed
        field set; plus `--count` emitting a single integer on stdout and nothing else; plus `--state=<csv>` passing
        the filter through to the REST query so `session-glance --count --state=waitinginput` is the whole of
        `6-tmux-ambient`'s data path.
  - [x] Document the contract as a mini-API section in the entry README: every flag, the exact stdout shape per mode,
        what is stable vs incidental, and the exit codes. AC 2 is satisfied by the README statement, not by the code.
  - [x] Invalid flag or invalid `--state` token exits non-zero with a one-line message naming the bad input and the
        accepted set. Match the daemon's own error vocabulary where it applies (`crates/daemon/src/api/filter.rs`
        rejects unknown tokens with a message that lists the accepted set; a client-side pre-check that disagrees with
        the daemon's is worse than no pre-check).

- [x] **`daemon-down-clarity` (AC: 6, 5)**
  - [x] Two distinct daemon-down failure modes exist and only one is currently guarded. Handle both:
        (a) `server.json` missing or unreadable, which the sibling entries already render as
        `cannot read <path>: ... Is the daemon running? Try \`bowerbird start\`.`;
        (b) `server.json` present but stale and the connection refused, which is what "daemon stopped mid-day" (the
        AC 6 provoked adversity) actually produces. Node surfaces (b) as a bare `TypeError: fetch failed`, which is
        the stack-trace-shaped failure AC 6 forbids. Catch it and render a message naming the address and the fix.
  - [x] Keep the top-level shape `main(...).catch((e: Error) => { console.error(e.message); process.exit(1); })`. That
        pattern (message only, never the stack) is what makes AC 6's "clear message, not a stack trace" true by
        construction; every sibling entry uses it.
  - [x] Exit non-zero on both paths.

- [x] **`glob-derive-docs-drift-guard` (AC: 4, 5)**
  - [x] `tests/cli_docs_drift.rs`: replace the hardcoded `REQUIRED_COOKBOOK_ENTRIES` const (currently three literal
        `docs/cookbook/<name>/README.md` paths, lines 37-41) with a `docs/cookbook/*/` glob-derived list. Mirror CI's
        own entry filter: `.github/workflows/ci.yml:86` skips any subdir without a `package.json`, so the derivation
        must skip those too or a legitimate non-entry docs subdir turns the suite red.
  - [x] Retire the now-hardcoded counts and message strings that the epic AC names: `required_docs_exist`'s message
        ("the five top-level docs plus the three per-entry cookbook READMEs"), and
        `cookbook_readme_lists_three_required_entries` (both its `three` in the test name and its three literal
        `](name/)` needles) which must derive its needle set from the same glob.
  - [x] `quickstart_internal_links_resolve`'s `DOCS_TO_CHECK` also hardcodes the three entry READMEs (lines 184-186);
        derive that tail from the same glob so a new entry's README gets link-checked automatically.
  - [x] `cookbook_entry_consts_match_directory_listing` becomes tautological once the const is glob-derived (it
        compares the glob against itself). Do not simply delete it: per the Story 5.13 lesson ("deleting the guard
        without a successor is the kind of coverage regression review flags"), either delete it WITH an explicit
        note that glob-derivation is its successor by construction, or repurpose it to assert the derivation is
        non-empty and contains every entry dir that has a `package.json`. Record whichever you choose in Completion
        Notes.
  - [x] **A13, and this is a required step, not a nicety:** before keeping the refactor, deliberately break a
        cookbook README (e.g. delete the `## Files` heading from the new entry's README, or add a fenced ` ```ts `
        block to it), run the guard, and OBSERVE IT RED. Restore, re-run, observe green. Record both run ids in Debug
        Log References. A refactored guard that has only ever been green is unverified.
  - [x] **A13 positive companion** (epics.md:1423, extended scope: every negative assertion carries a positive
        assertion proving the precondition fired): the glob derivation must assert it found at least the expected
        entries before asserting they all conform. A guard that silently derives an empty list passes every downstream
        assertion vacuously. That is precisely the "coin flip wearing a green checkmark" the convention names.

- [x] **`register-entry-in-guards` (AC: 5)**
  - [x] `tests/cli_examples_drift.rs`: the `ENTRIES` const (three literals) and
        `entries_const_matches_directory_listing` are the exact twin of the `cli_docs_drift.rs` pair being globbed.
        Decide and record: glob-derive it too (recommended, keeps the two files from diverging and matches the AC's
        intent) or add `"session-glance"` to the literal list. Leaving it hardcoded while its twin is globbed is the
        outcome to avoid.
  - [x] `tests/cli_examples.rs`: update the file's doc comment (it currently says "the three cookbook reference
        entries") and add one smoke test for the new entry per the epic's integration checklist.
  - [x] `tests/cli_examples.rs::cookbook_entries_fail_clearly_when_daemon_down` hardcodes the three entry names in its
        loop. Add `session-glance`. This is the CI-side counterpart of AC 6's provoked adversity: it asserts non-zero
        exit plus a `server.json` mention on stderr with no daemon running. Note it covers failure mode (a) only;
        mode (b) (stale `server.json`, connection refused) is not covered by it, so add coverage for (b) or disclose
        the gap in Completion Notes.

- [x] **`smoke-test` (AC: 1, 2, 5)**
  - [x] Add the entry's smoke to `tests/cli_examples.rs` following the in-file pattern: `TempDir` data dir, ephemeral
        daemon port, `start_daemon` / `bowerbird replay` / `spawn_example` / assert on stdout / `stop_daemon` +
        `force_stop`. Env is passed per-child via `Command::env`; never `std::env::set_var` (banned by `clippy.toml`).
  - [x] Assert on the entry's TEXT output, per the epic's output-seam convention (epics.md:1425). The smoke's job is
        the formatter, not the delivery.
  - [x] Assert the machine mode too (`--count` returning a parseable integer, or the JSON mode's field set). AC 2
        calls this output a mini-API; an unasserted mini-API is a promise, and `6-tmux-ambient` is the consumer that
        will pay for it breaking.
  - [x] The bundled replay fixture drives the assertions. Check what states and `cwd` values the fixture actually
        produces before writing expectations; `rest_cursor_pagination_*` shows the house style of asserting on stable
        content shape rather than on ids that shift with the daemon's `RecordingStarted` sentinel.
  - [x] Timeouts around child waits and reads are hang detectors, not latency assertions. Use the generous-timeout
        shape already in the file; never sleep to synchronize with the daemon.

- [x] **`entry-readme` (AC: 2, 3, 5)**
  - [x] Write `docs/cookbook/session-glance/README.md` in the machine-enforced five-section shape, in this exact
        order, as level-2 headings: `## What this is`, `## Run it`, `## How it works`, `## How to apply it`,
        `## Files`. Prose only. The only fenced blocks permitted anywhere in the file are bare fences and ` ```sh `
        (`tests/cli_docs_drift.rs::scan_prose_only_markdown` enforces an allowlist, not a blacklist: ` ```ts `,
        ` ```js `, ` ```tsx `, ` ```json ` and tilde fences all fail).
  - [x] The output contract (AC 2) and the canonical repo derivation (AC 3) both get stated here in prose. Suggested
        home: the contract under `## Run it` or `## How it works`; the derivation under `## How it works` with its
        edge cases named plainly (git worktrees, `cwd` below the repo root, `cwd: null`) rather than papered over.
        The epic's sibling story sets the precedent for naming a limitation instead of hiding it (epics.md:1490-1492).
  - [x] Length target ~50-150 lines; the three existing entry READMEs land between 46 and 70. The real test is "one
        entry = one question the reader had."
  - [x] Relative links only, and they must resolve on disk: `quickstart_internal_links_resolve` resolves every
        non-http link target relative to the file's parent.

- [x] **`cookbook-index` (AC: 5)**
  - [x] `docs/cookbook/README.md`: add a table row for the new entry in the index table, in the markdown link form
        `[`session-glance/`](session-glance/)`. The guard pins the `](name/)` link form specifically so a quick-run
        command path cannot satisfy the index assertion.
  - [x] Add the entry to the `## Quick run` fenced `sh` block, matching the existing per-entry style.
  - [x] Do not disturb the reconciliation markers `cookbook_readme_carries_cargo_zone_note` pins
        (`Not a Cargo zone`, `TypeScript`, `project-context.md`, `members = ["crates/*"]`).

- [ ] **`dogfood-gate` (AC: 6)**
  - [ ] Run the entry from the story branch (pre-land build) for 3-5 working days of real sessions, used unprompted.
        Not one demo run. If running pre-land is annoying, fix that friction first, or the gate quietly becomes
        post-land (epics.md:1421).
  - [ ] Provoke the named adversity: stop the daemon mid-day and run the entry. The pass condition is a clear
        message, not a stack trace. If it produces `TypeError: fetch failed`, that is a finding, fix it and note it.
  - [ ] Record the harvest note in the "Dogfood Gate Evidence" section below BEFORE the story lands: at least one
        concrete behavior change driven by use, or an explicit "no changes needed, here's what I watched for."
  - [ ] The gates serialize across the epic. Four dogfood windows is roughly a month of wall-clock; that is the plan,
        not a problem to optimize away by overlapping windows into meaninglessness.

- [x] **`verify` (AC: all)**
  - [x] `scripts/test.sh` green (never raw `cargo test`; see Dev Notes "Test execution"). `cargo fmt --check` and
        `cargo clippy --all-targets --workspace -- -D warnings` green.
  - [x] `cd docs/cookbook/session-glance && npm ci && npm run typecheck` green (mirrors CI's `typecheck-examples`).
  - [x] `git diff | grep $'^+.*—'` is empty (no emdashes in added lines). Generated prose reliably
        reintroduces them; the `—` escape keeps this file itself sweep-clean.
  - [x] `python3 scripts/check-file-list.py docs/bmad/implementation-artifacts/6-session-glance.md --base main`
        exits 0 before Status flips to `review`. Exit 1 is drift, not a script failure: fix the record, not the audit.
        `--ignore` is only for paths genuinely outside this story's authorship, and using it belongs in Completion
        Notes.
  - [x] No `crates/protocol/src` touch, so no protocol-changelog entry. Do not manufacture one.

## Dogfood Gate Evidence

Filled in by the dev/maintainer before this story lands. Required by AC 6 and the epic's dogfood gate protocol
(epics.md:1415-1421). An empty section here means the story is not landable regardless of CI.

> **STATUS: PENDING. This is the only thing standing between the story and `done`.** The implementation is complete
> and the story is at `review`; the gate is a human activity that cannot be produced in a dev session, because it
> measures wall-clock exposure to real sessions. The maintainer performs it from branch `story/6-session-glance`
> before the branch lands. Nothing below may be filled in by an agent.
>
> What the maintainer needs to do:
>
> 1. Check out `story/6-session-glance` and run the entry against the real daemon over 3-5 working days, unprompted,
>    when a session is actually being wondered about. Not one demo run. If running it pre-land is annoying enough
>    that it does not happen, fix that friction first (epics.md:1421); a gate that quietly slips to post-land is the
>    failure mode this protocol exists to prevent.
> 2. Provoke the named adversity for real: stop the daemon mid-day, then run the entry. Note that a graceful
>    `bowerbird stop` removes `server.json` and exercises failure mode (a); to exercise mode (b), the one this story
>    fixed, the daemon has to die uncleanly (`kill -9`, a crash, an OOM). Paste what it printed, verbatim.
> 3. Write the harvest note below.
>
> The CI-side counterparts already exist and are green, but they are not the gate:
> `tests/cli_examples.rs::cookbook_entries_fail_clearly_when_daemon_down` (mode a) and
> `session_glance_names_the_address_when_server_json_is_stale` (mode b). What they cannot tell you is whether the
> glance is worth reaching for.

**Exposure window:** _(dates; 3-5 working days of real sessions, used unprompted, from the story branch)_

**Provoked adversity (named by the AC): daemon stopped mid-day.** _(what was run, what the entry printed verbatim,
verdict: clear message or stack trace)_

**Harvest note:** _(at least one concrete behavior change driven by use, OR an explicit "no changes needed, here is
what I watched for")_

## Dev Notes

### REST shape, verified against source (do not invent this)

`GET /sessions` is handled by `crates/daemon/src/api/sessions.rs::list`. It returns **a bare JSON array**, not an
envelope. Each element is `protocol::SessionListItem` (`crates/protocol/src/rest.rs:38-53`):

| Field | Type | Notes |
|---|---|---|
| `source` | string | Part of the natural key. Never key on `session_id` alone. |
| `session_id` | string | |
| `current_state` | string | **PascalCase on the wire**: `Idle`, `Working`, `WaitingInput`, `Ended`, `Unknown`. |
| `last_event_kind` | string | |
| `last_event_at_ms` | number | epoch-ms |
| `updated_at` | number | epoch-ms |
| `last_pid` | number or null | The FR44 stretch (pid-to-tmux-pane) would start here. Out of scope, see below. |
| `cwd` | string or null | Story 5.7. **Verbatim**: no canonicalization, no `~` expansion, no symlink resolution. |
| `started_at` | number or null | Story 5.7, epoch-ms of the session's FIRST observed event, daemon-derived set-once. |

Query params (Story 5.8, ADR 0008), all optional, all default-unfiltered, invalid values `400` with `{"error": msg}`:

- `?state=<csv>` of case-insensitive tokens, applied in Rust against the **read-time derived** `current_state`.
- `?since=<epoch-ms>` exclusive `updated_at` lower bound (SQL).
- `?limit=<n>` SQL row cap.

Unknown query keys are a `400` (the params struct is `deny_unknown_fields`). Auth is `Authorization: Bearer <token>`
on every route except `/healthz` and `/readyz`.

### The `?state=` filter has no negation

AC 1 says "every non-Ended session." The filter grammar has no `!ended` and no negation of any kind. The accepted
tokens are exactly `idle`, `working`, `waitinginput`, `ended`, `unknown` (case-insensitive, parsed by
`crates/daemon/src/api/filter.rs::parse_state_token`). So "non-Ended" is spelled as the positive CSV
`?state=idle,working,waitinginput,unknown`.

Two traps in that sentence:

- **Include `unknown`.** It is a real decode-only catch-all variant (`crates/protocol/src/state.rs:27-28`) reserved
  for future additive `current_state` values. Omitting it means a future daemon's new state silently vanishes from the
  glance, which is the opposite of what an attention surface is for. If you deliberately omit it, say so in the README.
- **The filter tokens are lowercase; the rendered field is PascalCase.** `?state=waitinginput` filters correctly and
  the response says `"current_state":"WaitingInput"`. Any client-side re-filter or label lookup that assumes the two
  spellings match will silently produce an empty group.

Also note `Ended` is explicitly **non-terminal** (`crates/protocol/src/state.rs:22-26`): a session can transition out
of `Ended` on its next hook event (typically `claude --resume`). "Live" here means "not currently Ended," not
"never ended."

### Canonical repo-from-`cwd` derivation (AC 3, the durable artifact)

This is the piece later stories cite instead of reinterpreting FR44 (epics.md:1427), so it needs to be pinned in two
durable places (code doc comment on a named export + the entry README), not implied by the implementation.

The substrate deliberately ships `cwd` as a mechanical fact and refuses to derive repo itself (Axiom 4, ADR 0006:
"Derivations *from* these (repo/project/branch from `cwd`, session age from `started_at`) are presenter concerns,
never daemon fields"). So the derivation living in a presenter is correct by design, not a workaround.

**Proposed rule** (create-story decision, see "Decisions made without asking" below; revise it only by updating the
doc comment and the README together, since later stories bind to it):

1. `cwd` is `null` -> a single named bucket (e.g. `(unknown repo)`). Do not drop the session; a session with no `cwd`
   is exactly the one you would otherwise never notice.
2. Walk up from `cwd` to the nearest ancestor containing a `.git` entry; render that ancestor's basename.
   Check for **existence**, not for a directory: in a git worktree `.git` is a *file*, and an `isDirectory()` check
   would walk past the worktree to the main repo.
3. No `.git` ancestor found, or the path is not readable (session on a since-deleted dir, or a `cwd` from another
   machine) -> `basename(cwd)`.
4. Never throw. An unreadable path is a bucket, not a crash.

Known imprecisions to state plainly in the README rather than paper over:

- **Git worktrees** resolve to the worktree directory's basename, which is usually the branch name, not the repo name.
  That is arguably the more useful grouping for the maintainer's `~/worktrees/{repo}/{branch}` layout, but it is a
  behavior, so name it.
- **A `cwd` below the repo root** (Claude launched from a subdirectory) resolves correctly to the repo via rule 2;
  that is the reason rule 2 exists instead of a bare `basename(cwd)`.
- The derivation touches the filesystem. That is acceptable here (the entry runs on the same host as the sessions)
  but it means the function is not purely testable; keep the path-walking in one small helper so the formatting side
  stays unit-testable.

### Machine mode: what `6-tmux-ambient` actually needs

AC 2 says "e.g. a count/format flag" and leaves the shape open. The binding constraint is the epic's only hard code
dependency: `6-tmux-ambient` invokes this CLI (epics.md:1413) to render a status-line count like "2 blocked"
(PRD FR43, prd.md:596: "wired from the same query as `session-glance`").

Recommended surface, which makes tmux-ambient's whole data path one command:

- `--count` -> a single integer on stdout, nothing else. Shell-substitutable without parsing.
- `--state=<csv>` -> passed through to the REST query, so `session-glance --count --state=waitinginput` is "how many
  blocked."
- `--format=json` -> NDJSON, one object per session, fixed field set. The escape hatch for a consumer that needs more
  than a count.
- Exit codes documented alongside: 0 success (including zero sessions), non-zero on any failure.

Whatever shape ships, the README states it as a mini-API and the smoke asserts it. AC 2 is a contract AC: it is
satisfied by the documented contract plus a test pinning it, not by the flag existing.

### Cookbook integration checklist (epics.md:1429, definition-of-done)

Verified against the real files, not the epic's summary. Every item below has a machine guard unless noted:

| Surface | What this story does | Guard |
|---|---|---|
| `tests/cli_docs_drift.rs` | glob-derive the entry list, count, and message strings | AC 4; the guard is itself the subject |
| `tests/cli_examples_drift.rs` | `ENTRIES` + required-files + engines floor cover the new entry | `entries_const_matches_directory_listing` goes red today if you add the dir without touching it |
| `tests/cli_examples.rs` | doc comment update + one smoke test + daemon-down loop entry | the smoke |
| `docs/cookbook/README.md` | index table row (`](session-glance/)` link form) + Quick run block | `cookbook_readme_lists_three_required_entries` (being globbed) |
| `docs/cookbook/session-glance/package-lock.json` | committed | `each_entry_has_required_files`; CI `npm ci` |
| five-section README shape, no fenced ts | prose only, headings in order | `every_cookbook_entry_has_canonical_five_sections` + `scan_prose_only_markdown` allowlist |
| CI typecheck | automatic, `.github/workflows/ci.yml` globs `docs/cookbook/*/` and skips dirs without `package.json` | the job |

The last row is why the glob refactor matters: CI already picks up a new entry dir automatically, while the hardcoded
Rust guard lists do not, so a fourth entry can be typechecked-but-unguarded. That gap is what
`cookbook_entry_consts_match_directory_listing` was added for during the 5.13 review, and glob-derivation is the
structural fix it was a stopgap for.

### A13, extended scope (epics.md:1423)

Two obligations, both landing on the `glob-derive-docs-drift-guard` task:

1. **Observed failing.** The refactored guard must be watched red against a deliberately broken README before it is
   kept. Restore, watch green, keep. Record both run ids. Team agreement A13 exists because 5.16 shipped a guard test
   that passed 10/10 without ever executing the mechanism it claimed to verify.
2. **Positive companion.** Every negative assertion carries a positive assertion proving the precondition fired. Here:
   assert the glob actually derived a non-empty list containing the expected entries BEFORE asserting they all
   conform. A derivation that silently yields an empty list makes every downstream assertion vacuously true.

This is the smaller sibling of the discipline that lands with force on `6-transition-alerts` (snapshot suppression);
the epic extends it to every negative assertion in the epic, and the glob refactor is one.

### Testing discipline

The suite runs in parallel and CI's 4-vCPU runners can starve a thread for seconds, so timing assumptions that hold on
a fast laptop are bugs. Do not restate the rules; follow them:

- **Always** run tests via `scripts/test.sh`, never raw `cargo test`. A second concurrent `cargo test` process in this
  worktree is the confirmed trigger for this project's intermittent hangs. See `CLAUDE.md` §Running tests for the
  lock, the timeout, the `target/test-logs/<run>/run.log` capture, and `--unlock`.
- Full rationale with the CI failure history behind each rule: `docs/bmad/project-context.md` §Deterministic test
  discipline (no real `sleep()` for synchronization; hang guards are 30s, not "how fast it should be"; no
  `std::env::set_var`, which `clippy.toml` bans; inject env per-child via `Command::env`).
- `tests/cli_examples.rs` is already parallel-safe by construction (TempDir data dirs, ephemeral ports read from each
  test's own `server.json`). Copy that shape; do not introduce a shared fixture.
- The entry's smoke needs Node 22.6+ on PATH. The file's `node_22_6_available()` gate skips cleanly when it is absent;
  keep using it rather than failing.

### Critical gotchas

- **Do not rename `tests/cli_examples.rs`.** `tests/release_pipeline_docs.rs` references it by name in its release.yml
  Node-setup assertions (Story 5.13 gotcha, still true).
- **`release_pipeline_docs.rs` pins exact substrings** in `README.md`, `INSTALL.md`, and `ci.yml`. This story should
  not need to touch any of those three, but if a doc edit drifts into them, run
  `scripts/test.sh --test release_pipeline_docs` immediately rather than at the end.
- **The `typecheck-examples` job id stays stable.** Branch protection does not pin it (verified during 5.13), but
  there is no reason to churn a check name; this story does not need to edit the job at all, since it already globs.
- **`docs/cookbook/.gitignore`** already ignores `node_modules/` and `*.log` for all entries. Do not commit
  `node_modules`, and do not add a per-entry `.gitignore`.
- **Historical artifacts are not swept.** `docs/bmad/**` and `docs/research/**` legitimately reference the
  three-entry world as a record of what was true. Only living surfaces change.
- **No ADR needed.** The ADR triggers (project-context.md §ADR triggers) are protocol/crate/runtime/ingest/schema
  changes; a presenter-side derivation convention fires none of them. The canonical record for AC 3 is the code doc
  comment plus the README, which is what the epic conventions ask for. If review disagrees, the next free ADR number
  is 0012 (0011 is Story 5.15's crates.io deferral).
- **FR44 stretch is out of scope.** The pid-to-tmux-pane hop (matching `last_pid` into tmux pane PIDs) is explicitly
  excluded from all gating ACs in this epic (epics.md:1427). It may appear as a "How to apply it" mention in the
  README. If pursued, it is its own story or an explicit deferral with a trigger.

### Build-order and dependency context

Order for the epic is `6-session-glance` -> `6-transition-alerts` -> `6-tmux-ambient` -> `6-live-board-port` ->
`6-live-board-dogfood` (epics.md:1413). This **supersedes** the PRD Phase 3 Scope order at
`docs/bmad/planning-artifacts/epics.md:172` and `prd.md:135-138`, which list live-board third; the epic conventions
are the current statement and say so explicitly (live-board is the riskiest story and moves last). The only hard code
dependency is `6-session-glance` -> `6-tmux-ambient`.

**Carried items that are NOT this story** (epics.md:1431): the `Reaction`/`tool-reactions.toml` open question is
decided during the cycle, not here, and must not be resolved by forcing a consumer into existence, so do not reach
for `reaction` in this entry's output. Epic 5 retro AI-3 (File-List decide-or-retire) was the pre-story gate and is
closed: commit `356c795` wired `scripts/check-file-list.py` at the point of writing, which is why the `verify` task
above runs it.

### Project Structure Notes

- Story file: `docs/bmad/implementation-artifacts/6-session-glance.md`, matching sprint-status key `6-session-glance`.
  Slug, not ordinal, in both places.
- New entry lives at `docs/cookbook/session-glance/`, taking the cookbook from three entries to four. The directory
  name IS the entry name everywhere (guards, CI glob, README link form, smoke `spawn_example` argument).
- `docs/cookbook/*/` is a Node project zone, deliberately excluded from the Cargo workspace
  (`members = ["crates/*"]`, asserted by `cookbook_not_in_root_cargo_toml_members`). Adding an entry must not touch
  the root `Cargo.toml`.
- No new crate, no migration, no bench impact, no protocol surface.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Epic 6 conventions and cross-cutting rules] lines 1409-1431: slug
  keys, build order + DAG, dogfood gate protocol, A13 extended scope, output-seam convention, FR44 anchoring, cookbook
  integration checklist, carried items.
- [Source: docs/bmad/planning-artifacts/epics.md#Story 6-session-glance] lines 1433-1464: the six ACs copied above.
- [Source: docs/bmad/planning-artifacts/epics.md] line 172-179: Phase 3 additional requirements (superseded build
  order, zero-substrate-change constraint, cookbook integration surface).
- [Source: docs/bmad/planning-artifacts/prd.md] FR40 (line 593), FR43 (596), FR44 (597), Phase 3 Scope (131-138),
  cookbook deliverable row (509), Phase 3 success gate (81).
- [Source: crates/daemon/src/api/sessions.rs] `SessionsParams` + `list`: the real `?state=`/`?since=`/`?limit=`
  handling, bare-array response, 400 shape.
- [Source: crates/daemon/src/api/filter.rs] `parse_state_token` / `parse_state_filter` / `state_matches`: the five
  accepted tokens, case-insensitivity, no-negation grammar, error message vocabulary.
- [Source: crates/protocol/src/rest.rs] `SessionListItem` field set (38-53).
- [Source: crates/protocol/src/state.rs] `SessionCurrentState` PascalCase wire format, `Unknown` decode-only
  catch-all, `Ended` non-terminal (6-29).
- [Source: tests/cli_docs_drift.rs] `REQUIRED_COOKBOOK_ENTRIES` 37-41, `required_docs_exist` message 43-54,
  `scan_prose_only_markdown` fence allowlist 65-94, five-section guard 96-122,
  `cookbook_entry_consts_match_directory_listing` 134-167, `DOCS_TO_CHECK` 176-187,
  `cookbook_readme_lists_three_required_entries` 519-538.
- [Source: tests/cli_examples_drift.rs] `ENTRIES` const, `entries_const_matches_directory_listing`,
  `each_entry_has_required_files` (incl. `package-lock.json`), `each_entry_package_json_declares_node_22_6_engine`,
  `cookbook_readme_carries_cargo_zone_note`, `cookbook_not_in_root_cargo_toml_members`.
- [Source: tests/cli_examples.rs] daemon orchestration helpers, `node_22_6_available`, `spawn_example`,
  `read_stdout_until`, `cookbook_entries_fail_clearly_when_daemon_down`.
- [Source: docs/cookbook/rest-cursor-pagination/src/index.ts] `loadServerInfo`, `resolveToken`, the
  `main().catch(e => { console.error(e.message); process.exit(1) })` shape, REST + Bearer usage.
- [Source: docs/cookbook/state-session-fanout/README.md] the five-section README in practice.
- [Source: docs/cookbook/README.md] index table + Quick run + Cargo-zone note.
- [Source: .github/workflows/ci.yml] `typecheck-examples` job 57-88: the `docs/cookbook/*/` glob and the
  `package.json` skip that the Rust glob derivation must mirror.
- [Source: docs/bmad/project-context.md] Axiom 4 + §Cookbook discipline + §Deterministic test discipline +
  §Substrate-not-actor invariants (`cwd`/`started_at` are mechanical facts; repo/age are presenter derivations).
- [Source: docs/bmad/implementation-artifacts/epic-5-retro-2026-08-02.md] A13 (line 196), A14, A15.
- [Source: docs/bmad/implementation-artifacts/5-13-cookbook-consolidation.md] closest analog: the cookbook shape,
  guard-replacement discipline ("deleting the guard without a successor is a coverage regression"), pinned-string
  gotchas.
- [Source: docs/bmad/implementation-artifacts/5-8-server-side-session-filter.md] the filter story this entry is the
  first consumer of.
- [Source: CLAUDE.md] `scripts/test.sh` discipline, parallel-safe test rules, File List audit protocol.

## Dev Agent Record

### Agent Model Used

claude-opus-5[1m] (Claude Opus 5, 1M context), via `bmad-dev-story` on branch `story/6-session-glance`.

### Debug Log References

Every id below is a `target/test-logs/<run>/run.log` directory produced by `scripts/test.sh`. A13 requires that a
negative assertion be OBSERVED failing against deliberately broken input before the passing version is kept, so each
RED run names the break, and each break was reverted immediately after the observation.

**Guard refactor (AC 4), `cli_docs_drift.rs` + `cli_examples_drift.rs`:**

| Run id | State | What was done to provoke it |
|---|---|---|
| `20260802-160404-7689` | GREEN | Baseline after the refactor: 12 + 6 tests pass. |
| `20260802-160427-7813` | RED x3 | Four breaks in the new entry's README and the cookbook index at once: a ` ```ts ` fence added (prose-only allowlist red), the index table's `](session-glance/)` row deleted (`cookbook_readme_links_every_entry_directory` red, message names the derived count as 4), a link target pointed at a nonexistent file (`quickstart_internal_links_resolve` red on the new entry's README, which proves `DOCS_TO_CHECK`'s tail is derived). |
| `20260802-160439-7894` | RED | `## Files` demoted to a level-3 heading. `every_cookbook_entry_has_canonical_five_sections` red naming `docs/cookbook/session-glance/README.md` and the missing section. This is the break AC 4 names by example, run alone so the fence break above could not mask it. |
| `20260802-160448-7970` | RED | The entry README deleted outright. `required_docs_exist` red, message reads "one README per docs/cookbook/*/ entry (4 entries derived)" so the derivation is visibly what drove it. |
| `20260802-160459-8051` | RED x4 | **The A13 positive companion.** The `package.json` filter forced to skip everything, so the glob derives an empty list. Rather than four guards passing vacuously, all four fail with "derivation found 0 entries ([]), fewer than the floor of 3". Same in both files. |
| `20260802-160511-8178` | RED | `docs/cookbook/scratch-notes/` created with a README and no `package.json`. `every_cookbook_subdirectory_is_a_typechecked_entry` red. This is the successor guard for the deleted `cookbook_entry_consts_match_directory_listing`, and this run is the proof it guards something. |

**Entry behavior (AC 1, 2, 6), `cli_examples.rs`:**

| Run id | State | What was done to provoke it |
|---|---|---|
| `20260802-160527-8289` | GREEN | Baseline: all 9 tests in the file pass, including the three new session-glance smokes. |
| `20260802-160543-8616` | RED | `DEFAULT_STATES` widened to include `ended`. Red, but only via the count fence timing out at 30s. Treated as a finding about the FENCE, not accepted as the observation: fixed `wait_for_glance_count` to fail fast on an overshoot, since the count only rises as rows commit so a too-high count can never be fixed by waiting. |
| `20260802-160652-9142` | RED | Same break, re-observed after that fix: fails in 0.46s with "reported 4, more than the 3 non-Ended sessions the fixture defines. The ?state= filter is not excluding Ended." |
| `20260802-160710-9285` | RED | A bug-shaped break that isolates the assertion itself: `--count` keeps the correct filter while the text-render path silently refetches unfiltered. `sess-delta is Ended and must not appear in the default glance; got: claude/sess-delta Ended 0s`. |
| `20260802-160721-9426` | RED | The `fetch` try/catch removed from the entry. `session_glance_names_the_address_when_server_json_is_stale` red with stderr exactly `fetch failed`. This is the pre-fix behavior AC 6 forbids, observed rather than asserted. |
| `20260802-160735-9528` | RED | The test's `SIGKILL` swapped for a graceful `stop_daemon`. "server.json must SURVIVE an unclean daemon death; that is mode (b): NotFound". Proves the mode-(b) precondition is real and cannot silently degrade into a second copy of the mode-(a) test, and independently confirms that a clean stop removes `server.json` while an unclean death does not. |

**Full workspace:** `20260802-160753-9732`, `scripts/test.sh` (parallel), **652 passed / 0 failed**. Re-run after the
comment-only em-dash sweep and the story-record edits: `20260802-161344-12315`, also **652 passed / 0 failed**.

**Entry-local Node tests:** `npm test` in `docs/cookbook/session-glance/` is 16/16. Its negative assertions were
observed red too, against a deliberately broken `src/index.ts`: swapping `existsSync(.git)` for
`statSync(...).isDirectory()` made the worktree test report `main-repo` instead of `feature-branch`, and disabling the
`--state` token pre-check made both `assert.throws` cases fail with "Missing expected exception". Both breaks reverted.

### Completion Notes List

**What shipped, by AC.**

- **AC 1** (grouped, non-Ended, ages, one-shot, via `?state=`): `docs/cookbook/session-glance/`. Fetches
  `GET /sessions?state=idle,working,waitinginput,unknown` once, groups by derived repo, prints
  `<source>/<session_id>  <current_state>  <age>` per session, exits. `unknown` is in the default set deliberately
  (decode-only catch-all; omitting it would let a future daemon state vanish from an attention surface). Asserted by
  `cli_examples.rs::session_glance_groups_live_sessions_by_repo_with_ages`.
- **AC 2** (machine mode + documented contract): `--count`, `--state=<csv>`, `--format=json`, stated as a mini-API in
  the entry README's "Run it" and pinned key-for-key by
  `cli_examples.rs::session_glance_machine_modes_pin_the_output_contract`.
- **AC 3** (canonical repo derivation): `deriveRepo` is a named export with the rule in its doc comment AND in the
  README's "How it works", including the three named imprecisions (worktree resolves to the branch-shaped directory
  name, `cwd` below the repo root resolves upward correctly, the derivation touches the filesystem).
- **AC 4** (glob-derive the docs-drift guard): done, with the scope widening recorded below.
- **AC 5** (cookbook integration checklist + green CI): every row of the Dev Notes checklist is satisfied;
  `scripts/test.sh` 652/0, `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` clean,
  `npm ci && npm run typecheck` clean from a wiped `node_modules`.
- **AC 6** (dogfood gate): **NOT satisfied. Deliberately left pending** and the story stops at `review` because of it.
  The CI-side counterpart of the provoked adversity IS done and green (both failure modes), and the gap the story
  named is FIXED rather than merely disclosed, but the 3-5 day exposure window and the harvest note are maintainer
  work. See the Dogfood Gate Evidence section for the instructions.

**AC 4 scope widening, recorded as an AC-text supersession** (same shape as the 5.18 lesson: the AC governs the goal,
its enumeration was incomplete). The AC names `tests/cli_docs_drift.rs`. The list was hardcoded in four places there
(`REQUIRED_COOKBOOK_ENTRIES`, `required_docs_exist`'s message, `DOCS_TO_CHECK`'s tail,
`cookbook_readme_lists_three_required_entries`) with an exact twin at `tests/cli_examples_drift.rs::ENTRIES`, and a
third instance in `tests/cli_examples.rs::cookbook_entries_fail_clearly_when_daemon_down`'s loop. All were
glob-derived. Leaving any of them literal would have relocated the divergence to the next entry rather than removing
it, which defeats the AC's intent. The `cli_examples.rs` loop is the widest step and is justified separately: the
daemon-down contract (non-zero exit, `server.json` named on stderr) is cookbook-wide, not entry-specific.

**`cookbook_entry_consts_match_directory_listing`: deleted, with a named successor** (Story 5.13 lesson, decision
recorded as the task requires). It compared the hardcoded const against the `docs/cookbook/*/` listing; once the const
IS that listing, it compares the glob to itself. Its three intents were re-homed rather than dropped:

1. "Every entry dir is covered by the shape guards" is now true by construction, since every guard iterates the glob.
2. "No entry dir escapes coverage" survives as
   `cli_examples_drift.rs::every_cookbook_subdirectory_is_a_typechecked_entry`, which asserts the one thing
   glob-derivation genuinely cannot: a `docs/cookbook/` subdirectory with no `package.json` is skipped by BOTH CI's
   typecheck loop and the derivation, so it would look like an entry to a reader while having zero coverage. Today
   there are none; adding one is now a deliberate edit to that test.
3. "The derivation actually found something" is the A13 positive companion inside `cookbook_entry_dirs()`.

A deletion comment at the old site names all three, so the successor is findable from where the guard used to be.

**The daemon-down gap was fixed, not just disclosed.** The story named it and it is real: the daemon removes
`server.json` only on a CLEAN shutdown (`crates/daemon/src/main.rs`), so a crash or `kill -9` leaves it pointing at a
dead address, and Node reports that as a bare `TypeError: fetch failed` with neither the address nor the fix in the
message. The entry catches it and names both, including the OS error code. Run `20260802-160735-9528` independently
confirmed the two modes are genuinely distinct: swapping the test's `SIGKILL` for a graceful stop made `server.json`
disappear and the mode-(b) precondition fail.

**Fixture design, because the bundled one could not do this job.** `fixtures/replay-demo.jsonl` ends every session on
`Stop` (all Idle), carries no `cwd`, and carries no `pid`. That last one is disqualifying rather than merely
unhelpful: `projection/liveness.rs` emits a synthetic `SessionEnded` for any row whose `last_pid` is null
(`no_pid_at_upgrade`), so within one 5s probe tick every bundled-fixture session becomes `Ended` and the glance
correctly shows nothing. The smokes therefore replay their own fixture via `bowerbird replay <file>` with three
DISTINCT live pids (distinct because a shared pid trips the Story 5.11 supersession path, which would end the
predecessor). No production code was changed to make the tests work.

**One test-side finding, fixed in place.** The first observation of the leaky-filter break went red only by exhausting
the 30s hang guard, because `wait_for_glance_count` polled until the expected count appeared. The count only rises as
rows commit, so overshooting it can never be fixed by waiting; the fence now fails fast on an overshoot with a message
naming the filter. That is run `20260802-160543-8616` (slow, ambiguous) versus `20260802-160652-9142` (0.46s, names
the cause).

**Deliberately NOT done.**

- No `crates/` change of any kind, no `crates/protocol/src` touch, no schema change, so no protocol-changelog entry
  was manufactured. The only Rust written is test code.
- No ADR. The triggers (protocol / crate / runtime / ingest / schema) are all unfired; the canonical record for AC 3
  is the code doc comment plus the README, which is what the epic conventions ask for.
- The FR44 pid-to-tmux-pane stretch is out of scope and appears only as a named non-goal in the README's "How to
  apply it".
- `reaction` / `tool-reactions.toml` is untouched. It is a carried epic item to be decided during the cycle, and
  reaching for it in this entry's output would have decided it by forcing a consumer into existence.

**File List audit.** `python3 scripts/check-file-list.py docs/bmad/implementation-artifacts/6-session-glance.md
--base main` exits 0, reporting `12 changed in git | 12 declared`. No `--ignore` was used, so there is nothing to
disclose on that front. This was the audit's first real exercise since commit `356c795` wired it, and it behaved
correctly: it resolved the merge-base against `main` without help, counted the untracked-but-new entry files and the
committed test edits alike, and reported CLEAN on the first invocation with no false positives (notably it did not
trip over `docs/cookbook/session-glance/node_modules/`, which the cookbook `.gitignore` already excludes). No finding
to report against the audit itself.

**One nit on the story file itself, not a finding worth acting on.** The `verify` task's em-dash sweep
(`git diff | grep $'^+.*—'`) is clean for every file this story authored: code, tests, and the entry README have no
em-dashes in added lines. The only hits in the diff are inside this story file's own description OF the check, which
create-story wrote with literal em-dashes while claiming the escape kept the file sweep-clean. Left alone: rewriting
Dev Notes prose is outside the sections dev-story may modify, and the sweep's purpose (keeping authored prose clean)
is met.

### File List

- `docs/bmad/implementation-artifacts/6-session-glance.md` (this story file: task checkboxes, Dev Agent Record,
  Dogfood Gate Evidence pending note, File List, Change Log, Status)
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (story key `6-session-glance` -> `in-progress` -> `review`)
- `docs/cookbook/README.md` (index table row + Quick run block for the new entry; "the existing three" generalized)
- `docs/cookbook/session-glance/README.md` (new)
- `docs/cookbook/session-glance/package.json` (new)
- `docs/cookbook/session-glance/package-lock.json` (new)
- `docs/cookbook/session-glance/tsconfig.json` (new)
- `docs/cookbook/session-glance/src/index.ts` (new)
- `docs/cookbook/session-glance/tests/glance.test.ts` (new)
- `tests/cli_docs_drift.rs` (glob-derived entry lists; `cookbook_entry_consts_match_directory_listing` deleted with a
  named successor; `cookbook_readme_lists_three_required_entries` renamed
  `cookbook_readme_links_every_entry_directory`)
- `tests/cli_examples_drift.rs` (glob-derived `ENTRIES`; `entries_const_matches_directory_listing` repurposed as
  `every_cookbook_subdirectory_is_a_typechecked_entry`)
- `tests/cli_examples.rs` (three session-glance smokes, glob-derived daemon-down loop, fast-failing count fence, doc
  comment update)

## Change Log

- 2026-08-02: Story created via bmad-create-story. Epic 6 conventions (epics.md:1409-1431) read as binding and folded
  into tasks; ACs copied from epics.md:1439-1464 without paraphrase. Real API shape verified against
  `crates/daemon/src/api/sessions.rs`, `crates/daemon/src/api/filter.rs`, and `crates/protocol/src/rest.rs` rather
  than taken from the epic prose. Real guard shape verified against `tests/cli_docs_drift.rs`,
  `tests/cli_examples_drift.rs`, `tests/cli_examples.rs`, and `.github/workflows/ci.yml`. Slug key preserved in both
  the filename and the sprint-status entry.

- 2026-08-02: Implemented via bmad-dev-story on branch `story/6-session-glance`. Twelve of thirteen tasks complete;
  `dogfood-gate` is deliberately open, which is why the story stops at `review` and not `done` (AC 6's 3-5 day
  exposure window and harvest note are maintainer work, and the Dogfood Gate Evidence section now carries the
  instructions). Presenter-only as scoped: zero `crates/` changes, no `crates/protocol/src` touch, no schema change,
  no ADR, no protocol-changelog entry. AC 4's scope was widened past its own enumeration (five hardcoded entry lists
  glob-derived across three test files, not one) and the widening is recorded in Completion Notes as an AC-text
  supersession. `cookbook_entry_consts_match_directory_listing` deleted with a named successor;
  `cookbook_readme_lists_three_required_entries` renamed and de-counted. Daemon-down failure mode (b) (stale
  `server.json`, refused connection, bare `TypeError: fetch failed`) is fixed and covered, not just disclosed. A13
  honored throughout: eleven RED runs against deliberately broken input, each named in Debug Log References with the
  break that produced it and each reverted after the observation. `scripts/test.sh` 652 passed / 0 failed
  (`20260802-160753-9732`; re-confirmed after the comment-only em-dash sweep by `20260802-161344-12315`),
  `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` clean, `npm ci && npm run typecheck`
  clean from a wiped `node_modules`, entry-local `npm test` 16/16. File-List audit CLEAN on the first invocation
  (12 changed in git, 12 declared, no `--ignore`).
