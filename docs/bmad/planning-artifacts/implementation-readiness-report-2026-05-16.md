---
stepsCompleted:
  - step-01-document-discovery
  - step-02-prd-analysis
  - step-03-epic-coverage-validation
  - step-04-ux-alignment
  - step-05-epic-quality-review
  - step-06-final-assessment
documentsInventoried:
  - docs/bmad/planning-artifacts/prd.md
  - docs/bmad/planning-artifacts/product-brief-bowerbird.md
  - docs/bmad/planning-artifacts/product-brief-bowerbird-distillate.md
  - docs/bmad/project-context.md
missingDocuments:
  - architecture
  - epics-and-stories
  - ux-design
---

# Implementation Readiness Assessment Report

**Date:** 2026-05-16
**Project:** bowerbird

---

## Document Inventory

| Document | Status | Path |
|---|---|---|
| PRD | ✅ Found | `docs/bmad/planning-artifacts/prd.md` |
| Architecture | ❌ Missing | — |
| Epics & Stories | ❌ Missing | — |
| UX Design | ❌ Missing | — |
| Product Brief | ✅ Found (supplemental) | `docs/bmad/planning-artifacts/product-brief-bowerbird.md` |
| Product Brief Distillate | ✅ Found (supplemental) | `docs/bmad/planning-artifacts/product-brief-bowerbird-distillate.md` |
| Project Context | ✅ Found (supplemental) | `docs/bmad/project-context.md` |

---

## PRD Analysis

### Functional Requirements

FR1: The shim can capture Claude Code hook events and deliver them to the daemon without adding perceptible latency to Claude Code's operation
FR2: The shim can operate without network timeouts or blocking calls that could delay Claude Code's hook execution
FR3: Tool builders can install and remove the bowerbird hook from Claude Code's configuration without manually editing configuration files
FR4: The Claude Code adapter can normalize Claude Code hook payloads into the canonical protocol event format
FR5: The shim can log failure information to a dedicated log file without writing to stdout or stderr
FR6: The daemon can persist incoming events to a local event log atomically with their associated session state projection
FR7: The daemon can survive unexpected termination without leaving the event log in a corrupt or inconsistent state
FR8: Tool builders can query the event log with a cursor to retrieve events from a specific point forward
FR9: The daemon exposes the oldest available event identifier so tools can detect whether they have missed events
FR10: Tool builders can subscribe to a stream of agent activity events over a persistent connection
FR11: Tool builders can filter their subscription to specific topics at session, source, or global scope
FR12: Tool builders can subscribe to activity across all sessions simultaneously using a wildcard subscription
FR13: The daemon can notify subscribed tools when new sessions appear without requiring reconnection
FR14: The daemon can notify a tool when it has missed events due to slow consumption, including how many events were missed
FR15: The daemon can deliver a current-state snapshot to a connecting tool without requiring a separate query
FR16: Multiple tools can connect to and receive the same event stream simultaneously without affecting each other
FR17: The daemon can send a shutdown notification to connected tools before terminating
FR18: Tool builders can retrieve a list of known agent sessions
FR19: Tool builders can retrieve the current projected state of a specific session
FR20: Tool builders can retrieve paginated event history for a session from a given cursor position
FR21: Tool builders can retrieve per-session event statistics
FR22: Tool builders can check daemon liveness without authenticating
FR23: Tool builders can check daemon readiness — including storage and broadcaster state — without authenticating
FR24: The daemon can track multiple concurrent agent sessions, distinguishing them by both source and session identifier
FR25: The daemon can maintain a current-state projection per session, updated in the same operation as event storage
FR26: The daemon can tolerate missing hook events without entering an inconsistent or stuck state
FR27: Tool builders can install bowerbird without a Rust development environment using prebuilt binaries from GitHub Releases
FR28: Tool builders can install bowerbird from source using the Rust toolchain
FR29: Tool builders can start and stop the daemon independently of the Claude Code hook configuration
FR30: Tool builders can check the daemon's current status and version from the command line
FR31: Tool builders can replay a recorded event sequence through the daemon's full pub/sub path without a live Claude Code session
FR32: Tool builders can export a real session's events to a file for replay or debugging
FR33: Tool builders can access reference implementations demonstrating event subscription, multi-session fan-out, and dropped-frame recovery
FR34: Tool builders can run all reference implementations against bundled fixture data without a live agent session
FR35: Tool builders can access documentation covering: quickstart (no live agent required), tool-building guide, protocol reference, and recipe cookbook
FR36: The protocol guarantees that tools built against v1 continue to work on any v1.x daemon release without modification
FR37: The daemon accepts inbound events via a socket accessible only to the current OS user
FR38: Tool builders can authenticate REST and WebSocket connections using a bearer token
FR39: Tool builders can access structured changelog information identifying the type and nature of any protocol changes between releases

