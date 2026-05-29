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
   - The `WaitingInput` definition (wherever `SessionCurrentState` is described) reads "session is blocked on user input with work queued behind the answer (`permission_prompt` / `elicitation_dialog`, incl. `AskUserQuestion`)"; note explicitly that `idle_prompt` does NOT produce `WaitingInput`.

6. **`docs/protocol-changelog.md` gains exactly one `type: behavioral` entry** under the active `v1.0 → v1.1` section, stating that `idle_prompt` no longer produces `WaitingInput` (reserved for `permission_prompt` / `elicitation_dialog`); presenters see strictly fewer `WaitingInput` transitions; this **supersedes the `idle_prompt` classification** in the Story 5.3 `Notification → WaitingInput` behavioral entry (which claimed `idle_prompt` is input-required). Cites Story 5.6 and ADR 0005. `(Resolves: 5.6)`.

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

- TDD red phase: `cargo test -p bowerbird-daemon projection::state` → 3 failures before the code change (`transition_notification_transient_preserves_prior` with the new `IdlePrompt` case, `transition_notification_idle_prompt_prior_idle_yields_idle`, `transition_notification_idle_prompt_without_prev_defaults_to_idle`) — all asserting the new transient behavior against the still-input-required code. Confirms the tests exercise the change.
- TDD green phase: after the one-arm move, same target → 24 passed.
- Full verification: `cargo test --workspace -- --test-threads=1` → 485 passed (28 suites, ~27s); `cargo fmt --check` clean; `cargo clippy --all-targets --workspace -- -D warnings` → no issues.

### Completion Notes List

- **The change is a one-arm move, exactly as the proposal specified.** `Some(NotificationType::IdlePrompt)` moved from the `WaitingInput` match pattern to the preserve-prior pattern in `transition`'s `EventKind::Notification` arm. No other arm, constant, or function touched.
- **`WaitingInput` is now narrowed to two types:** `permission_prompt` and `elicitation_dialog` (incl. `AskUserQuestion`) are the only `notification_type` values that produce it.
- **"Preserve prior," not hard `→ Idle`:** an `idle_prompt` after a normal turn-end reads `Idle` (prior was `Idle`), draining the deck's live-idle `WaitingInput` wall; but an `idle_prompt` following a still-pending `permission_prompt` block keeps the session in `WaitingInput` (does not clobber a real block). Locked by `transition_notification_idle_prompt_prior_waiting_input_preserved`.
- **`last_event_kind`/`last_event_at_ms` still update** on the moved type — the preserve-prior branch already did this; the prior-`Idle` test asserts `last_event_kind == Notification` to pin it.
- **Changelog entry added deliberately.** The CI changelog gate (`tests/protocol_changelog_gate.rs`) does NOT fire for this story (no `crates/protocol/src/*.rs` edit), so the `type: behavioral` entry under `v1.0 → v1.1` is history hygiene per the story's "changelog gate nuance" note, not a gate-satisfier. It explicitly supersedes (does not delete) the Story 5.3 `idle_prompt` classification entry.
- **ADR 0004 §3 table row amended** to `→ preserve prior (transient)`; the top-of-file Status amendment note was already present from the ADR 0005 creation commit (2a8fa1e).
- **AC #7 self-check confirmed:** `git diff --stat` touches no file under `crates/protocol/src/` and adds no migration. `NotificationType` keeps all seven variants.
- **Task 7 deferred** (manual, optional, non-gating): requires a live daemon + the external `bowerbird-deck` repo to confirm the live-idle wall drains. Not runnable in this session; left unchecked deliberately.

### File List

- `crates/daemon/src/projection/state.rs` (modified — `transition` Notification arm + doc comment; 3 new tests, 2 updated tests)
- `docs/decisions/0004-daemon-observed-session-liveness.md` (modified — §3 table `idle_prompt` row)
- `docs/protocol.md` (modified — `SessionCurrentState`/`WaitingInput` definition, `notification_type` extraction prose, `Notification` hook-kind row)
- `docs/protocol-changelog.md` (modified — one new `type: behavioral` entry under `v1.0 → v1.1`)
- `docs/bmad/implementation-artifacts/5-6-idle-prompt-reclassified-as-transient.md` (modified — Status, task checkboxes, Dev Agent Record)
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (modified — story status + `last_updated`)

### Change Log

- 2026-05-29 — Story 5.6 implemented: `idle_prompt` reclassified from input-required (`→ WaitingInput`) to transient (preserve-prior) per ADR 0005. One-arm change in `transition`, 5 test updates/additions, ADR 0004 §3 amended, `docs/protocol.md` (3 spots) + `docs/protocol-changelog.md` corrected. Verification green (485 tests serialized, fmt, clippy). No wire-format/migration change.
