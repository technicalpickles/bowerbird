# Story 5.6: `idle_prompt` reclassified as transient (not input-required)

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As the bowerbird maintainer dogfooding `bowerbird-deck`,
I want `idle_prompt` notifications to stop forcing a session into `WaitingInput`,
so that the deck's `WaitingInput` column contains only sessions genuinely blocked on me (permission / elicitation), and finished-but-idle sessions read `Idle` instead of ratcheting up to "needs you" the longer they sit.

**Why now.** Live deck snapshot 2026-05-29: 13 of 15 *live* (non-`Ended`) sessions rendered `WaitingInput`, aged 5m–1h51m, none actually blocked. ADR 0004's liveness probe already drained the 90 *dead* sessions to `Ended`; this is a second, distinct population — **live, idle** sessions held in `WaitingInput` purely by the `idle_prompt → WaitingInput` classification. `idle_prompt` fires ~60s after a turn ends (`Stop → Idle`) when the user hasn't replied, so classifying it as input-required makes the idle nudge a one-way ratchet: the less is happening, the more "needs-you"-flagged a session looks. That inverts the signal.

This is the deliberate per-type decision ADR 0004 explicitly invited ("if a future `notification_type` doesn't fit cleanly into 'input-required' or 'transient,' the per-type rules need a deliberate decision"). It is **not** a reversal of 0004 — the typed-field model and the two-bucket structure stand; one type moves buckets.

## Acceptance Criteria

1. **Code — `transition` moves `IdlePrompt` to the transient bucket.** In `crates/daemon/src/projection/state.rs::transition`, the `EventKind::Notification` arm classifies `Some(NotificationType::IdlePrompt)` into the **preserve-prior** branch (joining `AuthSuccess` / `ElicitationResponse` / `ElicitationComplete` / `Unknown` / `None`), NOT the `WaitingInput` branch. After the change:
   - `PermissionPrompt` and `ElicitationDialog` are the **only** notification types that yield `SessionCurrentState::WaitingInput`.
   - `IdlePrompt` with a prior state returns that prior `current_state` (e.g. prior `Idle` → `Idle`; prior `WaitingInput` → `WaitingInput`); `IdlePrompt` with no prior defaults to `Idle` (via the existing `.unwrap_or(SessionCurrentState::Idle)`).
   - `last_event_kind` and `last_event_at_ms` still update for `IdlePrompt` (only `current_state` is preserved) — the preserve-prior branch already does this; do not regress it.
   - No other arm of `transition` changes. No change to `current_state_for_read`, `STALE_WORKING_MS`, or `last_pid` carry-forward.

2. **Code comments updated.** The `EventKind::Notification` arm's doc comment (currently lines 65–70) is rewritten so the two-bucket listing reads `PermissionPrompt | ElicitationDialog → WaitingInput` and `IdlePrompt | AuthSuccess | ElicitationResponse | ElicitationComplete | Unknown | None → preserve prior`, with an inline note that `IdlePrompt` was reclassified per ADR 0005 (idle nudge ~60s after `Stop`, not a block; "preserve prior" reads as `Idle` after a normal turn-end but does not clobber a still-pending permission/elicitation block).

3. **Tests updated and added** in the `state.rs` test module:
   - `transition_notification_input_required_yields_waiting_input` no longer iterates `IdlePrompt` — its loop is `[PermissionPrompt, ElicitationDialog]` only.
   - `transition_notification_transient_preserves_prior` gains `Some(NotificationType::IdlePrompt)` to its `cases` list (asserting it preserves the prior `Working` and still updates `last_event_kind`/`last_event_at_ms`).
   - New test: `IdlePrompt` with prior `Idle` → `Idle` (the common idle-nudge-after-`Stop` path).
   - New test: `IdlePrompt` with prior `WaitingInput` → `WaitingInput` (a pending `permission_prompt` block is NOT clobbered by a subsequent idle nudge).
   - (Optional but recommended) `IdlePrompt` with no prior → `Idle`.

4. **ADR 0004 amended in part.** `docs/decisions/0004-daemon-observed-session-liveness.md`: the §3 notification-type table row for `idle_prompt` changes from `→ WaitingInput` to `→ preserve prior (transient)`, and a Status note is added at the top: `Amended in part by ADR 0005 (idle_prompt reclassified transient) — 2026-05-29.` 0004 is amended, not superseded — its liveness probe, `Ended` state, `SessionEnded` event, and `PostToolUse → Working` refinement all stand.

