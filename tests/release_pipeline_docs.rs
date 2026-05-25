//! Story 3.4 cross-file invariants for the release pipeline and install
//! documentation.
//!
//! These tests do NOT build tarballs or run the release workflow — both would
//! require a real GitHub Actions runner or a clean Rust install env that this
//! repo's test harness cannot provide. Instead they pin the load-bearing
//! invariants that *can* drift silently in a future story:
//!
//!   - **AC #2:** the musl-deferral statement appears in README.md,
//!     INSTALL.md, AND `.github/workflows/release.yml`'s notes template.
//!     Reword in one place but not the others and a tarball user lands on a
//!     surface that contradicts the release page.
//!   - **AC #5:** the seven-point `bowerbird install` walkthrough (a–g) and
//!     the supporting markers (`PreToolUse` / `PostToolUse` / `Stop` /
//!     `Notification`, `--no-start`, `BOWERBIRD_CLAUDE_SETTINGS`, the
//!     `~/.bowerbird/` data directory + mode `0700`, the keychain
//!     `service=bowerbird-daemon` entry, and `bowerbird uninstall`'s
//!     data-preserving semantics) appear in BOTH README.md and INSTALL.md.
//!   - **AC #6:** `.github/workflows/ci.yml` invokes `cargo test --workspace
//!     -- --test-threads=1` for the daemon contract suite serialization
//!     (Epic 2 retro AI-3 / Discovery #3). Drop the flag and the contract
//!     tests flake under parallel execution.
//!   - **AC #7:** `docs/bmad/planning-artifacts/architecture.md`'s
//!     "WebSocket subsystem" section names all six runtime config knobs from
//!     `crates/daemon/src/config.rs::Config::with_bowerbird_dir`. The pinning
//!     sentence in the section ("Defaults are committed at …; the table
//!     above MUST be updated in the same commit as any field-default change")
//!     is the doc-drift guardrail; this test is its automated mirror.
//!   - **AC #1 (structural intent only):** `release.yml` references the
//!     three documented target triples and the eight tarball-staging entries
//!     (3 binaries + `tool-reactions.toml` + `LICENSE` + `LICENSE-MIT` +
//!     `LICENSE-APACHE` + `README.md` + `INSTALL.md` + `CHANGELOG.md`).
//!     Actually building a tarball is the job of `scripts/tarball-smoke-test.sh`,
//!     a local-only smoke test that exercises the staging+tar logic against
//!     already-built binaries.
//!
//! All tests here read files with paths anchored at `env!("CARGO_MANIFEST_DIR")`,
//! which cargo sets to the workspace-root crate's directory at compile time —
//! i.e. the workspace root itself for this crate. The tests are hermetic
//! (read-only) and have no ordering requirements.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_workspace_file(rel: &str) -> String {
    let p = workspace_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn assert_contains_all<P: AsRef<Path>>(label: &str, path: P, body: &str, needles: &[&str]) {
    let missing: Vec<&&str> = needles.iter().filter(|n| !body.contains(**n)).collect();
    assert!(
        missing.is_empty(),
        "{label} ({}) missing required substrings: {:?}",
        path.as_ref().display(),
        missing,
    );
}

// ---------------------------------------------------------------------------
// AC #2: musl-deferral statement appears in README.md, INSTALL.md, and
// `.github/workflows/release.yml`. The exact wording can evolve, but the
// load-bearing phrase "musl Linux is deferred post-V1" and the NFR9 anchor
// must remain — a user landing on any of the three surfaces should see the
// same message.
// ---------------------------------------------------------------------------

#[test]
fn musl_deferral_statement_appears_in_readme() {
    let readme = read_workspace_file("README.md");
    assert_contains_all(
        "AC #2 musl deferral",
        "README.md",
        &readme,
        &[
            "musl Linux is deferred post-V1",
            "(NFR9)",
            "cargo install --git",
        ],
    );
}

