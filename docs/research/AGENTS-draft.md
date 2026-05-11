# AGENTS.md

Project-specific rules for both human contributors and AI agents working in this codebase. Read this before touching code.

## What this project is

`bowerbird` is a local daemon that observes coding agents (Claude Code in v1; others later) and exposes their state via pub/sub. Read the [README](README.md) for the elevator pitch and [docs/design/](docs/design/) for the full design rationale.

The substrate's job is to preserve underlying data and expose it to many presenters cheaply. It does not interpret data into application-level concepts (personas, voices, sprites, moods) — that's presenter responsibility.

## Before writing code

1. **Check the no-list** ([docs/no-list.md](docs/no-list.md)). If what you're about to build is listed as `never`, stop. If it's `not yet`, file a discussion before writing code.

2. **Check the relevant ADR** in [docs/decisions/](docs/decisions/). Load-bearing decisions are documented with alternatives considered. If you're about to revert one, read the ADR first and understand the cost.

3. **Check the wire protocol** ([docs/protocol.md](docs/protocol.md)). If your change would require a protocol bump, that's a bigger lift than a normal feature — file a discussion.

4. **Check `docs/cookbook/`** to see if a similar pattern already exists. If yes, follow it. If your change makes the pattern obsolete, update the cookbook in the same PR.

## Coding conventions

### Languages

- **Rust everywhere** for the core (`crates/`). Edition 2021. Stable toolchain.
- **YAML** for adapter configs (`adapters/<source>/*.yaml`). Schemas are in `crates/protocol/schemas/`.
- **TypeScript** for example presenters. No build step beyond `tsc`. Bun-compatible.
- **Shell scripts** kept to install/uninstall and CI. Anything more complex goes in Rust.

### Hot-path rules (shim crate)

The shim is invoked by Claude Code on every hook event. Its budget is **<5ms cold start, p95**. Treat the shim as performance-critical:

- **No allocation on the success path.** Use `&str` everywhere; serialize directly to the network buffer.
- **No async runtime.** Synchronous std::net is faster for one POST.
- **No structured logging on the success path.** Log to stderr only on failure.
- **No config loading at runtime.** Embed defaults at compile time; read overrides from a single small file at startup.
- **No retry on failure.** If the daemon is down, write the event to `~/.bowerbird/spool/` and return immediately. The daemon picks up spooled events on startup.
- **Fire-and-forget always.** The shim never blocks Claude even if the daemon is unhealthy.

Benchmark `shim/benches/hot_path.rs` runs in CI. If your change pushes p95 above 5ms, it doesn't land.

### Daemon code style

- **Tokio for async.** Single-threaded runtime unless contention shows up in profiles.
- **Rusqlite for storage**, in WAL mode with `synchronous=NORMAL`. The event log doesn't need transactional durability beyond "the latest few events might be lost on a hard crash."
- **One module per concern.** `daemon/src/projection.rs`, `daemon/src/pubsub.rs`, `daemon/src/storage.rs`, `daemon/src/ingest.rs`. If a module exceeds ~800 lines, split it.
- **Errors with `anyhow` for top-level paths, `thiserror` for typed error contracts.** Don't unwrap in production code.
- **Public types live in `protocol` crate; internal types live in `daemon` crate.** When in doubt, internal.

### Protocol crate (the stable surface)

Anything in `protocol/` is part of the public API. Changes need a discussion and an ADR.

- **Versioned.** `protocol@v1` is what ships. `v2` would be a parallel namespace, not an in-place change.
- **Wire format is JSON.** Not because it's the most efficient — because it's debuggable from a curl command and parseable from any language.
- **All public types implement `Serialize` and `Deserialize` and have `#[serde(deny_unknown_fields)]`.** Forward-compatibility happens through additive changes within a version, not loose parsing.
- **No dependencies that aren't already in the daemon.** Adding a dep to `protocol` adds it to every consumer.

### Tests