5. **`docs/protocol.md` narrowed.** Both locations that currently list `idle_prompt` as input-required are corrected:
   - The `notification_type` extraction prose (≈line 352): input-required types become `permission_prompt`, `elicitation_dialog`; `idle_prompt` joins the transient ("preserve prior") list.
   - The `Notification` row of the hook-kind table (≈line 366): same correction.
   - The `WaitingInput` definition (wherever `SessionCurrentState` is described) reads "session is blocked on user input with work queued behind the answer (`permission_prompt` / `elicitation_dialog`, incl. `AskUserQuestion`)"; note explicitly that `idle_prompt` does NOT *transition a session into* `WaitingInput` (it preserves prior state, so a session already in `WaitingInput` stays there — the nudge neither creates nor clears the block).

6. **`docs/protocol-changelog.md` gains exactly one `type: behavioral` entry** under the active `v1.0 → v1.1` section, stating that `idle_prompt` no longer *transitions a session into* `WaitingInput` (reserved for `permission_prompt` / `elicitation_dialog`; `idle_prompt` preserves prior state, including a prior `WaitingInput`); presenters see strictly fewer `WaitingInput` transitions; this **supersedes the `idle_prompt` classification** in the Story 5.3 `Notification → WaitingInput` behavioral entry (which claimed `idle_prompt` is input-required). Cites Story 5.6 and ADR 0005. `(Resolves: 5.6)`.

7. **No wire-format change, no migration, no new field.** `crates/protocol/src/*.rs` is NOT modified. `NotificationType` keeps all seven variants. No SQLite migration. The change is strictly a narrowing of `WaitingInput` (fewer transitions into it); old presenters decoding with `#[serde(other)]` are unaffected (state set unchanged; only `WaitingInput` frequency drops).

8. **Verification green.** `cargo test --workspace -- --test-threads=1`, `cargo fmt --check`, and `cargo clippy --all-targets --workspace -- -D warnings` all pass.

## Tasks / Subtasks