**Total FRs: 39**

### Non-Functional Requirements

NFR1: The shim must add no more than 5ms at the p95 percentile to Claude Code's hook execution time (hard constraint; benchmarked from day one via `shim/benches/hot_path.rs`)
NFR2: The daemon must introduce no perceptible lag under normal single-developer load on a modern laptop; performance is tuned when evidence warrants, not speculatively
NFR3: The daemon must be ready to accept connections within 2 seconds of cold start on reference hardware; verified via the health endpoint (FR22)
NFR4: The event log is unbounded for V1; the documented V1 escape hatch is deleting or truncating `~/.bowerbird/bower.db` directly; a dedicated `bowerbird gc` command for managed truncation is post-V1
NFR5: When the host filesystem is full (ENOSPC), the daemon logs the drop at error level and closes the ingest connection; the shim treats any write error as fire-and-forget and exits 0 without blocking Claude Code
NFR6: The event log survives unexpected daemon termination; any event acknowledged to the shim is durable on restart (guaranteed by WAL-mode atomic writes)
NFR7: The daemon accepts unbounded event ingest rate in V1 for single-developer workloads; no rate limiting or burst protection; this is a documented design limitation
NFR8: Prebuilt binaries target currently-supported macOS versions on both x86_64 and arm64
NFR9: Linux prebuilts target glibc-based distributions; musl deferred post-V1
NFR10: The `cargo install` path requires only the Rust stable toolchain; no nightly features
NFR11: The daemon bearer token is a UUID4 value, stored in the system keychain (macOS Keychain / Linux Secret Service) and retrieved via `bowerbird auth token`
NFR12: Fallback order when keychain unavailable: (1) environment variable, (2) on-disk config file in `~/.bowerbird/`; fallback mechanism is documented
NFR13: If no token is resolvable via any fallback path, the daemon exits non-zero with a human-readable error to stderr
NFR14: Token rotation requires a daemon restart; the daemon reads the token once at startup and does not hot-reload it
NFR15: The shim failure log is created with mode `0600` regardless of the process umask
NFR16: The daemon logs at error level by default; `-v` and `-vv` flags expose progressively more detail; each log line follows the format `<ISO8601 timestamp> <LEVEL> <message>`; structured JSON logging deferred to V2
NFR17: On unexpected crash, the daemon writes crash information to `~/.bowerbird/`; no external crash reporting
NFR18: A daemon metrics endpoint is deferred until usage patterns justify it; health and readiness endpoints (FR22, FR23) are sufficient for V1
NFR19: No breaking changes to the REST or WebSocket protocol within any v1.x release series; tools built against v1.0 continue to work on any v1.x daemon without modification (anchors FR36)
NFR20: The daemon's ingest socket listen backlog is at minimum 128; the shim exits non-zero on `ECONNREFUSED` or socket-not-found (daemon unreachable), and exits 0 on mid-write errors (transient daemon issues, backpressure)
NFR21: The daemon auto-migrates the SQLite schema on startup; migration failures are fatal with a human-readable error to stderr
NFR22: The V1 event log schema includes a timestamp column on all event rows to support future event-log management without schema changes

**Total NFRs: 22**

### Additional Requirements & Constraints

