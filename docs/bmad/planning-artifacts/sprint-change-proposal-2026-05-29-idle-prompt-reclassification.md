# Sprint Change Proposal — `idle_prompt` reclassified as transient (not input-required)

**Date:** 2026-05-29
**Author:** pickles (via correct-course)
**Status:** Accepted (2026-05-29, @pickles)
**Scope classification:** Minor (direct dev implementation)
**Related:** ADR 0004 (`docs/decisions/0004-daemon-observed-session-liveness.md`, amends §3); ADR 0005 (`docs/decisions/0005-idle-prompt-transient-not-input-required.md`, the decision record this proposal operationalizes); Story 5.3 (`docs/bmad/implementation-artifacts/5-3-daemon-observed-session-liveness.md`, **done** — this proposal modifies its shipped behavior via a new story)

---

## Section 1: Issue Summary

**Problem statement.** `WaitingInput` over-reports. During live dogfooding of `bowerbird-deck` (2026-05-29), 13 of 15 *live* (non-`Ended`) sessions rendered as `WaitingInput`, aged 5m–1h51m, with 90 more `Ended` correctly hidden. None of the 13 were actually blocked on the maintainer — they were finished/idle sessions that had emitted an `idle_prompt` notification.

**Root cause.** ADR 0004 §3 classifies `idle_prompt` into the input-required bucket (`→ WaitingInput`), alongside the genuine hard blocks `permission_prompt` and `elicitation_dialog`. But `idle_prompt` is not a block — Claude Code fires it ~60s after a turn ends (`Stop → Idle`) when the user hasn't replied. Treating it as input-required makes the idle nudge a **one-way ratchet**: any finished session escalates from `Idle` back up to `WaitingInput` simply by sitting there. The longer nothing happens, the more "needs-you"-flagged the session looks. That is backwards.

