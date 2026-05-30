# 0005. `idle_prompt` is transient, not input-required (amends ADR 0004 §3)

Date: 2026-05-29
Status: Accepted
Deciders: @pickles
Related: ADR-0004 (`docs/decisions/0004-daemon-observed-session-liveness.md` — this ADR amends its §3 notification-type classification, one row); sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md (operationalizes this decision); Story 5.3 (`docs/bmad/implementation-artifacts/5-3-daemon-observed-session-liveness.md` — done; this ADR changes its shipped `transition` behavior via a new minor story)
Implementation: `crates/daemon/src/projection/state.rs` (one match arm in `transition`); `docs/protocol.md` (§`SessionCurrentState` / notification semantics); `docs/protocol-changelog.md` (one `type: behavioral` entry)
Affects sections: ADR-0004 §3 table (`idle_prompt` row)

## Context

ADR 0004 §3 made `WaitingInput` typed-field-driven: the adapter extracts Claude's `notification_type` and the projection's `transition` maps it to a state. 0004 sorted the six notification types into two buckets:

- **input-required → `WaitingInput`:** `permission_prompt`, `idle_prompt`, `elicitation_dialog`
- **transient → preserve prior:** `auth_success`, `elicitation_response`, `elicitation_complete` (+ `Unknown`/`None`)