**Technical Constraints:**
- Rust throughout core crates (protocol, shim, daemon, adapter-claude); TypeScript/Node for examples only
- Single-threaded Tokio runtime (current_thread); shim gets zero Tokio
- SQLite via rusqlite (bundled feature), WAL mode, synchronous=NORMAL
- axum for HTTP/WebSocket surface; deadpool-sqlite for async DB pooling
- JSON wire format; TOML for adapter config files
- Two socket surfaces: Unix domain socket for ingest (shim→daemon), TCP for tools
- `#![deny(unsafe_code)]` at every crate root
- CI must pass on macOS-latest and ubuntu-latest
- Shell script budget: < 200 lines total
- Core LOC budget: 5K–7K (alarm at 10K)

**Business Constraints:**
- Solo maintainer (pickles); contribution model auto-closes PRs by default
- No Windows support (explicit scope cut)
- No distro packaging (Homebrew + cargo install only)
- No HITL backflow, no tool blocking, no personas, no LAN/multi-host

**Integration Requirements:**
- Claude Code hook mechanism integration via shim
- `~/.claude/settings.json` atomic read/merge/write for hook installation
- macOS Keychain / Linux Secret Service for bearer token storage
- GitHub Actions CI; prebuilt binaries via GitHub Releases

### PRD Completeness Assessment

The PRD is thorough and well-structured with:
- 39 clearly numbered and categorized Functional Requirements
- 22 clearly numbered Non-Functional Requirements
- 4 detailed user journeys with explicit capability mappings
- A phased MVP/post-MVP/vision scope breakdown
- Specific performance bars with measurement methodology
- A defined 10-contract-test acceptance gate
- Clear documentation deliverables enumerated

**Notable PRD gaps or areas requiring downstream clarification:**
- No explicit requirement for the `bowerbird auth token` CLI beyond NFR11/NFR12 — it is listed in the CLI surface table but not as a numbered FR
- Shim-when-daemon-is-down behavior is still marked Open in project-context; PRD (NFR5/NFR20) partially addresses it (fire-and-forget, exit 0) but the full decision is not locked
- Event-log truncation policy explicitly deferred (NFR4)
- `bowerbird gc` explicitly post-V1 — not a V1 requirement
- The `bowerbird replay` / `bowerbird export` commands appear in the CLI surface table and FR31/FR32 but the replay file format and bundled fixture strategy have implementation detail left open

---

## Epic Coverage Validation

### ⚠️ CRITICAL: No Epics & Stories Document Found

No epics or stories document was identified during document discovery. This means there is **zero traceability** between the 39 PRD Functional Requirements and any planned implementation work.

### Coverage Matrix

