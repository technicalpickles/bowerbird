# 0004. Daemon-observed session liveness (supersedes Story 5.3's "presenter does `kill()`")

Date: 2026-05-27
Status: Accepted (§3 `idle_prompt` row amended by ADR 0005, 2026-05-29 — `idle_prompt` reclassified from input-required to transient/preserve-prior)
Deciders: @pickles
Related: ADR-0001 (no conflict); sprint-change-proposal-2026-05-27-pid-liveness.md (this ADR amends its presenter-side approach); Story 1.6 (`docs/bmad/implementation-artifacts/1-6-session-projection-and-hook-unreliability-tolerance.md`); Story 5.1 dogfooding (`docs/bmad/implementation-artifacts/5-1-first-party-presenter-tool.md`); Story 5.2 (the PostToolUse "preserve prior" rule this ADR refines)
Implementation: `crates/daemon/src/projection/state.rs`, `crates/daemon/src/main.rs` (new probe task), `crates/protocol/src/state.rs` (new `Ended` variant), `crates/protocol/src/event.rs` (new `SessionEnded` variant), `crates/adapter-claude/src/normalize.rs` (extract `notification_type`)
Affects sections: `docs/protocol.md` §`SessionCurrentState` and §`EventKind`; `docs/presenter-authoring.md` §State subscription; `docs/protocol-changelog.md` (three new entries — see Consequences)

## Context

Three defects surfaced during Story 5.1 dogfooding of `bowerbird-deck` (the first-party presenter, in-progress 2026-05-27):

1. **Accumulating `WaitingInput` ghosts.** The deck showed ~48 sessions as `WaitingInput`, all >10 min stale, most >1h, several >24h. None were actually waiting for the maintainer's input — they were sessions whose terminals had been closed without firing `Stop`, frozen on the last `Notification` event they emitted. Root cause: Story 1.6's blind `Notification → WaitingInput` collapse plus the absence of a stale-`WaitingInput` fallback. See `crates/daemon/src/projection/state.rs:54`, `:233`.

2. **No mechanical signal for "session process is gone."** `EventKind` does not have a `SessionEnd` variant; Claude Code's `Stop` hook fires at end-of-turn, not on process exit, so a session whose terminal closes mid-turn has no closing event. Sprint-change-proposal-2026-05-27-pid-liveness.md (approved as Story 5.3) addressed this by adding a `last_pid: Option<i32>` field to the projection and asking presenters to call `kill(pid, 0)` themselves.

3. **Notification semantics are richer than the substrate reflects.** Claude Code's `Notification` payload carries a typed `notification_type` field with six documented values (`permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_response`, `elicitation_complete`). The substrate stores the field verbatim in `events.payload` but discards it at projection time, then substitutes a one-bit "input required" conclusion that is wrong ~half the time (60% of real Notifications in the operating corpus are `idle_prompt`, which is the *least* actionable of the input-required types). Story 1.6's design rationale explicitly punted finer-grained semantics to presenters; that punt has not aged well — every presenter would have to reinvent the same classification using the same six values.

During Story 5.1 dogfooding the maintainer surfaced an axiom-level question: **Story 5.3's presenter-side `kill()` approach is inconsistent with how the substrate handles other mechanical facts.** Hook firings are observed by the daemon and broadcast as events; process death is also a mechanical fact, but the approved 5.3 design pushes its observation into every presenter. The reframe: *observing process death IS a mechanical fact, equivalent in nature to observing a hook firing. The semantic ("should this session be rendered?") still lives in the presenter, but the observation belongs in the substrate.*

This ADR resolves all three defects with one design move: the daemon owns liveness probing, emits a new mechanical event when it observes process death, and uses Claude's typed `notification_type` field to drive a small, inspectable `WaitingInput` rule.

## Decision

**The daemon observes; presenters interpret. Specifically:**

### 1. New protocol shapes

- `SessionCurrentState::Ended` — daemon-observed condition: the session's `last_pid` is no longer alive. **Not terminal** — a session can transition out of `Ended` via any standard hook event (typically a `UserPromptSubmit` from `claude --resume <session_id>`). The state name describes the current observation, not a permanent verdict.
- `EventKind::SessionEnded` — daemon-emitted, per-session (`source = "claude", session_id = <real>`), broadcast to presenters via the normal `events.*` WS topic. Payload carries `{"reason": "pid_dead" | "no_pid_at_upgrade", "pid": <last_pid or null>, "observed_at_ms": <epoch_ms>}`.

Old presenters decoding with `#[serde(other)]` see `Ended` as `Unknown` and `SessionEnded` events as `Unknown`-kind — additive-compat per the Story 4.4 catch-all pattern.

### 2. Daemon-side liveness probe