#[test]
fn musl_deferral_statement_appears_in_install_md() {
    let install = read_workspace_file("INSTALL.md");
    // INSTALL.md is allowed to be more concise and cross-link to README.md,
    // but it MUST mention musl-based distributions explicitly so a
    // tarball-extracted user doesn't believe Alpine/Void are supported.
    assert_contains_all(
        "AC #2 musl deferral",
        "INSTALL.md",
        &install,
        &["musl-based distributions"],
    );
}

#[test]
fn musl_deferral_statement_appears_in_release_workflow_notes() {
    let release = read_workspace_file(".github/workflows/release.yml");
    assert_contains_all(
        "AC #2 musl deferral",
        ".github/workflows/release.yml",
        &release,
        &[
            "musl Linux is deferred post-V1",
            "(NFR9)",
            "cargo install --git",
        ],
    );
}

// ---------------------------------------------------------------------------
// AC #5: the `bowerbird install` walkthrough markers (a–g) and supporting
// values appear in BOTH README.md and INSTALL.md. The two files are the two
// user entry points (GitHub repo page vs. tarball post-extract); a drift
// between them is a silent doc bug. Substring matches are the right
// granularity — the prose evolves, but the marker tokens are stable.
// ---------------------------------------------------------------------------

/// The minimum-discoverability markers the walkthrough must mention.
/// Both files must contain every one of these.
const WALKTHROUGH_MARKERS: &[&str] = &[
    // (a) file path + env override
    "~/.claude/settings.json",
    "BOWERBIRD_CLAUDE_SETTINGS",
    // (b) atomic write contract
    "atomic",
    // (c) the four hook kinds (PATH-relative invocation)
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "Notification",
    // (d) data directory + mode
    "~/.bowerbird/",
    "0700",
    // (e) daemon auto-start opt-out
    "--no-start",
    // (f) uninstall semantics
    "bowerbird uninstall",
    // (g) keychain entry
    "service=bowerbird-daemon",
];

#[test]
fn readme_install_walkthrough_covers_a_through_g_markers() {
    let readme = read_workspace_file("README.md");
    assert_contains_all(
        "AC #5 README walkthrough markers",
        "README.md",
        &readme,
        WALKTHROUGH_MARKERS,
    );
}

#[test]
fn install_md_walkthrough_covers_a_through_g_markers() {
    let install = read_workspace_file("INSTALL.md");
    assert_contains_all(
        "AC #5 INSTALL.md walkthrough markers",
        "INSTALL.md",
        &install,
        WALKTHROUGH_MARKERS,
    );
}

// ---------------------------------------------------------------------------
// AC #6: the CI workflow serializes the daemon contract suite via
// `cargo test --workspace -- --test-threads=1`. Epic 2 retro AI-3 documented
// the failure mode (process-wide state shared by signal handlers, the
// keychain mock backend, BOWERBIRD_DATA_DIR fixtures, and assert_cmd
// subprocesses); a regression here re-introduces the entire class of flake.
// ---------------------------------------------------------------------------

#[test]
fn ci_workflow_sets_up_node_22_6() {
    // Story 4.2 AC #4: the CI workflow must install Node 22.6+ before the
    // cargo test step so `tests/cli_examples.rs` can spawn `node
    // --experimental-strip-types` against the bundled examples. Without
    // this, the smoke tests would silently skip on CI (their
    // `node_22_6_available` gate returns false and the assertions never
    // run), regressing protection for the three reference examples.
    let ci = read_workspace_file(".github/workflows/ci.yml");
    assert!(
        ci.contains("actions/setup-node@v4"),
        "AC #4: .github/workflows/ci.yml must use `actions/setup-node@v4` \
         to install Node before the cargo test step (required by \
         tests/cli_examples.rs for --experimental-strip-types)"
    );
    assert!(
        ci.contains("node-version: '22.6'"),
        "AC #4: .github/workflows/ci.yml must pin Node to 22.6 — this is \
         the minimum version supporting --experimental-strip-types, the \
         no-build-step entry point for the TypeScript examples"
    );
}

