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

**`idle_prompt` moves from the input-required bucket to the transient bucket.**

- **`WaitingInput` is reserved for genuine hard blocks:** `permission_prompt` and `elicitation_dialog` (and `AskUserQuestion`, which Claude Code delivers as `elicitation_dialog`). A hard block = the agent has work it cannot proceed with until the user answers.
- **`idle_prompt` → preserve prior** (joining `auth_success`, `elicitation_response`, `elicitation_complete`). Not a hard `→ Idle`:
  - In the common case the prior state is `Idle` (the turn ended via `Stop`), so the rendered result is `Idle`. The live-idle wall drains.
  - If a `permission_prompt`/`elicitation_dialog` block is still pending and the user merely sat idle, "preserve prior" keeps the session correctly in `WaitingInput` — a subsequent idle nudge does not clobber a real block.

0004's typed-field model and its two-bucket structure are unchanged. One type changes buckets.

### Boundary: the conversational "your turn" case is out of scope (Axiom 1)

`idle_prompt` also fires when Claude ends a turn with a *question in prose* and waits for a reply. That case — "the agent asked you something but isn't blocked" — is genuinely "your turn," but the substrate **cannot observe it**: the question lives in the assistant's message text, and bowerbird never reads message content (Axiom 1: the substrate observes hook events; it does not interpret transcript content). Reclassifying `idle_prompt → transient` defaults this case to `Idle`. Surfacing it would require reading transcripts, which Axiom 1 forbids. It is parked as a known, accepted limitation — not a bug this design can fix without crossing the axiom.

## Consequences

- **Behavioral, additive-safe.** `WaitingInput` is *narrowed* — strictly fewer transitions into it. No new false-positive blocks are possible. Old presenters decoding with `#[serde(other)]` are unaffected (the state set is unchanged; only the frequency of `WaitingInput` drops). One `type: behavioral` entry in `docs/protocol-changelog.md`.
- **The deck's `WaitingInput` column becomes meaningful:** it now contains only sessions with work genuinely blocked on the user. Finished/idle sessions read `Idle`.
- **ADR 0004 is amended in part,** not superseded — its liveness probe, `Ended` state, `SessionEnded` event, and `PostToolUse → Working` refinement all stand. Only the `idle_prompt` row of its §3 table changes.
- **Risk — idle ghosts.** Pre-0004, stale `WaitingInput` was the symptom of dead sessions; that is now handled by the liveness probe → `Ended`, independently of this change. "Preserve prior" for `idle_prompt` cannot reintroduce stale `WaitingInput` for live sessions: a live session that never blocked has prior `Idle`/`Working`, never `WaitingInput`. The 5-minute stale-`Working` read-time fallback continues to cover dropped-`Stop` cases.
- **`Ended` is not preserved by a notification hook.** "Preserve prior" must not preserve `Ended`: a notification hook arriving for an `Ended` session is evidence the process is alive (Claude fired it), so it transitions `Ended → Idle`, consistent with ADR 0004's non-terminal-`Ended` contract. This applies to every preserve-prior notification type, not just `idle_prompt`; `idle_prompt` joining this branch is what made the case reachable via the most common stray hook. Pinned by `transition_from_ended_preserve_prior_notification_yields_idle`.
- **Existing rows are NOT auto-remediated; this is a future-event-only change.** The `transition` change affects only *future* projections, and "preserve prior" deliberately keeps a stored `WaitingInput` as `WaitingInput`. So rows the *pre-5.6* daemon wrote as `WaitingInput` from an `idle_prompt` stay stuck after deploy until the session receives a non-idle hook (or the liveness probe ends a dead process). A code-review pass (2026-05-29) prototyped a one-time startup reprojection-repair to drain them, but the maintainer chose to **remove it** and handle existing rows operationally (manual DB fix or truncate the local `bower.db`) for V1. The repair was a *data* reprojection (no schema migration), and the deeper gap it exposed — bowerbird has no general mechanism for non-schema "data migrations" (backfills / reprojections gated on history completeness) — is tracked in `docs/bmad/implementation-artifacts/deferred-work.md`. A proper data-migration facility, with the truncation-safety guard the repair would have needed, is the right home for this rather than a one-off in Story 5.6.

## Alternatives considered

- **Hard `idle_prompt → Idle`** (unconditional). Rejected: it would clobber a still-pending `permission_prompt` block if the user sat idle long enough to trigger an idle nudge. "Preserve prior" gets the same Idle result in the normal case without that failure mode, and reuses 0004's existing transient bucket rather than adding a third rule.
- **Orthogonal `pending_input` flag on the projection** (separate from `current_state`). Already considered and rejected in ADR 0004 ("i don't like having latest notification on the projection. i want a slight interpolation of it"). This ADR stays within that agreed boundary — the typed-field classification *is* the slight interpolation; we only re-sort one type.
- **Read the transcript to distinguish "asked a question" from "just idle."** Rejected on Axiom 1 — the substrate does not interpret message content. See the Boundary section.
