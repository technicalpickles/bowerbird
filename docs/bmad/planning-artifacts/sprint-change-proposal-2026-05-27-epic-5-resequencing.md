# Sprint Change Proposal — Epic 5 resequencing for dogfooding-first ordering

Date: 2026-05-27
Author: pickles (via bmad-correct-course)
Status: Approved 2026-05-27
Related: `sprint-change-proposal-2026-05-26.md` (Epic 5 introduction), `sprint-change-proposal-2026-05-27.md` (Story 5.7 projection correctness insert — now Story 5.2), `sprint-change-proposal-2026-05-27-pid-liveness.md` (Story 5.8 PID liveness insert — now Story 5.3), `epics.md` §Epic 5

## 1. Issue summary

Epic 5 as drafted opens with Story 5.1 (presenter) and then sequences CI/release hardening (5.2 bench gates, 5.3 release E2E) and install/cookbook/docs polish (5.4, 5.5, 5.6) *before* the two dogfooding-surfaced correctness fixes (5.7 projection correctness, 5.8 PID liveness). That ordering frustrates Epic 5's stated intent — the maintainer is supposed to be using the presenter daily and harvesting friction (`epics.md:895`), but until the correctness fixes ship:

- **Without 5.7**, the deck ribbon flaps `Working → Idle → Working → Idle` on every tool call (per `sprint-change-proposal-2026-05-27.md`). The presenter is internally honest but visually unusable as a daily signal.
- **Without 5.8**, the deck session list shows 30+ tombstone sessions stretching back 25+ hours with no way to mark dead-process rows (per `sprint-change-proposal-2026-05-27-pid-liveness.md`). The session list is unusable as a list.

Running 6–8 weeks of bench-gate work, release-pipeline verification, install UX polish, cookbook consolidation, and first-time-reader docs work BEFORE making the presenter actually useful inverts the dogfooding loop the epic exists to operate. Bench gates and release polish don't need a working presenter; the dogfooding correctness fixes do.

## 2. Impact analysis

### Epic impact

Epic 5 is reordered, not rescoped. All eight planned stories (plus the closing tag) remain. No story is added, removed, or modified in content — only in sequence.

### Story impact — permutation of 5.2 through 5.8

| Old # | Story | New # | Rationale |
|---|---|---|---|
| 5.1 | First-party presenter tool | 5.1 | In-progress; unchanged |
| **5.7** | **Session state projection correctness** | **5.2** | Dogfood-surfaced; without it the ribbon flaps every tool call. Belongs adjacent to 5.1 |
| **5.8** | **Session-process liveness via PID capture** | **5.3** | Dogfood-surfaced; without it the session list is unusable. Belongs adjacent to 5.1 |
| 5.4 | Install UX polish + middleware closure | 5.4 | Mostly independent; affects re-install friction but not daily-use signal quality. Unchanged slot |
| **5.2** | **Bench gates load-bearing** | **5.5** | CI signal quality. Important for the tag but not for daily dogfooding |
| **5.3** | **Release pipeline end-to-end verification** | **5.6** | Required for the tag. Doesn't affect daily dogfooding |
| **5.5** | **Cookbook consolidation** | **5.7** | Reader-facing. Not load-bearing for the maintainer |
| **5.6** | **First-time-reader docs pass** | **5.8** | Reader-facing. Not load-bearing for the maintainer |
| 5.9 | Crates.io + v0.1.0 tag | 5.9 | Closing story; unchanged |

### Artifact conflicts

| Artifact | Touch | Why |
|---|---|---|
| `epics.md` (Story headers + self-references + cross-references between stories) | renumber + cross-ref fix | seven story headers, all `When Story 5.X lands/closes/started` AC self-references, two cross-references: Story 5.8 (new 5.8) "Story 5.5" → "Story 5.7" cookbook reference, and Story 5.8 (new 5.8) "Story 5.8 tags it" → "Story 5.9 tags it" |
| `docs/bmad/implementation-artifacts/sprint-status.yaml` | renumber + last_updated | match epics.md ordering |
| `docs/protocol-changelog.md` | update prospective entries | the v1.0 → v1.1 section has three Story-5.7 entries and three Story-5.8 entries (added by the two 2026-05-27 proposals); these are prospective (not-yet-shipped) and should be renumbered to 5.2 and 5.3 respectively |
| `sprint-change-proposal-2026-05-27.md` (projection correctness) | prepend disambiguation note | proposal references "Story 5.7" but the story is now 5.2 |
| `sprint-change-proposal-2026-05-27-pid-liveness.md` (PID liveness) | prepend disambiguation note | proposal references "Story 5.8" but the story is now 5.3 |
| `prd.md`, `architecture.md`, `docs/protocol.md`, `deferred-work.md` | no change required | none of these reference Story 5.X numbers; the 5.7/5.8 AC text in epics.md *projects* into prd/architecture during implementation, but no story-number reference is in the planning artifacts today |