0004 placed `idle_prompt` in the input-required bucket while explicitly noting (defect #3) that it is "the *least* actionable of the input-required types" and that ~60% of real notifications are `idle_prompt`.

During live `bowerbird-deck` dogfooding on 2026-05-29, 13 of 15 *live* (non-`Ended`) sessions rendered as `WaitingInput`, aged 5m–1h51m — none actually blocked on the maintainer. 0004's liveness probe had correctly drained the *dead*-session ghosts (90 `Ended` rows hidden), but a second, distinct population remained: **live, idle** sessions held in `WaitingInput` solely by the `idle_prompt` classification.

The mechanism is a one-way ratchet. Claude Code fires `idle_prompt` ~60s after a turn ends (`Stop → Idle`) when the user hasn't replied. With `idle_prompt → WaitingInput`, a finished session escalates from `Idle` back up to `WaitingInput` just by sitting there — the less is happening, the more "needs-you"-flagged it appears. That inverts the signal's meaning.

The distinction `idle_prompt` was being asked to carry is twofold, and each half has a better home:

- **"Should I pay attention" (attention axis)** is already and more faithfully carried by `current_state`: `WaitingInput` = work blocked on you; `Working` = busy; `Idle`/`Ended` = nothing pending. `idle_prompt` adds no attention information the state machine doesn't already have — it just nudges.
- **A genuine hard block** — work queued behind the user's answer — is what `permission_prompt`, `elicitation_dialog`, and `AskUserQuestion` (which surfaces as `elicitation_dialog`) represent. Those stay `WaitingInput`.

0004's Consequences section anticipated exactly this: "if a future `notification_type` doesn't fit cleanly into 'input-required' or 'transient,' the per-type rules need a deliberate decision rather than the 'preserve prior' default." This ADR is that deliberate decision.

## Decision

**`idle_prompt` moves out of the input-required bucket.**

- **`WaitingInput` is reserved for genuine hard blocks:** `permission_prompt` and `elicitation_dialog` (and `AskUserQuestion`, which Claude Code delivers as `elicitation_dialog`). A hard block = the agent has work it cannot proceed with until the user answers.
- **`idle_prompt` → `Idle`, except a prior `WaitingInput` is preserved.** (The original proposal said "move to the preserve-prior bucket"; **code-review D3 refined it** — see the Alternatives.) The idle nudge fires ~60s after a turn ends, so its arrival is positive evidence the turn is *over*: Claude does not ping idle mid-work. So `idle_prompt` resolves to `Idle`, which:
  - drains the live-idle wall (the common case: turn ended, session idle); and
  - covers a **dropped `Stop`** — a finished session whose `Stop` hook was lost still lands on `Idle` on the next idle nudge, instead of pinning a stale `Working` that the nudge's `last_event_at_ms` refresh would otherwise keep alive past the read-time stale-`Working` fallback.
  - The ONE exception: a still-pending `permission_prompt`/`elicitation_dialog` block (prior `WaitingInput`) is preserved — an idle nudge neither creates nor clears a real block. So `idle_prompt` never *transitions a session into* `WaitingInput`.

0004's typed-field model and its two-bucket *framing* stand; `idle_prompt` gets a dedicated rule (`Idle` unless a block is pending) rather than sharing the generic preserve-prior branch.

### Boundary: the conversational "your turn" case is out of scope (Axiom 1)

`idle_prompt` also fires when Claude ends a turn with a *question in prose* and waits for a reply. That case — "the agent asked you something but isn't blocked" — is genuinely "your turn," but the substrate **cannot observe it**: the question lives in the assistant's message text, and bowerbird never reads message content (Axiom 1: the substrate observes hook events; it does not interpret transcript content). Reclassifying `idle_prompt → transient` defaults this case to `Idle`. Surfacing it would require reading transcripts, which Axiom 1 forbids. It is parked as a known, accepted limitation — not a bug this design can fix without crossing the axiom.

## Consequences

- **Behavioral, additive-safe.** `WaitingInput` is *narrowed* — strictly fewer transitions into it. No new false-positive blocks are possible. Old presenters decoding with `#[serde(other)]` are unaffected (the state set is unchanged; only the frequency of `WaitingInput` drops). One `type: behavioral` entry in `docs/protocol-changelog.md`.
- **The deck's `WaitingInput` column becomes meaningful:** it now contains only sessions with work genuinely blocked on the user. Finished/idle sessions read `Idle`.
- **ADR 0004 is amended in part,** not superseded — its liveness probe, `Ended` state, `SessionEnded` event, and `PostToolUse → Working` refinement all stand. Only the `idle_prompt` row of its §3 table changes.
- **Risk — idle ghosts.** Pre-0004, stale `WaitingInput` was the symptom of dead sessions; that is now handled by the liveness probe → `Ended`, independently of this change. `idle_prompt → Idle` cannot reintroduce stale `WaitingInput` for live sessions: it only ever results in `Idle` or a preserved prior `WaitingInput`, never a *new* `WaitingInput`.
- **Dropped-`Stop` interaction (code-review D3).** Because `idle_prompt → Idle` (rather than preserving a prior `Working`), an idle nudge after a dropped `Stop` resolves the finished session to `Idle` directly. This was the reason the rule is not a plain preserve-prior: a preserve-prior `idle_prompt` would keep a stale `Working` AND refresh `last_event_at_ms` on every nudge, defeating the read-time stale-`Working` fallback (potentially indefinitely if idle nudges repeat). The 5-minute stale-`Working` fallback remains the backstop for the case where BOTH `Stop` and any subsequent `idle_prompt` are dropped.
- **`Ended` is not preserved by a notification hook.** "Preserve prior" must not preserve `Ended`: a notification hook arriving for an `Ended` session is evidence the process is alive (Claude fired it), so it transitions `Ended → Idle`, consistent with ADR 0004's non-terminal-`Ended` contract. This applies to every preserve-prior notification type, not just `idle_prompt`; `idle_prompt` joining this branch is what made the case reachable via the most common stray hook. Pinned by `transition_from_ended_preserve_prior_notification_yields_idle`.
- **Existing rows are NOT auto-remediated; this is a future-event-only change.** The `transition` change affects only *future* projections, and a stored `WaitingInput` is preserved by `idle_prompt` and by the other preserve-prior types. So rows the *pre-5.6* daemon wrote as `WaitingInput` from an `idle_prompt` stay stuck after deploy until the session receives **a hook that transitions out of `WaitingInput`** — `UserPromptSubmit`/`PreToolUse`/`PostToolUse` (→ `Working`), `Stop` (→ `Idle`), or the daemon's `SessionEnded` liveness signal (→ `Ended`). A later `idle_prompt` or other transient notification does NOT clear them (it preserves the prior `WaitingInput`). A code-review pass (2026-05-29) prototyped a one-time startup reprojection-repair to drain them, but the maintainer chose to **remove it** and handle existing rows operationally (manual DB fix or truncate the local `bower.db`) for V1. The repair was a *data* reprojection (no schema migration), and the deeper gap it exposed — bowerbird has no general mechanism for non-schema "data migrations" (backfills / reprojections gated on history completeness) — is tracked in `docs/bmad/implementation-artifacts/deferred-work.md`. A proper data-migration facility, with the truncation-safety guard the repair would have needed, is the right home for this rather than a one-off in Story 5.6.

## Alternatives considered

- **Plain preserve-prior for `idle_prompt`** (the original proposal: drop `idle_prompt` into the generic preserve-prior bucket). Initially adopted, then **rejected in code-review D3.** Preserve-prior keeps a prior `Working` as `Working`; combined with the fact that every event refreshes `last_event_at_ms`, an `idle_prompt` after a dropped `Stop` keeps a finished session reading `Working` and resets the 5-minute stale-`Working` read-time fallback on each nudge — potentially indefinitely. That contradicted this ADR's own claim that the stale-`Working` fallback covers dropped-`Stop`.
- **Hard `idle_prompt → Idle`** (unconditional). Rejected: it would clobber a still-pending `permission_prompt`/`elicitation_dialog` block if the user sat idle long enough to trigger an idle nudge. **The chosen rule is this with one guard:** `idle_prompt → Idle` *except* a prior `WaitingInput` is preserved. That resolves a dropped `Stop` (the preserve-prior failure above) while keeping a real block — the best of both, at the cost of `idle_prompt` getting its own arm instead of sharing the transient bucket.
- **Orthogonal `pending_input` flag on the projection** (separate from `current_state`). Already considered and rejected in ADR 0004 ("i don't like having latest notification on the projection. i want a slight interpolation of it"). This ADR stays within that agreed boundary — the typed-field classification *is* the slight interpolation; we only re-sort one type.
- **Read the transcript to distinguish "asked a question" from "just idle."** Rejected on Axiom 1 — the substrate does not interpret message content. See the Boundary section.
