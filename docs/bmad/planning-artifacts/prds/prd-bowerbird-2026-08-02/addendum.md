# Addendum: bowerbird PRD Phase 3 refresh (2026-08-02)

Depth that informed the Phase 3 PRD update but does not belong in the PRD body. Companion to `.decision-log.md` in this directory.

## Live-only subscription flag: design analysis (deferred with trigger)

The gap: `subscribe` cannot express "no snapshot, only new things". `states: []` is documented as unfiltered (send everything), not none. The protocol is strict-inbound by design (a `states` filter on an event topic closes the connection rather than being ignored, on the stated grounds that a discarded filter is a silent lie about presenter intent). By that same logic an inexpressible intent is a real gap, not a convenience.

Mechanics if/when built: the daemon keeps per-connection `snapshotted_keys: HashSet<(source, session_id)>` (`crates/daemon/src/api/ws.rs`), and a live frame for an uncovered session also inserts into it, so coverage accretes from the live stream. A `dropped` frame clears the set. The one real design question for a `snapshot: false` flag is whether skipped keys count as covered:

- Mark them covered: simple, but initial state becomes unreachable without a new connection. Footgun.
- Leave them uncovered (recommended): the keys stay eligible, so a later snapshot-bearing re-subscribe delivers exactly the backlog that was skipped, riding the existing filter-widening rule. Start quiet, ask for the backlog when wanted. Composes with shipped machinery.

Compat note: inbound `ClientMessage` uses `deny_unknown_fields`, so a new subscribe field is rejected by older daemons (connection close). Additive-on-outbound does not cover inbound; presenter guidance would need a version gate (check `hello.daemon_version`). This is the strongest argument for deferring until demand is proven.

Why deferred: the `transition-alerts` entry needs the per-key map pattern anyway for reconnect correctness (a `dropped` frame clears snapshot coverage, and re-subscribes re-snapshot; the map is what keeps unchanged sessions quiet through that). So the pattern is not a workaround for a missing flag; it is load-bearing regardless. Trigger for revisiting: daily use of `transition-alerts` shows the pattern is clumsy in practice.

## Event-before-State ordering guarantee (the enrichment join relies on it)

`crates/daemon/src/projection/session.rs` (post-commit publish): Event is published before State so a presenter consuming both sees Event then State in the same order, per-session. This means by the time a state frame says "now WaitingInput", the event frame carrying `notification_type` (in `events.payload`, verbatim) has already arrived on the same connection. The `transition-alerts` two-topic join (state for the edge, events for the why) needs no buffering heuristics. The daemon also only publishes a StateFrame when `current_state` actually changed (Story 5.2), so every live state frame IS a transition; the per-key map exists to tell snapshot frames from live frames (same wire shape, no marker) and to survive reconnects, not to detect changes.

## First-sighting limitation of the map pattern

A brand-new session appearing mid-run is also a first sighting of its key, so a session that starts and blocks within its very first observed frame is recorded silently instead of alerting. Rare (a new session's first state is normally `Working`) but real; the cookbook entry must state it rather than paper over it. Optional refinement: consult `started_at`; a session that started seconds ago is not from the snapshot. That refinement is also the flagship Axiom 4 example: mechanical fact on the wire, interpretation in the presenter.

## WHERE: pid-to-tmux-pane mapping (presenter-side)

The wire carries `cwd` (repo naming is free) and `last_pid` (Claude Code's PID via shim `getppid`). tmux exposes every pane's root PID: `tmux list-panes -a -F '#{pane_pid} #{session_name}:#{window_index}.#{pane_index}'`. A presenter matches `last_pid` into a pane by walking process ancestry from each `pane_pid`. From a pane identity, the hop is `tmux switch-client`. All interpretation stays presenter-side; the substrate ships only the mechanical facts. Scope per maintainer: WHERE (repo at minimum) is the floor requirement on every attention surface; the hop is stretch, pursued only if it proves cheap.

## bowerbird-deck disposition

Decision (2026-08-02): archive the repo once the `live-board` cookbook entry lands. README pointer to the entry, GitHub archive. Rationale: the deck's role (long-lived live view) moves into the cookbook where it doubles as teaching material; a separate sibling repo was one more thing to maintain and did not stick as a daily driver. The deck README's version pin ("tracks bowerbird main / v0.1.0") becomes moot at archive time.

## Dropped themes (recorded so they are not re-derived from scratch)

From the 2026-08-02 discovery session's theme space, two candidates did not survive journey derivation:

- **Stuck-session detection** (Working for 40 minutes, WaitingInput for 10): the narrated journeys say blocked/done notifications are the fix; timeout heuristics were not demanded. If revisited, note it collides productively with the `STALE_WORKING_MS` retirement question (deferred-work, Story 5.3 entry 1): running a stuck detector would generate exactly the evidence that retirement decision is waiting on.
- **`Reaction::Pause` attention signals**: no journey touched reactions. Folded into the PRD's open question on the `Reaction` surface as a whole.

Both survive as "How to apply it" mentions in the new entries, not as entries.

## Cookbook integration surface (for whoever builds the entries)

Adding a fourth entry touches guards that hardcode three: `tests/cli_docs_drift.rs` README list and its "three per-entry cookbook READMEs" message string; `tests/cli_examples.rs` doc comment plus a smoke test per new entry; `docs/cookbook/README.md` table and Quick run block; the canonical five-section README shape is machine-enforced (no fenced `ts` blocks); each entry needs `package-lock.json` because CI's `typecheck-examples` runs `npm ci`. CI globs `docs/cookbook/*/` so new entries are picked up automatically.

Team agreement A13 applies with force to `transition-alerts`: the snapshot-suppression behavior is a guard, so its test must be observed failing against broken code (make first-sighting alert, watch red, fix, keep).