### Technical impact

None. No code touched. No wire-protocol change. No infrastructure change. Pure planning-artifact reordering.

### Out-of-scope (deliberately deferred)

- **Cross-references inside the historical proposal docs (2026-05-26 cookbook consolidation, 2026-05-26 Epic 5 introduction, 2026-05-27 projection correctness, 2026-05-27 PID liveness):** not retroactively rewritten. Each gets a top-of-file disambiguation note instead, preserving the historical decision record while pointing forward to the new numbering. This avoids revisionism (the proposals were approved at the older numbering) while disambiguating for future readers.
- **Story-file slugs in `docs/bmad/implementation-artifacts/`:** the existing `5-1-first-party-presenter-tool.md` keeps its filename (Story 5.1 unchanged). No other story files exist yet — they'll be created by `bmad-create-story` against the new numbering.

## 3. Recommended approach

**Selected: Option 1 — Direct Adjustment (resequencing).** No story added, removed, or modified in content. Renumber in place.

| Option considered | Verdict |
|---|---|
| 1. Direct Adjustment (renumber in place) | ✅ Viable, low risk, low effort. Zero code change |
| 2. Rollback completed work | ❌ N/A — only 5.1 is in-progress; no completed Epic 5 work to roll back |
| 3. PRD MVP review / scope reduction | ❌ Not needed — reorder, not rescope |
| 4. Keep prior order and accept the dogfooding inversion | ❌ Not viable — defeats Epic 5's stated purpose; the maintainer would spend 6+ weeks polishing CI/release for a substrate they aren't actually using as their daily signal |

**Rationale:** The dogfooding-correctness stories (projection correctness, PID liveness) are downstream of Story 5.1's discovery loop — both came in via `5.X-hotfix`-style proposals because the presenter surfaced gaps. Sequencing them adjacent to 5.1 means the maintainer can use the presenter as intended within the first few weeks of Epic 5, not at the end. The CI/release/docs work is sequenced after dogfooding stability is in place, so the v0.1.0 tag (5.9) ships against a substrate that's been continuously dogfooded — which is the entire point of Epic 5's dogfooding-validation-phase line in `sprint-status.yaml`.

Effort: **Low** (~30 minutes of mechanical renumbering across 4 files). Risk: **Low** (planning-artifact-only; reversible via git).

## 4. Detailed change proposals

### 4.1 `epics.md` — full Epic 5 renumber

Apply the permutation in §2 Story impact table. Specifically:

- Story header lines `### Story 5.X: <title>` updated to the new number for the seven moved stories (5.2, 5.3, 5.5, 5.6, 5.7, 5.8 — and stories that just shifted into those slots).
- All `**When** Story 5.X lands` / `**When** Story 5.X closes` / `**When** Story 5.X is started` AC text updated to the new number for that same story.
- Cross-references: in the new Story 5.2 (was 5.7) AC text, "compiled against the pre-Story-5.7 protocol enum" → "compiled against the pre-Story-5.2 protocol enum"; in the new Story 5.3 (was 5.8) AC text, "compiled against the pre-Story-5.8 protocol type" → "compiled against the pre-Story-5.3 protocol type"; also "Story 5.4's migration-idempotency contract test" in the new Story 5.3 AC text stays as Story 5.4 (the install-UX story did not move).
- In the new Story 5.8 (was 5.6, first-time-reader docs): "once Story 5.8 tags it" → "once Story 5.9 tags it" (the closing tag story is 5.9, not 5.8); "cross-references to the cookbook target the per-entry directory shape introduced by Story 5.5" → "...introduced by Story 5.7" (cookbook consolidation is now 5.7).
- In the new Story 5.9 (closing tag, was 5.9 with text already pointing at 5.7/5.5/5.4): "Story 5.2 AI-1/AI-2/AI-3, Story 5.4's five entries, Story 5.5's deferred-work-104 closure, Story 5.9's AI-3/AI-5" → "Story 5.5 AI-1/AI-2/AI-3, Story 5.4's five entries, Story 5.7's deferred-work-104 closure, Story 5.9's AI-3/AI-5" (bench gates moved from 5.2 to 5.5; cookbook moved from 5.5 to 5.7).
- New `revisions:` entry at the top of the file recording the 2026-05-27 resequencing.

