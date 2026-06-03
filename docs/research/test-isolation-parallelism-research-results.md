# Isolating and Parallelizing Heavyweight Rust Integration Tests

A best-practices recommendation for workspaces that mix fast, well-isolated unit tests with a minority of heavyweight integration/contract tests that share process-wide or OS-level state. The goal: run the genuinely-conflicting tests serially (or with capped concurrency) while everything else keeps running in parallel — instead of forcing the whole workspace to `--test-threads=1`.

**Bottom line up front.** Design collisions away wherever feasible (per-test temp dirs, ephemeral ports, per-subprocess env, unique namespaces). For the residual collisions that *can't* be designed away, the best isolation-per-unit-of-config-complexity is **cargo-nextest test groups**: a few lines of `nextest.toml` express "this filtered subset runs with `max-threads = 1`, everything else stays parallel," with no test source changes and no per-test annotation drift. Use `serial_test` instead only if you must stay on `cargo test` and can't adopt nextest. Reserve a blanket `--test-threads=1` for nothing — it is almost always cargo-cult once you've actually identified the shared state.

---

## 1. Diagnostic checklist: what state is *actually* shared?

Before reaching for any serialization mechanism, pin down what collides. Most "we have to run serial" beliefs survive only because nobody audited them. Work through these in order.

### 1a. Split tests into two populations

The failure modes and fixes differ, so classify every heavyweight test first:

- **In-process tests** — `#[test]` / `#[tokio::test]` that exercise library code directly in the test binary's own process, possibly standing up an in-process server. These share the *test binary's* address space with every other test in the same binary.
- **Subprocess tests** — tests that spawn a real binary via `assert_cmd` / `std::process::Command` (the daemon under test, a helper). The spawned process has its own address space; what these share with each other is strictly *OS-level* resources.

A test can be both (in-process orchestration that also spawns a subprocess). Classify by the strongest coupling it has.

### 1b. Inventory what is process-global *within a single test binary*

The default Rust harness runs all `#[test]` functions in one process across multiple threads. Anything process-global is therefore shared across concurrently-running tests in that binary:

- **Signal-handler registration** (SIGTERM/SIGINT installed at startup). Process-global by definition — the last writer wins, and handlers installed by one test fire for another.
- **`static` / `OnceCell` / `lazy_static` singletons** — a single shared instance for the whole process.
- **`std::env::set_var` / `remove_var`** — mutates the process environment table. This leaks across every test in the same binary *regardless of temp dirs*, and is the single most common "it's flaky only in parallel" culprit. (It is also `unsafe` as of the 2024 edition precisely because of this.)
- **A shared tokio runtime** built as a global singleton.
- **Global logging/tracing subscriber init** (`set_global_default`) — succeeds once; later inits silently no-op or panic, so test ordering changes behavior.

### 1c. Inventory what is shared *at the OS level across processes*

These collide between concurrently-running test *processes* (separate test binaries, or subprocesses they spawn) — process isolation does **not** help here:

