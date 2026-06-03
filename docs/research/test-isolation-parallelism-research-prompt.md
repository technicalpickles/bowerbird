# Research prompt: isolating and parallelizing heavyweight Rust integration tests

A reusable prompt for a research agent. The agent does **not** need access to any
codebase. It produces a written best-practices recommendation about how to isolate
and partition integration/contract tests in Rust, so the conflicting ones run
serially while everything else runs in parallel.

---

## Prompt

> **Research task: test isolation and parallel/serial partitioning for heavyweight Rust integration tests**
>
> You are researching, not implementing. You do not have access to any codebase. Produce a written best-practices recommendation grounded in the Rust testing ecosystem as it actually exists, with concrete config/code examples.
>
> **Background.** A common pattern in Rust projects: a workspace has fast, well-isolated unit tests *and* a set of heavyweight integration/contract tests that share process-wide or OS-level state. Typical sources of that shared state:
>
> - Real subprocesses spawned via `assert_cmd` / `std::process::Command` (a binary under test, a daemon, a helper process).
> - Process-global OS signal-handler registration (e.g. SIGTERM/SIGINT handlers a daemon installs at startup).
> - Filesystem fixtures, even when each test uses its own temp dir — collisions still happen via fixed (non-tempdir) paths, PID files, lock files, or Unix domain sockets at well-known locations.
> - A daemon singleton enforced by a PID file + `flock`.
> - Fixed TCP ports (as opposed to ephemeral `127.0.0.1:0` binds).
> - Shared OS keychain/keyring backends.
> - In-process global state: `static`/`OnceCell` singletons, a shared tokio runtime, env vars set process-wide via `std::env::set_var` (which leaks across tests in the same test binary regardless of temp dirs).
>
> The blunt fix many teams reach for is to serialize the *entire* workspace test run:
>
> ```
> cargo test --workspace -- --test-threads=1
> ```
>
> That works but throws away all parallelism, adding real wall-clock cost to CI for the sake of a small minority of genuinely-conflicting tests. There's usually a better-targeted answer.
>
> **Two questions to answer:**
>
> 1. **In general, how should integration/contract tests at this level be isolated** — tests that spawn real subprocesses, install OS signal handlers, and touch the filesystem/keychain/ports? What are the established Rust-ecosystem patterns, and what are their tradeoffs?
> 2. **How should a suite like this be partitioned so only the genuinely-conflicting tests run serially (or with capped concurrency) while everything else runs in parallel** — instead of forcing the whole workspace to `--test-threads=1`?
>
> **Diagnostic framing to include.** Before any team applies a serialization mechanism, they should pin down *what state is actually shared*. Give the reader a checklist for distinguishing real collisions from cargo-culted serialization:
>
> - Separate **in-process** tests (`#[tokio::test]` / `#[test]` exercising library code directly, possibly instantiating an in-process server) from **subprocess** tests (spawning a real binary). The failure modes and the right fixes differ.
> - Identify what is *actually* process-global within a single test binary: signal-handler registration, `static`/`OnceCell` singletons, `std::env::set_var`, a shared tokio runtime, global logging/tracing subscriber init.
> - Identify what is *actually* shared at the OS level across concurrently-running test processes: fixed ports, well-known socket/PID/lock paths, a single keychain entry, a shared scratch directory.
> - Note the common false alarm: tests that *look* unsafe but are actually fine because they use per-test temp dirs, pass env vars *per-subprocess* via `Command::env(...)` rather than process-global `set_var`, and bind ephemeral ports. If isolation is already this good, a global serial flag may be pure cargo-cult — flag that possibility explicitly.
>
> **For question 1, cover at least:**
> - **`cargo-nextest`** and its process-per-test isolation model (each test runs in its own process) vs the default libtest threaded model (all tests in a binary share one process). Be specific about which class of bug process-per-test isolation *actually* fixes (shared in-process signal handlers, in-process global state, `set_var` leakage) and which it does **not** (true OS-level resource contention like a fixed port or a single keychain entry).
> - **The `serial_test` crate** (`#[serial]`, `#[serial(group)]`, `#[parallel]`) for marking a subset serial while the rest run in parallel — including how grouping keys let disjoint groups still run concurrently with each other.
> - **nextest test groups** (`[[test-groups]]` plus `[[profile.*.overrides]]` with `max-threads`) for capping concurrency on a named subset by filter expression, without touching test source.
> - **Binary/file-level partitioning**: separate integration-test binaries (each file in `tests/` is its own binary) run as independent processes; how concurrency is controlled *within* a binary vs *across* binaries.
> - **Designing the collision away**: making tests independent by construction (unique temp dirs, ephemeral ports, per-subprocess env, unique keychain namespaces/in-memory keyring backends, avoiding process-global signal handlers in test builds) so serialization isn't needed at all.
> - Tradeoffs for each: maintenance burden, whether it needs a new dev-dependency, CI config complexity, blast radius when someone adds a new test, and any interaction with `cargo build` parallelism (note the trap where a `.cargo/config.toml` test alias also serializes builds).
>
> **For question 2, deliver a decision framework:**
> - A rubric for sorting tests into buckets: *fully parallel* / *concurrency-capped group* / *strictly serial* — driven by the specific shared resource each test touches.
> - The recommended enforcement mechanism for each bucket, with example config and/or annotations (nextest test-groups config snippet; `serial_test` annotations; filter expressions).
> - How a contributor runs the suite locally and how CI should invoke it, replacing a blanket `--test-threads=1`.
> - A migration path from "everything serial" to "targeted serialization": smallest safe first step, what to verify at each stage, and how to prove the flake class stays dead (e.g. run the now-parallel subset under repeat/stress to confirm no regression before removing the global flag).
> - A rough sense of the wall-clock payoff and when it's *not* worth the added config complexity (small suites, rarely-run tests).
>
> **Output:** a markdown report with these sections — (1) a diagnostic checklist for identifying real shared state, (2) the general isolation patterns with explicit tradeoffs, (3) a partition decision rubric with the enforcement mechanism per bucket and concrete examples, (4) a migration path. Recommend the approach with the best isolation-per-unit-of-config-complexity and justify the pick. Prefer designing collisions away over annotating around them where feasible, and say when that's not feasible.
