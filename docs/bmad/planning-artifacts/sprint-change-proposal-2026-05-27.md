# Sprint Change Proposal — Session state projection correctness

Date: 2026-05-27
Author: pickles (via bmad-correct-course)
Status: Approved 2026-05-27
Related: `sprint-change-proposal-2026-05-26.md` (Epic 5 introduction), `epics.md` §Story 1.6 / §Story 2.2 / §Story 5.7 (this proposal), `docs/protocol.md:280`, `docs/protocol.md:334`, `docs/protocol.md:338`

## 1. Issue summary

Two coupled defects in the per-session state projection, surfaced during Epic 5 dogfooding while reviewing a sibling-repo presenter (pickletown's `/sessions` livestream page) against the just-shipped Epic 1+2+4 substrate.

### Defect A — State envelopes broadcast on every event

`crates/daemon/src/projection/session.rs:173` publishes one `BroadcastEnvelope::State` after every successful `projection::session::write`, unconditional on whether `current_state` actually changed. The intent in `docs/protocol.md:316` ("every session's state **changes**") and the wording of Story 2.2 both imply transitions-only emission. The implementation publishes on every event.

User-visible effect: presenter ribbon cards redraw on every `PreToolUse` / `PostToolUse` pair. Internally consistent (the state row's `last_event_at_ms` is updated and broadcast every time) but produces noisy re-renders that obscure real state changes.

### Defect B — PostToolUse → Idle is wrong; UserPromptSubmit is unwired

`crates/daemon/src/projection/state.rs:36` flips `current_state` to `Idle` on every `PostToolUse`. Combined with Defect A, this produces visible flapping: `Working → Idle → Working → Idle` cycling on each tool call. The semantic mistake is conflating "tool finished" with "agent done." The agent is alive between tool calls — composing the next call, thinking — but the state machine reads Idle during that gap.

Additionally, `crates/adapter-claude/src/install.rs:21` registers only four hooks (`PreToolUse`, `PostToolUse`, `Stop`, `Notification`), and `crates/adapter-claude/src/normalize.rs:69-72` only normalizes those four strings. Claude Code's `UserPromptSubmit` hook is never subscribed. The window between "user submits prompt" and "agent's first PreToolUse" therefore reads as Idle / WaitingInput when it should read as Working.

### Why this surfaced now

Epic 5 introduces a dogfooding validation phase deliberately positioned between substrate-completion and v0.1.0 tagging, specifically so that presenter-side wear surfaces correctness gaps in the substrate. The pickletown `/sessions` page subscribed to `state.session.*` and `events.claude.*` (the canonical multi-session router pattern from Story 4.2's cookbook entry) and the flapping was visible within the first observed Claude Code turn. Without the presenter, the defects would have shipped to v0.1.0 unchanged.

### Evidence

- `crates/daemon/src/projection/state.rs:36` — `EventKind::PostToolUse => SessionCurrentState::Idle` (the wrong transition)
- `crates/daemon/src/projection/session.rs:172-177` — `broadcaster.publish(BroadcastEnvelope::State { ... })` unconditional on state change
- `crates/adapter-claude/src/install.rs:21` — `HOOK_KINDS: &[&str] = &["PreToolUse", "PostToolUse", "Stop", "Notification"]` (UserPromptSubmit missing)
- `crates/adapter-claude/src/normalize.rs:69-72` — only those four hook strings mapped to EventKinds
- `docs/protocol.md:280` says "Emitted after every projection write" — `docs/protocol.md:316` says "every session's state **changes**" — same document contradicts itself
- The 5-minute read-time stale-Working fallback at `crates/daemon/src/projection/state.rs:19` (`STALE_WORKING_MS`, Story 1.6 AC #1) already exists and is the correct safety net for dropped hooks; the fix below relies on it more without changing it

## 2. Impact analysis

### Epic impact

- **Epic 1, Epic 2 (closed, retro'd)**: Original Story 1.6 defined the state machine; Story 2.2 wired the broadcast. Neither epic reopens — the remediation lands forward in Epic 5.
- **Epic 5 (in planning)**: One new story inserted before the v0.1.0 tag story (projection correctness must precede tagging). Existing Story 5.7 renumbers to 5.8.

### Story impact

| Story | Action |
|---|---|
| 5.6 (First-time-reader docs pass) | no change |
| **new 5.7 (Session state projection correctness)** | new story inserted |
| 5.7 → 5.8 (Crates.io namespace + v0.1.0 tag) | renumber only; AC text unchanged except the closing-condition reference "5.1 through 5.6" → "5.1 through 5.7" |

### Artifact conflicts

| Artifact | Touch | Why |
|---|---|---|
| `epics.md` (Story 5.7 insert, 5.7→5.8 renumber) | add + renumber | sequence the fix before v0.1.0 |
| `docs/bmad/implementation-artifacts/sprint-status.yaml` | insert + renumber | match epics.md ordering |
| `prd.md:206` | tighten | Marcus narrative's "goes green when Claude finishes" is ambiguous (could read as "tool finishes") — make it "finishes the turn" |
| `architecture.md:50, :1026` | amend | "no stuck state on missing PostToolUse" → "no stuck state on missing PostToolUse OR Stop"; the 5-min fallback now backstops both |
| `docs/protocol.md:280` | rewrite | broadcast rule moves from "every projection write" to "every `current_state` transition (plus snapshot on subscribe)" |
| `docs/protocol.md:334` | extend | `hook_kind` list adds `UserPromptSubmit` |
| `docs/protocol.md:338-352` | extend | EventKind table adds `UserPromptSubmit` row; intro changes "seven values" → "eight" |
| `docs/protocol-changelog.md` | add two entries | (a) behavioral: state-broadcast tightened to transitions-only; (b) schema: `UserPromptSubmit` EventKind variant added |

### Technical impact

- `crates/protocol/src/event.rs` — new `EventKind::UserPromptSubmit` variant; v1.0 presenters decode it as `EventKind::Unknown` via Story 4.4's `#[serde(other)]` catch-all (backward-compatible)
- `crates/adapter-claude/src/install.rs` — `HOOK_KINDS` gains a fifth entry; install/uninstall round-trip needs a contract test update
- `crates/adapter-claude/src/normalize.rs` — new mapping
- `crates/daemon/src/projection/state.rs` — `transition()` rule table changes (PostToolUse arm preserves prev; UserPromptSubmit arm returns Working); existing 5-min STALE_WORKING_MS read-time fallback unchanged
- `crates/daemon/src/projection/session.rs` — `write()` compares prev vs new `current_state` and conditionally publishes the `State` envelope; the `Event` envelope publish is unchanged
- `crates/protocol/tests/contract_protocol.rs` and `crates/daemon/tests/contract_daemon.rs` — updated to assert the new state-machine rules and the new variant
- No CI workflow changes; no infrastructure changes; no deployment surface changes

### Out-of-scope (deliberately deferred)

- Story 1.6 retro-rewriting: the AC text for Story 1.6 in `epics.md` is preserved as historical record; the new story is the authoritative current rule
- Per-session dropped-frame counter, drill-down view, REST event-history backfill on `/sessions` page load — these were already documented out-of-scope phase-1 cuts in `docs/superpowers/specs/2026-05-26-bowerbird-sessions-livestream-design.md` (pickletown side, not bowerbird)

## 3. Recommended approach

**Selected: Option 1 — Direct Adjustment.** Add one new Story 5.7 in Epic 5; renumber existing 5.7 → 5.8; update planning artifacts and protocol docs in lockstep.

| Option considered | Verdict |
|---|---|
| 1. Direct Adjustment (new story in Epic 5) | ✅ Viable, low risk, low effort (~1 PR, ~10 file edits, 5–8 contract test updates) |
| 2. Rollback Story 1.6 + 2.2 | ❌ Not viable — reverts most of the substrate |
| 3. PRD MVP review / scope reduction | ❌ Not needed — defect fix, not scope reduction |

**Rationale:** The defects are localized to two files (projection state machine + projection write publish) plus the adapter-claude hook registration. Both protocol-side changes are backward-compatible by construction (Story 4.4's `Unknown` catch-all absorbs the new variant; broadcast tightening only reduces message volume). MVP scope intact. Single story bundles all three fixes because they share a single user-facing outcome (ribbon cards no longer flap) and shipping any subset would leave a still-visible defect.

Effort: **Low** (~1 day implementation, ~0.5 day docs + contract tests). Risk: **Low** (Story 4.4's compatibility contract already covers the protocol shape; the 5-min stale-Working fallback already exists). Timeline impact: **+1 story to Epic 5**, no critical path delay.

## 4. Detailed change proposals

### 4.1 New Story 5.7 in `epics.md`

Insert between current Story 5.6 and current Story 5.7. Renumber existing 5.7 → 5.8.

```
### Story 5.7: Session state projection correctness

As a presenter author,
I want session-state broadcasts to fire only on actual `current_state`
transitions, and Working signals to cover the agent's full active period
(user prompt submission through tool completion — not just PreToolUse
moments),
So that ribbon UIs render only on meaningful state changes — no flap
between back-to-back tool calls, no false Idle gap during the agent's
between-tool thinking, no false Idle gap while the agent composes its
first tool call after a user prompt.

Closes the dogfooding finding in sprint-change-proposal-2026-05-27.md.

Acceptance Criteria:

Given a session in Working and an incoming PostToolUse event
When the projection writes the new state
Then `last_event_kind` and `last_event_at_ms` are updated AND
`current_state` remains Working (not Idle); subscribers to
`state.session.*` and `state.session.<id>` receive NO `state` envelope
for this event; subscribers to `events.*` still receive the `event`
envelope

Given N back-to-back PreToolUse/PostToolUse pairs for one session
When the events are ingested
Then subscribers to `state.session.*` receive exactly one `state`
envelope (the first PreToolUse's Idle→Working); subscribers to
`events.*` receive 2N event envelopes; `last_event_at_ms` still updates
on every PostToolUse

Given Claude Code running with bowerbird installed
When the user submits a prompt
Then UserPromptSubmit hook fires; the daemon ingests it; the
EventEnvelope has `kind=UserPromptSubmit`; `current_state` transitions
to Working (or remains Working); `last_event_at_ms` updates

Given a fresh `bowerbird install` against a Claude Code settings file
with no prior hooks
When installation completes
Then ~/.claude/settings.json registers five hooks (PreToolUse,
PostToolUse, Stop, Notification, UserPromptSubmit); `bowerbird
uninstall` removes all five; an existing install that pre-dates Story
5.7 surfaces "re-run `bowerbird install` to subscribe UserPromptSubmit"
when old-style hooks are detected

Given a v1.0 presenter compiled against the pre-Story-5.7 protocol enum
When it receives an event with `kind: "UserPromptSubmit"` from a
Story-5.7+ daemon
Then serde decodes it as `EventKind::Unknown` (Story 4.4 catch-all); no
crash, no panic, no protocol-violation close frame

Given crates/daemon/src/projection/state.rs after Story 5.7
When `transition()` is called with each EventKind variant
Then PostToolUse preserves prev.current_state; UserPromptSubmit returns
Working; PreToolUse returns Working; Stop returns Idle; Notification
returns WaitingInput; RecordingStarted/Ended/Unknown preserve prev
(unchanged); the 5-minute STALE_WORKING_MS fallback is unchanged and now
backstops both missing-Stop and missing-PostToolUse

Given the protocol surface
When Story 5.7 lands
Then crates/protocol/src/event.rs EventKind gains UserPromptSubmit;
crates/adapter-claude/src/normalize.rs maps the string; HOOK_KINDS in
crates/adapter-claude/src/install.rs adds it

Given the doc and contract-test surface
When Story 5.7 lands
Then docs/protocol.md:280 rewrites the broadcast emission rule to
transitions-only; docs/protocol.md:334 and :338 add UserPromptSubmit;
docs/protocol-changelog.md gains two entries (behavioral: tighten state
broadcast to transitions-only; schema: UserPromptSubmit EventKind);
contract_protocol.rs and contract_daemon.rs are updated for both rules

Given the planning artifacts
When Story 5.7 lands
Then prd.md:206 tightens "goes green when Claude finishes" to "goes
green when Claude finishes the turn"; architecture.md:50 and :1026
amend "no stuck state on missing PostToolUse" to "no stuck state on
missing PostToolUse or Stop"
```

### 4.2 Existing Story 5.7 → 5.8 renumber

Header change: `### Story 5.7: Crates.io namespace decision and v0.1.0 tag` → `### Story 5.8: Crates.io namespace decision and v0.1.0 tag`. AC closing condition `5.1 through 5.6` → `5.1 through 5.7`. All other AC text unchanged.

### 4.3 `sprint-status.yaml`

```diff
   5-6-first-time-reader-docs-pass: backlog
-  5-7-crates-io-namespace-and-v0-1-0-tag: backlog
+  5-7-session-state-projection-correctness: backlog
+  5-8-crates-io-namespace-and-v0-1-0-tag: backlog
   epic-5-retrospective: optional
```

Plus an additional `last_updated` line at the top:

```
# last_updated: 2026-05-27 (Story 5.7 session-state-projection-correctness inserted; old 5.7→5.8 per sprint-change-proposal-2026-05-27.md)
```

### 4.4 `prd.md:206`

```diff
-...He triggers a Claude Code tool call. The dot goes yellow. It goes green when Claude finishes.
+...He triggers a Claude Code tool call. The dot goes yellow. It goes green when Claude finishes the turn.
```

### 4.5 `architecture.md:49-50`

```diff
 - **Session Tracking (FR24–FR26):** `(source, session_id)` composite key;
   per-session projection; hook-unreliability tolerance (no stuck state on
-  missing `PostToolUse`).
+  missing `PostToolUse` or `Stop`; 5-minute read-time stale-Working
+  fallback backstops both).
```

### 4.6 `architecture.md:1026`

```diff
-| FR24–FR26: Session tracking | projection/session.rs UPSERT; no stuck state on missing PostToolUse ✅ |
+| FR24–FR26: Session tracking | projection/session.rs UPSERT; no stuck state on missing PostToolUse or Stop (5-min stale-Working fallback) ✅ |
```

### 4.7 `docs/protocol.md:280`

```diff
-Emitted after every projection write AND as a snapshot on subscribe to
-any `state.*` topic (Story 2.3). Snapshot frames apply the read-time
-stale-`Working` → `Idle` fallback...
+Emitted (a) on every `current_state` transition resulting from a
+projection write, and (b) as a snapshot on subscribe to any `state.*`
+topic (Story 2.3). Projection writes that update `last_event_kind` or
+`last_event_at_ms` without changing `current_state` produce no live
+`state` envelope — presenters compute freshness from the `events.*`
+stream. Snapshot frames apply the read-time stale-`Working` → `Idle`
+fallback...
```

### 4.8 `docs/protocol.md:334`

```diff
-- **`hook_kind` requirement** (Story 1.8). Every ingest line MUST carry
-  a `hook_kind` field whose value is one of `PreToolUse`, `PostToolUse`,
-  `Stop`, `Notification`.
+- **`hook_kind` requirement** (Story 1.8, extended in Story 5.7). Every
+  ingest line MUST carry a `hook_kind` field whose value is one of
+  `PreToolUse`, `PostToolUse`, `Stop`, `Notification`, `UserPromptSubmit`.
```

### 4.9 `docs/protocol.md:338-352` (EventKind table)

```diff
 ## EventKind enum

-The seven values from [`crates/protocol/src/event.rs:9`](...):
+The eight values from [`crates/protocol/src/event.rs:9`](...):

 | Value | User-facing? | Meaning |
 |---|---|---|
+| `UserPromptSubmit` | yes | The user submitted a prompt; Claude is about to start a turn |
 | `PreToolUse` | yes | Claude is about to invoke a tool |
 | `PostToolUse` | yes | A tool invocation completed |
 | `Stop` | yes | Claude finished a turn |
 | `Notification` | yes | A non-tool side-channel event (e.g. permission prompt) |
 | `RecordingStarted` | **no — internal sentinel** | Daemon started a recording session |
 | `RecordingEnded` | **no — internal sentinel** | Daemon ended a recording session |
 | `Unknown` | **decode-only catch-all** | ... |
```

### 4.10 `docs/protocol-changelog.md` — two new entries under v1.0 → v1.1

```
- **type: behavioral** — State-broadcast emission tightened to
  `current_state`-transitions-only (Story 5.7). Previously every
  successful `projection::session::write` emitted one
  `BroadcastEnvelope::State` after the SQLite commit (Story 2.2). As of
  this release, the daemon compares the prior and new `SessionState`
  records read in the same transaction and emits a `State` envelope only
  when `current_state` differs; writes that update `last_event_kind` or
  `last_event_at_ms` without a state transition produce zero `State`
  envelopes (the `Event` envelope is unchanged — every event still
  publishes one). Snapshot-on-subscribe semantics (Story 2.3) are
  unchanged: `state.*` subscribers still receive an initial snapshot
  frame per matching session before live frames begin. Presenters that
  compute time-since-last-event from `state.session.*` must move that
  computation to the `events.*` stream (the `last_event_at_ms` field on
  the state row remains accurate for REST responses and snapshots, but
  is no longer pushed on every event). Backward-compatible: v1.0
  presenters receive strictly fewer state envelopes; no schema change;
  no new error condition. Resolves the Story 5.7
  sprint-change-proposal-2026-05-27.md dogfooding finding about
  ribbon-card flap.

- **type: schema** — `UserPromptSubmit` `EventKind` variant added and
  subscribed (Story 5.7). New variant in `crates/protocol/src/event.rs`
  emitted when the user submits a prompt to Claude Code; the
  `adapter-claude` crate now subscribes the `UserPromptSubmit` hook
  (Claude Code hook kind of the same name) and normalizes it in
  `crates/adapter-claude/src/normalize.rs`. `bowerbird install` writes
  five hook entries to `~/.claude/settings.json` (was four);
  `bowerbird uninstall` removes all five; the install command detects
  pre-5.7 four-hook installs and surfaces a re-run hint. State-machine
  semantics in `crates/daemon/src/projection/state.rs`: the
  `UserPromptSubmit` arm of `transition()` returns
  `SessionCurrentState::Working`. v1.0 presenters compiled against the
  pre-5.7 protocol enum decode `kind: "UserPromptSubmit"` as
  `EventKind::Unknown` via the Story 4.4 `#[serde(other)]` catch-all —
  the event still parses and the connection stays open. Additive within
  v1.x. Resolves the Story 5.7 sprint-change-proposal-2026-05-27.md
  dogfooding finding about user-prompt-to-first-tool gap reading Idle.
```

## 5. Implementation handoff

**Scope classification: Moderate.** One new story; planning-artifact updates; protocol-changelog entries; contract-test updates. Not "minor" because the protocol-changelog gate (`tests/protocol_changelog_gate.rs`) will enforce the two new entries and the schema/behavioral category labels need to be correct. Not "major" because no epic-level restructure and no MVP scope change.

**Routing:**
- **Story-automator** picks up Story 5.7 next via `bmad-create-story` and produces `docs/bmad/implementation-artifacts/5-7-session-state-projection-correctness.md` (the implementation story file). All edits in §4.3–§4.10 above land in the same PR as the implementation work to satisfy AC text and keep the protocol-changelog CI gate green.
- **Developer agent** implements the four code changes (state.rs, session.rs, install.rs, normalize.rs, event.rs), updates contract tests, runs `cargo test --workspace -- --test-threads=1`, and submits the PR.
- **No PM/Architect involvement needed.** PRD narrative and architecture coverage table are mechanical wording fixes already specified in §4.4–§4.6.

**Success criteria (definition of done):**
1. Per Story 5.7's ten ACs, all green
2. `cargo test --workspace` passes (including the protocol-changelog gate at `tests/protocol_changelog_gate.rs`)
3. `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` pass
4. The pickletown `/sessions` page, run against the post-5.7 daemon, no longer shows ribbon-card flap during a Claude Code session with multiple tool calls (manual smoke; out-of-scope for bowerbird CI but the originating dogfooding signal)
5. `bowerbird uninstall && bowerbird install` against a pre-5.7 settings file results in all five hook entries present

## 6. Approval

Sign-off from pickles via bmad-correct-course interactive review (2026-05-27).

Approved: [x]
Date: 2026-05-27