- [x] Task 1: Move `IdlePrompt` to the transient branch in `transition` (AC: #1, #2)
  - [x] Edit `crates/daemon/src/projection/state.rs`: in the `EventKind::Notification` match arm, remove `| Some(NotificationType::IdlePrompt)` from the `WaitingInput` pattern and add it to the preserve-prior pattern.
  - [x] Rewrite the arm's doc comment (lines ~65–70) to the new two-bucket listing + the ADR 0005 reclassification note.
- [x] Task 2: Update and add tests (AC: #3)
  - [x] `transition_notification_input_required_yields_waiting_input`: drop `IdlePrompt` from the loop (leave `PermissionPrompt`, `ElicitationDialog`).
  - [x] `transition_notification_transient_preserves_prior`: add `Some(NotificationType::IdlePrompt)` to `cases`.
  - [x] Add `transition_notification_idle_prompt_prior_idle_yields_idle`.
  - [x] Add `transition_notification_idle_prompt_prior_waiting_input_preserved` (the load-bearing "don't clobber a real block" case).
  - [x] (Optional) Add `transition_notification_idle_prompt_without_prev_defaults_to_idle`.
- [x] Task 3: Amend ADR 0004 (AC: #4)
  - [x] Change the §3 table `idle_prompt` row to `→ preserve prior (transient)`.
  - [x] Add the top-of-file `Amended in part by ADR 0005 … 2026-05-29` Status note. (Already present from ADR 0005 creation commit 2a8fa1e; verified.)
- [x] Task 4: Correct `docs/protocol.md` (AC: #5)
  - [x] Fix the `notification_type` extraction prose (≈line 352).
  - [x] Fix the `Notification` hook-kind table row (≈line 366).
  - [x] Narrow the `WaitingInput` definition in the `SessionCurrentState` section (≈line 289).
- [x] Task 5: Add the changelog entry (AC: #6)
  - [x] Append one `type: behavioral` entry under `v1.0 → v1.1`, explicitly noting it supersedes the Story 5.3 `idle_prompt` classification, `(Resolves: 5.6)`.
- [x] Task 6: Verify (AC: #7, #8)
  - [x] `cargo test --workspace -- --test-threads=1` green (485 passed, 28 suites).
  - [x] `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` green.
  - [x] Confirm `git diff --stat` touches NO file under `crates/protocol/src/` and adds no migration (AC #7 self-check — confirmed: only `crates/daemon/src/projection/state.rs` + docs + status files).
- [ ] Task 7 (manual, optional — needs a live daemon): re-run `bowerbird-deck` against the live daemon and confirm the live-idle `WaitingInput` wall drains to `Idle`, only genuine `permission_prompt`/`elicitation_dialog`/`AskUserQuestion` blocks remain `WaitingInput` (proposal §5 success criterion). This is external-repo validation, not a CI gate. **Deferred** — requires a live daemon + the external `bowerbird-deck` repo; not runnable in this session. Left unchecked deliberately (optional, non-gating).

### Review Findings

- [x] [Review][Decision] Existing false `WaitingInput` rows are not remediated by this branch — The one-arm change only affects future projections. Startup calls `rebuild_missing_projections` (`crates/daemon/src/main.rs:166`), but that path rebuilds only sessions with no projection row and skips existing rows (`crates/daemon/src/projection/session.rs:443`, `crates/daemon/src/projection/session.rs:477`, `crates/daemon/src/projection/session.rs:484`). The new `IdlePrompt` behavior also deliberately preserves prior `WaitingInput` (`crates/daemon/src/projection/state.rs:80`, `crates/daemon/src/projection/state.rs:83`, `crates/daemon/src/projection/state.rs:88`; locked by `transition_notification_idle_prompt_prior_waiting_input_preserved`). That means live sessions already misclassified under the old `idle_prompt -> WaitingInput` rule can stay stuck after deploy unless a non-idle hook changes them, a one-time repair/reprojection is added, or the story explicitly accepts a future-event-only fix. This conflicts with the proposal success criterion that the live-idle wall drains (`docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md:148`) and needs a maintainer decision because AC #7 also says no migration/no new field.
  - **Maintainer decision (pickles, 2026-05-29) — FIRST resolved with a targeted startup repair, then REVERSED in the second review pass.** The repair (`repair_idle_prompt_waiting_input`) was prototyped, but on the second pass the maintainer chose to **remove it** and handle existing rows operationally (manually fix the local `bower.db` or truncate it) for V1. This change is therefore **future-event-only**: existing pre-5.6 `WaitingInput` rows clear when the session next receives a non-idle hook or the liveness probe ends a dead process. Rationale: the repair exposed a deeper gap — bowerbird has no general non-schema "data migration" facility, and the truncation-safety question (finding D2 below) is its hard part. That belongs in a real facility, not a one-off here. Tracked in `docs/bmad/implementation-artifacts/deferred-work.md` (§5.6 item 1). The repair function, its startup call, its tests, and the `recompute_state_from_log` refactor were all reverted.
- [x] [Review][Patch] Presenter-facing and local guidance still teach the old `idle_prompt -> WaitingInput` classification — `docs/presenter-authoring.md:190` still says only `permission_prompt`, `idle_prompt`, and `elicitation_dialog` cause `WaitingInput`, directly contradicting Story 5.6's new contract. Two contract-test comments also repeat the old bucket (`crates/daemon/tests/contract_daemon.rs:1271`, `crates/daemon/tests/contract_daemon.rs:1460`), and the current epic artifact still has the old Story 5.3 AC text (`docs/bmad/planning-artifacts/epics.md:1013`). Update these to say only `permission_prompt` and `elicitation_dialog` transition into `WaitingInput`; `idle_prompt` preserves prior state.
  - **Resolved:** `docs/presenter-authoring.md` rendering-`WaitingInput` paragraph rewritten to the two-type bucket + `idle_prompt`-as-transient note. Both `contract_daemon.rs` doc-comments (helper + full-sequence test) corrected to the post-5.6 buckets (comments only; no assertion change). `epics.md:1013` Story 5.3 AC left as as-shipped history with an explicit "Superseded for `IdlePrompt` by Story 5.6" note pointing at the 5.6 section (rewriting 5.3's AC verbatim would misrepresent what 5.3 shipped).
- [x] [Review][Patch] Updated docs overstate the `idle_prompt` invariant — `docs/protocol.md:289` says `idle_prompt` "does NOT produce `WaitingInput`" and `docs/protocol-changelog.md:53` says it "no longer produces `SessionCurrentState::WaitingInput`." The implementation's exact contract is narrower: `idle_prompt` no longer transitions a non-`WaitingInput` session into `WaitingInput`; it preserves prior state, so a prior `WaitingInput` remains `WaitingInput`. Tighten the wording in `docs/protocol.md`, `docs/protocol-changelog.md`, and the story completion notes (`docs/bmad/implementation-artifacts/5-6-idle-prompt-reclassified-as-transient.md:161`, `docs/bmad/implementation-artifacts/5-6-idle-prompt-reclassified-as-transient.md:180`) so presenter authors do not infer a hard `idle_prompt -> not WaitingInput` guarantee.
  - **Resolved:** all three reworded from "does NOT produce / no longer produces" to "does NOT transition a session into / no longer transitions a session into," each with an explicit clause that a session already in `WaitingInput` stays there on a later idle nudge (the nudge neither creates nor clears the blocked state).
- [x] [Review][Patch] Dev Agent Record file list omits a modified planning file — The branch modifies `docs/bmad/planning-artifacts/epics.md` to insert/resequence Story 5.6, but the story's `### File List` only lists `state.rs`, ADR/protocol docs, the story file, and sprint status (`docs/bmad/implementation-artifacts/5-6-idle-prompt-reclassified-as-transient.md:169`). Add `docs/bmad/planning-artifacts/epics.md` so the handoff accurately records all touched artifacts.
  - **Resolved:** `epics.md` added to the File List, plus the two post-review follow-up files (`presenter-authoring.md`, `contract_daemon.rs`).
- [x] [Review][Decision] Resolve `Ended + IdlePrompt` semantics before patching — Current code puts `Some(NotificationType::IdlePrompt)` in the preserve-prior branch (`crates/daemon/src/projection/state.rs:80-90`). That satisfies Story 5.6's local "prior state returns prior current_state" wording, but it conflicts with ADR 0004's non-terminal `Ended` contract: "`Ended` can transition out ... via any standard hook event" (`docs/decisions/0004-daemon-observed-session-liveness.md:30`). The existing `transition_from_ended_resumes_on_hook_event` test covers `UserPromptSubmit`, `Stop`, and `PermissionPrompt` from `Ended`, but not `IdlePrompt` (`crates/daemon/src/projection/state.rs:401-424`). Result: a session marked `Ended` by the liveness probe can receive an `idle_prompt` as its first later hook and remain hidden as `Ended`; because `current_state` does not change, no `state.session.*` frame is published. Decide whether a real `IdlePrompt` hook should leave `Ended` (likely `Ended + IdlePrompt -> Idle`) or whether Story 5.6 intentionally makes `IdlePrompt` the one standard hook that preserves `Ended`; then encode the decision in `transition`, docs, and a regression test.
  - **Maintainer decision (pickles, 2026-05-29): `Ended → Idle`, uniformly.** A notification hook arriving at all is evidence the process is alive, so the preserve-prior branch no longer preserves `Ended` — a prior `Ended` resurrects to `Idle` for *every* preserve-prior type (idle_prompt, auth_success, elicitation_response/complete, Unknown, None), honoring ADR 0004's non-terminal-`Ended` contract. Encoded in `transition` (`crates/daemon/src/projection/state.rs`, Notification arm), the arm doc comment, ADR 0005 Consequences, and the 5.6 changelog entry. Regression test: `transition_from_ended_preserve_prior_notification_yields_idle` (asserts every preserve-prior type + `None` resurrects `Ended → Idle`).
- [x] [Review][Decision] Startup repair can drain genuine blocks when the event log is partial — `repair_idle_prompt_waiting_input` targets every stored `WaitingInput` row, refolds the currently retained event log via `recompute_state_from_log`, and rewrites any row whose recompute is not `WaitingInput` (`crates/daemon/src/projection/session.rs:661-697`). That is safe only if the relevant session history is complete enough. This codebase explicitly supports event-history truncation/purge for query behavior (`crates/daemon/tests/contract_daemon.rs:2825-2857`) while `session_projections` can survive. If the original `permission_prompt` / `elicitation_dialog` event was purged but a later `idle_prompt` remains, the repair refold starts from the surviving idle nudge (`IdlePrompt` with no prior -> `Idle`) and can incorrectly clear a genuine blocked row. Decide the intended safety policy: skip repair when history may be incomplete, require explicit complete-history evidence before rewriting, constrain repair to clear idle-prompt-stuck rows with stronger evidence, or explicitly accept/document this as a generic reprojection repair with possible stale-projection correction behavior. The patch depends on that choice.
  - **Maintainer decision (pickles, 2026-05-29): remove the repair entirely.** Rather than build truncation-safety into a one-off, the repair is removed and existing rows are handled operationally (manual `bower.db` fix or truncate) for V1. The general gap — bowerbird has no non-schema "data migration" facility, and history-completeness safety is its hard part — is deferred to a proper facility (`docs/bmad/implementation-artifacts/deferred-work.md` §5.6 item 1). The repair function, startup call, tests, and `recompute_state_from_log` refactor were reverted (daemon code for this story is now just the `transition` change in `state.rs`). This also moots the two repair-specific [Patch] findings below.
- [x] [Review][Patch] Story and epic AC wording still state a hard `idle_prompt -> not WaitingInput` invariant — The implementation preserves prior state, so `idle_prompt` after an existing `WaitingInput` still leaves the session in `WaitingInput`. The story ACs still say `idle_prompt` "does NOT produce `WaitingInput`" / "no longer produces `WaitingInput`" (`docs/bmad/implementation-artifacts/5-6-idle-prompt-reclassified-as-transient.md:39`, `:41`), and the epic ACs repeat "does not produce" / "no longer produces" (`docs/bmad/planning-artifacts/epics.md:1147`, `:1151`). Update those ACs to the precise contract: `idle_prompt` does not transition a session into `WaitingInput`; it preserves prior state, including prior `WaitingInput`.
  - **Resolved:** story AC #5 bullet 3 and AC #6, plus epics ACs (`epics.md:1147`, `:1151`), reworded to "does not *transition a session into* `WaitingInput`; preserves prior state, including a prior `WaitingInput`."
- [x] [Review][Patch] Dev Agent Record and ADR metadata understate the final code surface — The Dev Agent Record still says the AC #7 self-check found "only `crates/daemon/src/projection/state.rs` + docs + status files" (`docs/bmad/implementation-artifacts/5-6-idle-prompt-reclassified-as-transient.md:70`), "Single code file: `crates/daemon/src/projection/state.rs`" (`:133`), "No other arm, constant, or function touched" (`:171`), and records the earlier 485-test run (`:167`, `:199`) even though the resolved branch added `crates/daemon/src/main.rs`, `crates/daemon/src/projection/session.rs`, and two contract tests, with later notes claiming 487 tests. ADR 0005's `Implementation:` metadata also lists only `state.rs`, `docs/protocol.md`, and the changelog (`docs/decisions/0005-idle-prompt-transient-not-input-required.md:7`). Reword the Dev Agent Record so the `transition` change is described as one-arm, but the final story includes the startup repair surfaces and one final verification count. Update ADR 0005 metadata to include `crates/daemon/src/projection/session.rs` and `crates/daemon/src/main.rs`.
  - **Resolved (with the repair now reverted, the surface shrank rather than grew):** the final daemon code surface is `crates/daemon/src/projection/state.rs` ONLY (the `transition` Notification arm: the one-arm `IdlePrompt` move + the `Ended → Idle` refinement). `session.rs` and `main.rs` are back to their pre-repair state. Dev Agent Record completion notes + verification count updated to one final figure; ADR 0005 `Implementation:` metadata left as `state.rs` + docs (accurate again).
- [x] [Review][Patch] (MOOT — repair removed) Repair skips malformed projection rows without diagnostics — The startup repair silently drops rows whose stored projection JSON cannot deserialize via `serde_json::from_str(&state_json).ok()?` (`crates/daemon/src/projection/session.rs:661-666`). A repair intended to correct persisted bad state can report `0` drained while skipping rows it could not inspect, giving operators no signal that the repair was incomplete. Log parse failures with `source` and `session_id` (and preferably count skipped rows or otherwise distinguish "nothing to repair" from "could not inspect row") before continuing.
  - **Moot:** the repair was removed (see the D2 decision above). No silent-skip code remains.
- [x] [Review][Patch] (MOOT — repair removed) Repair tests cover `PermissionPrompt` but not `ElicitationDialog` hard blocks — `repair_preserves_genuine_permission_block` only verifies `PermissionPrompt` followed by `IdlePrompt` stays `WaitingInput` (`crates/daemon/tests/contract_daemon.rs:2419-2451`). `ElicitationDialog` is the other input-required type and is the path used for `AskUserQuestion`; a replay/parser typo or drift there would drain a genuine block without failing this repair-specific contract. Add a repair test for `ElicitationDialog` followed by `IdlePrompt` staying `WaitingInput` and returning `drained == 0`.
  - **Moot:** the repair (and its tests) were removed. The `transition`-level guarantee that `ElicitationDialog` then `idle_prompt` stays `WaitingInput` is still covered by the pure-function state tests (`transition_notification_idle_prompt_prior_waiting_input_preserved` plus the input-required/transient table tests).

## Dev Notes

### The exact code change (do not improvise)

This is a one-arm move. The proposal (§4.1) specifies it verbatim. In `crates/daemon/src/projection/state.rs`, the `EventKind::Notification` arm currently is:

```rust
EventKind::Notification => match notification_type {
    Some(NotificationType::PermissionPrompt)
    | Some(NotificationType::IdlePrompt)
    | Some(NotificationType::ElicitationDialog) => SessionCurrentState::WaitingInput,
    Some(NotificationType::AuthSuccess)
    | Some(NotificationType::ElicitationResponse)
    | Some(NotificationType::ElicitationComplete)
    | Some(NotificationType::Unknown)
    | None => prev
        .map(|s| s.current_state)
        .unwrap_or(SessionCurrentState::Idle),
},
```

Move `Some(NotificationType::IdlePrompt)` from the first (WaitingInput) pattern to the second (preserve-prior) pattern. Nothing else in the function changes.

### Why "preserve prior" and not a hard `→ Idle`

`IdlePrompt` joins the existing transient bucket. In the common case prior state is `Idle` (turn ended via `Stop`), so it renders `Idle` and the wall drains. But if a genuine `permission_prompt` block is still pending and the user merely sat idle long enough to trigger an idle nudge, "preserve prior" keeps the session correctly in `WaitingInput` instead of a hard `→ Idle` clobbering a real block. This reuses 0004's bucket structure rather than adding a third rule. (ADR 0005 Decision + Alternatives.)

### Regression guard — what NOT to break

- **`PostToolUse → Working` (Story 5.3 AC #9)** is unrelated and must stay unconditional. Do not touch that arm.
- **`SessionEnded → Ended` and `Ended` non-terminality** (Story 5.3) are unrelated. Do not touch.
- **`last_pid` carry-forward / overwrite-on-Some** is independent of state logic and applies on every arm — leave it.
- **`current_state_for_read` stale-`Working` fallback** is unchanged and still backstops dropped-`Stop`. Per ADR 0005 Consequences, "preserve prior" for `idle_prompt` cannot reintroduce stale `WaitingInput` for live sessions: a live session that never blocked has prior `Idle`/`Working`, never `WaitingInput`.
- **The preserve-prior branch updates `last_event_kind`/`last_event_at_ms`** (the event still happened) — only `current_state` is held. The existing branch already does this correctly; the moved `IdlePrompt` pattern inherits it for free. The new prior-`Idle` test should assert `last_event_kind == Notification` to lock this in.

### Changelog gate nuance (read this — it's a trap)

The CI changelog gate (`tests/protocol_changelog_gate.rs`, Story 4.4) fires **only when a PR modifies `crates/protocol/src/*.rs`**. This story does NOT touch `crates/protocol/src/` — it edits `crates/daemon/src/projection/state.rs` plus docs. So the gate will **not** force a changelog entry, and it will **not** fail if you forget one. You must add the `type: behavioral` entry **deliberately** per AC #6 / ADR 0005 §4.5 — do not skip it just because CI is green, and do NOT manufacture a protocol-crate edit to trigger the gate. The entry is correctness/history hygiene, not a gate-satisfier.

This new entry **supersedes** the existing Story 5.3 behavioral entry (currently in `docs/protocol-changelog.md` under v1.0 → v1.1) that reads "Despite the name `idle_prompt`, Claude Code's idle pings … ARE classified as input-required." Do not delete the 5.3 entry (changelog is append-only history); your new entry states the reclassification and references that it supersedes the 5.3 classification.

### Test discipline

- Tests are pure-function table tests over `transition` — no async, no sleep, no daemon. Follow the existing `state.rs` test style (the `t(...)` helper passes `None` for `notification_type`/`pid`; for notification cases call `transition(...)` directly with the typed value, as the existing notification tests do).
- `unwrap()`/`expect()` are fine in tests (project-context: "Deterministic test discipline").
- Run the workspace suite **serialized**: `cargo test --workspace -- --test-threads=1`. The daemon contract + CLI E2E suites share process-wide state and hang under parallel execution (Epic 2 retro AI-3; codified in CI). The unit tests you're adding don't need it, but the full-workspace green check does.

### Project structure notes

- Single code file: `crates/daemon/src/projection/state.rs` (daemon crate — `thiserror` internal, no `anyhow` here; pure functions, no `unwrap` in non-test code; this change adds neither).
- Doc files: `docs/decisions/0004-daemon-observed-session-liveness.md`, `docs/protocol.md`, `docs/protocol-changelog.md`.
- No new modules, no new deps, no `Cargo.toml` changes. Module stays well under the ~800-line cap.
- Effort per the proposal: ~1 dev session (one code arm, ~3 test updates, 3 doc edits).

### Scope boundary — explicitly NOT in this story

Documented so the dev doesn't scope-creep (proposal §6, ADR 0005 Boundary):

- **`reaction` → tool-taxonomy redesign** (MAJOR, needs its own ADR + sprint-change-proposal). The `reaction` Pause/Continue primitive is inert and conflates tool-identity with attention; that's a protocol-surface redesign. Do NOT touch `reaction` or `tool-reactions.toml` here.
- **Deck column changes** live in the external `bowerbird-deck` repo, not this project.
- **"Idle but asked a question in prose"** is an accepted Axiom-1 limitation — the substrate cannot read message content to distinguish "asked you something" from "just idle." Reclassifying to transient defaults this to `Idle` on purpose. Do NOT add transcript reading to surface it.

### References

- [Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md] — the accepted proposal; §4.1–4.5 are the change spec, §5 the handoff, §6 the deferred work.
- [Source: docs/decisions/0005-idle-prompt-transient-not-input-required.md] — the decision record (Context / Decision / Boundary / Consequences / Alternatives).
- [Source: docs/decisions/0004-daemon-observed-session-liveness.md#§3] — the table being amended; original two-bucket classification.
- [Source: crates/daemon/src/projection/state.rs] — `transition` (lines 42–121, Notification arm 71–82) and its test module (existing notification tests at lines 217–280).
- [Source: docs/protocol-changelog.md] — active `v1.0 → v1.1` section; the Story 5.3 `Notification → WaitingInput` entry this supersedes.
- [Source: docs/protocol.md] — `SessionCurrentState` (≈289), `notification_type` extraction (≈352), `Notification` hook-kind row (≈366).
- [Source: docs/bmad/project-context.md#Substrate-not-actor invariants] — "exactly one normalization (tool→reaction)"; Axiom 1 (substrate observes, does not interpret); Axiom 4 (mechanical facts in protocol, semantics in presenter) — the framing behind keeping `idle_prompt` interpretation out of `WaitingInput`.
- [Source: docs/bmad/planning-artifacts/epics.md#Story 5.3] — Story 5.3 ACs #7/#8 (the notification-type buckets this story re-sorts).

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context) via the `bmad-dev-story` workflow.

### Debug Log References

- TDD red phase (initial): `cargo test -p bowerbird-daemon projection::state` → 3 failures before the code change, all asserting the new transient behavior against the still-input-required code. Confirms the tests exercise the change.
- TDD green phase: after the one-arm move, state tests passed.
- Final verification (after both code-review passes; repair reverted, `Ended → Idle` added): `cargo test --workspace -- --test-threads=1` → **486 passed**, no failures; `cargo fmt --check` clean; `cargo clippy --all-targets --workspace -- -D warnings` → no issues.

### Completion Notes List

The final daemon code surface for this story is **`crates/daemon/src/projection/state.rs` only** — the `EventKind::Notification` arm of `transition`. (The second code-review pass prototyped a startup repair touching `session.rs`/`main.rs`; the maintainer reverted it — see Review Findings. Those files are back to their pre-repair state.)

- **The core change is a one-arm move.** `Some(NotificationType::IdlePrompt)` moved from the `WaitingInput` match pattern to the preserve-prior pattern. `permission_prompt` and `elicitation_dialog` (incl. `AskUserQuestion`) are now the only `notification_type` values that *transition a session into* `WaitingInput`.
- **`WaitingInput` is narrowed, not a hard guarantee.** `idle_prompt` preserves prior state, so a session already in `WaitingInput` stays there on a later idle nudge — the narrowing is about transitions *into* `WaitingInput`, not "`idle_prompt` ⇒ never `WaitingInput`." Locked by `transition_notification_idle_prompt_prior_waiting_input_preserved`.
- **`Ended → Idle` refinement (code-review D1).** The preserve-prior branch no longer preserves `Ended`: a notification hook is evidence the process is alive, so a prior `Ended` resurrects to `Idle` for every preserve-prior type, honoring ADR 0004's non-terminal-`Ended` contract. Locked by `transition_from_ended_preserve_prior_notification_yields_idle`.
- **`last_event_kind`/`last_event_at_ms` still update** on the preserve-prior types — only `current_state` is held (or resurrected from `Ended`).
- **Changelog entry added deliberately.** The CI changelog gate (`tests/protocol_changelog_gate.rs`) does NOT fire for this story (no `crates/protocol/src/*.rs` edit), so the `type: behavioral` entry under `v1.0 → v1.1` is history hygiene, not a gate-satisfier. It supersedes (does not delete) the Story 5.3 `idle_prompt` classification entry and also records the `Ended → Idle` refinement.
- **ADR 0004 §3 table row amended** to `→ preserve prior (transient)`; the top-of-file Status amendment note was already present from the ADR 0005 creation commit (2a8fa1e).
- **Existing rows are NOT auto-remediated (future-event-only).** The repair that would have drained pre-5.6 `WaitingInput` rows was removed; operators fix the local `bower.db` by hand or truncate it for V1. The general non-schema "data migration" gap is deferred (`deferred-work.md` §5.6 item 1).
- **AC #7 holds:** no `crates/protocol/src/` change, no SQLite migration; `NotificationType` keeps all seven variants.
- **Task 7 deferred** (manual, optional, non-gating): re-run `bowerbird-deck` against a live daemon to confirm the wall drains. Not runnable in this session; with the repair removed, the pre-5.6 wall is drained operationally (manual fix/truncate), not by the daemon.

### File List

- `crates/daemon/src/projection/state.rs` (modified — `transition` Notification arm: `IdlePrompt` → preserve-prior + `Ended → Idle` refinement; doc comment; updated/added pure-function tests incl. `transition_from_ended_preserve_prior_notification_yields_idle`)
- `docs/decisions/0004-daemon-observed-session-liveness.md` (modified — §3 table `idle_prompt` row)
- `docs/decisions/0005-idle-prompt-transient-not-input-required.md` (modified — Consequences: `Ended → Idle`, future-event-only, repair removed; data-migration gap deferred)
- `docs/protocol.md` (modified — `SessionCurrentState`/`WaitingInput` definition, `notification_type` extraction prose, `Notification` hook-kind row)
- `docs/protocol-changelog.md` (modified — one `type: behavioral` entry under `v1.0 → v1.1`)
- `docs/presenter-authoring.md` (modified — `WaitingInput` rendering guidance narrowed; `idle_prompt` documented as transient)
- `crates/daemon/tests/contract_daemon.rs` (modified — two stale doc-comments corrected to the post-5.6 buckets; comments only, no assertion change)
- `docs/bmad/planning-artifacts/epics.md` (modified — Story 5.6 section; superseded/extended notes on Story 5.3 ACs; AC wording precise)
- `docs/bmad/implementation-artifacts/deferred-work.md` (modified — §5.6: non-schema data-migration facility deferred)
- `docs/bmad/implementation-artifacts/5-6-idle-prompt-reclassified-as-transient.md` (modified — Status, ACs, task checkboxes, Review Findings, Dev Agent Record)
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (modified — story status + `last_updated`)

### Change Log

- 2026-05-29 — Story 5.6 implemented: `idle_prompt` reclassified from input-required (`→ WaitingInput`) to transient (preserve-prior) per ADR 0005. One-arm change in `transition`, test updates/additions, ADR 0004 §3 amended, `docs/protocol.md` (3 spots) + `docs/protocol-changelog.md` corrected. No wire-format/migration change.
- 2026-05-29 — Code review pass 1: three `[Patch]` findings resolved (stale presenter/contract/epics docs; overstated invariant wording; File List). The existing-rows `[Decision]` was first resolved with a targeted startup repair.
- 2026-05-29 — Code review pass 2: two `[Decision]` + four `[Patch]` findings. **D1:** preserve-prior branch now resurrects `Ended → Idle` (a hook means the process is alive; honors ADR 0004) — uniform across preserve-prior types, + regression test. **D2:** the startup repair was **removed** (maintainer choice) in favor of operational `bower.db` fix/truncate for V1; the non-schema data-migration gap is deferred. Patches: story/epic AC wording made precise; Dev Agent Record + ADR metadata corrected; two repair-specific patches mooted by the removal. Final: **486 workspace tests** serialized, fmt + clippy clean.