#[test]
fn ci_workflow_runs_workspace_tests_single_threaded() {
    let ci = read_workspace_file(".github/workflows/ci.yml");
    // The exact command — note the `--` separator is what makes
    // `--test-threads=1` reach the test binary instead of cargo.
    assert!(
        ci.contains("cargo test --workspace -- --test-threads=1"),
        "AC #6: .github/workflows/ci.yml must invoke \
         `cargo test --workspace -- --test-threads=1` for the daemon \
         contract-test serialization (Epic 2 retro AI-3). Found ci.yml \
         without that exact step."
    );
}

// ---------------------------------------------------------------------------
// AC #7: architecture.md's "WebSocket subsystem" section names all six
// runtime config knobs. The pinning sentence in the section makes it a
// commit-discipline invariant: any change to a default in
// `crates/daemon/src/config.rs::Config::with_bowerbird_dir` must also
// update the architecture.md table. This test is the automated mirror — if
// a knob is renamed or removed in the section, the test fails.
// ---------------------------------------------------------------------------

const WS_CONFIG_KNOBS: &[&str] = &[
    "ws_max_connections",
    "ws_ping_interval",
    "ws_pong_timeout",
    "ws_broadcast_capacity",
    "shutdown_drain_timeout",
    "ws_broadcast_coalesce_window",
];

#[test]
fn architecture_md_documents_all_six_ws_config_knobs() {
    let arch = read_workspace_file("docs/bmad/planning-artifacts/architecture.md");
    assert_contains_all(
        "AC #7 WebSocket subsystem knob table",
        "docs/bmad/planning-artifacts/architecture.md",
        &arch,
        WS_CONFIG_KNOBS,
    );
}

#[test]
fn architecture_md_pins_table_to_daemon_config_source() {
    // The doc-drift guardrail: if the pinning sentence is dropped, future
    // contributors lose the "must update both in the same commit" signal.
    let arch = read_workspace_file("docs/bmad/planning-artifacts/architecture.md");
    assert!(
        arch.contains("crates/daemon/src/config.rs"),
        "AC #7: architecture.md WebSocket subsystem must cite \
         `crates/daemon/src/config.rs` as the source of truth for the \
         knob defaults — without that pointer the doc-drift hazard is \
         not preventable by reading the section alone."
    );
}

// ---------------------------------------------------------------------------
// AC #1 (structural intent only — full smoke is `scripts/tarball-smoke-test.sh`):
// release.yml references the three documented target triples and stages all
// eight tarball entries. Substring assertions catch the "someone deleted a
// `cp` step" regression mode.
// ---------------------------------------------------------------------------

const RELEASE_TARGET_TRIPLES: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];

#[test]
fn release_workflow_targets_three_documented_triples() {
    let release = read_workspace_file(".github/workflows/release.yml");
    assert_contains_all(
        "AC #1 release.yml target triples",
        ".github/workflows/release.yml",
        &release,
        RELEASE_TARGET_TRIPLES,
    );
}

/// The eight tarball staging entries Task 3.5 documents. Each one MUST be
/// referenced by a `cp` (or equivalent) in the workflow — otherwise a tag
/// push produces a tarball that is missing pieces a user needs.
const TARBALL_STAGING_ENTRIES: &[&str] = &[
    "bin/",
    "adapters/claude",
    "tool-reactions.toml",
    "LICENSE",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "README.md",
    "INSTALL.md",
    "CHANGELOG.md",
];

#[test]
fn release_workflow_stages_all_documented_tarball_entries() {
    let release = read_workspace_file(".github/workflows/release.yml");
    assert_contains_all(
        "AC #1 release.yml tarball staging entries",
        ".github/workflows/release.yml",
        &release,
        TARBALL_STAGING_ENTRIES,
    );
}