**Why ADR 0004 didn't catch it.** 0004's headline defect (#1) was the *same* `WaitingInput` wall — but its theory was that those rows were **dead** sessions (terminals closed without firing `Stop`), and its fix was the liveness probe + `Ended` state to drain them. That fix works: the 90 `Ended` rows are now correctly hidden. What remains is a *second*, distinct population 0004 did not address: **live, idle** sessions held in `WaitingInput` purely by the `idle_prompt` classification.

**This is the deliberate decision 0004 invited.** ADR 0004's Consequences section states verbatim:

> if a future `notification_type` doesn't fit cleanly into "input-required" or "transient," the per-type rules need a deliberate decision rather than the "preserve prior" default.

`idle_prompt` is exactly that case. This proposal is the deliberate decision, not a reversal of 0004 — 0004's typed-field model and its two-bucket structure are preserved; one type moves buckets.

**Evidence.** Live deck snapshot, 2026-05-29:

```
state         tool (reaction)                   pid      age
WaitingInput  Read (Continue)                   40016    12m
WaitingInput  Bash (Continue)                   16312    13m
WaitingInput  AskUserQuestion (Unknown)         96527    14m
WaitingInput  Bash (Continue)                   34466    17m
WaitingInput  - (-)                             33511    1h22m
... (13 of 15 live sessions in WaitingInput; 90 Ended hidden)
```

The 1h+ rows are idle-nudged finished turns, not blocks.

---

## Section 2: Impact Analysis

**Epic impact.** Epic 5 (dogfooding-and-correctness, in-progress). No re-scope. This is a correctness refinement surfaced by exactly the dogfooding Epic 5 exists to do.

**Story impact.**
- **Story 5.3 (done)** — owns the `transition` function and the notification-type buckets (ACs #7, #8). Its shipped behavior changes. Because it is *done*, the change is carried by a **new minor story**, not a rewrite of 5.3's ACs. Story 5.3 gets a pointer note.
- **No other story** depends on `idle_prompt → WaitingInput`. Story 5.1 (deck) consumes `current_state` but doesn't assume idle ⇒ WaitingInput.

**Artifact conflicts.**
- `ADR 0004` §3 table — the `idle_prompt → WaitingInput` row. Amended (see §4), with a superseded-in-part note.
- `docs/protocol.md` §`SessionCurrentState` / notification semantics — `WaitingInput` definition narrows to "genuine input-required block."
- `docs/protocol-changelog.md` — one `type: behavioral` entry (old presenters see fewer `WaitingInput` events; strictly a reduction, additive-safe).

**Technical impact.** One match arm in `crates/daemon/src/projection/state.rs::transition`. Move `Some(NotificationType::IdlePrompt)` from the input-required arm to the transient ("preserve prior") arm. Plus test updates. No wire-format change, no migration, no new field.

**Not in scope of THIS proposal** (documented in §6 so the reasoning isn't lost): the `reaction`→tool-taxonomy redesign, the deck column changes, and the "idle-but-asked-in-prose" constraint. Those are tracked as follow-on work.

---

## Section 3: Recommended Approach

**Direct adjustment.** Add one minor story to Epic 5 that moves `idle_prompt` to the transient bucket, with regression tests, and updates the decision/doc artifacts. No rollback, no MVP re-scope.

- **Effort:** ~1 dev session (one code arm, ~3 test updates, 3 doc edits).
- **Risk:** Low. Strictly *narrows* `WaitingInput` (fewer false positives); cannot create new false-positive blocks. Old presenters using `#[serde(other)]` are unaffected (state set unchanged; only frequency drops).
- **Timeline:** Slots before or after Story 5.5 (bench-gates); no dependency either way.

**Why "preserve prior" and not a hard `→ Idle`:** `idle_prompt` joins the existing transient bucket (`AuthSuccess`/`ElicitationResponse`/`ElicitationComplete`). In the common case the prior state is `Idle` (the turn ended via `Stop`), so the rendered result is `Idle`. But if a genuine `permission_prompt` block is still pending and the user merely sat idle, "preserve prior" keeps the session correctly in `WaitingInput` instead of a hard `→ Idle` clobbering a real block. Safer, and it reuses 0004's bucket structure rather than adding a third rule.

---

## Section 4: Detailed Change Proposals

### 4.1 Code — `crates/daemon/src/projection/state.rs::transition`

**OLD:**
```rust
//   PermissionPrompt | IdlePrompt | ElicitationDialog → WaitingInput
//   AuthSuccess | ElicitationResponse | ElicitationComplete | Unknown | None → preserve prior
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

**NEW:**
```rust
//   PermissionPrompt | ElicitationDialog → WaitingInput (genuine hard block: work queued behind the user)
//   IdlePrompt | AuthSuccess | ElicitationResponse | ElicitationComplete | Unknown | None → preserve prior
// IdlePrompt reclassified from input-required to transient per ADR 0005:
// it is an idle nudge (~60s after Stop), not a block. "Preserve prior" means it
// reads as Idle after a normal turn-end but will not clobber a still-pending
// permission/elicitation block if the user merely sat idle.
EventKind::Notification => match notification_type {
    Some(NotificationType::PermissionPrompt)
    | Some(NotificationType::ElicitationDialog) => SessionCurrentState::WaitingInput,
    Some(NotificationType::IdlePrompt)
    | Some(NotificationType::AuthSuccess)
    | Some(NotificationType::ElicitationResponse)
    | Some(NotificationType::ElicitationComplete)
    | Some(NotificationType::Unknown)
    | None => prev
        .map(|s| s.current_state)
        .unwrap_or(SessionCurrentState::Idle),
},
```

**Rationale:** `idle_prompt` is an idle nudge, not a block. Moving it to the transient bucket drains the live-idle `WaitingInput` wall while leaving genuine blocks (`permission_prompt`, `elicitation_dialog`, and `AskUserQuestion` which surfaces as `elicitation_dialog`) correctly flagged.

### 4.2 Tests — `crates/daemon/src/projection/state.rs` (test module)

- Update the test asserting `IdlePrompt → WaitingInput` to assert `IdlePrompt` preserves prior state.
- Add: `IdlePrompt` with prior `Idle` → `Idle` (the common idle-nudge-after-Stop path).
- Add: `IdlePrompt` with prior `WaitingInput` → `WaitingInput` (a pending permission block is not clobbered by a subsequent idle nudge).

### 4.3 ADR 0004 — `docs/decisions/0004-daemon-observed-session-liveness.md`

- §3 table: change the `idle_prompt` row from `→ WaitingInput` to `→ preserve prior (transient)`.
- Add a Status note at the top: `Amended in part by ADR 0005 (idle_prompt reclassified transient) — 2026-05-29.`

### 4.4 ADR 0005 — new

Created at `docs/decisions/0005-idle-prompt-transient-not-input-required.md` (see separate artifact). Records the decision, the live-idle-vs-dead distinction, and the Axiom-1 boundary on the deferred prose-question case.

### 4.5 Docs

- `docs/protocol.md`: narrow the `WaitingInput` definition to "session is blocked on user input with work queued (permission/elicitation)"; note `idle_prompt` does not produce `WaitingInput`.
- `docs/protocol-changelog.md`: one `type: behavioral` entry — "`WaitingInput` no longer produced by `idle_prompt`; reserved for `permission_prompt`/`elicitation_dialog`. Presenters see fewer `WaitingInput` transitions."

---

## Section 5: Implementation Handoff

**Scope:** Minor → direct implementation by the Developer agent.

**Steps:**
1. Create a minor story under Epic 5 via `bmad-create-story` (suggested title: "`idle_prompt` reclassified as transient"). ACs derived from §4.1–4.5.
2. Implement the one-arm change + tests (§4.1, §4.2).
3. Land ADR 0005, amend ADR 0004, update `protocol.md` + `protocol-changelog.md` (§4.3–4.5).
4. Re-run the deck against the live daemon; confirm the live-idle `WaitingInput` wall drains to `Idle` and only genuine blocks remain `WaitingInput`.

**Success criteria:** A deck snapshot where `WaitingInput` contains only sessions with a pending `permission_prompt`/`elicitation_dialog`/`AskUserQuestion`; finished/idle sessions read `Idle`. Workspace tests + fmt + clippy green.

---

## Section 6: Deferred / Follow-on Work (NOT proposed here)

Documented so the analysis behind them survives. Each needs its own proposal/story before action.

- **`reaction` → tool taxonomy (MAJOR).** The `reaction` (Pause/Continue) primitive is currently inert (the seeded `tool-reactions.toml` maps every tool to `Continue`) and conflates two jobs: tool-identity and "should I react." Attention is better carried by `current_state`; identity wants a canonical cross-agent **tool taxonomy** (`Bash`/`shell`/`run_terminal_cmd` → `Shell`), exposing both raw tool name and canonical category on the wire. This overturns the product brief's "one normalization = tool name → reaction enum" claim and is a protocol-surface redesign. **Requires its own major sprint-change-proposal + ADR + likely a brief revisit.** Do not fold into this minor change.
- **Deck column (MINOR, external repo `bowerbird-deck`).** Show the canonical tool only while `Working` (= live activity), blank otherwise; drop the `(reaction)` parenthetical. Downstream of the taxonomy work. Tracked in the deck repo, not this bmad project.
- **"Idle but asked you a question in prose" (DEFERRED, long-term).** When Claude ends a turn with a question in prose but isn't blocked, the substrate cannot distinguish it from "idle, done" — the question lives in assistant message text, which bowerbird does not read (Axiom 1: observe, don't interpret; never parse transcript content). Reclassifying `idle_prompt → transient` is the lever that defaults this case to `Idle`. Surfacing it would require reading message content, which is an Axiom-1 violation. Parked as a known, accepted limitation.
