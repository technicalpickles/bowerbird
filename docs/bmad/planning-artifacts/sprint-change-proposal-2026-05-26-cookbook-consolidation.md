# Sprint Change Proposal — Cookbook consolidation (single-directory shape, pocketflow pattern)

Date: 2026-05-26
Author: pickles (via bmad-correct-course)
Status: Draft — pending §6 sign-off
Related: `sprint-change-proposal-2026-05-26.md` (Epic 5 introduction), `epics.md` §Story 4.3 / §Story 5.5 / §Story 5.6, `deferred-work.md:104`, `project-context.md:524-545`

## 1. Issue summary

Story 4.3 ("Documentation suite", commit `de5ae48`, merged 2026-05-25) shipped `docs/cookbook/` as a parallel directory to `examples/`, with each cookbook entry duplicating the canonical pattern as a copy-pasted code block. A CI test (`tests/cli_docs_drift.rs::cookbook_include_directives_match_example_anchors`) asserts byte-identity between the cookbook code block and the `// cookbook-begin:NAME` / `// cookbook-end:NAME` anchored region in the paired example source. The `<!-- cookbook-include: ... -->` HTML comment in each cookbook .md file is decorative — no preprocessor consumes it.

This shape violates three things the project already committed to:

1. **Story 4.2 AC** (`epics.md:817-819`): "the cookbook entry **automatically reflects the change via include anchors** — no manual copy-paste required (cookbook-example coupling invariant)."
2. **Story 4.3 AC** (`epics.md:843`): "Code (**inlined via anchor, not copy-pasted**) → Variants."
3. **project-context.md §Cookbook discipline** (L526): "Examples in `examples/` are the source of truth. Cookbook entries explain them. **Do not hand-copy snippets** — they rot."

Story 4.3's `deferred-work.md:104` ("Cookbook inlining mechanism") was opened to track that the actual inlining work (mdBook `{{#include}}` directives, or a hand-rolled build step) had not been chosen. It was never closed; the manual copy-with-drift-check has been the shape since 2026-05-25.

### Why this surfaced now

The maintainer is setting up bowerbird locally for sustained dogfooding (per Epic 5 framing). While reading the cookbook against the paired examples, the duplication became visible. The structural question — "why are these two directories at all?" — surfaced alongside the inlining-mechanism question. The answer, on reflection, is that they don't need to be two directories.

Pocketflow's `cookbook/` (and pi-mono's analogous shape) demonstrate the alternative: each cookbook entry is a self-contained directory containing both prose (`README.md`) and runnable code (`main.py`, `utils.py`, etc.). One artifact per pattern. No duplication, no drift-check, no inlining mechanism question. The README focuses on what the entry is and how to run it; the code files speak for themselves.

### Evidence

- `docs/cookbook/state-session-fanout.md:17` — the inert `<!-- cookbook-include: ... -->` comment + duplicated 50-line code block
- `examples/multi-session-router/src/index.ts` — paired source with `// cookbook-begin:state-session-fanout` anchors around the duplicated region
- `tests/cli_docs_drift.rs::cookbook_include_directives_match_example_anchors` — the drift-check compensating for the duplication
- `pocketflow/cookbook/pocketflow-chat/` — target structure: README.md (1.1K), main.py (1.6K), utils.py (585B), requirements.txt (31B), all colocated

## 2. Impact analysis

### Epic impact

