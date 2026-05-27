# Story 5.2: Session state projection correctness

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a presenter author,
I want session-state broadcasts to fire only on actual `current_state` transitions, and `Working` signals to cover the agent's full active period (user prompt submission through tool completion, not just `PreToolUse` moments),
so that ribbon UIs render only on meaningful state changes — no flap between back-to-back tool calls, no false `Idle` gap during the agent's between-tool thinking, no false `Idle` gap while the agent composes its first tool call after a user prompt.

Source: `docs/bmad/planning-artifacts/epics.md` §"Story 5.2: Session state projection correctness" (lines 930–974). Defect rationale and remediation design in `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27.md`. Resequenced from 5.7 → 5.2 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md` (dogfooding-first ordering: this story sits adjacent to 5.1 so the maintainer's dogfooding window has a useful presenter signal).

**In flight:** Story 5.1 (`5-1-first-party-presenter-tool`) is `in-progress` — Tasks 1–4 + 7 shipped; Tasks 5–6 (5-day dogfooding window + friction capture) are pending the maintainer's calendar. Both defects this story closes were surfaced *by* that 5.1 presenter against the running daemon. The two stories will likely merge in adjacent PRs; nothing in 5.2 blocks on 5.1 reaching `done`, and the 5.1 dogfooding window will be more useful once 5.2 ships.

**What is already done (pre-staged in commit 78c26b3 / 2026-05-27 course-correction):**

- `docs/protocol-changelog.md` lines 44–45 — both Story 5.2 entries already present (one `type: behavioral` for transitions-only state broadcast, one `type: schema` for `UserPromptSubmit`).
- `docs/protocol.md:280` — broadcast emission rule already rewritten ("Emitted (a) on every `current_state` transition resulting from a projection write, and (b) as a snapshot on subscribe…").
- `docs/protocol.md:334` — `hook_kind` requirement list already includes `UserPromptSubmit`, BUT the inline cross-reference still says "Story 1.8, extended in Story 5.7"; this needs the `5.7 → 5.2` renumber per the resequencing proposal §4.1.
- `docs/protocol.md:340–351` — EventKind table already shows eight values including `UserPromptSubmit` row.
- `docs/bmad/planning-artifacts/prd.md:206` — "goes green when Claude finishes the turn" already in place.
- `docs/bmad/planning-artifacts/architecture.md:50–51` and `:1027` — "no stuck state on missing `PostToolUse` or `Stop`" already in place.

**What this story actually ships (the code half is the entire remaining lift):**

The substrate changes (event.rs, state.rs, session.rs, install.rs, normalize.rs) and the matching contract tests. The docs are already on disk; this PR makes the code match them.

## Acceptance Criteria

1. **Given** a session in `Working` and an incoming `PostToolUse` event **When** the projection writes the new state **Then** `last_event_kind` and `last_event_at_ms` are updated AND `current_state` remains `Working` (not `Idle`); subscribers to `state.session.*` and `state.session.<id>` receive NO `state` envelope for this event; subscribers to `events.*` still receive the `event` envelope.

2. **Given** N back-to-back `PreToolUse`/`PostToolUse` pairs for one session (initial state: `Idle`) **When** the events are ingested **Then** subscribers to `state.session.*` receive exactly one `state` envelope (the first `PreToolUse`'s `Idle`→`Working`); subscribers to `events.*` receive 2N event envelopes; `last_event_at_ms` still updates on every `PostToolUse` in the stored row.

3. **Given** Claude Code running with bowerbird installed **When** the user submits a prompt **Then** the `UserPromptSubmit` hook fires; the daemon ingests it; the `EventEnvelope` has `kind=UserPromptSubmit`; `current_state` transitions to `Working` (or remains `Working` if already there); `last_event_at_ms` updates.

4. **Given** a fresh `bowerbird install` against a Claude Code settings file with no prior hooks **When** installation completes **Then** `~/.claude/settings.json` registers five hooks (`PreToolUse`, `PostToolUse`, `Stop`, `Notification`, `UserPromptSubmit`); `bowerbird uninstall` removes all five; an existing install that pre-dates Story 5.2 surfaces "re-run `bowerbird install` to subscribe `UserPromptSubmit`" when only the four legacy hooks are detected.

5. **Given** a v1.0 presenter compiled against the pre-Story-5.2 protocol enum **When** it receives an event with `kind: "UserPromptSubmit"` from a Story-5.2+ daemon **Then** serde decodes it as `EventKind::Unknown` (Story 4.4 catch-all); no crash, no panic, no protocol-violation close frame.

6. **Given** `crates/daemon/src/projection/state.rs` after Story 5.2 **When** `transition()` is called with each `EventKind` variant **Then** `PostToolUse` preserves `prev.current_state` (returns `prev` unchanged on `current_state` but updates `last_event_kind`/`last_event_at_ms`); `UserPromptSubmit` returns `Working`; `PreToolUse` returns `Working`; `Stop` returns `Idle`; `Notification` returns `WaitingInput`; `RecordingStarted` / `RecordingEnded` / `Unknown` preserve prev (unchanged); the 5-minute `STALE_WORKING_MS` fallback is unchanged and now backstops both missing-`Stop` and missing-`PostToolUse`.

7. **Given** the protocol surface **When** Story 5.2 lands **Then** `crates/protocol/src/event.rs` `EventKind` gains `UserPromptSubmit`; `crates/adapter-claude/src/normalize.rs` maps the string `"UserPromptSubmit"` → `EventKind::UserPromptSubmit`; `HOOK_KINDS` in `crates/adapter-claude/src/install.rs` adds `"UserPromptSubmit"`.

8. **Given** the contract-test surface **When** Story 5.2 lands **Then** `crates/protocol/tests/contract_protocol.rs` and `crates/daemon/tests/contract_daemon.rs` are updated for both rules (state-broadcast diffing AND `UserPromptSubmit` lifecycle); `crates/adapter-claude/tests/contract_install.rs` exercises all five hook kinds; the existing `state_machine_full_sequence_determinism` test (`contract_daemon.rs:1242`) is updated to reflect the new `PostToolUse` → `prev` rule.

9. **Given** docs cross-references **When** Story 5.2 lands **Then** `docs/protocol.md:334` renumbers the inline reference "extended in Story 5.7" → "extended in Story 5.2"; the planning-artifact pre-staging (architecture.md, prd.md, protocol-changelog.md) is verified left untouched (it already reflects this story's outcome).

10. **Given** the protocol-changelog gate at `tests/protocol_changelog_gate.rs` **When** this PR modifies `crates/protocol/src/event.rs` **Then** the gate passes — either because the PR refines one of the two pre-staged Story 5.2 entries (the gate uses `git diff -U0` against origin/main and counts any `+`-prefixed line containing `type: schema|behavioral|security`), or because the PR adds a small supplemental entry capturing an aspect the pre-staging missed. See Dev Notes §"Changelog gate is the silent landmine" for the call.

## Tasks / Subtasks

- [x] **Task 1: Add `EventKind::UserPromptSubmit` to the protocol crate** (AC: #5, #7)
  - [x] Edit `crates/protocol/src/event.rs`. The variant ordering matters for serde and for human readers — slot `UserPromptSubmit` **before** `PreToolUse` so the wire-emitted variant order matches the chronological lifecycle (user submits → Claude tools up → Claude finishes). `Unknown` MUST remain last with `#[serde(other)]` (this is the Story 4.4 catch-all contract — preserved).
  - [x] Run `cargo build -p protocol` to confirm. No new dependencies; no new derive. Variant identifier IS the wire string (no `rename_all`).