- **Fixed TCP/UDP ports** (anything that isn't an ephemeral `127.0.0.1:0` bind).
- **Well-known socket / PID / lock paths** — a Unix domain socket, PID file, or `flock` target at a fixed (non-tempdir) path. The daemon-singleton-via-PID-file-plus-`flock` pattern is explicitly an OS-global mutex by design.
- **A single OS keychain/keyring entry** at a shared service+account name.
- **A shared scratch directory** at a fixed path (vs a per-test temp dir).

### 1d. Rule out the false alarms

A test that *looks* unsafe is often already fine. Flag a global serial flag as probable cargo-cult if all of these hold:

- Each test uses its **own temp dir** (`tempfile::TempDir`), not a fixed path.
- Env vars are passed **per-subprocess** via `Command::env(...)`, *not* via process-global `std::env::set_var`. Per-subprocess env is fully isolated; it never leaks.
- Ports are **ephemeral** (`:0`), with the assigned port read back (from the bound socket's local address, or written by the subprocess to a file in its temp dir).
- Keychain/keyring uses a **per-test unique namespace** or an in-memory backend.

If isolation is already this good, the suite is parallel-safe and the `--test-threads=1` is pure ritual — remove it (after verification; see §4).

### Diagnostic summary table

| Shared resource | Scope | Fixed by process-per-test? | Fixed by serial/group? | Best designed-away form |
|---|---|---|---|---|
| Signal handlers | in-process, process-global | **Yes** | Yes | don't install in test builds |
| `static`/`OnceCell` singleton | in-process | **Yes** | Yes | per-test instance |
| `std::env::set_var` leakage | in-process | **Yes** | Yes (same binary) | per-subprocess `Command::env` |
| Global tracing subscriber | in-process | **Yes** | Yes | `with_default` guard, not global |
| Fixed TCP/UDP port | OS-level | **No** | Yes | ephemeral `:0` bind |
| Fixed socket/PID/lock path | OS-level | **No** | Yes | path inside per-test temp dir |
| Single keychain entry | OS-level | **No** | Yes | unique namespace / in-mem backend |
| Shared scratch dir | OS-level | **No** | Yes | per-test `TempDir` |

The split down the "process-per-test" column is the crux: **process isolation fixes in-process global state but does nothing for OS-level resource contention.** A fixed port collides whether the two contenders are threads or processes.

---

## 2. General isolation patterns and their tradeoffs

### 2a. cargo-nextest's process-per-test model

Nextest runs **each test in its own process**, where the default `cargo test`/libtest model runs all tests in a binary as threads in one shared process. <cite index="3-1">With nextest, the default execution model is now, and will always be, process-per-test.</cite> Nextest itself frames per-test isolation as <cite index="3-1">a principled, zero-coordination solution</cite> for tests against global state — singletons become separated per test, and tests that must alter environment variables or other global context stop interfering.

**What process-per-test actually fixes:** every item in §1b. Each test gets a fresh process, so `static`/`OnceCell` singletons are reborn per test, `set_var` can't leak between tests, signal handlers are per-process, and a global subscriber init is the only init in its process.

**What it does *not* fix:** every item in §1c. Two test processes binding the same fixed port still collide; one shared keychain entry is still one entry; a fixed socket/PID path is still contended. Nextest's own docs are explicit that the right fix for those is to design them away (ephemeral ports, sockets in temp dirs) or, failing that, to use test groups (§3) as a logical mutex.

Secondary benefits worth noting: per-process stdout/stderr capture, per-test timeouts, retry/flaky detection, and the fact that one test crashing the process doesn't cancel its siblings. <cite index="9-1">Cargo-nextest runs every test in its own process. This matters for tests that rely on global state, external APIs, graphics contexts, or other resources that may not behave well when reused across tests.</cite>

**Tradeoffs:** new dev-tool dependency (a binary, installed in CI — not a `Cargo.toml` dependency); doctests are not run by nextest and must still go through `cargo test --doc`; process-spawn overhead per test is real but usually dwarfed by heavyweight integration setup. Adopting nextest is the lowest-source-churn way to neutralize the entire §1b class at once.

### 2b. The `serial_test` crate

A widely-used (`#[serial]` / `#[parallel]`) annotation crate — over 75 million downloads, current version 3.x. <cite index="14-1">Multiple tests with the serial attribute are guaranteed to be executed in serial.</cite> <cite index="14-1">Other tests with the parallel attribute may run at the same time as each other, but not at the same time as a test with serial.</cite> Tests with neither attribute have no timing guarantees relative to the serial set, so when you mark some tests serial you typically mark their would-be-colliding peers `#[parallel]`.

**Grouping keys let disjoint groups run concurrently.** <cite index="13-1">If you want different subsets of tests to be serialised with each other, but not depend on other subsets, you can add a key argument to serial, and all calls with identical arguments will be called in serial.</cite> So `#[serial(keychain)]` and `#[serial(port_8080)]` run concurrently with *each other* while each remains internally serial — a per-resource mutex rather than one global lock. <cite index="13-1">Multiple comma-separated keys will make a test run in serial with all of the sets with any of those keys.</cite>

**In-process vs cross-process.** `#[serial]` uses an in-process lock, so it only serializes tests within the **same** test binary. For tests that run as separate processes (doctests, separate integration binaries), the crate offers `file_serial` / `file_parallel`, which lock via the filesystem. <cite index="18-1">Note that there are no guarantees about one test with serial and another with file_serial as they lock using different methods.</cite> Pick one mechanism per resource; don't mix `serial` and `file_serial` on tests that contend for the same thing.

**Tradeoffs:** adds a real dev-dependency *and* per-test source annotations. Blast radius is the chief weakness — a contributor adding a new test that touches a shared resource must remember to annotate it; forget, and you get a fresh flake with no config-level safety net. Annotations also live in source, so the policy is scattered across files rather than centralized. Works fine under plain `cargo test`, which is its main reason to exist.

### 2c. nextest test groups

Nextest expresses capped concurrency declaratively in config, with **no test source changes**. <cite index="10-1">Nextest allows users to specify test groups for sets of tests. This lets you configure groups of tests to run serially or with a limited amount of concurrency.</cite> <cite index="10-1">Tests that aren't part of a test group are not affected by these concurrency limits. If the limit is set to 1, this is similar to cargo test with the serial_test crate, or a global mutex.</cite>

You declare named groups with a `max-threads` limit and bind tests to them by **filter expression**:

```toml
# .config/nextest.toml
[test-groups]
resource-limited  = { max-threads = 4 }
serial-integration = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'test(resource_limited::)'
test-group = 'resource-limited'

[[profile.default.overrides]]
filter = 'package(integration-tests)'
platform = 'cfg(unix)'
test-group = 'serial-integration'
```

<cite index="10-1">Any tests whose name contains resource_limited:: will be limited to running four at a time</cite> — a logical semaphore with four permits — and <cite index="10-1">on Unix platforms, tests in the integration-tests package will be limited to running one at a time, i.e. serially</cite>, a logical mutex. Everything outside both groups runs at the global concurrency limit. The active group is exposed at runtime via the `NEXTEST_TEST_GROUP` env var (`@global` if ungrouped), and `cargo nextest show-config test-groups` prints exactly which tests landed in which group — a real audit affordance the annotation approach lacks.

**Tradeoffs:** requires nextest. Filter expressions must actually match the intended tests (verify with `show-config`/`list`). The big win is centralization and low blast radius: the policy is one file, scoped by package/name/path predicates, so a new test that matches an existing filter (e.g. lands in the integration package) is captured automatically without touching it.

### 2d. Binary/file-level partitioning

Each file in `tests/` compiles to its **own** integration-test binary. Within a binary, libtest runs `#[test]`s as parallel threads; across binaries, behavior differs by runner. Under `cargo test`, <cite index="23-1">if the package contains multiple test targets, each target compiles to a special executable as aforementioned, and then is run serially.</cite> So plain `cargo test` already serializes *across* integration binaries while parallelizing *within* each — a coarse, accidental form of isolation. Nextest instead schedules all tests from all binaries into one global parallel pool.

You can exploit this: put all mutually-conflicting tests in **one** `tests/serial_stuff.rs` file and rely on within-binary control, or isolate a resource into its own binary. But it's a blunt instrument — it conflates "same file" with "same resource," and the cross-binary serialization under `cargo test` is a side effect, not a guarantee you should lean on. Prefer explicit groups.

**Interaction with `cargo build` parallelism (the alias trap).** `--test-threads` controls *runtime* concurrency only. Build parallelism is governed separately by `--jobs` / `build.jobs`. The trap: teams hide the serial flag in a `.cargo/config.toml` and accidentally also throttle builds. Note that `[build] jobs = 1` and `cargo test -- --test-threads=1` are unrelated knobs — but a careless test alias or a stray `build.jobs = 1` will serialize *compilation* across the whole workspace, which is pure wall-clock loss with zero isolation benefit. Per the Cargo docs, <cite index="23-1">the --jobs argument affects the building of the test executable but does not affect how many threads are used when running the tests.</cite> Keep the two concerns separate: never put `jobs = 1` in config to "fix tests," and prefer a runner-level group config over a `[alias]` that bakes in `--test-threads=1`.

### 2e. Designing the collision away (preferred)

Make tests independent by construction so no serialization is needed:

- **Unique temp dirs** (`tempfile::TempDir`) for every fixture, PID file, lock file, and socket — put the well-known path *inside* the temp dir and pass it to the subprocess.
- **Ephemeral ports**: bind `127.0.0.1:0`, read the assigned port from the socket's local address; for a subprocess, have it write the port to a file in its temp dir (or expose it on stdout). Nextest's docs recommend exactly this — bind port 0 and communicate the actual port back, or use a Unix domain socket in a temp dir (which also works on Windows).
- **Per-subprocess env**: `Command::env("VAR", val)` instead of `std::env::set_var`. This is isolated to that child and never leaks.
- **Unique keychain namespaces** (per-test service/account names) or an **in-memory keyring backend** selected under `cfg(test)`.
- **No process-global signal handlers in test builds**: gate handler installation behind a runtime flag or `cfg` so the in-process server under test doesn't register them, or only exercise signal handling in dedicated subprocess tests.

**Tradeoffs:** the most durable answer — zero ongoing config, zero annotation drift, and the suite is correct by construction so a new test can't reintroduce the flake class. Upfront refactoring cost is the price, and a few collisions genuinely can't be designed away: a single real OS keychain you must integration-test against, a port that an external fixed dependency hard-codes, or hardware/GPU contexts limited to one-per-machine. Those residual cases are what groups/serialization exist for.

---

## 3. Partition decision rubric

Sort each heavyweight test into one of three buckets, **driven by the specific resource it touches**, then apply the matching enforcement.

### The rubric

| Bucket | When | Enforcement |
|---|---|---|
| **Fully parallel** | Touches no shared in-process global *and* no shared OS resource (or all collisions designed away per §2e). The common case after a real audit. | None. Default scheduling. |
| **Concurrency-capped group** | Contends for a resource that tolerates *N* > 1 concurrent users but not unlimited (a service handling ≤4 connections; a pool; rate-limited external API). | nextest test group with `max-threads = N`. |
| **Strictly serial** | Contends for a true singleton: one keychain entry, one fixed port, one PID/`flock` daemon singleton, one fixed socket path that can't be relocated. | nextest test group with `max-threads = 1`, scoped per-resource by filter. Or `#[serial(key)]` if on `cargo test`. |

Bucket **per resource, not per test**: a test touching two singletons belongs to the stricter handling for each. With nextest, prefer per-resource groups (`serial-keychain`, `serial-port-8080`) so disjoint singletons still run concurrently — the same disjoint-groups-run-concurrently property `serial_test`'s keys give you, but centralized in config.

### Enforcement examples

**Per-resource serial + capped groups (nextest, recommended):**

```toml
# .config/nextest.toml
[test-groups]
keychain   = { max-threads = 1 }   # one OS keychain entry
daemon     = { max-threads = 1 }   # PID-file + flock singleton
ext-api    = { max-threads = 3 }   # rate-limited; 3 concurrent OK

[[profile.default.overrides]]
filter = 'test(/keychain_/)'
test-group = 'keychain'

[[profile.default.overrides]]
filter = 'test(/daemon_/) + package(daemon-contract-tests)'
test-group = 'daemon'

[[profile.default.overrides]]
filter = 'test(/api_contract_/)'
test-group = 'ext-api'
```

`keychain` and `daemon` tests each run one-at-a-time but **concurrently with each other** and with everything ungrouped; `ext-api` runs three-at-a-time. Confirm placement with `cargo nextest show-config test-groups` and `cargo nextest list`.

**Equivalent annotations (serial_test, if stuck on `cargo test`):**

```rust
#[test]
#[serial(keychain)]            // disjoint from daemon; runs concurrently with it
fn keychain_round_trip() { /* ... */ }

#[test]
#[serial(daemon)]
fn daemon_starts_and_stops() { /* ... */ }

// would collide with the keychain test if run concurrently:
#[test]
#[parallel(keychain)]
fn keychain_read_only() { /* ... */ }
```

For tests in separate integration binaries / doctests, use `#[file_serial(keychain)]` instead (filesystem-based lock), and keep all keychain tests on the *same* locking mechanism.

### Local vs CI invocation (replacing the blanket flag)

- **Local, nextest:** `cargo nextest run` (groups applied automatically) + `cargo test --doc` for doctests.
- **CI, nextest:** add a `ci` profile if CI needs different limits, run `cargo nextest run --profile ci`. Build parallelism stays at default `--jobs`; do **not** add `build.jobs = 1`.
- **If on `cargo test`:** `cargo test --workspace` (annotations enforce serialization in-process). Drop `-- --test-threads=1` entirely once tests are annotated or designed-away.

---

## 4. Migration path: from "everything serial" to targeted serialization

Move in small, verifiable steps. The objective at each stage is to *prove the flake class stays dead* before widening parallelism.

1. **Audit (no behavior change).** Apply §1's checklist. Produce a list mapping each heavyweight test to the specific resource(s) it touches, and tag each as designed-away-able or genuinely-singleton. Most teams discover the majority are already isolated.

2. **Smallest safe first step — adopt the runner, keep the cap.** Introduce nextest with a single conservative group covering *everything currently serial* (`filter = 'package(integration-tests)'`, `max-threads = 1`). This reproduces today's behavior for the heavyweight set but immediately frees the unit tests to run fully parallel. Verify: green run, and CI wall-clock drops. This step has near-zero risk because the conflicting set is still serial.

3. **Split the monolithic group into per-resource groups.** Replace the one catch-all serial group with per-resource groups (§3). Now disjoint singletons run concurrently. Verify with `show-config` that every previously-serial test landed in exactly one group and none fell through to `@global` unintentionally.

4. **Design collisions away, one resource at a time.** For each resource flagged designed-away-able: convert to ephemeral port / temp-dir socket / per-subprocess env / unique namespace, then *remove that test from its group*. After each conversion, **stress the now-parallel test** to prove the flake is gone: `cargo nextest run --no-capture -E 'test(/that_test/)' ` run under repeat, e.g. nextest's `--test-threads` raised plus a repeat loop, or run the subset in a tight loop (20–100×) to force interleavings. Only remove the group entry once it survives stress.

5. **Shrink the serial set to the irreducible core.** What remains in `max-threads = 1` groups should be only the true singletons that can't be relocated (real keychain, externally-fixed port, hardware context). Document *why* each remains serial next to its filter, so the next contributor doesn't have to re-derive it.

6. **Lock in low blast radius.** Because the policy is filter-based config, a new test that matches an existing filter is captured automatically. Add a brief CONTRIBUTING note: "tests touching the OS keychain match `keychain_*`; that's how they get serialized." This is the structural advantage of groups over annotations — new tests are caught by pattern rather than by a contributor remembering an attribute.

### Wall-clock payoff, and when it isn't worth it

The payoff scales with `(parallel-eligible test time) × (cores − 1)`. A suite that was serial only because of a handful of keychain tests, but spends most of its wall-clock in parallel-safe heavyweight setup, can see multi-x speedups — nextest reports being up to ~3× faster than `cargo test` on suitable suites even before targeted grouping. The gain is largest when there are many independent heavyweight tests and many cores.

**Not worth the config complexity when:** the suite is small (a few tests, runs in seconds), the tests are rarely run (a nightly contract job, not per-PR), or essentially *all* heavyweight tests genuinely contend for the same single resource (then serial is correct and a group buys nothing over the default). In those cases, stop at step 2 — adopt the runner if you want the other nextest benefits, or even leave a single documented serial group, and don't build out per-resource partitioning that won't pay for itself.

---

## Recommendation and justification

**Design collisions away first** (§2e): it's the only approach with zero ongoing maintenance and no way for a future contributor to reintroduce the flake. Where a collision genuinely can't be designed away, **enforce it with cargo-nextest test groups** (§2c/§3).

Test groups win on isolation-per-unit-of-config-complexity because the entire serialization policy is a handful of declarative lines in one file, scoped by filter expressions, with no test-source changes and a built-in audit (`show-config`). New tests are captured by pattern, so blast radius is minimal — the chief failure mode of `serial_test` (a contributor forgets the annotation and ships a flake) largely disappears. Nextest also fixes the *entire* in-process global-state class (§1b) for free via process-per-test, so for many suites the only thing left to express is OS-level singletons.

Choose **`serial_test`** over groups only when you must remain on `cargo test` and cannot adopt nextest; accept the per-test annotation burden and the higher blast radius, and use grouping keys so disjoint resources stay concurrent.

Reserve **`--test-threads=1`** for essentially nothing once you've done the audit — and never encode it (or `build.jobs = 1`) in `.cargo/config.toml`, where it silently taxes builds for no isolation gain.

---

*This sensitive-topic note doesn't apply here; the report contains only engineering guidance. Configuration snippets reflect the cargo-nextest and serial_test documentation as of mid-2026 — verify `max-threads`/filter syntax against your installed nextest version, since config schema can evolve.*
