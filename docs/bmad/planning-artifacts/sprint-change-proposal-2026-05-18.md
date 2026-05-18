# Sprint Change Proposal — Ingest wire framing drift (HTTP → NDJ)

Date: 2026-05-18
Author: pickles (via bmad-correct-course)
Status: Approved (pending §6 sign-off)
Related: [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md), `docs/bmad/implementation-artifacts/deferred-work.md` line 37

## 1. Issue summary

Stories 1.3 (Unix socket ingest endpoint), 1.4 (Claude Code adapter), and 1.5 (shim binary) shipped against a **newline-delimited JSON** wire contract — one `{object}\n` request, one status-line response (`200\n` / `503\n` / `400 <reason>\n`). The planning artifacts (PRD, architecture, epics) still describe the same surface as `POST /ingest` over HTTP/1.1 with "raw hook JSON verbatim, no normalization in shim."

The decision was ratified retroactively in [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md) on 2026-05-18. ADR-0002's own §Consequences acknowledges the drift:

> PRD and architecture are now stale on these two points. Either amend in-place when next touched, or rely on this ADR + `Related:` backlinks. Not blocking.

This proposal does the in-place amendment so future agents and contributors (especially the eventual authors of story 4.3 docs suite and 4.4 protocol-compatibility contract tests) source from a single coherent set of artifacts.

The shim also injects one transport-routing field, `hook_kind`, derived from a `--hook-kind` CLI flag. Claude Code's `hook_event_name` is preserved verbatim. ADR-0002 §Decision item 2 reframes this as transport routing, not interpretive normalization, so Axiom 1 ("substrate observes; does not interpret") is intact.

### Evidence

- `docs/decisions/0002-ingest-wire-framing-and-hook-kind.md` — ratified decision
- `crates/daemon/src/ingest/handler.rs:53-57` — actual handler reads one line, dispatches on `hook_kind`, defaults to `"PreToolUse"` (the default itself is the deferred follow-up; see §3)
- `crates/shim/src/socket.rs` — writes `wire_bytes` ending in `\n`, reads one status line
- `docs/bmad/implementation-artifacts/1-5-shim-binary-with-hot-path-event-delivery.md` — story 1.5 context block explicitly warns dev agents about the contradiction and points at ADR-0002 as the source of truth
- `docs/bmad/implementation-artifacts/deferred-work.md` line 37 — tracks the next required tightening

## 2. Impact analysis

### Epic impact

- **Epic 1 (Agent activity captured and queryable via REST)** — already-shipped stories 1.3/1.4/1.5 match the real (NDJ) contract. 1.6 (session projection) and 1.7 (REST query API) don't touch the ingest wire. **One new tail story added: 1.8 (tighten daemon `hook_kind`)** to retire the silent default once the shim is the only first-party ingest client.
- **Epic 2 (WS streaming)** — unaffected. Different surface (TCP, axum).
- **Epic 3 (install/lifecycle/auth/release)** — behaviorally unaffected. Story 3.1 (install) is where the `~/.claude/settings.json` entry that sets `--hook-kind` per hook event gets written; ADR-0002 already names this and the architecture text now points at the ADR.
- **Epic 4 (DX/replay/docs/protocol-compat)** — would have soft-impacted story 4.3 (docs suite) and 4.4 (protocol-compatibility contract test suite) if the drift were left in. After this proposal lands, both stories can source from a single coherent set of artifacts.

### Artifact impact