| FR Number | PRD Requirement (summary) | Epic Coverage | Status |
|---|---|---|---|
| FR1 | Shim captures hooks without perceptible latency | **NOT FOUND** | ❌ MISSING |
| FR2 | Shim operates without blocking calls | **NOT FOUND** | ❌ MISSING |
| FR3 | Install/remove hook via CLI without manual config editing | **NOT FOUND** | ❌ MISSING |
| FR4 | Adapter normalizes Claude Code payloads to canonical format | **NOT FOUND** | ❌ MISSING |
| FR5 | Shim logs failures to file, not stdout/stderr | **NOT FOUND** | ❌ MISSING |
| FR6 | Daemon persists events atomically with projection | **NOT FOUND** | ❌ MISSING |
| FR7 | Daemon survives unexpected termination without corruption | **NOT FOUND** | ❌ MISSING |
| FR8 | Cursor-based event log query | **NOT FOUND** | ❌ MISSING |
| FR9 | Daemon exposes oldest available event ID | **NOT FOUND** | ❌ MISSING |
| FR10 | Subscribe to event stream over persistent connection | **NOT FOUND** | ❌ MISSING |
| FR11 | Filter subscription by topic | **NOT FOUND** | ❌ MISSING |
| FR12 | Wildcard subscription across all sessions | **NOT FOUND** | ❌ MISSING |
| FR13 | Notify tools of new sessions without reconnect | **NOT FOUND** | ❌ MISSING |
| FR14 | Notify tool of missed events with lag count (dropped frame) | **NOT FOUND** | ❌ MISSING |
| FR15 | Deliver state snapshot on connect | **NOT FOUND** | ❌ MISSING |
| FR16 | Multiple simultaneous tool connections | **NOT FOUND** | ❌ MISSING |
| FR17 | Send shutdown notification to connected tools | **NOT FOUND** | ❌ MISSING |
| FR18 | List known sessions via REST | **NOT FOUND** | ❌ MISSING |
| FR19 | Retrieve current projected state of a session | **NOT FOUND** | ❌ MISSING |
| FR20 | Retrieve paginated event history | **NOT FOUND** | ❌ MISSING |
| FR21 | Retrieve per-session event statistics | **NOT FOUND** | ❌ MISSING |
| FR22 | Liveness check without auth | **NOT FOUND** | ❌ MISSING |
| FR23 | Readiness check without auth | **NOT FOUND** | ❌ MISSING |
| FR24 | Track multiple concurrent sessions by (source, session_id) | **NOT FOUND** | ❌ MISSING |
| FR25 | Maintain current-state projection per session | **NOT FOUND** | ❌ MISSING |
| FR26 | Tolerate missing hook events without inconsistent state | **NOT FOUND** | ❌ MISSING |
| FR27 | Install via prebuilt binaries (no Rust toolchain needed) | **NOT FOUND** | ❌ MISSING |
| FR28 | Install from source via cargo | **NOT FOUND** | ❌ MISSING |
| FR29 | Start/stop daemon independently of hook config | **NOT FOUND** | ❌ MISSING |
| FR30 | Check daemon status and version via CLI | **NOT FOUND** | ❌ MISSING |
| FR31 | Replay recorded event sequence without live agent | **NOT FOUND** | ❌ MISSING |
| FR32 | Export session events to file for replay/debug | **NOT FOUND** | ❌ MISSING |
| FR33 | Reference implementations for subscription, fan-out, dropped-frame recovery | **NOT FOUND** | ❌ MISSING |
| FR34 | Run reference implementations against bundled fixtures | **NOT FOUND** | ❌ MISSING |
| FR35 | Documentation: quickstart, tool-building guide, protocol ref, cookbook | **NOT FOUND** | ❌ MISSING |
| FR36 | v1.x protocol backward compatibility guarantee | **NOT FOUND** | ❌ MISSING |
| FR37 | Ingest socket accessible only to current OS user | **NOT FOUND** | ❌ MISSING |
| FR38 | Bearer token authentication for REST and WebSocket | **NOT FOUND** | ❌ MISSING |
| FR39 | Structured protocol changelog | **NOT FOUND** | ❌ MISSING |

### Missing Requirements

**All 39 Functional Requirements are uncovered.** There are no epics or stories documents to provide traceability.

### Coverage Statistics

- Total PRD FRs: **39**
- FRs covered in epics: **0**
- Coverage percentage: **0%**
- Total PRD NFRs: **22**
- NFRs covered in epics: **0**

### Impact Assessment

This is the single most significant readiness gap. Without epics and stories, there is no:
- Work breakdown for developers or AI agents to execute against
- Sequencing or dependency ordering across the codebase
- Acceptance criteria that map back to PRD requirements
- Any means to confirm that all 39 FRs and 22 NFRs will be implemented

**Recommendation:** Epics and stories must be created before implementation can begin. The PRD is complete and sufficient to drive epic decomposition immediately.

---

## UX Alignment Assessment

### UX Document Status

**Not Found** — and assessment concludes: **not required for this project type.**

### Rationale

bowerbird is a headless infrastructure substrate. It has no user-facing UI of its own:

- **CLI surface** (`bowerbird install`, `bowerbird start`, `bowerbird status`, etc.) is the only interactive surface bowerbird provides directly.
- **WebSocket + REST API** is consumed by tool builders programmatically.
- **Reference examples** (`examples/`) are demonstration tools authored by tool builders — not part of bowerbird's own UI.