### 4.2 `sprint-status.yaml` — full Epic 5 renumber

```diff
   5-1-first-party-presenter-tool: in-progress
-  5-2-bench-gates-load-bearing: backlog
-  5-3-release-pipeline-end-to-end-verification: backlog
+  5-2-session-state-projection-correctness: backlog
+  5-3-session-process-liveness-pid-capture: backlog
   5-4-install-ux-polish-and-middleware-closure: backlog
-  5-5-cookbook-consolidation: backlog
-  5-6-first-time-reader-docs-pass: backlog
-  5-7-session-state-projection-correctness: backlog
-  5-8-session-process-liveness-pid-capture: backlog
+  5-5-bench-gates-load-bearing: backlog
+  5-6-release-pipeline-end-to-end-verification: backlog
+  5-7-cookbook-consolidation: backlog
+  5-8-first-time-reader-docs-pass: backlog
   5-9-crates-io-namespace-and-v0-1-0-tag: backlog
   epic-5-retrospective: optional
```

Plus an additional `last_updated` line:

```
# last_updated: 2026-05-27 (Epic 5 resequenced for dogfooding-first ordering per sprint-change-proposal-2026-05-27-epic-5-resequencing.md)
```

### 4.3 `docs/protocol-changelog.md` — update prospective Story 5.7/5.8 entries

The v1.0 → v1.1 section has six prospective entries (none shipped yet):

- Three entries tagged `(Resolves: 5.7)` for projection correctness work → retag `(Resolves: 5.2)`.
- Three entries tagged `(Resolves: 5.8)` for PID liveness work → retag `(Resolves: 5.3)`.
- Inline references inside those entries to "Story 5.7" → "Story 5.2"; "Story 5.8" → "Story 5.3"; "pre-Story-5.7 protocol enum" → "pre-Story-5.2 protocol enum"; "pre-Story-5.8 protocol type" → "pre-Story-5.3 protocol type".

### 4.4 Disambiguation notes on prior 2026-05-27 proposals

Prepend a single boxed note at the top of each:

`sprint-change-proposal-2026-05-27.md` (projection correctness):

```
> **Note (2026-05-27 resequencing):** This proposal references "Story 5.7" throughout. The Epic 5 resequencing of 2026-05-27 (see `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`) renumbered the projection-correctness story to **Story 5.2**. The proposal text below is preserved verbatim from approval-time; read "Story 5.7" as "Story 5.2" when referring to current Epic 5.
```

`sprint-change-proposal-2026-05-27-pid-liveness.md` (PID liveness):

```
> **Note (2026-05-27 resequencing):** This proposal references "Story 5.8" throughout. The Epic 5 resequencing of 2026-05-27 (see `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`) renumbered the session-process-liveness story to **Story 5.3**. The proposal text below is preserved verbatim from approval-time; read "Story 5.8" as "Story 5.3" when referring to current Epic 5.
```

## 5. PRD MVP impact

None. Scope unchanged; sequence reordered.

## 6. Implementation handoff

**Scope classification: Minor.** Pure planning-artifact reordering; no code; no Story Dev Notes; no contract tests. Direct execution by the Developer agent.

**Success criteria:**

1. `epics.md` Epic 5 section reads top-to-bottom as: 5.1 presenter → 5.2 projection correctness → 5.3 PID liveness → 5.4 install UX → 5.5 bench gates → 5.6 release E2E → 5.7 cookbook → 5.8 first-time-reader docs → 5.9 v0.1.0 tag.
2. `sprint-status.yaml` Epic 5 keys mirror the new ordering.
3. All cross-references in `epics.md` AC text point at the correct new story numbers (no dangling "Story 5.7" / "Story 5.8" / "Story 5.5" inside other stories' AC text).
4. `protocol-changelog.md` prospective entries retagged.
5. Disambiguation notes on the two prior 2026-05-27 proposals.

## 7. Trade-offs and alternatives summary

The trade-off worth restating: leaving the dogfooding-correctness stories at the tail of Epic 5 would defer the maintainer's actual daily-use signal for the duration of CI/release/cookbook/docs work (~6 weeks of estimated effort across 5.2–5.6 of the prior numbering). The cost of that deferral is the dogfooding-validation-phase running against a broken-for-daily-use presenter, which inverts Epic 5's stated purpose. The cost of resequencing is one batch of planning-artifact edits and three disambiguation notes. The trade is clear; no alternatives considered are competitive.
