# Sprint Change Proposal: Story 5.9 data-dir/socket are test seams, not a relocatable-install feature

Date: 2026-06-05
Author: @pickles (with dev-agent analysis)
Scope classification: **Minor** (Direct Adjustment to Story 5.9 spec + ADR 0007; no rollback, no MVP change)
Related: Story 5.9 (`docs/bmad/implementation-artifacts/5-9-daemon-start-on-login.md`); ADR 0007 (`docs/decisions/0007-daemon-start-on-login.md`); ADR 0003 (shim hot-path discipline); `sprint-change-proposal-2026-06-01-dogfood-triage.md` (Finding 1, the parent of Story 5.9)

## 1. Issue Summary

Story 5.9 has been through **five `bmad-code-review` passes** producing roughly **30 findings**, none of which touched `crates/` (the entire surface is `src/commands/{install,start,stop,uninstall,launch_agent}.rs` + docs/tests). The story has bounced `review -> in-progress -> review` five times.

Reviewing the findings across all five passes, one cluster (call it "theme 1") accounts for about **9 of the 30 findings** and recurs every pass:

- Pass 1 F1 (plist carries no daemon env), Pass 3 F2 (`start` ignores registered env), Pass 3 F4 (relative ingest sock), Pass 4 F2 (no-env plist falls back to CLI dir), Pass 4 F3 (uninstall ignores registered env), Pass 4 F5 (custom socket parent), Pass 5 F1 (shim socket divergence), Pass 5 F3 (plist env load fallible), Pass 5 F4 (`stop` ignores registered env).

Every one of these has the same shape: *command X resolves the daemon's data-dir/socket from a different source than the launchd plist registered, so a `BOWERBIRD_DATA_DIR=/A` install diverges from a later `start`/`stop`/`uninstall`/shim run that lacks that env.* Each pass patches one more call site to read the plist; the next pass finds the next site that does not.

### Root cause

`BOWERBIRD_DATA_DIR` and `BOWERBIRD_INGEST_SOCK` are **test-isolation env seams**, not a designed user-facing "configurable data location" feature. Evidence:

1. The daemon's own resolver comment (`crates/daemon/src/main.rs:95-99`) states the vars exist so "tests can run isolated daemon instances against a TempDir," and notes a *future* `--data-dir` flag for "Story 3.2's lifecycle CLI."
2. **There is no `--data-dir` (or `--socket`) clap flag.** Story 3.2's intended flag never shipped. The only interface is the env var.
3. **The vars are undocumented for users.** Neither appears in `INSTALL.md` or `README.md` as a knob.
4. **The thin shim cannot follow a custom data dir.** `crates/shim/src/main.rs:115-123` reads only `BOWERBIRD_INGEST_SOCK`, never `BOWERBIRD_DATA_DIR`. Story 5.9 must not touch the shim (ADR 0003 hot-path discipline), so a custom-data-dir install is *structurally* unfollowable by the shim no matter how many CLI commands learn to read the plist. Pass 5 F1 is that contradiction finally surfacing.

Story 5.9 correctly embeds the daemon env into the launchd plist (launchd does not inherit the install-time shell env, so a launchd-started daemon needs *some* env baked in: this is real and was Pass 1 F1). But that embedding accidentally **promoted test plumbing into a production contract** requiring install/start/stop/uninstall/shim to all agree on an arbitrary custom location. No real user reaches that scenario: the default `~/.bowerbird` makes every component resolve identically. The divergence is only ever created by a test (or by hand-exporting an undocumented env var at install but not at runtime).

## 2. Impact Analysis

- **Epic impact:** Epic 5 only. No re-sequencing.
- **Story impact:** Story 5.9 gains one scope-constraint AC and a Dev-Notes section. Three open Pass-5 findings (F1, F3, F4) are dismissed as "won't fix: spec corrected." Three (F2, F5, F6) remain as genuine dev work.
- **Artifact conflicts:** ADR 0007 needs a scope-clarification subsection. `architecture.md` and `INSTALL.md` need **no** change (custom relocatable installs were never claimed there).
- **Technical impact:** Dissolves the theme-1 churn class without code. The plist still embeds the resolved env (needed for launchd env-inheritance and for the test harness, which sets the env consistently across its own commands). No new feature, no shim change.

## 3. Recommended Approach

**Direct Adjustment.** Declare custom data-dir/socket a test-only seam in the spec and ADR; the supported install is the default path; the plist embed exists for launchd env-inheritance, not relocatable installs. Then resolve the three real Pass-5 findings in dev-story.

Rejected alternatives:

- **Make it a real feature** (add `--data-dir`, persist to a config file every component including the shim reads): real work, its own story, and it requires touching the shim (ADR 0003 conversation). Out of scope for a dogfood-supervision story.
- **Reject non-default locations in code** (install errors on a custom `BOWERBIRD_DATA_DIR`/`INGEST_SOCK`): same effect as the chosen approach but breaks the test harness, which relies on the env override to run launchctl-seam tests against a TempDir. The seam must keep working for tests; we just stop treating cross-command divergence as a product bug.

## 4. Detailed Change Proposals

### 4.1 Story 5.9: new AC 12 (scope constraint)

ADD after AC 11:

