# 0002. Ingest socket wire framing and `hook_kind` field ownership

Date: 2026-05-18
Status: Accepted
Deciders: @pickles
Related: ADR-0001 (no conflict); supersedes a TBD note in `architecture.md:984-985` and a contradictory claim in `prd.md:365`
Implementation: `crates/daemon/src/ingest/handler.rs`, `crates/shim/src/**` (to land in Story 1.5)
Affects context.md sections: "Wire format: JSON via serde" (line 171), "Shim hot-path discipline" (line 334)

## Context

Two questions about the shim→daemon ingest path were left unresolved or contradicted across the planning docs:

1. **Wire framing.** PRD line 365 specifies "POST /ingest via HTTP/1.1 over the Unix domain socket." Architecture line 984 marks framing as "TBD at implementation time." Story 1.3 shipped a newline-delimited JSON protocol (single `{json}\n` line in, `200\n` / `503\n` / `400 <reason>\n` line out) — no HTTP verb, headers, or chunked framing. That choice was never documented as a decision, so the PRD remains misleading and the next implementer (Story 1.5) had to reverse-engineer the daemon code to know what to send.

2. **`hook_kind` field ownership.** Architecture lines 618 and 869 say "raw hook JSON bytes on the wire; no normalization in shim." But the daemon handler requires a top-level `hook_kind` field to dispatch to the right adapter codepath (`handler.rs:63-66`), and Claude Code's actual hook stdin payload uses `hook_event_name`, not `hook_kind`. Something has to bridge the two. Story 1.4 left a deferred-work note saying "revisit when shim guarantees `hook_kind`" — that moment is Story 1.5.

## Decision

1. **Wire framing is newline-delimited JSON.** One `{object}\n` request, one status line response (`200\n`, `503\n`, or `400 <reason>\n`). Not HTTP. The PRD's HTTP/1.1 wording is superseded by this ADR; the architecture's "TBD" is resolved.

2. **The shim injects `hook_kind` as a top-level field** before forwarding, using a `--hook-kind <KIND>` CLI argument that the install command (Story 3.1) will write per hook-event entry in `~/.claude/settings.json`. Claude Code's original `hook_event_name` field (if present) is preserved verbatim in the payload — the shim only *adds*, never *renames* or *removes*. This injection is reframed as **transport routing** (a label saying which hook fired), not normalization (which remains adapter-claude's job — interpreting reactions, session state, etc.). Axiom 1 ("substrate observes; does not interpret") is intact.

## Alternatives considered

- **Keep HTTP/1.1 wire framing.** Requires the shim to link an HTTP client (or hand-roll request lines + chunked framing). For a binary with a <5ms p99 budget and a one-line response, this is pure overhead. Rejected when Story 1.3 shipped NDJ; this ADR ratifies that.
- **Daemon handler reads `hook_event_name` as a fallback.** Pure passthrough for the shim, but couples the daemon to Claude Code's schema — exactly what `adapter-claude` exists to isolate. Rejected.
- **Adapter inspects raw bytes to derive hook_kind itself.** Cleanest layering, but requires changing `SourceAdapter::normalize`'s signature (it currently takes `hook_kind` as an explicit argument decided before the adapter is called). Larger blast radius than needed; defer until a second adapter forces the issue.

## Consequences

- **Story 1.5** implements the shim against the NDJ wire and the `--hook-kind` flag; story file already reflects this.
- **Daemon handler** (`handler.rs:63-66`) currently defaults missing `hook_kind` to `"PreToolUse"`. Once Story 1.5 ships and the shim is the only ingest client, that default should become a `400`. Tracked in `deferred-work.md` line 37; a follow-up story will tighten it.
- **PRD and architecture** are now stale on these two points. Either amend in-place when next touched, or rely on this ADR + `Related:` backlinks. Not blocking.
- **Future adapters** (Codex, Gemini, Cursor) will use the same `--hook-kind` flag with their own kind values. The flag becomes part of the cross-adapter shim contract.

## Revisit when

- A second hot-path consumer of the ingest socket (e.g. a bowerbird-internal probe, a second adapter) needs a richer request/response than NDJ supports (multi-frame batching, response body beyond a status line).
- The "shim is now mutating the payload" framing turns out to confuse a real contributor — at which point either rename `--hook-kind` to something more obviously transport-y (`--route` ?) or move kind-derivation into the adapter and pass the raw stdin verbatim.
- Claude Code stops sending `hook_event_name` (renames it), invalidating the "preserve verbatim" half of the decision.
