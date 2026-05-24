# Story Automator Learnings

## 2026-05-24 — Epic 2 (story 2.5)

### What worked
- **Source-of-truth verification recovered from a mid-run crash.** When the Claude session was killed during code-review bookkeeping, sprint-status.yaml + story file `Status:` confirmed the work was actually `done`. The orchestrator's in-memory state was stale but the recoverable invariant held. (v1.9.0 rule paying off.)
- **Per-step kill + marker heartbeat kept tmux tidy** across many spawn/monitor/kill cycles. No zombie sessions after the cleanup pass.
- **`verify-step` + `verify-code-review` are cheap and authoritative** — running them after every monitor cycle is the right default, even when the monitor returned `verified_complete`.

### What didn't work — upstream bugs (captured in taskwarrior, project `bmad-story-automator`)
- **Fish shell breaks `tmux_runtime.py:resolve_command_shell()`** (task uuid `07ff4036`). The generated command body is bash syntax, but the resolver prefers the user's interactive shell. Fish (or any non-bash) → pane dies with exit 127 before the agent launches. Local fix: reorder candidates to prefer bash. Upstream fix should either force bash, or generate POSIX-only command bodies.
- **Codex `-s workspace-write` sandbox blocks network egress** (task uuid `60a09d05`), which breaks `cargo`/`npm`/`pip` dep resolution in dev-story validation. Codex correctly leaves story status `in-progress`, but the orchestrator interprets `normal_completion` as success. Local fix: change to `-s danger-full-access`. Upstream should make this configurable (project setting or env var) — Rust/Node/Python projects need network during validation.
- **`commit-story` uses `git add -A`** and bundles orchestrator-side patches into the story commit. Awkward when the orchestrator itself needs hotfixes during the run. Upstream improvement: take a pathspec or use the story's File List as the scope.
- **Retrospective session "re-orchestrates itself" near wrap-up.** The retro prompt contains references to "load the first step file" that pulled claude back into an orchestrator loop in the same tmux pane. Killed cleanly but produced a confusing pane.

### Friction patterns
- **Stop hook fires every Claude turn-end during long child sessions.** During multi-minute codex/claude tmux sessions, the orchestrator-side Claude has nothing to do but wait — but each turn-end fires the marker-active stop hook, forcing a no-op response. The recovery doc handles this, but the noise/token cost adds up. Possible fixes upstream: hook honors a `waiting_on_child_session` marker state, or rate-limits firings.
- **`state-update` only handles flat keys.** Nested keys like `agentConfig.defaultPrimary` silently no-op with `keys_not_found`. Direct file edit required as a workaround.
- **Bash 10-min timeout vs. high-complexity codex sessions.** Monitor cycles need re-invocation pattern. Could be smoother if monitor reported intermediate `still-running` heartbeats.

### Per-task wall time (story 2.5, high complexity)
| Task | Agent | Wall time | Notes |
|---|---|---|---|
| create-story | codex | ~13 min | First attempt timed out at 13min, second pass completed |
| dev-story (attempt 1) | codex | ~70 min | All 7 tasks marked [x] but cargo blocked by sandbox |
| dev-story (attempt 2, post-fix) | codex | ~12 min | Validation pass after sandbox fix; full test suite green |
| automate | codex | <1 min | Test coverage plan only |
| code-review | codex | ~6 min | Added pool.close() + more tests |
| retrospective | claude | ~15+ min | Produced 34KB retro + 4 doc-drift fold-ins |

### Forward action
- Future runs default to **claude** for all tasks (codex was switched out per user choice after this run).
- Upstream bugs are in `task project:bmad-story-automator +upstream` for reporting.