A formal UX design document is appropriate for web apps, mobile apps, or desktop applications with visual workflows. bowerbird is none of these. The PRD's four user journeys (tool builder first tool, iterating without disruption, tool user installing a shared tool, troubleshooting) adequately capture the tool-builder experience and serve the role a UX document would otherwise play.

### CLI UX Considerations (implied, no separate document needed)

The CLI experience is partially defined by the PRD's CLI surface table and user journeys. No additional formal UX specification is needed before implementation; CLI behavior is defined at the story level.

### Alignment Issues

None — no UX ↔ PRD misalignments exist because no UX document is in scope.

### Warnings

⚠️ **Low-severity:** The PRD's documentation deliverables (`docs/presenter-authoring.md`, `docs/cookbook/`, `docs/protocol.md`, Quickstart) have their own usability and structure concerns. These are not UX design artifacts, but they represent the "user experience" of the developer documentation path. No formal doc-UX review has been done, but this is acceptable to defer to the story level.

No blocking UX gaps identified.

---

## Epic Quality Review

### 🔴 CRITICAL: No Epics Exist — Quality Review Cannot Be Performed

There are no epics or stories documents to evaluate. This step documents the structural gap.

**Best Practices Compliance Checklist (applied globally):**

| Criterion | Status |
|---|---|
| Epics deliver user value | ❌ N/A — no epics exist |
| Epics can function independently | ❌ N/A — no epics exist |
| Stories appropriately sized | ❌ N/A — no stories exist |
| No forward dependencies | ❌ N/A — no stories exist |
| Database tables created when needed | ❌ N/A — no stories exist |
| Clear acceptance criteria | ❌ N/A — no stories exist |
| Traceability to FRs maintained | ❌ N/A — no stories exist |

### Critical Violations

🔴 **V1 — No Epics or Stories Document:** The project has a complete PRD with 39 FRs and 22 NFRs but zero implementation planning artifacts. There is no work breakdown, no user-value decomposition, no acceptance criteria, and no sequencing.

### Guidance for Epic Creation

When epics are created, they must avoid these known pitfalls given bowerbird's nature:

**Watch for technical-milestone epics (forbidden):**
- ❌ "Set up SQLite schema" → ✅ "Tool builders can query their session's event history"
- ❌ "Implement WebSocket server" → ✅ "Tool builders can subscribe to live agent activity"
- ❌ "Shim binary implementation" → ✅ "Claude Code agent activity is captured without slowing Claude"

**Watch for ordering traps:**
- The shim (FR1–FR5) depends on the ingest socket (FR37), which depends on the daemon. The first epic must produce a working end-to-end path — even if minimal — not isolated layers.
- The protocol crate must exist before shim or daemon can be built. Treat it as a Story 1.x pre-condition, not its own epic.

**Suggested epic decomposition areas (not prescriptive):**
1. End-to-end event capture and persistence (shim → ingest → SQLite)
2. Real-time event streaming to tools (WebSocket pub/sub)
3. REST snapshot and history API
4. Installation, lifecycle, and CLI surface
5. Developer experience: replay, export, reference examples, documentation
6. Protocol stability: backward-compat guarantee, versioned changelog, contract tests

**Brownfield considerations:**
- The project is pre-MVP with significant design corpus but no code. Treat as greenfield for epic/story structure, but reference the existing ADR (`docs/decisions/0001-project-name.md`) and design docs to avoid re-deciding settled questions.

---

## Summary and Recommendations

### Overall Readiness Status

# ❌ NOT READY

bowerbird has an excellent PRD but is missing the implementation planning layer entirely. Implementation cannot begin in a structured, traceable way without epics and stories.

### Issues Summary

| Category | Severity | Count | Detail |
|---|---|---|---|
| Missing epics & stories | 🔴 Critical | 1 | 0% FR traceability; 39/39 FRs unplanned |
| Missing architecture document | 🟠 Major | 1 | No formal architecture doc; project-context.md partially compensates |
| Open design decisions | 🟡 Minor | 10 | Shim-when-down, MSRV, time/ID types, etc. — documented in project-context.md |
| PRD minor gaps | 🟡 Minor | 2 | `bowerbird auth token` not in FR list; replay fixture format underspecified |