- Background tokio task, `tokio::time::interval` with 5-second cadence and `MissedTickBehavior::Skip` to prevent tick queueing under transient slowness.
- One iteration runs **synchronously at startup**, after `run_migrations` and `rebuild_missing_projections`, **before** the WS server accepts connections. Presenters connecting any time after startup get correct state in their snapshot.
- Per-iteration logic: for each row in `session_projections`, if `last_pid IS NULL OR kill(last_pid, 0) != 0` → write a `SessionEnded` event with `observed_at_ms = now()` and drive the projection transition `→ Ended`. Otherwise no-op.
- The `last_pid IS NULL` branch is a one-time cleanup effect: pre-Story-5.3 rows have no PID at all (the column didn't exist when they were written), so first boot post-upgrade emits a synthetic `SessionEnded` for each one with `reason: "no_pid_at_upgrade"`. Future boots find none of these.

### 3. WaitingInput is typed-field-driven

The adapter extracts `notification_type` from the `Notification` payload at `crates/adapter-claude/src/normalize.rs`, passes it into the projection's `transition` function. New per-type rules in `projection/state.rs`:

| `notification_type` | Transition |
|---|---|
| `permission_prompt` | → `WaitingInput` |
| `idle_prompt` | → preserve prior (transient) — *amended by ADR 0005, 2026-05-29* |
| `elicitation_dialog` | → `WaitingInput` |
| `auth_success` | preserve prior |
| `elicitation_response` | preserve prior |
| `elicitation_complete` | preserve prior |
| *(unknown future type)* | preserve prior |

The events table needs no new column; `notification_type` stays in `events.payload` for archaeology.

### 4. PostToolUse refines from "preserve prior" to "→ Working"

Story 5.2's "PostToolUse preserves prior" rule was correct in spirit (the agent is alive between tool calls) but generated a bug under the new design: a session in `WaitingInput` whose tool call completed (e.g., elicitation_dialog → user responded → tool resumed → PostToolUse) would stay `WaitingInput` instead of transitioning back to `Working`. The fix is to say what Story 5.2 actually meant: **PostToolUse → Working unconditionally**. Tool activity = agent is in active state. The previous `WaitingInput + PostToolUse → WaitingInput` behavior was an unintended consequence of the "preserve" implementation choice.

### 5. No `STALE_WAITING_INPUT_MS` substrate fallback

Story 1.6's `STALE_WORKING_MS` (5 minutes, `Working → Idle`) existed because there was no mechanical signal for "the session is dead." This ADR provides that signal (`SessionEnded`). A symmetric `STALE_WAITING_INPUT_MS` would re-introduce the kind of time-based interpretation the new design eliminates. Stale rows are handled by liveness, not by a clock.

`STALE_WORKING_MS` itself is **retained** for now — it predates this ADR and removing it is independent work. Track as deferred.

## Why this is consistent with Axioms 1 and 4

Axiom 1 (architecture.md:498): *"the substrate observes; it does not interpret."*
Axiom 4 (sprint-change-proposal-2026-05-27-pid-liveness.md:22): *"mechanical facts in the protocol; semantics in the presenter."*

At first reading, "daemon runs a background probe that emits events" sounds like an Axiom 1 violation. The reframe that resolves it:

> **Observing process death is a mechanical fact, equivalent in nature to observing a hook firing.** `kill(pid, 0)` is a syscall whose return code is determined by the kernel's process table — there is no interpretation in checking it. The semantic ("should I render this session?", "is this session worth my attention?") still lives in the presenter, who combines `current_state`, `last_event_at_ms`, and its own UI policy to decide what to show.

Under this refinement:

- **Mechanical (substrate):** hook firings (observation: "Claude wrote bytes to the ingest socket"), process death (observation: "kill(pid, 0) returned ESRCH"), notification type (observation: "Claude's payload had `notification_type: idle_prompt`").
- **Semantic (presenter):** "should I switch to this session?", "is this row worth screen space?", "show with what color or symbol?".

The previous Story 1.6 mapping `Notification → WaitingInput` was an axiom violation hiding in plain sight: it threw away Claude's typed `notification_type` field and substituted a one-bit conclusion. The new typed-field rules are actually *more* faithful to Axiom 1 — they use what Claude told us instead of inventing our own one-bit summary.

## Alternatives considered

- **Story 5.3 as originally approved (presenter-side `kill()`).** Requires every presenter to implement its own liveness loop. Breaks for cross-machine consumers: a presenter on machine B watching a daemon on machine A *cannot* `kill()` machine A's PIDs. Adds duplicate work proportional to presenter count. Rejected.

- **Orthogonal `pending_input` flag on the projection** (instead of folding into `current_state`). Keeps `current_state` purely execution-axis (`Idle`/`Working`/`Ended`), surfaces input-required as a separate `Option<{at_ms, notification_type}>` field. The maintainer rejected putting `latest_notification` on the projection ("i don't like having latest notification on the projection. i want a slight interpolation of it") — the typed-field classification is the agreed-upon "slight interpolation" boundary. The orthogonal model is technically defensible but requires every presenter to compose two axes instead of one.

- **Synchronous probe in the ingest write path** (instead of a background task). Cheap (no extra task to manage), but means probe latency tracks event arrival — a long-idle session never gets re-probed because no events trigger it. Defeats the purpose. Rejected.

- **Cadence shorter than 5s** (e.g., 1s). `kill(pid, 0)` cost is empirically zero (microsecond-scale syscall) so 1s is feasible. But 5s already gives ≤5s ghost-lifetime in the deck, which is comfortably below ambient-awareness perception thresholds. The marginal value of 1s freshness doesn't justify 5× the per-tick work as the cluster scales. Defer to a tuning revisit if dogfooding surfaces a real need.

- **Cadence longer than 5s** (e.g., 30s). Stale ghosts visible for up to half a minute in the deck. The user-affordance "is there something for me to do here?" degrades noticeably at this lag. Rejected.

- **`EventKind::ProcessGone` + `SessionCurrentState::Dead`** (process-level vocabulary). Honest about what's mechanically observed (a PID disappeared), but ties the wire shape to PID-based detection, which is Claude-CLI-specific. A future adapter (browser-based agent, WebSocket-only agent) might not have PIDs at all; the substrate's wire shape shouldn't presume one. Rejected in favor of session-level vocabulary (`SessionEnded`/`Ended`).

- **`Stop`-driven death detection.** Re-use the existing `Stop` event by having the daemon emit it when a PID disappears. Conflates "agent finished its turn cleanly" with "agent process disappeared" — different events, different presenter affordances. Rejected.

## Consequences

- **Sprint-change-proposal-2026-05-27-pid-liveness.md is partially superseded.** Its `last_pid: i32` capture stays; its presenter-side `kill()` recipe is replaced by the substrate-side probe described here. Either amend the existing proposal in-place with a "2026-05-27 update" header, or supersede it with a new proposal that references this ADR.

- **`protocol-changelog.md` adds three new entries:**
  1. `type: schema` — new `EventKind::SessionEnded` variant, new `SessionCurrentState::Ended` variant.
  2. `type: behavior` — `WaitingInput` semantics sharpened: now driven by `notification_type` classification rather than blind `Notification → WaitingInput`. Old presenters see fewer false-positive `WaitingInput` events.
  3. `type: behavior` — `PostToolUse → Working` refinement of Story 5.2's "preserve prior."

- **A new implementation story** captures the transition-table changes (item 3 in the changelog). Could fold into the amended 5.3 if scope permits; cleaner as its own story since notification-typed rules are independent of liveness mechanics.

- **`docs/protocol.md`** needs updates at the `SessionCurrentState` section (add `Ended`, note it is non-terminal), the `EventKind` section (add `SessionEnded`), and a new note that `Notification` events carry `notification_type` that drives projection state. `docs/presenter-authoring.md` needs a new section on `Ended` rendering (recommended: hide entirely) and a "no `kill()` in presenters" guidance.

- **`bowerbird-deck` simplifies:** no `kill()` loop, no `setInterval` for liveness, no PID-recycling logic. Just subscribe to `state.session.*` and render. Story 5.1's dogfooding log will reference this ADR as a hotfix-class friction that resolved during the dogfooding window — exactly the signal Epic 5 was designed to surface.

- **The maintainer's accumulated DB drift** (the original 48-row ghost problem) resolves automatically on first boot after the upgrade: the eager startup probe emits 48 synthetic `SessionEnded` events; the projection transitions all 48 to `Ended`; the deck removes them from the rendered list. From "30+ stale rows" to "the handful of actually-alive sessions" in one boot cycle.

## Revisit when

- A second adapter (Codex, Gemini, Cursor) lands and its session-lifecycle signal is something other than PID death. The session-level vocabulary (`SessionEnded` rather than `ProcessGone`) was chosen to absorb this case, but the *probe implementation* in the daemon is PID-based today. A non-PID adapter would need its own liveness mechanism feeding the same `SessionEnded` event.

- Notification type taxonomy expands meaningfully. The MCP elicitation types may grow as MCP usage matures; if a future Claude Code release adds a notification_type that doesn't fit cleanly into "input-required" or "transient," the per-type rules in §3 need a deliberate decision rather than the "preserve prior" default.

- Cross-machine deployment becomes real. The 5s probe cadence is sized for a single-developer workstation. A daemon serving many remote presenters might want a slower cadence (less CPU on the daemon side) or a different probe model entirely (e.g., presenters declaring interest in specific sessions).

- `STALE_WORKING_MS` is finally retired. This ADR notes the carve-out; a future cleanup story should consider whether the 5-minute Working-decay rule is still load-bearing once liveness exists. Likely answer: it's not, since a long-running tool call that drops `PostToolUse` will also eventually be caught by the death probe when the user closes the terminal. But that's a separate decision.