- **Unit tests** live alongside the code (`mod tests`).
- **Integration tests** in `crates/daemon/tests/` and `crates/shim/tests/`.
- **Examples are tested.** Every example in `examples/` runs in CI against a fresh daemon. If an example breaks, either the example or the daemon gets fixed in the same PR.
- **Benchmarks gate the shim.** `shim/benches/hot_path.rs` must stay green.

### Adapter code

Each adapter lives in `crates/adapter-<source>/` with:

```
adapter-claude/
├── Cargo.toml
├── src/
│   ├── lib.rs          # public adapter API
│   ├── hooks.rs        # hook config installation/removal
│   ├── ingest.rs       # event normalization
│   └── projection.rs   # tool-name → reaction enum
└── README.md           # adapter-specific notes
```

Plus the data files in `adapters/<source>/`:

```
adapters/claude/
├── capabilities.yaml   # what this source supports
├── tool-reactions.yaml # tool name → reaction projection
└── settings-merge.json # template for ~/.claude/settings.json install
```

If your adapter needs to do something unusual (Codex uses TOML; OpenCode requires a plugin), document it in the adapter's README and reference it in [docs/adapter-authoring.md](docs/adapter-authoring.md).

## Documentation conventions

### When to update docs

Update docs **in the same PR** as the code change:

- New event kind → update `protocol.md`, the kind table in the source's `tool-reactions.yaml`, and at least one cookbook entry that uses it.
- New STATE topic → update `protocol.md` and the relevant cookbook entry.
- New REST endpoint → update `protocol.md`.
- New capability flag → update `capabilities.yaml` for adapters that support it, plus the capabilities section in `protocol.md`.
- New adapter → write its README plus add a section to `adapter-authoring.md`.

CI fails if a change to `protocol/src/*.rs` doesn't also touch a doc.

### Cookbook style

Every cookbook entry has the same shape:

1. **Problem**: one paragraph stating what the presenter wants to do.
2. **Approach**: one paragraph explaining which substrate signals to use and why.
3. **Code**: a working example, referenced from `examples/` (not inline copy-paste).
4. **Variants**: one or two notes on adapting the pattern (different timescales, different state mappings).

Each entry is ~80-150 lines including code references. If yours is longer, it's probably two entries.

### ADRs

Architecture Decision Records live in `docs/decisions/NNN-kebab-case-title.md`. Format:

```markdown
# NNN. Title

Date: YYYY-MM-DD

## Context
What's the situation that prompted this decision?

## Decision
What we chose, in one sentence.

## Alternatives considered
Each alternative with one paragraph on why rejected.

## Consequences
What changes as a result. What's now harder. What's now easier.
```

Existing ADRs: 001 (Rust for shim and daemon), 002 (SQLite for storage), 003 (two-channel pub/sub), 004 (11-value reaction enum), 005 (hook-route shim).

Write a new ADR for any decision that:

- Changes the wire protocol
- Adds or removes a crate
- Changes which language/runtime a crate uses
- Adds a new ingest model (HookProvider / PluginProvider / TranscriptProvider)
- Changes the storage schema in a non-additive way

## Working with AI agents

This project is built with the expectation that AI agents (Claude Code, Codex, Cursor, etc.) will be working alongside human contributors. Some rules that improve outcomes:

### For humans directing AI agents in this codebase

- Point the agent at this file first: `Please read AGENTS.md before making changes`.
- For non-trivial changes, point them at the relevant ADR and cookbook entry too.
- The shim's hot-path rules are the most common thing AI agents miss. Call them out explicitly.
- When proposing a feature, ask the agent to check `docs/no-list.md` first. If the feature is on the list, the conversation can end early.

### For AI agents working in this codebase

If you're an AI agent reading this:

- **Before any change, read the relevant ADR.** If you're about to revert a load-bearing decision, the ADR will tell you the alternatives that were considered. If you still think the decision should change, write a new ADR that supersedes the old one — don't just change the code.
- **Hot-path code in `shim/` has strict rules** listed above. If you're tempted to add structured logging, a config parse, or an HTTP retry to the shim, stop and ask.
- **Test your examples.** When you write or modify code in `examples/`, run `cargo test --workspace` to confirm CI will pass.
- **Update docs in the same PR.** Don't leave doc updates as a follow-up; they get forgotten.
- **If you're not sure whether your change is on the no-list, ask.** Erring toward "this might be a no" is cheap; building a feature that gets rejected is expensive.