**Total issues: 14 across 4 categories**

### What Is Working Well

- **PRD is thorough and implementation-ready.** 39 numbered FRs, 22 numbered NFRs, 4 user journeys, explicit performance bars, a 10-contract-test acceptance gate, and documentation deliverables — all present and clear.
- **project-context.md is an unusually strong substitute for an architecture document.** Technology choices (Tokio, axum, rusqlite, deadpool-sqlite), crate structure, pub/sub topology, SQLite pool design, tracing patterns, CI matrix, and module discipline are all documented with rationale. An architecture doc would largely restate this.
- **Scope discipline is strong.** Explicit no-list items, substrate-not-actor axioms, and clear MVP vs. post-MVP cuts mean epic authors won't be second-guessing what's in scope.
- **No UX gaps.** bowerbird's headless substrate nature means no UI design work is outstanding.

### Critical Issues Requiring Immediate Action

**1. Create Epics & Stories (BLOCKER)**

The only true blocker. All 39 FRs and 22 NFRs need to be decomposed into epics and user stories with acceptance criteria before any structured implementation can begin.

Suggested approach:
- Use the `bmad-create-epics-and-stories` skill against the PRD
- Ensure epics are user-value oriented (not technical milestones)
- Ensure each story produces a runnable, testable increment
- Ensure the 10 required contract tests land in specific stories, not as a free-floating checklist

**2. Resolve the 10 Open Design Questions (Pre-Implementation)**

The following Open questions in `project-context.md` must be locked in ADRs **before** the code that depends on them is written:

| Open Question | Affects | Must decide before |
|---|---|---|
| Shim-when-daemon-is-down | Shim binary, daemon startup | First shim event-emit code |
| Protocol gap detection (sequence numbers) | Protocol crate, presenter reconnect | v1 ships |
| MSRV (minimum Rust version) | Workspace Cargo.toml | First Cargo.toml commit |
| Time and ID types (SystemTime/chrono, UUIDv7/ULID) | Wire format, schema | First DB row written |
| Auth token storage (keychain vs. file fallback) | NFR11/NFR12 implementation | Daemon startup code |
| AGENTS.md naming | Contributing docs | Before doc structure is set |
| Event-log truncation policy | NFR4 | v1 ships (affects gap-detection behavior) |
| Adapter contract shape for future adapters | adapter-codex etc. | Second adapter begins |
| Reference SDK question (@bowerbird/presenter) | Post-MVP; revisit after first external tool | After first real presenter |
| Cookbook anchor tooling (mdBook vs. hand-rolled) | Doc build | Second cookbook entry |

### Recommended Next Steps

1. **Run `bmad-create-epics-and-stories`** — the PRD is ready; use it immediately to generate epics and stories. This unblocks all implementation.
2. **Resolve the top-4 Open questions as ADRs** before the corresponding code lands: shim-when-down, MSRV, time/ID types, auth token storage. These have concrete "must decide before X" triggers that are imminent.
3. **Treat `project-context.md` as the architecture document** for the epics session. It contains all the architectural decisions needed to write stories against. No separate architecture document is needed before epics can be created.
4. **Create `docs/no-list.md`** — referenced in the project axioms and ADR format but not yet written. Low effort, high value for keeping epics scoped correctly.
5. **After epics exist, re-run this readiness check** to validate FR coverage and epic quality.

### Final Note

This assessment identified **14 issues** across **4 categories**. The single critical blocker is the absence of epics and stories — everything else is pre-existing design work that the project-context.md has already largely addressed. The PRD quality is high; epic creation can begin immediately. The open design questions should be resolved in parallel with epic writing, targeted at the questions whose "must decide before" trigger is earliest.

---

*Assessment completed: 2026-05-16*
*Assessor: bmad-check-implementation-readiness workflow*
*Report: `docs/bmad/planning-artifacts/implementation-readiness-report-2026-05-16.md`*
