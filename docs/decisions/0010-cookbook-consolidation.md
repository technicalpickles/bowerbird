# 0010. Cookbook consolidation into self-contained directory entries

Date: 2026-08-01
Status: Accepted
Deciders: @pickles
Related: sprint-change-proposal-2026-05-26-cookbook-consolidation.md (operationalizes this decision; note its story/ADR numbers predate two renumberings, the story landed as 5.13 and this ADR as 0010); Story 4.2 (`docs/bmad/implementation-artifacts/4-2-reference-example-tools.md`, shipped the examples + anchor markers); Story 4.3 (`docs/bmad/implementation-artifacts/4-3-documentation-suite.md`, shipped the duplicated-prose cookbook shape this ADR removes); Story 5.13 (`docs/bmad/implementation-artifacts/5-13-cookbook-consolidation.md`, the implementation); deferred-work.md entry #4 ("Cookbook inlining mechanism", resolved here)
Implementation: `docs/cookbook/<name>/` (three entries, moved from `examples/<old-name>/` via `git mv`); `docs/cookbook/README.md` (index); `tests/cli_docs_drift.rs` (drift-check deleted, five-section README guard added); `tests/cli_examples_drift.rs` (anchor guard deleted, remaining guards retargeted); `tests/cli_examples.rs` (smoke paths retargeted); `.github/workflows/ci.yml` (typecheck loop retargeted)
Affects context.md sections: Repository layout, Cookbook discipline

## Context

Story 4.3 shipped `docs/cookbook/` as a directory parallel to `examples/`: each cookbook entry was a standalone `.md` file whose Code section duplicated, as a copy-pasted fenced block, an anchored region of the paired example's `src/index.ts`. A CI test (`cookbook_include_directives_match_example_anchors`) asserted byte-identity between the two copies, and an inert `<!-- cookbook-include: ... -->` comment marked where a future inlining mechanism would go.

That shape violated three commitments the project had already made: Story 4.2's AC ("the cookbook entry automatically reflects the change via include anchors, no manual copy-paste required"), Story 4.3's own AC ("inlined via anchor, not copy-pasted"), and project-context.md §Cookbook discipline ("do not hand-copy snippets, they rot"). The actual inlining mechanism (mdBook `{{#include}}` or a hand-rolled build step) was deferred to deferred-work.md entry #4 and never chosen. The duplicate-with-drift-check stood in the meantime.

While setting up sustained dogfooding, the maintainer read the cookbook against the paired examples and asked the structural question the inlining debate had been skipping: why are these two directories at all? Pocketflow's `cookbook/` (and pi-mono's analogous shape) demonstrate the alternative: one directory per pattern, containing both the prose README and the runnable code. One artifact, no duplication, no drift-check, no inlining mechanism to choose.

## Decision

Consolidate `docs/cookbook/` and `examples/` into a single surface: each cookbook entry is one self-contained directory `docs/cookbook/<name>/` named for the pattern (not the old example name), containing:

- `README.md`: prose only, five sections in order (What this is, Run it, How it works, How to apply it, Files), with no embedded TypeScript code blocks; readers who want code open `src/index.ts` directly.
- The runnable code: `src/index.ts`, `package.json`, `package-lock.json`, `tsconfig.json`, plus sidecar files like `tests/`.

The three entries are `state-session-fanout/` (was `examples/multi-session-router/`), `rest-cursor-pagination/` (was `examples/event-log-viewer/`), and `dropped-frame-recovery/` (was `examples/reconnect-recovery/`). `examples/` no longer exists; `docs/cookbook/README.md` is the index. The anchor markers and both anchor-related CI tests are deleted; a new guard pins the five-section README shape and the no-TypeScript-fences rule instead, so the consolidated shape is machine-enforced the same way the old one was.

## Consequences

- One source of truth per pattern. A change to an entry's code and its README is one PR in one directory; there is no second surface to keep in sync and no category of drift for CI to police.
- deferred-work.md entry #4 ("Cookbook inlining mechanism") dissolves rather than being solved: with prose and code colocated, there is nothing to inline.
- `docs/` becomes mixed prose-plus-code at a reader-facing surface (each entry carries `package.json`, `node_modules/` when installed, etc.). This was already true of `docs/bmad/`; the loss is aesthetic, the gain is concrete.
- The cookbook directories remain a Node project zone, not a Cargo zone: root `Cargo.toml` `[workspace] members = ["crates/*"]` is unchanged, CI typechecks each entry (`tsc --noEmit`), and the workspace-root smoke crate `tests/cli_examples.rs` still spawns `node --experimental-strip-types` against each entry on every PR. The guards that pinned these invariants for `examples/` are retargeted, not deleted.
- Coherence with Axiom 2 ("small at two scopes"): one directory per pattern is smaller per-component, and one cookbook surface instead of two coupled surfaces is smaller overall.

## Alternatives considered

- **mdBook with `{{#include}}` directives.** Rejected: adds a build dependency and a doc-build step to solve a duplication problem that only exists because there are two directories. The project has no doc build today; acquiring one to work around a structural choice is backwards.
- **Hand-rolled preprocessor consuming the anchor markers.** Rejected: same objection, plus bespoke tooling debt the project would own forever.
- **Keep the duplicate-with-drift-check shape.** Rejected: it is the documented violation of the project's own cookbook discipline, and every example evolution pays the copy-paste tax with CI as the enforcement.
- **Pocketflow pattern (chosen).** Proven in the wild, zero new tooling, and it eliminates the inlining question entirely instead of answering it.
