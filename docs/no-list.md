# No-list

These are *intentional* non-targets for bowerbird V1, not oversights. Proposing any of them as a feature will get a polite redirect to a future-story discussion. The list exists so contributors don't repeatedly re-litigate decisions already made.

If you're proposing a new daemon responsibility, read this before opening the issue — it's the cheapest discussion-saver in the project. Mirrors the reader-flow [`project-context.md` §Before writing code](bmad/project-context.md) describes: the no-list is the metal-detector at the end of the checklist.

## The cuts

**No Windows support.** No way to test it locally; better to scope-cut than ship something broken. Don't gratuitously write Windows-hostile code (path separators, line endings) but don't pay for it either. ([`project-context.md` §Scope cuts](bmad/project-context.md))

**No distro packaging.** Prebuilt tarball + `cargo install --git` is the V1 distribution surface. Homebrew, Debian, Arch, and nixpkgs are community-driven if they happen at all; the V1 maintainer does not have the bandwidth to chase distro-specific packaging conventions. ([`project-context.md` §Scope cuts](bmad/project-context.md))

**No HITL (Human-In-The-Loop) backflow.** bowerbird is read-only from the agent's perspective. There is no inbound channel from tools to Claude Code — no way for a presenter to send a message, prompt, or intervention back. ([`project-context.md` §Scope cuts](bmad/project-context.md))

**No tool blocking.** bowerbird observes, never intervenes. Presenters cannot prevent a tool call from executing, cannot delay it, cannot annotate it before Claude sees the result. The substrate is a side-channel, not a control plane. ([`project-context.md` §Scope cuts](bmad/project-context.md))

**No personas / agent-roles abstraction.** Sessions are the unit; identity is `(source, session_id)`. "What agent is this" is presenter-level interpretation — if your tool wants to label sessions by role, it does so in tool code, not by asking the daemon to maintain a persona model. ([`project-context.md` §Scope cuts](bmad/project-context.md))

**No LAN / multi-host.** `127.0.0.1` bind only. A future story can add multi-host but it requires real auth (mTLS, session tokens with rotation) — not the V1 single-user bearer token, which assumes loopback-only access. ([`project-context.md` §Scope cuts](bmad/project-context.md))

**No daemon-side activity-rate or metrics endpoint.** `GET /healthz` and `GET /readyz` are sufficient for V1; `GET /status` covers the small set of human-readable numbers (uptime, event count, WS client count). A future story can add Prometheus or similar when usage justifies it. (`NFR18`)

**No crates.io publishing of `bowerbird`.** The namespace may already be squatted on crates.io; reclaiming it requires owning the name. V1 distribution is GitHub-Releases prebuilt tarball + `cargo install --git https://github.com/technicalpickles/bowerbird --tag vX.Y.Z`. Crates.io publishing is deferred until the namespace question is resolved.

**No `bowerbird gc` event-log truncation command.** The V1 escape hatch is `rm -rf ~/.bowerbird/` (nuclear) or hand-truncate `bower.db` via `sqlite3`. A managed truncation command — selective by session, by age, by event-count — is post-V1 work. (`NFR4`)

**No musl Linux prebuilts.** glibc-only for the V1 prebuilt tarballs (Ubuntu 22.04+, Debian 12+, RHEL 9.0+). musl users (Alpine, Void) install from source via `cargo install --git`. The release pipeline notes this explicitly. (`NFR9`)

**No code signing or notarization on macOS.** Users clear quarantine via `xattr -d com.apple.quarantine`. An Apple Developer ID certificate and the notarization workflow are deferred post-V1; the cost-to-benefit on a solo-maintainer project does not justify the $99/year + automation effort yet. ([`README.md` §Install](../README.md#install))

**No structured JSON logging.** The daemon logs human-readable text at `error` / `info` / `debug` levels via `tracing-subscriber`. Structured JSON output (suitable for log aggregators like Datadog, Splunk, or `stern`) is deferred to V2. (`NFR16`)

**No rate limiting on the replay endpoint or any other surface.** bowerbird targets single-developer workloads; the 1 MiB request-body cap is the only structural limit. A future story can add rate limiting if/when multi-tenant deployments become a thing. (`NFR7`, [`docs/protocol-changelog.md`](protocol-changelog.md) Story 4.1 entry)

## Where this list comes from

The cuts above are drawn from three sources:

- **[`project-context.md` §Scope cuts](bmad/project-context.md)** — the canonical narrative source for the bulk of these cuts; lines 320-326 list the design-time non-targets.
- **The NFRs in [`docs/bmad/planning-artifacts/epics.md`](bmad/planning-artifacts/epics.md) §Non-Functional Requirements** — the formal contract. Each NFR-encoded cut above carries its NFR number.
- **[`docs/protocol-changelog.md`](protocol-changelog.md)** — entries where a behavior was explicitly scoped to "single-developer workload" or "deferred post-V1." The no-rate-limiting clause is restated in the Story 4.1 entry.

When in doubt, the rule is: if it requires infrastructure bowerbird doesn't have (Apple Developer ID, distro maintainer contacts, distributed-systems testing infra), it's a V2 conversation.
