# Coherence Review: Phase 3 PRD Amendment (2026-08-02)

Reviewer scope: internal contradictions between the new Phase 3 material and the retained V1 record, amendment claims vs. document reality, FR40-FR44 quality, decision-log landing check, consolidation leftovers, and em-dash discipline in the newly added sections. Calibration: solo maintainer, "still an experiment" stakes, deliberately minimal update. Judged against the PRD quality rubric's coherence and done-ness dimensions, weighted for those stakes.

Files reviewed:

- `docs/bmad/planning-artifacts/prd.md`
- `docs/bmad/planning-artifacts/prds/prd-bowerbird-2026-08-02/addendum.md`
- `docs/bmad/planning-artifacts/prds/prd-bowerbird-2026-08-02/.decision-log.md`
- Rubric: `.claude/skills/bmad-prd/assets/prd-validation-checklist.md` (present, applied)

## Overall verdict

The amendment is coherent and lands nearly everything the decision log records. The V1-as-historical-record framing is handled honestly and explicitly (Journey 3's `brew install` is flagged as historical, Installation Methods carries the correction, the Growth-list consolidation leaves a pointer stub, the `sync` frame row documents reality). One real gap: the "API Surface (v1 Stable)" reference tables were not updated for the ADR 0008 filters that the Phase 3 material explicitly depends on, so the PRD's own wire reference contradicts FR40 and Journey B. Everything else found is low severity.

## 1. Internal contradictions (new Phase 3 material vs. retained V1 material)

### Finding 1.1 (MEDIUM): API Surface tables omit the shipped ADR 0008 filters that FR40 and Journey B cite as load-bearing

FR40 requires `session-glance` to consume "REST `GET /sessions?state=`", and Journey B's requirements paragraph says "REST `?state=` filters (shipped, Story 5.8)". But the "REST Endpoints" table still describes `GET /sessions` as plain "List known sessions" with no mention of `?state=` / `?since=` / `?limit=`. Same on the WS side: ADR 0008 (per the decision log) added an optional `states` field to `Subscribe`, and the addendum's live-only-flag analysis reasons about `states: []` semantics at length, yet the PRD's subscribe wire examples show only `op` / `topic` and the surrounding text describes strict `deny_unknown_fields` parsing with no `states` field documented anywhere in the PRD body.

Net effect: the section titled "API Surface (v1 Stable)" describes a pre-ADR-0008 surface, while the Phase 3 additions treat the post-ADR-0008 surface as shipped fact. A reader building `session-glance` from the PRD's own reference section would conclude `?state=` does not exist. This is exactly the staleness class the amendment set out to fix (it fixed install paths, the `sync` row, and project context) and this one was missed.

Fix: add the three query params to the `GET /sessions` row and the optional `states` field to the subscribe message documentation (or a one-line pointer to ADR 0008 in both places, consistent with how ADR 0002 is pointed to from the ingest paragraph).

### Finding 1.2 (LOW): Journey Requirements Summary reads as complete but covers only Journeys 1-4

The "Journey Requirements Summary" table sits after Journey C, in the position of a summary for all seven journeys, but its "Required By" column only references Journeys 1-4. Phase 3 requirements exist only in Journey B's inline paragraph. Either scope the heading ("V1 Journey Requirements Summary") or add rows for A/B/C (per-key map pattern, events join, `cwd`/`started_at`, `?state=` filters, WHERE floor).

### Non-findings (checked, coherent)

- Journey 3's `brew install` / plain `cargo install` wording: retained but explicitly flagged as historical in both the Phase 3 journeys preamble and the Installation Methods correction. The Journey Requirements Summary table does not repeat the brew claim. Handled.
- "All four user journeys supported at v1" (MVP Feature Set): reads correctly as the V1 record given the Phase 3 preamble's "Journeys 1 through 4 above are the V1 record and stand unchanged."
- Cookbook arithmetic: "at least three" V1 entries plus four Phase 3 entries equals the stated "Phase 3 target: seven entries". Consistent across Product Scope, Documentation Requirements, and FR40-FR43.
- "Substrate changes this cycle: none planned" vs. Journey B's capability list: every cited capability is attributed to a shipped story (5.7, 5.8, 5.9). No hidden substrate work smuggled in.
- Phase 3 success gate, Phase 2 honest outcome, and the Measurable Outcomes addition tell one consistent story (usage was never measured, now it is; the surface shape inverts from destination TUI to interrupt/glance surfaces).
- Tool-reactions listed in MVP scope while the Reaction open question contemplates deprecation: not a contradiction; one is the historical shipped record, the other an honestly open question that names the tension itself.

## 2. Amendment claims vs. document reality

All checked claims hold: the Growth list truly appears once now (the Project Scoping "Post-MVP Features" heading survives only as a pointer stub, which is the right move for inbound references); the `sync` frame row exists and matches the "specced, never emitted" claim; Project Classification honestly records the shipped state while noting the git-history framing; the header and frontmatter carry the 2026-08-02 update marker.

### Finding 2.1 (LOW): frontmatter `status: draft` is arguably stale

The staleness pass updated `updated: 2026-08-02` but left `status: draft` on a document describing a shipped, tagged v0.1.0 product entering its third cycle. If `draft` is a workflow-state convention meaning "amendment in review", fine; if it is leftover from May, it should say something like `active` or `living`. Worth a deliberate decision either way.

## 3. FR quality: FR40-FR44

Overall: good. All five are capability-phrased ("Tool builders can..."), name their delivery vehicle (a specific cookbook entry), and are testable at the "does the entry exist, run, and do the named thing" level. The section preamble does useful work by stating the shared shape (five-section README, CI typecheck + smoke, validated by daily use) once instead of five times. Non-overlapping in the main: FR40/FR43 share a query but are distinct surfaces, and the FR text says so explicitly.

### Finding 3.1 (LOW): FR42 embeds a repo-lifecycle task inside a capability statement

"(absorbing the bowerbird-deck sibling repo, which is archived with a pointer once this entry lands)" is a project action, not a user-facing capability, and it is untestable as part of the FR. It is the right decision in the wrong container. Suggest moving the archival clause to the Phase 3 Scope list (where item 3 already states it) and letting FR42 be purely the live-board capability. Minor at these stakes.

### Finding 3.2 (LOW): FR44 partially overlaps FR40, acceptably

FR40 already specifies repo grouping; FR44 re-mandates repo naming as a cross-cutting floor over all four surfaces. This is deliberate layering (per-surface behavior vs. universal constraint) rather than accidental duplication, and FR44 earns its keep by also carrying the optional-pane stretch boundary and the substrate-axiom guard ("location facts, never location interpretation"). The word "optionally" makes the pane half untestable, but the decision log shows that is faithful to the maintainer's "stretch, only if cheap" call. No change needed; noted so nobody "fixes" it into a hard requirement.

## 4. Decision-log landing check (Update run 2026-08-02)

Walked all 15 entries. Fourteen landed verifiably in the PRD and/or addendum: cycle goals and stakes (Phase 3 paragraph, Phase 3 Scope note), cookbook-in-scope and the four-entry slate with build order (Phase 3 Scope), deck-into-cookbook and archival (FR42, Phase 3 Scope, addendum), Journey A/B/C narrations (the three journeys), hop-is-stretch and WHERE floor (Journey B, FR44, addendum), glance split (FR40 + FR42), pid-to-pane approach (FR44, addendum), snapshot-flag deferral with trigger (Growth list, addendum), sync dormancy (frame table, Growth list), Reaction open question (FR section tail), dropped themes (addendum). The two working-mode/coaching entries are process records with nothing to land.

### Finding 4.1 (LOW): "Promote-to-builtin remains a future option" did not land

The decision-log entry for the one-shot glance ends with "Promote-to-builtin remains a future option if invocation friction bites in daily use." Neither the PRD nor the addendum carries this. It is a natural Growth Features bullet (it is exactly the same shape as the live-only flag entry: triggered, not scheduled, with the trigger named). One line fixes it; without it, the future option lives only in the log, which the PRD body does not require readers to consult.

## 5. Duplicate or orphaned content from the consolidation

Clean. The Post-MVP Features stub is intentional and states why it points elsewhere. No third copy of any Growth item found; NFR4 (`gc` post-V1) and NFR18 (metrics deferred) reference the same deferrals consistently rather than duplicating the list. Journey 3's historical wording is annotated, not orphaned. No dangling references to the old duplicated list were found.

## 6. Em-dash check (newly added Phase 3 sections only)

The addendum contains zero em-dashes. All newly written PRD prose is clean: the Phase 3 success gate, Phase 3 measurable outcomes, Phase 3 Scope, the Phase 2 honest outcome and Phase 3 paragraphs, Journeys A/B/C and their preamble, the Installation Methods correction, the `sync` frame row, the cookbook doc-table addition, the FR40-FR44 block, and the Reaction open question.

### Finding 6.1 (INFO): one em-dash rides inside the newly consolidated Growth list

Line 146 ("Second agent adapter (Codex, Gemini, or Cursor) — validates...") sits inside the section rewritten on 2026-08-02, but the bullet text itself is carried-over V1 prose, same as the other historical hits (lines 120, 157, 392, 399, 454, 455, 503). Under the "historical prose exempt" rule this passes; flagging only because the consolidation technically touched that section, so a purist sweep of `git diff`-added lines would catch it.

## Summary of findings

| # | Severity | Finding |
|---|---|---|
| 1.1 | MEDIUM | API Surface (v1 Stable) tables omit ADR 0008 filters (`GET /sessions?state=`, WS `states`) that FR40 and Journey B treat as shipped fact |
| 1.2 | LOW | Journey Requirements Summary positioned after Journey C but covers only Journeys 1-4 |
| 2.1 | LOW | Frontmatter `status: draft` likely stale for a shipped v0.1.0 product |
| 3.1 | LOW | FR42 embeds the deck-archival project task inside a capability FR |
| 4.1 | LOW | Decision-log "promote-to-builtin remains a future option" (glance) landed in neither PRD nor addendum |
| 3.2 | LOW | FR44/FR40 overlap is deliberate layering; noted to prevent a false fix |
| 6.1 | INFO | One carried-over em-dash inside the 2026-08-02-consolidated Growth list (line 146) |
