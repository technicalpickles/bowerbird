# 0011. Crates.io publishing deferred at v0.1.0; no placeholder reservation

Date: 2026-08-02
Status: Accepted
Deciders: @pickles
Related: ADR 0001 (project name; this ADR covers the same name on a new registry surface); Story 5.15 (`docs/bmad/implementation-artifacts/5-15-crates-io-namespace-and-v0-1-0-tag.md`, the implementation); Epic 3 retro AI-3 / Epic 4 retro AI-5 (the pre-release namespace check this ADR closes); deferred-work.md §"Deferred from: Story 3.4" crates.io entry (annotated by this ADR, still deferred)
Implementation: N/A: process-only (no Cargo.toml metadata, no publish; distribution stays GitHub Releases tarballs + `cargo install --git --tag` per README/INSTALL.md)
Affects context.md sections: none

## Context

Story 5.15's AC 1 (Epic 3 retro AI-3, Epic 4 retro AI-5) requires a deliberate, documented decision on crates.io publishing before the v0.1.0 tag. The namespace was verified via the crates.io API on 2026-08-01 and re-verified 2026-08-02 (`cargo search` is unusable for this: it substring-matches, and `cargo info` resolves the local workspace first). Result: `bowerbird`, `bowerbird-daemon`, `bowerbird-shim`, and `adapter-claude` are all free (HTTP 404); `protocol` is TAKEN by an unrelated, established crate ("Easy protocol definitions", published 2016, ~160k downloads, HTTP 200). Rename candidates `bowerbird-protocol` and `bowerbird-adapter-claude` are free.

An earlier pre-check recorded in sprint-status (2026-08-01, during the 5.13 session) tested the wrong name set (`bowerbird-cli` and `bowerbird-client` are not workspace packages; `bowerbird-shim` and `protocol` were never checked), so its "namespace free" conclusion was incomplete: the brand names are free, but the workspace is not publishable as-is.

Beyond the name conflict, publishing at v0.1.0 has three independent blockers:

1. **The vendored libsqlite3-sys patch does not travel.** The workspace `[patch.crates-io]` swaps in a vendored libsqlite3-sys 0.36.0 carrying SQLite 3.51.3, because stock 0.36.0 bundles SQLite 3.51.1 with a unix-VFS lock-order inversion that deadlocks concurrent `sqlite3_close` on the same WAL database (this repo's confirmed test-hang root cause). Cargo patch sections apply only to the workspace that declares them: a crates.io consumer of a published `bowerbird-daemon` would build against stock libsqlite3-sys and get the deadlock back. The clean fix (libsqlite3-sys with SQLite >= 3.51.2 via rusqlite >= 0.39) is blocked on deadpool-sqlite support (deadpool issue #490). `cargo install --git --tag` is unaffected: it builds from the git tree with `--locked` and honors the patch.
2. **Path dependencies carry no `version` keys** (`protocol = { path = "crates/protocol" }` and friends); `cargo publish` requires version refs on every dependency, committing the workspace to synchronized version bumps it has not needed yet.
3. **Semver commitment is premature.** The stable surface is the wire protocol (REST/WS JSON, `protocol@v1`), not the Rust API. The protocol crate's Rust-source compatibility policy is explicitly undefined (deferred-work 2026-06-02 entry: additive required public fields break struct-literal consumers), so publishing would promise a stability the project has deliberately not designed yet.

## Decision

Do not publish any crate at v0.1.0 and do not place a placeholder reservation; distribution remains GitHub Releases tarballs plus `cargo install --git --tag`, and this ADR (summarized in the v0.1.0 release notes) is the documented namespace decision.

## Alternatives considered

- **Publish all five packages at v0.1.0.** Rejected: requires renaming the protocol crate first (its name is taken), and ships a known WAL-close deadlock to crates.io consumers of the daemon via blocker 1. Blockers 2 and 3 are mechanical-but-real on top. Not tag-week work, and the epic AC's own escape hatch (this ADR) exists precisely for this outcome.
- **Publish a minimal placeholder under `bowerbird` to reserve the brand.** Rejected by maintainer preference: crates.io permits name reservation but the practice is contested, a placeholder still requires metadata and a version to maintain, and the squatting risk for a niche name verified free today is judged low. Accepting that risk is part of this decision.
- **Rename `protocol` to `bowerbird-protocol` now, publish later.** Rejected: a crate rename touches every internal path dep, `docs/protocol.md` references, and the contract-test inventory, and buys nothing until a publish actually happens. The naming plan is recorded here instead: when publishing happens, `protocol` becomes `bowerbird-protocol` and `adapter-claude` becomes `bowerbird-adapter-claude` (both verified free 2026-08-02; availability is not guaranteed to hold).

## Consequences

- v0.1.0 ships with zero crates.io presence; the install surfaces are exactly what README, INSTALL.md, and the release notes already promise. No Cargo.toml changes, no new release-pipeline steps.
- The brand name `bowerbird` stays unreserved. If it is taken before a future publishing story, that story inherits a renaming decision (this ADR's availability snapshot is the evidence the name was free and the risk was accepted knowingly).
- A future publishing story starts from this ADR: the rename plan, the metadata requirements (each published crate needs `description`, `repository`, `keywords`, `categories`, and a `[package.metadata.docs.rs]` block, per the deferred-work Story 3.4 entry), version keys on path deps, and blocker 1's resolution are its checklist.
- The deferred-work Story 3.4 crates.io entry stays open (annotated with this ADR) rather than struck; the v0.1.0 release notes name it in the "what doesn't work yet" list.

## Revisit when

- deadpool-sqlite supports rusqlite >= 0.39 (deadpool issue #490 closes), removing the vendored-patch blocker.
- A second adapter, SDK consumer, or downstream project concretely asks for `cargo add bowerbird-*` (demand signal, per the reaction-enum precedent of following demand rather than anticipating it).
- Any of the five names in this ADR's availability snapshot gets registered by a third party (namespace threat; re-run the API check before acting on this trigger).
- A release-management story defines the Rust-API stability policy the protocol crate currently lacks.