#[test]
fn release_workflow_builds_shim_under_release_shim_profile() {
    // The shipped shim MUST be built under the release-shim profile (p99
    // ≤5ms hot-path budget). Two cargo invocations per matrix row is the
    // documented shape — workspace minus shim under `release`, shim alone
    // under `release-shim`.
    let release = read_workspace_file(".github/workflows/release.yml");
    assert!(
        release.contains("--profile release-shim"),
        "AC #1: release.yml must build bowerbird-shim under the \
         `release-shim` profile to preserve the hot-path performance \
         budget — without it, shipped tarballs ship a slower shim than \
         the CI bench gate verifies."
    );
    assert!(
        release.contains("--exclude bowerbird-shim"),
        "AC #1: release.yml must exclude bowerbird-shim from the default \
         `release` profile build — otherwise the shim is built twice and \
         the wrong artifact may land in the tarball."
    );
}

#[test]
fn release_workflow_enforces_locked_dependency_graph() {
    // NFR10 reproducibility contract: every `cargo build` in release.yml
    // passes `--locked` so the committed Cargo.lock is the exact dep
    // graph. The Cargo.lock-modifying build is a footgun in a release
    // pipeline — fail loudly instead of auto-resolving.
    let release = read_workspace_file(".github/workflows/release.yml");
    let locked_count = release.matches("--locked").count();
    assert!(
        locked_count >= 2,
        "AC #1 / NFR10: release.yml must pass `--locked` to every \
         cargo build (expected at least 2 occurrences: workspace + \
         shim builds), found {locked_count}"
    );
}

// ---------------------------------------------------------------------------
// Workspace consistency invariants the dev added per Task 6.2 — license
// metadata in every published crate. Worth pinning so a future story that
// adds a new crate cannot accidentally publish under no license.
// ---------------------------------------------------------------------------

const LICENSED_CARGO_MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "crates/protocol/Cargo.toml",
    "crates/daemon/Cargo.toml",
    "crates/shim/Cargo.toml",
    "crates/adapter-claude/Cargo.toml",
];

#[test]
fn every_published_crate_declares_mit_or_apache_license() {
    for manifest in LICENSED_CARGO_MANIFESTS {
        let body = read_workspace_file(manifest);
        assert!(
            body.contains("license = \"MIT OR Apache-2.0\""),
            "AC #1 / Task 6.2: {manifest} must declare `license = \"MIT \
             OR Apache-2.0\"` — without it, the prebuilt-binary \
             distribution has ambiguous redistribution rights."
        );
    }
}

// ---------------------------------------------------------------------------
// Story 4.3 AC #6: README.md links to docs/quickstart.md and docs/protocol.md
// as Markdown link targets, and the "in flight under Story 4.3" placeholder
// in §Protocol is replaced with a live link. This is the README↔docs coupling
// guardrail; it lives here (not in cli_docs_drift.rs) because release_pipeline_docs.rs
// already covers README-shape doc-drift (license badges, install commands,
// version literals).
// ---------------------------------------------------------------------------

#[test]
fn readme_links_to_quickstart_and_protocol_docs() {
    let readme = read_workspace_file("README.md");
    assert!(
        readme.contains("](docs/quickstart.md)"),
        "AC #6: README.md must link to docs/quickstart.md as a Markdown link \
         target (`](docs/quickstart.md)`) — the Quickstart section should \
         forward to the new doc"
    );
    assert!(
        readme.contains("](docs/protocol.md)"),
        "AC #6: README.md must link to docs/protocol.md as a Markdown link \
         target (`](docs/protocol.md)`) — the §Protocol section should \
         replace the Story 4.3 placeholder with a live link"
    );
    assert!(
        !readme.contains("in flight under Story 4.3"),
        "AC #6: README.md still carries the placeholder `in flight under \
         Story 4.3` — Story 4.3 has shipped, the placeholder must be replaced \
         with the live `docs/protocol.md` link"
    );
}

#[test]
fn workspace_root_ships_dual_license_files() {
    // The two license texts that the release tarball bundles. Their
    // presence at the workspace root is the legal precondition for
    // distributing prebuilt binaries.
    for file in ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE"] {
        let p = workspace_root().join(file);
        assert!(
            p.is_file(),
            "AC #1 / Task 6.2: {file} missing at workspace root — \
             tarball staging in release.yml will fail at the `cp` step \
             and the legal-clarity invariant is not met."
        );
    }
}