- [x] **Task 2: Update the state machine in the daemon** (AC: #1, #2, #3, #6)
  - [x] Edit `crates/daemon/src/projection/state.rs::transition`. Change the `match event_kind` block:
    - `PostToolUse` arm: stop returning `Idle`. Instead, return a `SessionState` whose `current_state` is `prev.map(|s| s.current_state).unwrap_or(SessionCurrentState::Working)` and whose `last_event_kind` and `last_event_at_ms` are updated. The default-to-`Working` matches the documented semantics — a `PostToolUse` without a preceding event is degenerate but should not surface as `Idle` (the agent was clearly working a moment ago).
    - Add a `UserPromptSubmit` arm that returns `Working` (same shape as the existing `PreToolUse` arm).
    - `PreToolUse`, `Stop`, `Notification`, `RecordingStarted`, `RecordingEnded`, `Unknown` arms unchanged.
  - [x] Update / replace the existing unit test `transition_pretooluse_then_posttooluse_yields_idle` — that test name now lies. Replace with `transition_posttooluse_preserves_working` (and one for `transition_posttooluse_without_prev_defaults_to_working` covering the degenerate path).
  - [x] Add a new unit test `transition_user_prompt_submit_yields_working` and `transition_user_prompt_submit_then_pretooluse_stays_working`.
  - [x] The `STALE_WORKING_MS = 300_000` read-time fallback (line 19) is unchanged. It now backstops both a dropped `Stop` AND the legacy missing-`PostToolUse` case — no code change, but document the broadened role in the doc comment above `STALE_WORKING_MS`.

- [x] **Task 3: Tighten the broadcast publish in `projection::session::write`** (AC: #1, #2)
  - [x] Edit `crates/daemon/src/projection/session.rs::write`. The closure already reads `prev_state` (line 99) and computes `new_state` via `transition()` (line 119); both are returned out of the closure (line 149). After commit, currently both `BroadcastEnvelope::Event` and `BroadcastEnvelope::State` are unconditionally published (lines 172–177).
  - [x] Add a transition check before the `State` publish: `if prev_state.map(|s| s.current_state) != Some(new_state.current_state) { broadcaster.publish(BroadcastEnvelope::State { ... }) }`. **NB:** the closure currently moves `prev_state` (via the `.optional()? .and_then(...)` chain) — you'll need to clone the `current_state` out of `prev_state` BEFORE the closure consumes it, or return it from the closure tuple alongside `new_state`. Return `(EventId, SessionState, Option<SessionCurrentState>)` from the closure and decide the publish at the call site.
  - [x] First-event semantics: when `prev_state` is `None`, treat the comparison as `None != Some(new_state.current_state)` → publish (so a new session's first state envelope is always broadcast). This matches AC #2's "exactly one envelope for N back-to-back pairs starting from `Idle`" — the first `PreToolUse` is the `None → Working` transition.
  - [x] **DO NOT skip the `Event` publish.** Every successful write still publishes `BroadcastEnvelope::Event`. Only `State` is gated. The doc comment on `write` (lines 28–42) should be updated to reflect the new behavior: "publishes one `BroadcastEnvelope::Event` followed by zero-or-one `BroadcastEnvelope::State` so any WS subscribers see the event before the resulting projection update IFF the projection update changed `current_state`."
  - [x] The `tracing::debug!("ws: published event + state envelopes")` log line (180) is now sometimes inaccurate. Either log conditionally or change the message to `"ws: published event envelope; state envelope { gated: bool }"`.

- [x] **Task 4: Subscribe the `UserPromptSubmit` hook in adapter-claude** (AC: #3, #4, #7)
  - [x] Edit `crates/adapter-claude/src/install.rs`. Append `"UserPromptSubmit"` to `HOOK_KINDS` (line 21). Order: place it before `"PreToolUse"` to match the chronological lifecycle (mirrors the EventKind variant ordering decision in Task 1) — `&["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "Notification"]`.
  - [x] Edit `crates/adapter-claude/src/normalize.rs`. Add `"UserPromptSubmit" => EventKind::UserPromptSubmit,` to the `match hook_kind` block (lines 68–74). Slot it before `"PreToolUse"` for the same lifecycle-ordering reason.
  - [x] **Reaction handling:** `UserPromptSubmit` does NOT carry a `tool_name` (only `PreToolUse` does — see lines 82–91). The existing `match kind { EventKind::PreToolUse => ... , _ => None }` block naturally handles this — `UserPromptSubmit` falls through to `None`, which is correct. No change needed.

- [x] **Task 5: Add legacy-hook detection in `bowerbird install`** (AC: #4)
  - [x] When `bowerbird install` runs against a settings.json that has the four pre-5.2 bowerbird hooks present (`PreToolUse`, `PostToolUse`, `Stop`, `Notification`) but is missing `UserPromptSubmit`, surface a one-line hint at the end of install output: `note: detected pre-Story-5.2 hooks; re-running install to subscribe UserPromptSubmit`. This is informational, not a blocker — the merge step in `merge_install_into` (lines 197–235) will add the new entry idempotently. The hint just makes the upgrade legible to operators who re-run after pulling Story 5.2.
  - [x] Implementation hint: track which of the legacy-4 hooks were present pre-merge (look at `array_contains_bowerbird` results before the loop adds), and if all four were present but the fifth was added, set a flag on `InstallOutcome`. Surface it from the CLI's `install` command.
  - [x] **Out of scope:** standalone detection-without-install. AC #4 only requires the hint emerge from a re-run of `bowerbird install` — not a separate `bowerbird doctor` mode. Don't build the latter.

- [x] **Task 6: Update the events-table round-trip test** (AC: #6, #7)
  - [x] Edit `crates/daemon/src/db/queries.rs::event_kind_db_string_round_trip_all_variants` (lines 191–205). Add `EventKind::UserPromptSubmit` to the iteration array. Without this, the round-trip test silently skips the new variant and a future serde-derive misconfiguration on it would be invisible.

- [x] **Task 7: Update the daemon contract tests** (AC: #1, #2, #6, #8)
  - [x] Edit `crates/daemon/tests/contract_daemon.rs::state_machine_full_sequence_determinism` (lines 1241–1275). The `cases` array at line 1246 currently encodes the OLD transition table — line 1248 asserts `PostToolUse → Idle`. Rewrite the array to encode the new table:
    - `(PreToolUse, Working)`, `(PostToolUse, Working)` — PostToolUse preserves Working
    - `(Stop, Idle)` — Stop is the canonical "agent done" transition
    - `(UserPromptSubmit, Working)`, `(PreToolUse, Working)`, `(Notification, WaitingInput)`, `(PreToolUse, Working)`, `(PostToolUse, Working)`, `(Stop, Idle)` — a realistic full turn
  - [x] Audit every other `PostToolUse → Idle` assertion in `contract_daemon.rs` (grep results: lines 1248, 1771, 1805, plus many in the state-projection rebuild / snapshot / fanout tests that USE PostToolUse as a way to land in Idle). For each, decide:
    - **Tests asserting "session ends at Idle":** insert a `Stop` after the `PostToolUse` so the final state is correct under the new rules. Example: line 1771 ("sess-b: PostToolUse → stored Idle") becomes "sess-b: PostToolUse + Stop → stored Idle". Many of these tests are testing rebuild / snapshot / atomicity, not the state-machine rule itself — they just need a Stop appended.
    - **Tests asserting the specific PostToolUse-causes-Idle rule:** these are now testing the OLD rule. Either delete or rewrite to test the NEW rule (PostToolUse preserves prev).
  - [x] Add a NEW contract test `state_broadcast_only_on_transition` that:
    1. Spawns a hermetic daemon with one WS subscriber on `state.session.*`.
    2. Ingests one `PreToolUse` (expects one `state` frame: `Idle→Working`).
    3. Ingests N back-to-back `PostToolUse`/`PreToolUse` pairs (expects ZERO additional `state` frames — `current_state` stays `Working` the whole time).
    4. Ingests one `Stop` (expects one `state` frame: `Working→Idle`).
    5. Asserts the event-frame count is `1 + 2N + 1` exactly.
  - [x] Add a NEW contract test `user_prompt_submit_drives_working_transition` that:
    1. Ingests one `Stop` (puts session at `Idle`).
    2. Ingests one `UserPromptSubmit` (expects one `state` frame: `Idle→Working`).
    3. Ingests one `PreToolUse` (expects ZERO additional state frames — already `Working`).
    4. Asserts the final stored `last_event_kind` is `PreToolUse` (not `UserPromptSubmit`) — `last_event_kind` always reflects the most recent event regardless of transition gating.
  - [x] Add a NEW contract test `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` that constructs a JSON string `{"event_id":1,"source":"claude","session_id":"x","kind":"UserPromptSubmit","reaction":null,"payload":"{}","created_at":0}`, deserializes it through a *separate* mock-presenter copy of the `EventKind` enum that ONLY has the legacy 7 variants + `Unknown` + `#[serde(other)]`, and asserts the result is `Unknown`. This is the forward-compat contract for AC #5 — Story 4.4's `#[serde(other)]` catch-all should already handle this; the test is a regression guard.

- [x] **Task 8: Update the protocol crate contract tests** (AC: #5, #7)
  - [x] Edit `crates/protocol/tests/contract_protocol.rs`. Add a `UserPromptSubmit` round-trip assertion alongside the existing PostToolUse round-trip at line 13-14. Add a round-trip assertion that the literal string `"UserPromptSubmit"` deserializes to `EventKind::UserPromptSubmit` AND a separate one that confirms a new mock enum-without-UserPromptSubmit decodes the same string as `Unknown` (this can be expressed in the protocol crate test via a local `#[derive]` enum, or moved to Task 7's daemon contract — pick whichever crate makes the dependency cleaner).
  - [x] The existing additive-compat round-trip test (the "outbound envelope with extra unknown field round-trips" canary from project-context.md line 594 — find it and confirm it still passes verbatim).

- [x] **Task 9: Update the adapter-claude install contract tests** (AC: #4, #7)
  - [x] Edit `crates/adapter-claude/tests/contract_install.rs`. The three `for kind in ["PreToolUse", "PostToolUse", "Stop", "Notification"]` loops (lines 35, 132, 254) need to become `for kind in ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "Notification"]`. Each loop is asserting different things — install idempotence, uninstall cleanup, the per-kind hook entry shape. All five hook kinds should pass the same assertions; the existing loop body should work unchanged.
  - [x] If a test pins the binary-name format `bowerbird-shim --hook-kind <KIND>` (see Story 3.4 changelog line 40), verify it still passes for `UserPromptSubmit` — no escaping or quoting edge cases.

- [x] **Task 10: Renumber the docs/protocol.md cross-reference and confirm pre-staging untouched** (AC: #9)
  - [x] Edit `docs/protocol.md:334`. Change `(Story 1.8, extended in Story 5.7)` → `(Story 1.8, extended in Story 5.2)`. This is the only remaining `Story 5.7` reference in `docs/protocol.md`; the resequencing proposal §4.1 explicitly calls it out.
  - [x] **Do NOT touch** the protocol-changelog Story 5.2 entries (lines 44–45) — they're already correct (already renumbered from 5.7 → 5.2 in the resequencing commit).
  - [x] **Do NOT touch** `architecture.md:50–51` or `:1027` or `prd.md:206` — these are pre-staged and correct.
  - [x] After this edit, run `grep -rn 'Story 5\.7' docs/` and confirm the only remaining hits are inside the historical sprint-change-proposal docs (which have their own disambiguation notes already) — no production-doc references.

- [x] **Task 11: Satisfy the protocol-changelog CI gate** (AC: #10)
  - [x] The gate (`tests/protocol_changelog_gate.rs`) requires this PR's diff against `origin/main` to include at least one `+`-prefixed line containing `type: schema`, `type: behavioral`, or `type: security` in `docs/protocol-changelog.md`. Since the two Story 5.2 entries already exist in main, an untouched changelog file FAILS the gate.
  - [x] **Easiest fix:** refine the existing Story 5.2 entries with one or two implementation-time clarifications (e.g., add a sentence about the legacy-hook detection rule from Task 5, or about the `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` contract test path). The `+` line from refinement counts.
  - [x] **Alternative:** add a third, smaller `type: behavioral` entry documenting an aspect not yet in the changelog — e.g., the install command's "re-run hint" detection rule.
  - [x] Verify by running `cargo test --workspace -- --test-threads=1 protocol_src_changes_require_changelog_entry` locally with `BOWERBIRD_CHANGELOG_GATE_BASE=origin/main` set (or in a PR where GitHub Actions sets `GITHUB_BASE_REF`). The test SKIPs cleanly on detached checkouts — if you see SKIPPED locally that's fine; CI will exercise it on the PR.

- [x] **Task 12: Run the full workspace test suite serialized** (AC: all)
  - [x] `cargo test --workspace -- --test-threads=1`. Serialized execution is required per Epic 2 retro AI-3 (workspace tests share process-wide state: subprocesses, signal handlers, `BOWERBIRD_DATA_DIR`, `BOWERBIRD_KEYRING_BACKEND`).
  - [x] `cargo fmt --check` — workspace-wide.
  - [x] `cargo clippy --all-targets --workspace -- -D warnings` — workspace-wide. Warnings are errors per project-context.md §Crate-wide invariants.
  - [x] If any contract test fails for a reason beyond what Task 7 specifically addressed, audit it: it's likely another `PostToolUse → Idle` assertion that was implicit in an unrelated test fixture (rebuild paths, snapshot dedup, etc.).

- [x] **Task 13: Manual smoke against the running daemon** (AC: #1, #2, #3, #4)
  - [x] After all tests pass, run `cargo install --path . --force` (or build a release binary) and re-install: `bowerbird uninstall && bowerbird install`. Confirm `~/.claude/settings.json` has all five hook entries; confirm `bowerbird uninstall` removes all five.
  - [x] Start a Claude Code session against the rebuilt daemon. Subscribe a presenter (e.g., the `bowerbird-deck` from Story 5.1 if it's running locally) to `state.session.*`. Trigger a series of tool calls and confirm the ribbon does NOT flap — `current_state` should hold `Working` from the user prompt through the entire turn, then transition once on `Stop`.
  - [x] **Out of scope:** this smoke is informal — it's how the maintainer regains confidence in the daemon's wire output. The contract tests are the actual gates.

- [x] **Task 14: Update `sprint-status.yaml`** (AC: implicit)
  - [x] When the story file moves to `in-progress`: `5-2-session-state-projection-correctness: in-progress`. Add a `last_updated` line.
  - [x] When the story moves to `review` (post `dev-story`): `5-2-session-state-projection-correctness: review`. Add a `last_updated` line.
  - [x] When the story moves to `done` (post `code-review`): `5-2-session-state-projection-correctness: done`. Add a `last_updated` line.

### Review Findings

- [x] [Review][Patch] UserPromptSubmit hook is installed but rejected by the shim [crates/shim/src/main.rs:88]

  What breaks: `bowerbird install` now writes a Claude Code hook command `bowerbird-shim --hook-kind UserPromptSubmit`, but the shim exits before sending that payload to the daemon. That means AC #3 is not true in a real installed Claude session: the `UserPromptSubmit` hook does not fire through bowerbird, the daemon never ingests the event, and the session stays `Idle` until the first later hook that the shim accepts.

  Evidence: `crates/adapter-claude/src/install.rs::HOOK_KINDS` includes `"UserPromptSubmit"`, and `docs/protocol.md` declares `UserPromptSubmit` as a valid ingest `hook_kind`. `crates/shim/src/main.rs::parse_hook_kind` still matches only `"PreToolUse" | "PostToolUse" | "Stop" | "Notification"`. Reproduced locally with:

  ```sh
  printf '{"session_id":"s1"}' | BOWERBIRD_SHIM_LOG=/private/tmp/bowerbird-shim-userprompt.log target/debug/bowerbird-shim --hook-kind UserPromptSubmit
  ```

  Actual result: exit code `1`, log line `invalid hook-kind: UserPromptSubmit`.

  Fix direction: add `"UserPromptSubmit"` to `parse_hook_kind`. Add a shim contract test that runs the shim with `--hook-kind UserPromptSubmit`, captures the mock ingest payload, and asserts the injected `"hook_kind":"UserPromptSubmit"` survives. Add or extend a daemon ingest round-trip test so a `UserPromptSubmit` payload reaches the ingest channel as `EventKind::UserPromptSubmit`.

- [x] [Review][Patch] State publish gating ignores the read-time stale-Working fallback [crates/daemon/src/projection/session.rs:127]

  What breaks: a state-only subscriber can observe a session as `Idle` via snapshot or REST because `current_state_for_read` turns a stale stored `Working` row into read-facing `Idle`. When new activity resumes for that session, `projection::session::write` compares stored `prev_state.current_state` (`Working`) to the new stored state (`Working`) and suppresses the live `State` frame. The state-only subscriber remains stuck on `Idle` even though the agent resumed work.

  Concrete path:

  1. A row is stored as `current_state=Working`, `last_event_at_ms` older than `STALE_WORKING_MS`.
  2. A presenter subscribes to `state.session.*`.
  3. Snapshot generation applies `current_state_for_read` and sends `Idle`.
  4. A new `UserPromptSubmit`, `PreToolUse`, or `PostToolUse` event arrives.
  5. `transition()` stores `Working`.
  6. `write()` compares stored `Working` to new `Working`, decides `state_changed == false`, and emits only the `Event` envelope.

  Fix direction: base the publish-gating comparison on the previous read-facing state, not only the previous stored state. For example, compute `prev_read_current_state = prev_state.as_ref().map(|s| current_state_for_read(s, now_ms))` inside the transaction before calling `transition()`, return that value from the closure, and compare it to `new_state.current_state` at the publish site. This preserves normal transition-only behavior while emitting `Idle -> Working` when the stale fallback was what subscribers could previously see.

  Regression coverage: add a contract test that seeds or writes a stale stored `Working` projection, subscribes to `state.session.*` and confirms the snapshot says `Idle`, then writes `UserPromptSubmit` or `PreToolUse` and asserts a live `State` frame arrives with `current_state=Working`. Also assert the `Event` frame still publishes.

- [x] [Review][Patch] Legacy reinstall hint text differs from the story-specified message [src/commands/install.rs:43]

  What breaks: the behavior is probably understandable to a human, but the implementation does not match the story's operator-facing text. Task 5 specifies the exact note: `note: detected pre-Story-5.2 hooks; re-running install to subscribe UserPromptSubmit`. The CLI currently prints `note: detected pre-Story-5.2 hooks; re-running install subscribed UserPromptSubmit`.

  Fix direction: change the string in `src/commands/install.rs` to match Task 5 exactly unless there is a deliberate product-copy reason to change the story. Add coverage that builds a pre-5.2 four-hook settings file, runs the CLI install path, and asserts stdout contains the exact hint. The lower-level `InstallOutcome.legacy_upgrade_detected` flag should also have unit coverage for three cases: legacy four hooks only -> true, fresh install -> false, already-upgraded five hooks -> false.

- [x] [Review][Patch] Forward-compat regression test does not exercise the full event shape required by Task 7 [crates/daemon/tests/contract_daemon.rs:1470]

  What is missing: AC #5 says a v1.0 presenter receives an event with `kind: "UserPromptSubmit"` and decodes that kind as `Unknown` without crashing. Task 7 asks for a full JSON event object. The implemented test deserializes only the bare JSON string `"UserPromptSubmit"` into a legacy enum, so it does not prove a legacy presenter can parse the actual event payload shape.

  Fix direction: replace or extend `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` with a local legacy event struct, for example:

  ```rust
  #[derive(Deserialize)]
  struct LegacyEvent {
      event_id: i64,
      source: String,
      session_id: String,
      kind: LegacyEventKind,
      reaction: Option<serde_json::Value>,
      payload: String,
      created_at: i64,
  }
  ```

  Deserialize a JSON object shaped like the story text: `{"event_id":1,"source":"claude","session_id":"x","kind":"UserPromptSubmit","reaction":null,"payload":"{}","created_at":0}`. Assert `event.kind == LegacyEventKind::Unknown` and the rest of the fields parse. If the wire frame path is easy to express, a `ServerMessage::Event`-like mock wrapper would be even closer to the presenter path.

- [x] [Review][Patch] UserPromptSubmit normalization lacks a direct adapter contract test [crates/adapter-claude/src/normalize.rs:68]

  What is missing: `adapter-claude` now maps the string `"UserPromptSubmit"` to `EventKind::UserPromptSubmit`, but no adapter contract test exercises that new match arm. Existing daemon-level tests construct `EventEnvelope` values directly, so they do not prove the adapter accepts the real hook string and preserves the native payload.

  Fix direction: add a test in `crates/adapter-claude/tests/contract_adapter.rs` with a representative JSON payload such as `{"session_id":"sess-ups","prompt":"hello"}`. Call `adapter.normalize("UserPromptSubmit", payload.as_bytes())`. Assert:

  - `envelope.source == "claude"`
  - `envelope.session_id == "sess-ups"`
  - `envelope.kind == EventKind::UserPromptSubmit`
  - `envelope.reaction == None`
  - `envelope.payload` still contains the original fields verbatim

  This test is intentionally adapter-scoped; it should fail if a future edit removes the match arm or accidentally treats `UserPromptSubmit` like `PreToolUse` and requires `tool_name`.

- [x] [Review][Patch] Story File List omits files changed by this review scope [docs/bmad/implementation-artifacts/5-2-session-state-projection-correctness.md]

  What is wrong: the branch diff includes planning/documentation files that the story File List does not list. That makes the Dev Agent Record incomplete and weakens later review/audit work.

  Add these files to the File List with short descriptions:

  - `docs/bmad/planning-artifacts/epics.md` — Epic 5 resequencing and Story 5.2/5.3 insertion updates.
  - `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27.md` — disambiguation note for the old Story 5.7 numbering.
  - `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-epic-5-resequencing.md` — added resequencing proposal.
  - `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-pid-liveness.md` — added PID liveness proposal that became the resequenced Story 5.3.

  Also re-check the story's "Files explicitly NOT updated" section. It should not imply `epics.md` or the sprint-change proposal docs were untouched when they are part of the branch diff.

## Dev Notes

### Why this story exists (the user-visible flap)

The pickletown `/sessions` livestream presenter — and now the Story 5.1 `bowerbird-deck` TUI — both surfaced the same defect within their first observed Claude Code turn: the ribbon card flapped `Working → Idle → Working → Idle` on every `PreToolUse`/`PostToolUse` pair. Internally the daemon was honest (`last_event_at_ms` correctly updated on every event), but the state-machine rule `PostToolUse → Idle` plus the unconditional broadcast on every projection write conspired to produce visible noise that obscured the only signal a presenter cares about: "is the agent actually working right now?"

Two coupled defects (per `sprint-change-proposal-2026-05-27.md`):

- **Defect A — over-broadcasting:** `projection::session::write` publishes a `State` envelope on every event, regardless of whether `current_state` actually changed. The intent in `docs/protocol.md:316` ("every session's state **changes**") and the wording of Story 2.2 both implied transitions-only emission. The implementation didn't honor it.
- **Defect B — wrong PostToolUse semantics:** `state.rs:36` flips `current_state` to `Idle` on every `PostToolUse`. The agent is alive between tool calls — composing the next call, thinking — but the state machine reads `Idle` during that gap. Additionally, `UserPromptSubmit` was never subscribed at all, so the window between "user submits prompt" and "first PreToolUse" also reads `Idle` (or `WaitingInput` if a `Notification` recently fired).

Together these defects make the substrate's headline signal — `state.session.*.current_state` — unusable for any ribbon UI that's supposed to surface "what is Claude doing right now."

### The fix is small but surgical — three files plus tests

- **state.rs `transition()`:** PostToolUse arm preserves prev; UserPromptSubmit arm returns Working. The 5-min `STALE_WORKING_MS` backstop (Story 1.6) already exists and is the right safety net for both a dropped `PostToolUse` and a dropped `Stop` — no change to the fallback. The defense-in-depth `RecordingStarted | RecordingEnded | Unknown` arm is unchanged.
- **session.rs `write()`:** diff prev_state vs new_state on `current_state`, gate the `State` publish. Event publish is unchanged.
- **install.rs / normalize.rs:** add the `UserPromptSubmit` hook string to the lists.

Everything else is contract test updates and one docs cross-reference renumber.

### The pre-staging is a blessing and a landmine

Commit 78c26b3 ("docs(epic-5): add Story 5.7 session state projection correctness via course-correction") pre-staged the entire docs surface (protocol.md, protocol-changelog.md, prd.md, architecture.md, epics.md) before the code. This is unusual for the project — most stories ship code and docs in the same PR. The reason it was done this way: the sprint-change-proposal-2026-05-27 review approved the change in detail (right down to the changelog entry wording) and the resequencing proposal needed those documents to point at "Story 5.2" rather than the now-stale "Story 5.7" reference.

**The blessing:** you don't have to write the changelog entries or the protocol.md updates from scratch. They're already there, already correct, already aligned with Story 5.2's numbering. Read them as your spec.

**The landmine:** the protocol-changelog gate (`tests/protocol_changelog_gate.rs`) compares the PR diff against `origin/main`. The two Story 5.2 changelog entries are already in main. An untouched changelog file in this PR means *no `+` line containing `type:` exists in the gate's view*, so it FAILS. Task 11 names the two ways to handle this — easiest is refining the existing entries with one implementation-time clarification. The gate is a compiled test, not a shell script, so the error message will be unambiguous when it fires.

### Changelog gate is the silent landmine — read me

To restate Task 11's mechanics so the dev agent doesn't lose CI on a "but the entry's already there" reflex:

The gate logic (verified by reading `tests/protocol_changelog_gate.rs:159–169`):
1. Resolve base ref (`BOWERBIRD_CHANGELOG_GATE_BASE` → `origin/$GITHUB_BASE_REF` → `origin/main`).
2. List paths changed in `<base>...HEAD` via `git diff --name-only`.
3. If any path matches `crates/protocol/src/*.rs`, trigger.
4. List added lines in `docs/protocol-changelog.md` via `git diff -U0 <base>...HEAD -- docs/protocol-changelog.md`, filter for `+` prefix.
5. Assert at least one such added line contains a `type:` header from the allowed set.

Since `event.rs` is changing in this PR, the gate fires. Since the changelog entries are already in main, an untouched changelog file produces zero `+`-prefixed `type:` lines. The gate fails.

**Resolution path** (in order of cleanliness):
1. **Refine the pre-staged entries.** Add an implementation-time sentence to one of them — say, naming the new contract test (`state_broadcast_only_on_transition`) or the legacy-hook detection rule (Task 5). The replaced line shows as a `+` line in `-U0` diff and trips the gate. This is the path of least drama.
2. **Add a small third entry.** Something the pre-staging didn't anticipate — e.g., the `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` contract test (which is a regression-guard for an existing capability, not a wire change, but the test's existence is a release-relevant fact).
3. **DO NOT delete the pre-staged entries** and re-add them in this PR — git would surface them as both `-` and `+` lines, which mathematically satisfies the gate, but it makes the changelog history nonsensical for anyone reading `git log -- docs/protocol-changelog.md`.

### Watch the `prev_state` move in `projection::session::write`

The current code at `crates/daemon/src/projection/session.rs:91–117` reads `prev_state` inside the `interact` closure and moves it through the `.optional()?.and_then(...)` chain into the `transition()` call at line 119. The result is `prev_state` is consumed by the time `new_state` exists.

For Task 3 you need BOTH `prev_state.map(|s| s.current_state)` AND `new_state.current_state` accessible at the post-commit publish site (lines 154 onward). The closure currently returns `(i64, SessionState)` — extend it to `(i64, SessionState, Option<SessionCurrentState>)` where the third element is `prev_state.as_ref().map(|s| s.current_state)`, captured before `transition` is called.

Then at the call site:

```rust
let (event_id_raw, new_state, prev_current_state) = interact_res?;
// ... build Event and publish unconditionally ...
broadcaster.publish(BroadcastEnvelope::Event(event));
if prev_current_state != Some(new_state.current_state) {
    broadcaster.publish(BroadcastEnvelope::State {
        source,
        session_id,
        state: new_state,
    });
}
```

First-event semantics fall out for free: `prev_current_state` is `None` when there's no prior row; `None != Some(...)` is true; the `State` envelope publishes. This is the correct behavior — a session's existence transition (`absent → Working`) is meaningful to subscribers.

### What "preserve prev" means in the PostToolUse arm

The current state.rs `PostToolUse` arm constructs a *new* `SessionState` with `current_state: Idle`. The new arm needs to construct a new `SessionState` whose `current_state` is `prev.current_state` (carry-forward) but whose `last_event_kind` and `last_event_at_ms` are the new event's values. This is NOT the same as the defense-in-depth `RecordingStarted | RecordingEnded | Unknown` arm (lines 50–56), which `return prev.cloned()` *unchanged* — including the old `last_event_kind`. For `PostToolUse`, the row gets updated (so subscribers reading via REST see the latest `last_event_kind` and the freshness timestamp); only the `current_state` is preserved.

Shape:

```rust
EventKind::PostToolUse => SessionState {
    current_state: prev.map(|s| s.current_state).unwrap_or(SessionCurrentState::Working),
    last_event_kind: event_kind,
    last_event_at_ms: now_ms,
},
```

The `unwrap_or(Working)` covers the degenerate "PostToolUse without prior state" case. This shouldn't happen in practice (a Claude Code session always emits `PreToolUse` before `PostToolUse` for a given tool call), but if it does, `Working` is the right answer — the agent was clearly active a moment ago.

### Ordering of EventKind variants in event.rs matters for two things

1. **Serde wire format:** the wire string IS the variant identifier (no `rename_all`). Variant ordering does NOT affect serialization. The new variant's wire string is `"UserPromptSubmit"` regardless of where it appears in the enum.
2. **Readability and `#[serde(other)]`:** `Unknown` MUST remain the last variant with `#[serde(other)]` — that's the Story 4.4 catch-all contract. The other variants can be in any order, but project convention is chronological lifecycle ordering: `UserPromptSubmit → PreToolUse → PostToolUse → Stop → Notification → RecordingStarted → RecordingEnded → Unknown`. Place the new variant accordingly.

### Why the broadcast-hub MIN_CAPACITY = 2 matters here

`crates/daemon/src/broadcast/hub.rs:29` floors `ws_broadcast_capacity` at 2 explicitly because Story 2.2 publishes Event-then-State as a pair, and a capacity of 1 would let State evict Event. Story 5.2 changes the publish pattern: writes that DON'T change `current_state` publish only Event (no State follow-up). For these writes, capacity 1 would technically be fine — but the MIN_CAPACITY floor stays at 2 because:

1. Some writes DO publish both (transition-causing writes), and the floor must accommodate them.
2. The contract is documented at the broadcast-hub level, not the publish-call-site level — keeping it consistent simplifies future readers.

You should NOT lower MIN_CAPACITY as part of this story. The capacity floor is independent of the publish gating.

### The dev session is shorter than it looks

The fix surface is small enough that an experienced agent can produce a passing PR in a half-day. The trap is the contract tests — there are a LOT of `PostToolUse → Idle` assertions in `contract_daemon.rs` (over 30 grep hits). Most are not testing the rule itself; they're testing rebuild, snapshot, atomicity, etc., and happen to use `PostToolUse` as a convenient way to land a session in `Idle`. Those need a `Stop` appended to keep working. Plan to spend more time on Task 7 than on Tasks 1–4 combined.

### Friction discovered during this work goes back into Story 5.1

Story 5.1 is the dogfooding loop; any substrate awkwardness you hit while implementing Story 5.2 (especially around the contract test landscape, the changelog gate, or the deferred-work backlog) gets captured per Story 5.1's AC #4 split. Don't quietly work around — that's the whole point of Epic 5's dogfooding-validation-phase.

### Project Structure Notes

- `crates/protocol/src/event.rs` — UPDATE — add `UserPromptSubmit` variant; current state has 7 variants + Unknown.
- `crates/daemon/src/projection/state.rs` — UPDATE — `transition()` PostToolUse + new UserPromptSubmit arm; STALE_WORKING_MS unchanged.
- `crates/daemon/src/projection/session.rs` — UPDATE — `write()` returns `prev_current_state` from closure; gate `State` publish on transition.
- `crates/daemon/src/db/queries.rs` — UPDATE — add `EventKind::UserPromptSubmit` to round-trip test iteration.
- `crates/adapter-claude/src/install.rs` — UPDATE — `HOOK_KINDS` array gains one entry; `InstallOutcome` may gain a "legacy upgrade detected" flag.
- `crates/adapter-claude/src/normalize.rs` — UPDATE — `match hook_kind` block gains one arm.
- `crates/protocol/tests/contract_protocol.rs` — UPDATE — add UserPromptSubmit round-trip + forward-compat assertions.
- `crates/daemon/tests/contract_daemon.rs` — UPDATE — rewrite `state_machine_full_sequence_determinism`; audit and update PostToolUse assertions; add two new contract tests.
- `crates/adapter-claude/tests/contract_install.rs` — UPDATE — three `HOOK_KINDS`-iterating loops gain `UserPromptSubmit`.
- `docs/protocol.md` — UPDATE — line 334 cross-reference renumber `5.7 → 5.2`.
- `docs/protocol-changelog.md` — UPDATE — refine one of the two pre-staged entries to satisfy the gate (Task 11).
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — UPDATE — status transitions (Task 14).

**Files explicitly NOT updated** (pre-staged + correct):
- `docs/bmad/planning-artifacts/architecture.md:50–51, :1027` — pre-staged.
- `docs/bmad/planning-artifacts/prd.md:206` — pre-staged.
- `docs/bmad/planning-artifacts/epics.md` Story 5.2 section — pre-staged (this is your spec).
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27.md` — historical; has the disambiguation note.

### Testing Standards

Per project-context.md §"Required contract tests" (lines 580–602):

- This story relates to two existing entries in that table:
  - "Hook unreliability tolerance: Fire `PreToolUse` without a matching `PostToolUse`; assert projection still reaches a sane state (not stuck in `working`)" — the 5-min STALE_WORKING_MS fallback covers this; no change needed.
  - "State-emission and event-INSERT atomicity: SIGKILL the daemon mid-load; on restart, assert projection rows and event-log rows agree. No half-state." — unchanged; the new publish-gating logic is post-commit, doesn't touch the transaction.
- Adds two new contract tests (Task 7): `state_broadcast_only_on_transition` and `user_prompt_submit_drives_working_transition`. These are new entries — should they be added to the project-context.md "Required contract tests" table? **Yes, but defer to Story 5.5 (Bench gates) or a follow-up doc pass.** This story already touches enough planning artifacts; adding two rows to the contract-test table can ride along with 5.5's "load-bearing" sweep or with the epic-5 retro.
- Forward-compat (AC #5, Task 7's third new test): not in the existing contract-tests table; the closest existing canary is "Outbound envelope additive-compat" (project-context.md line 594). The new `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` test is a focused regression-guard on that same Story 4.4 capability — it doesn't replace the canary, it complements it.
- Deterministic test discipline (project-context.md lines 642–646): NO `sleep()` for synchronization in the new contract tests. Use `tokio::test(start_paused = true)` + `tokio::time::advance` if you need timing; otherwise rely on the broadcast hub's deterministic ordering. The existing `state_machine_full_sequence_determinism` test is the right shape — copy its pattern.

### References

- `docs/bmad/planning-artifacts/epics.md:930-974` — Story 5.2 epic definition.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27.md` — full defect rationale and remediation design (note: text references "Story 5.7" throughout; that's the pre-resequencing number, currently 5.2 — there's a disambiguation note at the top of the proposal).
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-epic-5-resequencing.md` — the resequencing rationale (why this story is now 5.2 not 5.7).
- `docs/protocol-changelog.md` lines 44–45 — the two pre-staged Story 5.2 entries (behavioral + schema); read as spec.
- `docs/protocol.md:260-353` — pre-staged sections on event/state envelopes, ingest contract, EventKind table.
- `docs/bmad/planning-artifacts/architecture.md:45-59, :1020-1031` — pre-staged FR coverage table.
- `docs/bmad/project-context.md` §Project axioms (lines 40-59) — Axiom 1 (substrate observes; presenter interprets) and Axiom 4 (mechanical facts in the protocol; semantics in the presenter). The fix here is fundamentally about Axiom 4: state-transition diffing is a mechanical fact the daemon already knows; broadcasting on every event-write was the daemon stealthily encoding "you should react" semantics. Read the axioms before deciding any close call.
- `docs/bmad/project-context.md` §"Required contract tests" (lines 580-602) — the table this story adds two implicit entries to.
- `docs/bmad/implementation-artifacts/1-6-session-projection-and-hook-unreliability-tolerance.md` — Story 1.6 (the existing state machine). The 5-min STALE_WORKING_MS fallback was introduced here.
- `docs/bmad/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md` — Story 2.2 (the existing broadcast wiring). The Event-then-State pair pattern was introduced here.
- `docs/bmad/implementation-artifacts/2-3-new-session-discovery-and-state-snapshot-on-connect.md` — Story 2.3 (snapshot-on-subscribe). Unchanged by this story; the post-Story-5.2 doc says "snapshot frames precede live frames on the same connection; subsequent live frames continue without gap" — that still works.
- `docs/bmad/implementation-artifacts/4-4-protocol-compatibility-guarantee-and-contract-test-suite.md` — Story 4.4 (the `#[serde(other)] Unknown` catch-all that makes AC #5 possible).
- `tests/protocol_changelog_gate.rs` — the gate's full source; read before deciding Task 11's approach.
- `crates/protocol/src/event.rs` — `EventKind` enum (currently 7 variants + Unknown).
- `crates/daemon/src/projection/state.rs` — `transition()` (the function being modified).
- `crates/daemon/src/projection/session.rs` — `write()` (the function being modified).
- `crates/daemon/src/broadcast/hub.rs` — `BroadcastHub` (unchanged, but read the MIN_CAPACITY rationale).
- `crates/adapter-claude/src/install.rs` — `HOOK_KINDS` array.
- `crates/adapter-claude/src/normalize.rs` — `match hook_kind` block.
- `crates/daemon/src/db/queries.rs:127-205` — `event_kind_as_str` / `event_kind_from_db_str` (the SQLite round-trip; the test needs the new variant).
- `crates/daemon/src/ingest/handler.rs:70-85` — the `400 missing hook_kind` / `400 unknown hook_kind` error paths (will accept `UserPromptSubmit` automatically via `adapter-claude/src/normalize.rs::normalize`, no change needed).
- `crates/daemon/tests/contract_daemon.rs:1241-1275` — `state_machine_full_sequence_determinism` (the test you'll rewrite).
- `crates/adapter-claude/tests/contract_install.rs:35, :132, :254` — the three loops that need a fifth hook kind.

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context)

### Debug Log References

- Workspace tests run serialized via `cargo test --workspace -- --test-threads=1`; all 28 test executables pass.
- `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` both green.
- Protocol-changelog gate (`tests/protocol_changelog_gate.rs`) verified locally with `BOWERBIRD_CHANGELOG_GATE_BASE=origin/main` set.

### Completion Notes List

- **AC #1 (PostToolUse preserves Working, no state envelope):** state machine in `crates/daemon/src/projection/state.rs::transition` now maps `EventKind::PostToolUse` to `prev.current_state` (defaulting to `Working` when no prior row exists). Broadcast publish in `crates/daemon/src/projection/session.rs::write` gates `BroadcastEnvelope::State` on `prev_current_state != Some(new_state.current_state)`. Pinned by new contract test `state_broadcast_only_on_transition`.
- **AC #2 (N back-to-back pairs → 1 state envelope):** Same gating logic. The new `state_broadcast_only_on_transition` contract test explicitly exercises the 1 + 2N + 1 envelope cadence with N=3.
- **AC #3 (UserPromptSubmit ingest + Working transition):** `EventKind::UserPromptSubmit` added to protocol enum; `adapter-claude/src/normalize.rs` maps hook string to the variant; `state.rs::transition` returns `Working`. Pinned by new contract test `user_prompt_submit_drives_working_transition`.
- **AC #4 (5 hooks + legacy hint):** `HOOK_KINDS` in `crates/adapter-claude/src/install.rs` now contains five entries in lifecycle order. New `InstallOutcome.legacy_upgrade_detected` flag fires when pre-merge settings.json has all four legacy entries but no `UserPromptSubmit`; the `bowerbird install` CLI surfaces a one-line hint. Verified by extended `contract_install.rs` loops and the `install_writes_shim_command_with_hook_kind_for_each_known_kind` test.
- **AC #5 (forward-compat for legacy presenters):** New regression test `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` in `contract_daemon.rs` constructs a mock pre-5.2 enum and asserts the Story 4.4 `#[serde(other)]` catch-all maps `"UserPromptSubmit"` to `Unknown`.
- **AC #6 (full transition table):** `state_machine_full_sequence_determinism` rewritten to encode the new table (PostToolUse preserves prev, UserPromptSubmit → Working, Stop is canonical Idle trigger). `STALE_WORKING_MS` unchanged; doc comment updated to clarify it now backstops dropped `Stop` (the new Working→Idle transition trigger).
- **AC #7 (protocol surface):** `crates/protocol/src/event.rs` gains `UserPromptSubmit` before `PreToolUse` (lifecycle order); `Unknown` stays last with `#[serde(other)]`.
- **AC #8 (contract-test surface):** `contract_protocol.rs` adds round-trip and PascalCase assertions for `UserPromptSubmit`. `contract_daemon.rs` rewrites `state_machine_full_sequence_determinism` and adds three new tests. `contract_install.rs` extends three loops to all five hook kinds. The `event_kind_db_string_round_trip_all_variants` lib test in `db/queries.rs` adds `UserPromptSubmit` to the iteration.
- **AC #9 (cross-reference renumber):** `docs/protocol.md:334` updated from "Story 1.8, extended in Story 5.7" → "Story 1.8, extended in Story 5.2". No production-doc references to Story 5.7 remain; all surviving hits are inside historical sprint-change-proposal docs (with disambiguation notes already at the top).
- **AC #10 (changelog gate):** Refined the pre-staged "type: behavioral" Story 5.2 entry with two implementation-time clarifications (first-event semantics + `state_broadcast_only_on_transition` test reference). Gate test passes against `origin/main`.
- **Audit sweep:** Eight tests outside the explicit AC-targeted ones depended on the pre-5.2 `PostToolUse → Idle` rule or on state-envelope-per-event behavior. Each was updated to either send `Stop` (the new Idle trigger) or to assert the new transition-gated envelope count. Documented in code-side comments tagged "Story 5.2".

### File List

**Source files (7):**
- `crates/protocol/src/event.rs` — added `UserPromptSubmit` variant before `PreToolUse`.
- `crates/daemon/src/projection/state.rs` — `PostToolUse` arm preserves prev; new `UserPromptSubmit` arm returns `Working`; `STALE_WORKING_MS` doc comment updated; four new unit tests.
- `crates/daemon/src/projection/session.rs` — closure return type extended to `(i64, SessionState, Option<SessionCurrentState>)`; `BroadcastEnvelope::State` publish gated on the READ-FACING prev `current_state` (via `current_state_for_read`) so stale-Working recovery still emits a transition; doc comment updated to reflect zero-or-one State envelope semantics. (Read-facing gating added in review-fix pass per review finding #2.)
- `crates/daemon/src/db/queries.rs` — `event_kind_db_string_round_trip_all_variants` now includes `UserPromptSubmit`.
- `crates/shim/src/main.rs` — `parse_hook_kind` accepts the new `UserPromptSubmit` value (review finding #1; without this, the installed hook command fails at the shim CLI boundary).
- `crates/adapter-claude/src/install.rs` — `HOOK_KINDS` adds `UserPromptSubmit` (lifecycle order); new `LEGACY_HOOK_KINDS` constant; `InstallOutcome.legacy_upgrade_detected` field; `settings_has_only_legacy_bowerbird_hooks` detection helper; three new unit tests covering the legacy-upgrade flag (review finding #3).
- `crates/adapter-claude/src/normalize.rs` — `match hook_kind` block gains `UserPromptSubmit` arm before `PreToolUse`.
- `src/commands/install.rs` — surfaces the legacy-upgrade hint when present (exact Task 5 copy: "re-running install to subscribe UserPromptSubmit", review finding #3).

**Test files (5):**
- `crates/protocol/tests/contract_protocol.rs` — `UserPromptSubmit` added to `event_kind_serializes_pascal_case`; new `user_prompt_submit_round_trips` test.
- `crates/daemon/tests/contract_daemon.rs` — rewrote `state_machine_full_sequence_determinism`; audited and updated eight other tests (sess-b list test, snapshot multi-session, state-current-topic-filter, state-wildcard-preserves-session, specific-id-subscription, overlapping-subscriptions, state-only-subscriber-cursor, two replay tests); added four new contract tests (`state_broadcast_only_on_transition`, `user_prompt_submit_drives_working_transition`, `state_broadcast_publishes_when_stale_working_recovers` [review finding #2], `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` — expanded to full event JSON shape per review finding #4).
- `crates/shim/tests/contract_shim.rs` — added `shim_accepts_user_prompt_submit_hook_kind` (review finding #1).
- `crates/adapter-claude/tests/contract_adapter.rs` — added `normalize_user_prompt_submit_round_trip` (review finding #5).
- `crates/adapter-claude/tests/contract_install.rs` — three loops now iterate all five hook kinds.

**Documentation (2 directly edited this session):**
- `docs/protocol.md` — line 334 renumbered Story 5.7 → 5.2.
- `docs/protocol-changelog.md` — refined the pre-staged Story 5.2 behavioral entry with implementation-time clarifications (first-event semantics + new contract-test reference) to satisfy the changelog gate against origin/main.

**Planning artifacts on the branch (carried over from `a616a35` Epic 5 resequencing commit — not edited this dev session but part of the PR diff against origin/main):**
- `docs/bmad/planning-artifacts/epics.md` — Epic 5 resequencing and Story 5.2/5.3 insertions.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27.md` — disambiguation note for the old Story 5.7 numbering.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-epic-5-resequencing.md` — the resequencing proposal itself.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-pid-liveness.md` — PID liveness proposal that became Story 5.3.

**Bookkeeping (1):**
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — story status transitions ready-for-dev → in-progress → review → in-progress (review-receipt) → review with timestamped notes.

**Files explicitly NOT updated by THIS dev session** (pre-staged in `78c26b3` and verified intact):
- `docs/bmad/planning-artifacts/architecture.md:50–51, :1027` — pre-staged.
- `docs/bmad/planning-artifacts/prd.md:206` — pre-staged.

### Change Log

- 2026-05-27: Added `UserPromptSubmit` to protocol `EventKind` (before `PreToolUse`, `Unknown` still last with `#[serde(other)]`).
- 2026-05-27: Updated daemon state machine — `PostToolUse` preserves `prev.current_state`; `UserPromptSubmit` returns `Working`.
- 2026-05-27: Gated `BroadcastEnvelope::State` publish in `projection::session::write` on `current_state` transitions only; first-event semantics covered via `None != Some(new_state.current_state)`.
- 2026-05-27: Wired `UserPromptSubmit` hook into `adapter-claude` install + normalize paths; added legacy-hook detection + CLI hint.
- 2026-05-27: Updated daemon contract tests for new transition table and broadcast gating; added three new contract tests; audited and updated eight collateral tests.
- 2026-05-27: Renumbered `docs/protocol.md:334` cross-reference Story 5.7 → 5.2; refined pre-staged protocol-changelog entry to satisfy gate.
- 2026-05-27: All workspace tests, `cargo fmt --check`, and `cargo clippy --all-targets --workspace -- -D warnings` green.
- 2026-05-27 (review-fix pass): Addressed all six review findings. Shim now accepts `UserPromptSubmit` at the CLI parse boundary (#1 — was a genuine runtime AC #3 break). State publish gating now compares the read-facing prev state via `current_state_for_read` so stale-Working → fresh-Working still emits a State envelope (#2 — added regression test `state_broadcast_publishes_when_stale_working_recovers` which fails on the pre-fix code). Install hint copy matched to Task 5 spec verbatim, plus three unit tests covering the `legacy_upgrade_detected` flag (#3). Forward-compat test expanded to deserialize a full Event-shaped JSON object through a mock legacy presenter (#4). New `normalize_user_prompt_submit_round_trip` adapter contract test (#5). File List broadened to acknowledge planning-artifact docs carried from the resequencing commit on this branch (#6). All tests + fmt + clippy still green.
