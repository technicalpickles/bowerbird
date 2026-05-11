# ADR 0001: Project name is `bowerbird`

**Status:** Accepted
**Date:** 2026-05-11

## Context

The project needed a real name. Placeholder names `claude-state-bus` and `agent-state-bus` both had problems:

- `claude-state-bus` binds the substrate to a single vendor (Claude), which is wrong: the project ingests from Claude Code, Codex, Gemini, and Cursor (per `09-multi-agent-support.md`).
- `agent-state-bus` reads as generic; the `agent-*` prefix is heavily used in the AI tooling space and carries no distinct identity.
- Neither name is brandable. Each is a functional compound that describes the wiring, not the project.

A name is wanted that:

- Doesn't bind to one vendor
- Reads as a single pronounceable word
- Has clean namespace across crates.io, npm, PyPI, Homebrew, and GitHub
- Carries the substrate-not-actor design principle from `07-agent-type-and-foundations.md`
- Fits the existing `technicalpickles` repo dialect (homesick-pattern: one word, hidden technical meaning)

## Decision

The project is named **`bowerbird`**.

Bowerbirds are observer birds that collect bright objects and arrange them in their bower (a small structure they build) to attract visitors. The metaphor encodes the substrate's two load-bearing behaviors in one word:

- **Collects** — every hook event is preserved verbatim in the SQLite event log, native payloads intact (per `08-design-sketch-v2.md`).
- **Arranges for display** — presenters (lamps, sprites, dashboards, voice tools) subscribe to the substrate the way bowerbird visitors see the arranged bower.

The bird is also the right *vibe*: it observes, it doesn't intervene. That's the substrate-not-actor principle from `07-agent-type-and-foundations.md` made legible without explanation.

## Naming implications adopted

- Repo: `github.com/technicalpickles/bowerbird`
- Binary: `bowerbird` (full name); `bb` available as a documented shell alias if useful
- Crates: `bowerbird-protocol`, `bowerbird-shim`, `bowerbird-daemon`, `bowerbird-adapter-claude`
- Homebrew: `brew install bowerbird`
- Cargo: `cargo install bowerbird`
- Install command: `bowerbird install`
- Daemon command: `bowerbird daemon`
- State directory: `~/.bowerbird/` (auth token at `~/.bowerbird/server.json`)
- Default bind: unchanged, `127.0.0.1:9876`

The metaphor extends naturally without forcing. Available vocabulary if/when useful:

- The SQLite event log = the **bower** (e.g. `~/.bowerbird/bower.db`)
- Subscribers = **visitors**

None of this is normative; presenters are free to ignore the metaphor.

## Namespace verification (as of 2026-05-11)

| Registry | Status |
|---|---|
| crates.io (`bowerbird`) | FREE |
| crates.io (sub-crates: `bowerbird-shim`, `bowerbird-daemon`, `bowerbird-protocol`, `bowerbird-adapter`, `bowerbird-cli`, `bowerbird-core`, `bowerbird-rs`) | All FREE |
| npm | FREE |
| PyPI | FREE |
| Homebrew core formula | FREE |
| GitHub | 23 exact-name repos exist, none in AI/agent/observability/dev-tools space |

Top existing GitHub `bowerbird` repos for reference (different domains, not blocking):

- `ara3d/bowerbird` (64★, C#) — Revit (architecture/CAD) plug-in framework
- `ropensci/bowerbird` (52★, R) — scientific dataset collection package
- All others <5★ in unrelated domains (file organizer, color picker, single-cell genomics)

## Alternatives considered

Three other viable names emerged from the naming brainstorm (`docs/bmad/brainstorming/brainstorming-session-2026-05-11-0849.md`):

- **`magpie-d`** (rejected) — Magpie is also a corvid hoarder, conceptually similar to bowerbird. The bare name `magpie` is taken on crates.io (Othello library, 23K downloads) and GitHub has a programming language and an ICLR 2025 ML paper both using the name. Salvaging via the unix daemon convention (`magpie-d`) works namespace-wise but reads less brandable than a clean bare name. Bowerbird's metaphor (collect + arrange for display) is also a stronger fit than magpie's (just collect).

- **`bystander`** (rejected) — Encodes the substrate-not-actor philosophy literally; free on crates.io. Nearest GitHub neighbor is `jonhoo/bystander` (Jon Gjengset, well-known Rust educator, 30★) which would be a small but real namespace neighbor in the same ecosystem. Bowerbird gets a cleaner Rust neighborhood at the cost of a slightly less philosophy-direct name.

- **`brine-tap`** (rejected) — Honored the `technicalpickles` pickle/brine dialect (`brineworks`, `pickled-*` repos). Functional and clean. Lost to bowerbird because the hyphenated compound reads as personal-project-style; for a substrate meant to be adopted by third-party tool authors, a clean single-word name carries less cognitive overhead.

The full set of 119 candidates and the four-round filtering process is in `docs/bmad/brainstorming/brainstorming-session-2026-05-11-0849.md`.

## Consequences

- Existing design docs in `docs/research/` are renamed in place from `claude-state-bus` to `bowerbird`. The git history before this commit is the source for the placeholder name; no compatibility shim is needed since nothing has shipped.
- `_bmad/` configs were updated to `project_name: bowerbird`. The on-disk directory remains `agent-state-bus` for now; renaming the working directory is left to the user.
- The brainstorming session file at `docs/bmad/brainstorming/brainstorming-session-2026-05-11-0849.md` preserves the naming history including placeholder references and is intentionally not rewritten.
- Namespace squatting is not done preemptively. If interest in the project grows before publication, `cargo publish` a placeholder crate and `gh repo create technicalpickles/bowerbird` to lock the GitHub name.

## Revisit conditions

The name should only be revisited if:

- A more prominent `bowerbird` emerges in the AI tooling or observability space before publication, creating real confusion.
- A trademark conflict surfaces (none found at decision time).
- The metaphor turns out to actively mislead users about what the substrate does.