- **Epic 4 (closed, retro'd)**: Cannot be reopened. The remediation lands forward in Epic 5.
- **Epic 5 (in planning)**: Add one new story. Renumber two existing stories. No story-content rewrites required for 5.1-5.4.

### Story impact

| Story | Change |
|---|---|
| New 5.5 | **Cookbook consolidation** (this proposal introduces it) |
| Old 5.5 → new 5.6 | "First-time-reader docs pass." Light AC touch-up at `epics.md:1026` ("docs path Quickstart → presenter-authoring → protocol → cookbook") to confirm the cookbook navigation target is the new directory shape. |
| Old 5.6 → new 5.7 | "Crates.io namespace decision and v0.1.0 tag." Closing condition at `epics.md:1052` ("all Epic 5 stories 5.1 through 5.5 are complete") updates to "5.1 through 5.6." |
| 5.1, 5.2, 5.3, 5.4 | No change. Independent work. |

### Artifact conflicts

**Planning artifacts:**

- `prd.md:327` — "Reference example tools | TypeScript / Node | Lives in `examples/`" → reword to `docs/cookbook/<name>/`
- `prd.md:445` — "`docs/cookbook/` | v1 ships at least three entries paired with reference examples" → reword (entries become self-contained; "paired with" terminology drops)
- `prd.md:450` — coupling invariant paragraph → simplifies (no separate pair to keep in sync; one artifact per pattern)
- `architecture.md:760-829` §Complete Project Directory Structure → surgical rewrite: drop `examples/` subtree, expand `docs/cookbook/` subtree to show per-entry shape
- `architecture.md:915, 946` → path retargets (`examples/` → `docs/cookbook/`)
- `project-context.md:242-258` §Repository layout → directory diagram updated
- `project-context.md:524-545` §Cookbook discipline → major rewrite. Invert the "examples/ are the source of truth, cookbook explains them" framing. The cookbook *is* the source of truth; the README explains, the code runs. The inlining-mechanism discussion (mdBook `{{#include}}`, marked regions + build step) is removed as obsolete. Function-name anchor discipline (L535) is preserved as a cross-reference rule.
- `project-context.md:202` §Reference SDK question → untouched (orthogonal)

**Implementation artifacts:**

- `deferred-work.md:104` ("Cookbook inlining mechanism") → strike through with backlink to Story 5.5's merge commit. The whole question dissolves with the duplication.
- `deferred-work.md:101, 102, 105, 106, 107` → path retargets only (`examples/*/` → `docs/cookbook/*/`)
- `sprint-status.yaml` → new `5-5-cookbook-consolidation: backlog`; existing `5-5-first-time-reader-docs-pass` → `5-6-…`; existing `5-6-crates-io-namespace-and-v0-1-0-tag` → `5-7-…`
- ADR 0005 (new) → "Cookbook consolidation into self-contained directory entries." Documents the trade-off, the considered alternatives (mdBook includes, hand-rolled preprocessor, pocketflow pattern), and declares `Affects context.md sections: Repository layout, Cookbook discipline`.

**Code artifacts:**

- `examples/multi-session-router/`, `examples/event-log-viewer/`, `examples/reconnect-recovery/` → moved (via `git mv`) to `docs/cookbook/state-session-fanout/`, `docs/cookbook/rest-cursor-pagination/`, `docs/cookbook/dropped-frame-recovery/` respectively
- `examples/README.md` → deleted (its content folds into `docs/cookbook/README.md`, which gets a rewrite)
- `examples/.gitignore` → moves with the contents
- Each entry's `src/index.ts` → `// cookbook-begin:NAME` / `// cookbook-end:NAME` comment markers deleted (no longer load-bearing). Code itself unchanged.
- Each entry's `README.md` → rewritten in pocketflow shape (no embedded code; sections: *What this is*, *Run it*, *How it works* (high-level prose + optional diagram), *How to apply it*, *Files*). Existing per-example READMEs (44-58 lines each) get folded in.
- `docs/cookbook/state-session-fanout.md`, `docs/cookbook/rest-cursor-pagination.md`, `docs/cookbook/dropped-frame-recovery.md` → **deleted** as standalone files. Their *Problem* / *Approach* / *Variants* prose blocks fold into the new per-entry README.md under "What this is" and "How to apply it."
- `docs/cookbook/README.md` → rewritten as cookbook index (replaces `examples/README.md`)

**Tests + CI:**

- `tests/cli_examples.rs` → path retargets (`examples/*/src/index.ts` → `docs/cookbook/*/src/index.ts`); `each_example_source_carries_cookbook_anchors` deleted
- `tests/cli_docs_drift.rs::cookbook_include_directives_match_example_anchors` → **deleted** (no duplication to drift-check)
- `tests/cli_docs_drift.rs` (rest of file) → audit other functions for path references; retarget as needed
- `.github/workflows/ci.yml` → loops over `examples/*/` rewritten to loop over `docs/cookbook/*/`

**Reader-facing docs:**

- `README.md:7-8, 162-166` → `examples/` references retargeted to `docs/cookbook/`
- `docs/quickstart.md:19` → command path updated (`examples/multi-session-router/src/index.ts` → `docs/cookbook/state-session-fanout/src/index.ts`)
- `docs/presenter-authoring.md` → grep pass for `examples/` cross-refs, retarget
- `INSTALL.md:73` (tool-reactions.toml troubleshooting) → already targets `~/.bowerbird/`, not affected; double-check no `examples/` refs in this file

### Technical impact

- **Production Rust code**: None.
- **Protocol surface**: None. No protocol-changelog.md entry needed.
- **NFRs**: None. No performance budget affected.
- **CI runtime**: Slight reduction (one drift-check test deleted).
- **Repo size**: Marginal reduction (deleted duplicated code blocks; deleted standalone cookbook .md prose files).

## 3. Recommended approach

**Direct Adjustment (Option 1), with hybrid ADR work.** Add new Story 5.5; modify existing stories within Epic 5's structure; write ADR 0005 documenting the decision; surgical revisions to PRD, architecture, project-context.

**Alternatives considered:**

- *Rollback Story 4.3*: Not viable. Story 4.3 includes ~1300 other lines (presenter-authoring.md, protocol.md, quickstart.md, no-list.md, INSTALL.md/README updates, multiple test files). Forward-fix is the correct shape.
- *PRD MVP review*: Not warranted. The launch deliverable ("`docs/cookbook/` ships ≥3 entries") survives — gets simpler.
- *Fold into existing Story 5.4 (Install UX polish)*: Rejected. 5.4 already absorbs 5 deferred-work entries; adding a structural cookbook restructure dilutes its focus.
- *Decimal numbering (5.4.5) or "5.X-hotfix" shape*: Rejected. BMAD doesn't use decimals. Hotfix is reserved for issues discovered mid-execution of an in-progress story; this surfaced pre-execution.

**Trade-offs:**

- *Gain*: Single artifact per pattern. No drift-check to maintain. Inlining-mechanism question dissolves. Pocketflow alignment for readers familiar with that pattern.
- *Loss*: Cookbook entries are no longer "documentation-only" content under `docs/`; they now contain executable code subtrees with their own `package.json` and `tsconfig.json`. `docs/` becomes mixed prose-plus-code (it already had this property for `docs/bmad/`, but now extends to a reader-facing surface).
- *Verdict*: The loss is mostly aesthetic. The gain (one source of truth, eliminating an entire category of CI drift-check) is concrete.

### Justification

- **Technical risk**: Low. No production code, no protocol changes, no NFR impact.
- **Timeline impact**: Adds one story to Epic 5. Sequenced before 5.6 (first-time-reader docs pass) so cross-refs target the final shape.
- **Maintainability**: Removes the duplicate-with-drift-check pattern, which would otherwise need ongoing maintenance every time an example evolved.
- **Coherence with project axioms**: Axiom 2 ("Small at two scopes, not one") favors collapsing two directories into one when neither needs to exist separately. The new shape is smaller at both per-component (one dir per pattern) and overall (one cookbook surface instead of two coupled surfaces).
- **Business value**: V1 ships with a cookbook that matches the maintainer's mental model and the project's own discipline directive.

## 4. Detailed change proposals

Grouped by artifact type. Specific OLD → NEW shown where text is small enough; for larger rewrites, the change is described in terms of intent + reference to the relevant section.

### 4.1 Story additions / modifications

#### New Story 5.5: Cookbook consolidation

```
### Story 5.5: Cookbook consolidation into self-contained directory entries

As the bowerbird maintainer,
I want each cookbook entry to be one self-contained directory under `docs/cookbook/<name>/`
containing prose (README.md) and runnable code (src/, package.json, tsconfig.json) colocated,
So that the cookbook is the canonical home of the working examples — no duplication, no drift-check,
no separate examples/ surface to navigate.

Closes Story 4.2 AC at epics.md:817-819, Story 4.3 AC at epics.md:843, and project-context.md
§Cookbook discipline directive (L526) "do not hand-copy snippets — they rot." Closes deferred-work.md
entry #4 ("Cookbook inlining mechanism").

**Acceptance Criteria:**

**Given** the existing `examples/multi-session-router/`, `examples/event-log-viewer/`, and
`examples/reconnect-recovery/` directories
**When** Story 5.5 lands
**Then** they have been `git mv`'d to `docs/cookbook/state-session-fanout/`,
`docs/cookbook/rest-cursor-pagination/`, and `docs/cookbook/dropped-frame-recovery/` respectively;
the `examples/` directory no longer exists; `cargo build --workspace`, `cargo test --workspace`,
and the TypeScript smoke tests all pass against the new paths.

**Given** the three standalone cookbook prose files (`docs/cookbook/state-session-fanout.md`,
`docs/cookbook/rest-cursor-pagination.md`, `docs/cookbook/dropped-frame-recovery.md`)
**When** Story 5.5 lands
**Then** they have been deleted; their Problem / Approach / Variants content folded into the
per-entry `docs/cookbook/<name>/README.md` files alongside the existing per-example README
content.

**Given** each new `docs/cookbook/<name>/README.md`
**When** a reader opens it
**Then** the README contains no embedded TypeScript code blocks — only prose sections
(*What this is*, *Run it*, *How it works*, *How to apply it*, *Files* with relative links to
src/index.ts and any other code files). Code is read by opening src/index.ts directly.

**Given** the `// cookbook-begin:NAME` / `// cookbook-end:NAME` comment markers in each
`src/index.ts`
**When** Story 5.5 lands
**Then** the markers have been deleted; the test `tests/cli_examples.rs::each_example_source_
carries_cookbook_anchors` (or its current equivalent) has been deleted; the test
`tests/cli_docs_drift.rs::cookbook_include_directives_match_example_anchors` has been deleted.

**Given** the smoke-test crate `tests/cli_examples.rs`
**When** Story 5.5 lands
**Then** all `examples/*/src/index.ts` path references have been retargeted to
`docs/cookbook/*/src/index.ts`; the CI workflow at `.github/workflows/ci.yml` similarly retargets.

**Given** the planning and project-context artifacts
**When** Story 5.5 lands
**Then** `prd.md:327, 445, 450`, `architecture.md:760-829, 915, 946`, and
`project-context.md:242-258, 524-545` have been updated to reflect the single-directory shape;
`deferred-work.md:104` is struck through with a backlink to this story's merge commit.

**Given** the project's update protocol (project-context.md L77: "Every merged ADR includes
Affects context.md sections: field")
**When** Story 5.5 lands
**Then** ADR 0005 has been authored at `docs/decisions/0005-cookbook-consolidation.md` documenting
the decision, considered alternatives (mdBook `{{#include}}`, hand-rolled preprocessor, pocketflow
pattern), the chosen path, and `Affects context.md sections: Repository layout, Cookbook discipline`.

**Given** reader-facing docs
**When** Story 5.5 lands
**Then** `README.md`, `docs/quickstart.md`, and `docs/presenter-authoring.md` have all `examples/`
path references retargeted to `docs/cookbook/<name>/`.
```

#### Modified Story 5.6 (was 5.5: First-time-reader docs pass)

OLD AC at `epics.md:1026`:

```
**Given** the docs path Quickstart → presenter-authoring → protocol → cookbook (PRD §Documentation Requirements line 436)
**When** the first-time reader graduates from Quickstart and reaches `docs/presenter-authoring.md`
**Then** the first paragraph names the audience switch ("you've seen it work; now you're going to build something") rather than starting directly in technical detail
```

NEW:

```
**Given** the docs path Quickstart → presenter-authoring → protocol → cookbook (PRD §Documentation Requirements line 436)
**When** the first-time reader graduates from Quickstart and reaches `docs/presenter-authoring.md`
**Then** the first paragraph names the audience switch ("you've seen it work; now you're going to build something") rather than starting directly in technical detail; cross-references to the cookbook target the per-entry directory shape introduced by Story 5.5 (e.g. `docs/cookbook/state-session-fanout/`), not the pre-5.5 standalone .md files.
```

Rationale: Anchor the cross-ref direction explicitly so the reader-pass work doesn't accidentally regenerate references to the old shape.

#### Modified Story 5.7 (was 5.6: Crates.io namespace decision and v0.1.0 tag)

OLD AC at `epics.md:1052-1054`:

```
**Given** all Epic 5 stories 5.1 through 5.5 are complete and any hotfix stories are merged
**When** the maintainer tags `v0.1.0`
**Then** the release workflow runs end-to-end producing artifacts;
```

NEW (single character change after renumber):

```
**Given** all Epic 5 stories 5.1 through 5.6 are complete and any hotfix stories are merged
**When** the maintainer tags `v0.1.0`
**Then** the release workflow runs end-to-end producing artifacts;
```

### 4.2 PRD edits

#### `prd.md:327`

OLD: `| Reference example tools | TypeScript / Node | Lives in `examples/`; CI smoke-tested |`

NEW: `| Reference example tools | TypeScript / Node | Lives in `docs/cookbook/<name>/`; CI smoke-tested |`

#### `prd.md:445`

OLD: `| `docs/cookbook/` | v1 ships at least three entries paired with reference examples. Must exist at launch — not a post-launch deliverable. |`

NEW: `| `docs/cookbook/` | v1 ships at least three entries, each a self-contained directory under `docs/cookbook/<name>/` containing prose README + runnable code. Must exist at launch — not a post-launch deliverable. |`

#### `prd.md:448-450`

OLD:

```
### Code Examples

Reference examples in `examples/`, CI smoke-tested against a live daemon. Each example is paired with a cookbook entry. The coupling invariant: a developer changes a function in the reference example, runs the doc build, and the cookbook entry reflects the change without manual editing. Toolchain choice is left to the implementer; the invariant is what the PRD requires.
```

NEW:

```
### Code Examples

Reference examples live in `docs/cookbook/<name>/`, CI smoke-tested against a live daemon. Each cookbook entry is a self-contained directory: the README explains *what it is* and *how to apply the pattern*; the colocated `src/`, `package.json`, and `tsconfig.json` are the runnable canonical code. There is no separate cookbook surface to keep in sync — the prose and code share the same directory, so a developer changing the code and updating the README is one PR, not a coupling problem.
```

### 4.3 Architecture edits

`architecture.md:760-829` §Complete Project Directory Structure → surgical rewrite of the directory diagram. Specifically:

- Delete the `examples/` subtree (current L773-795)
- Expand the `docs/cookbook/` line (current L798) into a multi-line subtree showing one representative entry's shape (README.md, package.json, tsconfig.json, src/index.ts)
- Update workspace-manifest comment at L761: `examples/ is a Node project zone` → `docs/cookbook/*/ is a Node project zone`
- Update L915 and L946 path references

Full new diagram block to be drafted in the dev-story phase; the intent is documented here for the story file.

### 4.4 project-context.md edits

#### §Repository layout (L242-258)

Directory diagram update: drop `examples/` line, expand `docs/cookbook/` to show per-entry shape with README + src/ + package.json. Update accompanying prose at L260 ("Why this shape") to mention the cookbook colocates prose and code.

#### §Cookbook discipline (L524-545) — major rewrite

OLD framing (paraphrased): Examples in `examples/` are the source of truth; cookbook entries explain them; do not hand-copy snippets; use mdBook `{{#include}}` or marked regions + build step.

NEW framing: Each cookbook entry is a self-contained directory under `docs/cookbook/<name>/` containing both the prose README and the runnable code. The README explains *what this is* and *how to apply the pattern*; the code under `src/` (and any sidecar files like `tests/`) is the canonical executable form. No inlining mechanism is needed because there is nothing to inline — readers who want code open `src/index.ts` directly. CI smoke-tests the code on every PR (the workspace-root `tests/cli_examples.rs` crate handles this). Function-name anchor discipline preserved for cross-references *between* docs (e.g. presenter-authoring.md citing `fan_out_with_backpressure() in docs/cookbook/state-session-fanout/src/index.ts`).

Reader-path diagram (L549-562) preserved as-is.

### 4.5 ADR 0005 (new)

Path: `docs/decisions/0005-cookbook-consolidation.md`

Content sketch (to be drafted in dev-story phase):

- **Status**: Accepted (date of Story 5.5 merge)
- **Context**: Story 4.3 shipped a two-directory shape with duplicated code and a drift-check, which violated its own AC + project-context.md §Cookbook discipline. The inlining-mechanism question (mdBook `{{#include}}`, hand-rolled preprocessor) had been deferred and never resolved.
- **Decision**: Consolidate `docs/cookbook/` + `examples/` into a single `docs/cookbook/<name>/` directory per pattern, each containing prose README + runnable code.
- **Considered alternatives**: mdBook with `{{#include}}` directives (rejected: adds a build dependency, and the duplication problem only existed because the two directories existed); hand-rolled preprocessor (rejected: same as above, plus adds project-specific tooling debt); pocketflow pattern (accepted: proven, zero new tooling, eliminates the question entirely).
- **Trade-offs**: `docs/` becomes mixed prose-plus-code at a reader-facing surface (already true for `docs/bmad/`). Marginal aesthetic loss; concrete gain of eliminating a category of drift.
- **Affects context.md sections**: Repository layout, Cookbook discipline

### 4.6 Test + CI edits

- Delete `tests/cli_docs_drift.rs::cookbook_include_directives_match_example_anchors`
- Audit other functions in `tests/cli_docs_drift.rs` for path references; retarget `examples/` → `docs/cookbook/`
- Delete `tests/cli_examples.rs::each_example_source_carries_cookbook_anchors` (or rename if there's a different shape worth keeping)
- Retarget `tests/cli_examples.rs` smoke-spawn paths
- Update `.github/workflows/ci.yml` shell loops over `examples/*/`

### 4.7 Reader-facing docs

- `README.md` — Reference Examples section (L7-8 and L162-166) rewritten to point at `docs/cookbook/`
- `docs/quickstart.md:19` — example invocation path updated
- `docs/presenter-authoring.md` — grep for `examples/` cross-refs, retarget
- `INSTALL.md` — verify no `examples/` references

### 4.8 sprint-status.yaml

Lines currently reading:

```yaml
  5-5-first-time-reader-docs-pass: backlog
  5-6-crates-io-namespace-and-v0-1-0-tag: backlog
```

Become:

```yaml
  5-5-cookbook-consolidation: backlog
  5-6-first-time-reader-docs-pass: backlog
  5-7-crates-io-namespace-and-v0-1-0-tag: backlog
```

`last_updated` comment refreshed to note the addition.

## 5. Implementation handoff

**Scope classification: Moderate.**

The change spans planning artifacts (PRD, epics, architecture, project-context), implementation artifacts (sprint-status, deferred-work), code organization (file moves under git), code files (README rewrites, deleting anchor comments), tests, CI, reader-facing docs, and a new ADR. It does not touch production Rust code, protocol surface, or NFRs.

**Handoff plan:**

1. **This proposal lands** (via `bmad-correct-course` workflow completion) with:
   - This Sprint Change Proposal document
   - `epics.md` updates: new Story 5.5 inserted, existing 5.5→5.6, existing 5.6→5.7, light AC touch-ups on the now-5.6 and now-5.7
   - `sprint-status.yaml` updates: new `5-5-cookbook-consolidation`, renumber two existing entries
2. **Story-automator picks up new Story 5.5**:
   - `bmad-create-story` produces the dev-ready story file at `docs/bmad/implementation-artifacts/story-files/5-5-cookbook-consolidation.md`
   - `bmad-dev-story` implements: file moves, README rewrites, ADR 0005 drafting, PRD/architecture/project-context surgical edits, test/CI updates, deferred-work strikethrough
   - `bmad-code-review` reviews the implementation PR with full BMAD discipline
3. **Sequencing within Epic 5**: New Story 5.5 lands before new Story 5.6 (first-time-reader docs pass). 5.1, 5.2, 5.3, 5.4 can run in parallel with 5.5 since they touch disjoint surfaces.

**Success criteria for implementation:**

- `cargo build --workspace`, `cargo test --workspace`, and the TypeScript smoke tests pass against the new paths
- The drift-check CI test is removed (its absence is itself a signal that the duplication is gone)
- A reader opening `docs/cookbook/state-session-fanout/` sees one directory with prose + runnable code, indistinguishable in shape from `pocketflow/cookbook/pocketflow-chat/`
- ADR 0005 names the decision in archive-quality prose with an `Affects context.md sections:` field
- `deferred-work.md:104` is struck through with a backlink to the merge commit

**Routing:**

- Sprint Change Proposal sign-off: **pickles** (this workflow)
- Story file creation: **story-automator** via `bmad-create-story`
- Implementation: **story-automator** via `bmad-dev-story` (or Developer agent if invoked directly)
- Review: **story-automator** via `bmad-code-review`

## 6. Approval

Sign-off required from pickles before:

1. epics.md changes land
2. sprint-status.yaml changes land
3. story-automator is invited to pick up Story 5.5