- **PRD** — 1 amendment (line 365: API surface, ingest endpoint description).
- **Architecture** — 3 amendments (Shim wire format ~line 618, Ingest boundary ~line 869, completeness summary ~line 984).
- **Epics** — 4 amendments (story 1.2 ENOSPC AC ~line 260; story 1.3 ACs ~lines 304/309/320) plus 1 new story block (1.8).
- **Sprint status** — 1 line added (`1-8-tighten-daemon-hook-kind: backlog`).
- **Story 1.3 dev story file** (`docs/bmad/implementation-artifacts/1-3-unix-socket-ingest-endpoint.md`) — left as-is; historical record of how the decision was made. Implemented behavior is canonical.
- **UX / deployment / CI** — N/A. (CI's shim bench gate is a separate concern; see ADR-0003.)

### Technical impact

Zero behavioral change to shipped code. Story 1.8 is the only new implementation work (single handler change at `crates/daemon/src/ingest/handler.rs:53-57` + test churn in `crates/daemon/tests/contract_daemon.rs`).

## 3. Recommended approach

**Selected: Option 1 — Direct adjustment.** In-place amendments to PRD, architecture, and epics, plus a single new follow-up story (1.8) in epic 1's tail.

Rationale considering:

- **Effort and timeline impact** — Low. Docs-only edits + one small new story. Zero rework on shipped code.
- **Technical risk and complexity** — Low. The risky part (the decision itself) already happened during story 1.3 and was ratified by ADR-0002.
- **Team morale and momentum** — Positive. Eliminates a "wait, which doc is right?" friction point for the next agent to start work in this area.
- **Long-term sustainability and maintainability** — Improves. Story 4.3 (docs suite) and 4.4 (contract tests) will now inherit a coherent baseline.
- **Stakeholder expectations and business value** — Unaffected. MVP scope unchanged.

### Alternatives considered

- **Option 2 — Rollback.** Nonsensical here. The code is correct; the docs are the lag.
- **Option 3 — MVP review.** Not warranted. Zero MVP impact; this is housekeeping plus a single, scoped follow-up story.
- **Do nothing, rely on ADR-0002 + backlinks alone.** ADR-0002 explicitly authorizes this path ("Not blocking"). Rejected because (a) the next stories to consume these artifacts (4.3 docs, 4.4 protocol-compat) are docs-heavy and inherit drift directly, and (b) the deferred-work entry for the `hook_kind` default doesn't auto-promote to a story without this pass.

## 4. Detailed change proposals (applied 2026-05-18)

### 4.1 PRD — `docs/bmad/planning-artifacts/prd.md` line 365 (API surface)

**Before**:

```
**Ingest endpoint (Unix socket):**
`POST /ingest` via HTTP/1.1 over the Unix domain socket. Returns synchronously after the event is accepted into the write queue ...
```

**After**:

```
**Ingest endpoint (Unix socket):**
Newline-delimited JSON over the Unix domain socket — one `{event-object}\n` request, one status-line response (`200\n` on accept, `503\n` under backpressure, `400 <reason>\n` on malformed payload). See [ADR-0002] for the wire contract; supersedes the earlier "POST /ingest via HTTP/1.1" wording. The daemon returns the `200` synchronously after the event is accepted into the write queue ...
```

### 4.2 Architecture — `docs/bmad/planning-artifacts/architecture.md` Shim wire format (~line 618)

**Before**:

```
**Shim wire format:** shim writes raw hook JSON verbatim to the Unix socket.
No normalization in shim. Daemon calls
`adapter_claude::normalize(hook_kind, raw) -> Result<NormalizeResult>`.
```

**After**:

```
**Shim wire format:** newline-delimited JSON over the Unix socket (one
`{object}\n` line in, one status line out: `200\n` / `503\n` / `400 <reason>\n`).
The shim writes the hook JSON with one transport-routing field injected
(`hook_kind`, from the `--hook-kind` CLI flag); Claude Code's original
`hook_event_name` is preserved verbatim. No interpretive normalization in shim;
that remains adapter-claude's job. Daemon calls
`adapter_claude::normalize(hook_kind, raw) -> Result<NormalizeResult>`.
See [ADR-0002].
```

### 4.3 Architecture — Ingest boundary (~line 869)

**Before**:

```
- Raw hook JSON bytes on the wire; no normalization in shim
```

**After**:

```
- Newline-delimited JSON wire framing (one `{object}\n` in, one status line out); shim injects `hook_kind` as transport routing but adds no interpretive normalization. See [ADR-0002].
```

### 4.4 Architecture — completeness summary (~line 984)

**Before**:

```
**One deferred implementation detail:** Ingest socket wire framing
(length-prefixed vs newline-delimited) is TBD at implementation time. This
does not affect any other component's design.
```

**After**:

```
**Resolved during Story 1.3 implementation:** Ingest socket wire framing is
newline-delimited JSON (one `{object}\n` request, one status-line response).
Ratified by [ADR-0002].
```

### 4.5 Epics — Story 1.2 ENOSPC AC (~line 260)

**Before**: `**Given** a running daemon that has acknowledged a `POST /ingest` event with a 200 response`

**After**: `**Given** a running daemon that has acknowledged an ingest write with a `200` status line`

### 4.6 Epics — Story 1.3 ACs (lines 304, 309, 320)

Three related amendments; see §6 of the worktree's epics.md for applied wording. Net effect: replace "POST /ingest" with "ingest line on the Unix socket"; replace bare `400` with `400 <reason>\n`; add backlink to ADR-0002 on the first occurrence.

### 4.7 Epics — new Story 1.8

New block inserted between story 1.7 and the epic 2 separator. Story title: "Tighten daemon `hook_kind` to a required transport field." Four acceptance criteria covering: missing-field → `400 missing hook_kind`, unknown-value → `400 unknown hook_kind: <value>`, contract-test suite still passes, deferred-work entry struck.

### 4.8 Sprint status — `docs/bmad/implementation-artifacts/sprint-status.yaml`

Added: `1-8-tighten-daemon-hook-kind: backlog` under epic 1.

## 5. Implementation handoff

**Scope classification: Minor.**

All documentation amendments are already applied in this branch. The single piece of implementation work is **Story 1.8**, which slots cleanly into the existing epic-1 sequence after 1.6 and 1.7.

| Recipient | Responsibility |
|---|---|
| `bmad-create-story` (when 1.8 is next in the sprint cadence) | Generate the per-story implementation file for 1.8 from the epic block. |
| `bmad-dev-story` (downstream) | Implement: remove the `"PreToolUse"` default at `crates/daemon/src/ingest/handler.rs:53-57`; return `400 missing hook_kind\n` instead. Update affected contract tests. Strike `deferred-work.md` line 37 with a backlink to the merging commit. |
| `pickles` | Decide whether 1.8 leapfrogs 1.6/1.7 or runs in its natural slot. Default recommendation: keep the existing 1.6 → 1.7 → 1.8 order. |

### Success criteria

- PRD, architecture, and epics now read coherently with the shipped daemon + shim behavior; ADR-0002 is backlinked from each amendment site.
- Sprint status reflects the new story.
- Once 1.8 lands, the `handler.rs:53-57` deferred-default goes away, the contract tests are green, and the deferred-work entry is struck.

## 6. Acknowledgements

- [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md) was the load-bearing reference for this proposal — without it, the drift would have been a forensic exercise.
- Story 1.5's context block (`docs/bmad/implementation-artifacts/1-5-shim-binary-with-hot-path-event-delivery.md` lines 177–190) made the contradiction explicit during 1.5 implementation, which is what surfaced the gap for this proposal.