> 12. **Custom data-dir/socket are test seams, not a relocatable-install feature.** `BOWERBIRD_DATA_DIR` and `BOWERBIRD_INGEST_SOCK` exist for test isolation (running an isolated daemon against a TempDir) and for launchd env-inheritance (the plist embeds the *resolved* values because launchd does not inherit the install-time shell env). They are NOT a supported user-facing "install bowerbird somewhere other than `~/.bowerbird`" feature: there is no `--data-dir`/`--socket` flag, the vars are undocumented for users, and the thin shim (`crates/shim`, untouched per ADR 0003) reads only `BOWERBIRD_INGEST_SOCK` and never `BOWERBIRD_DATA_DIR`, so it cannot follow a relocated data dir. The supported install is the default path (`$HOME/.bowerbird`), where install/start/stop/uninstall/shim all resolve identically. Code-review findings about "command X diverges from the registered plist env when a custom data-dir/socket is set at install but not at runtime" are **out of scope** for this story: that scenario is reachable only by a test (which sets the env consistently) or by an undocumented manual export, not by any product path. A genuine relocatable-install feature (a `--data-dir` flag persisted to a config file that every component, including the shim, reads) is a deliberate follow-up story, not this one.

### 4.2 Story 5.9: new Dev Notes subsection

ADD under Dev Notes (after "Architecture / project-context compliance"):

> ### Scope: data-dir/socket env vars are test seams (review pass 5)
>
> See AC 12. The plist embeds the resolved `BOWERBIRD_DATA_DIR` (and `BOWERBIRD_INGEST_SOCK` when set) for one reason: launchd starts the daemon from a minimal environment and does not inherit the shell env present at `bowerbird install` time, so a launchd-started daemon must read its location from the plist. That is NOT a promise that a custom location is honored consistently across every lifecycle command and the shim. The shim reads only `BOWERBIRD_INGEST_SOCK` and never `BOWERBIRD_DATA_DIR`, and the story does not touch the shim (ADR 0003), so a relocated data dir is structurally unfollowable by the hot path. Do not implement per-command plist-env reads to chase divergence in a custom-location scenario: the default `$HOME/.bowerbird` install is the supported surface and resolves identically everywhere. If relocatable installs become a real requirement, that is a `--data-dir`-flag-plus-config-file story that deliberately revisits the shim.

### 4.3 ADR 0007: new Consequences subsection

ADD after "Review-pass-1 refinements (2026-06-03)":

> ### Review-pass-5 scope clarification (2026-06-05)
>
> Five review passes kept re-discovering the same class of finding: a lifecycle command resolving the daemon's data-dir/socket from a different source than the launchd plist registered, under a custom `BOWERBIRD_DATA_DIR`/`BOWERBIRD_INGEST_SOCK` install. This clarifies the scope of the env embedding: `BOWERBIRD_DATA_DIR`/`BOWERBIRD_INGEST_SOCK` are **test-isolation seams** (run an isolated daemon against a TempDir) and the plist embeds their *resolved* values **only** because launchd does not inherit the install-time shell env. They are not a supported relocatable-install feature: there is no `--data-dir` flag, the vars are undocumented for users, and the shim (untouched here per the hot-path discipline above) reads only `BOWERBIRD_INGEST_SOCK` and never `BOWERBIRD_DATA_DIR`, so it cannot follow a relocated data dir. The supported install is the default `$HOME/.bowerbird`, where every component resolves identically. Cross-command divergence under a custom location is a test-controlled scenario, not a product path, and is out of scope. A real relocatable-install feature (a `--data-dir` flag persisted to a config file every component, including the shim, reads) is a deliberate follow-up, tracked in `deferred-work.md`.

### 4.4 Story 5.9: Pass-5 findings retriage

The six Pass-5 findings split by this scope clarification:

**Dismissed (won't fix: spec corrected by AC 12 / §4.3):**

- **F1** (LaunchAgent socket diverges from shim socket): the shim cannot follow a custom socket by design; default install has no divergence.
- **F3** (plist env load fallible instead of silent-empty): only matters when distinguishing "custom env present but unreadable" from "no env"; under the default install there is no custom env to misread, and the launchd-default fallback is correct.
- **F4** (`bowerbird stop` ignores registered env): `stop` is PID-file SIGTERM at the default path; the only divergence is a custom-data-dir install, now out of scope.

**Remain (genuine, resolve in dev-story): independent of the data-dir scope:**

- **F2** (check PID/singleton holder before bootstrap/kickstart even when the socket is down): real, because the daemon acquires the singleton *before* binding the socket, so "socket down" does not prove "no competitor." Default-install-relevant.
- **F5** (legacy `launchctl load`/`unload` fallback can be bypassed by fallible `print` verification): real correctness bug in the modern-API-unavailable path, independent of data dir.
- **F6** (`--no-stop` docs overstate what plist removal does to an already-loaded in-session launchd job): real docs/behavior gap, independent of data dir.

## 5. Implementation Handoff

**Minor scope -> Developer agent (dev-story).**

1. Apply the AC 12 / Dev-Notes / ADR-0007 edits in §4.1-4.3.
2. Annotate Pass-5 F1/F3/F4 in the story's Review Findings as dismissed-by-scope (cite this proposal); leave F2/F5/F6 open.
3. Resolve F2/F5/F6 under the normal red-green-refactor dev-story loop.

Success criteria: the next code-review pass no longer files divergence findings against a custom-location scenario (theme 1 is closed by spec), and F2/F5/F6 are resolved with tests on the default-install path.