## Common pitfalls

A list maintained from real mistakes that have been made or are likely to be made.

### Pitfall: emitting state changes from the daemon before the event is durably stored

The state-change emission and the event INSERT must happen in the same transaction. Otherwise a subscriber gets a transition notification for an event that doesn't yet appear in the event log, and a presenter that resnapshots via REST will see inconsistent data.

### Pitfall: assuming `session_id` is globally unique

It isn't. The natural key is `(source, session_id)`. Always use both. Claude session IDs and Codex session IDs can collide; just because they haven't yet doesn't mean they won't.

### Pitfall: putting presenter UI concerns in the projection layer

The projection computes `current_state` from events. It does not compute "how the lamp should color this." If you find yourself adding a field like `is_user_attention_needed` to the projection, that's a presenter concern. Stop, file a discussion.

### Pitfall: adding a STATE topic per session field

`state.session.<id>.current_state` is one topic. `state.session.<id>.context` is another. `state.session.<id>.attachment` is another. Resist the urge to add `state.session.<id>.branch`, `state.session.<id>.remote_url`, etc. Field-level topics multiply quickly and most don't change often. Use `state.session.<id>` (whole-row change) plus the three high-frequency ones above.

### Pitfall: assuming hook delivery is reliable

It isn't. Claude Code can drop hooks if the shim is slow or if Claude itself is killed mid-tool-call. The daemon's projection must be robust to missing `PostToolUse` after a `PreToolUse` — fall through to a sane state. Pixel Agents' dual-mode pattern (hooks + JSONL fallback) is what we'd add at M3 if reliability issues materialize.

### Pitfall: writing to `~/.claude/settings.json` non-atomically

`bowerbird install` must do an atomic file replacement. Read, parse, merge, write to `settings.json.tmp`, rename. Anything else risks leaving the user's Claude config in a broken state if interrupted.

### Pitfall: spawning subprocesses on the hot path

The shim must not call `git`, `tmux`, or any other subprocess on the success path. All such enrichment happens daemon-side, where the cost is amortized. The shim's job is just "translate hook payload to wire format, POST, exit."

## Repository-wide commands

```bash
# Build everything
cargo build --workspace

# Run all tests including examples
cargo test --workspace

# Check formatting and lints
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Benchmark the shim
cargo bench -p bowerbird-shim

# Run the daemon against a clean state (for local dev)
RUST_LOG=debug cargo run -p bowerbird-daemon -- \
  --data-dir /tmp/csb-dev --hook-token dev-token

# Install hook config (writes to ~/.claude/settings.json — be careful)
cargo run -p bowerbird -- install --dry-run
```

CI runs `cargo fmt`, `cargo clippy`, `cargo test --workspace`, `cargo bench --no-run`, and the example smoke tests. A PR doesn't merge until all of these pass on macOS and Linux.

## Decision authority

The maintainer has final authority over scope. Contributors can build extensions, adapters, and presenters freely — those don't require maintainer approval. Core changes require discussion and merge approval.

Decisions are made in this order of preference:

1. **Existing ADR or no-list entry settles it.** Cheapest case.
2. **Discussion converges on a path.** Update the ADRs or no-list as part of the resolution.
3. **Maintainer decides** if discussion doesn't converge. The maintainer commits to writing the reasoning down (new ADR or no-list update).

There is no committee, no voting, no consensus requirement. The discipline that keeps the project healthy is the maintainer reading every discussion thread and updating the no-list quarterly.

## Questions

If something in this file is unclear or contradicts something in the code, file an issue with the label `agents-md`. Document drift between AGENTS.md and reality is the most expensive kind of drift because it makes both humans and AI agents wrong simultaneously.