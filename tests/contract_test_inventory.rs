//! Story 4.4 / AC #3 — contract test inventory.
//!
//! Pins the 10 required contract tests by name so a rename or accidental
//! deletion fails CI loudly. The architecture document
//! (`docs/bmad/planning-artifacts/architecture.md` §Additional Contract Tests
//! Identified) is the source-of-truth for the list; this test is the
//! compiled enforcement layer ("compiled tests beat greps" per Epic 3 retro
//! Team Agreement A7).
//!
//! The matcher is a hand-rolled string scan — `fn <name>`, `async fn <name>`,
//! or `mod <name>` — over the file body. No regex dependency.
//!
//! When a contract test is intentionally renamed (e.g., a follow-up story
//! splits or merges tests), update the `REQUIRED_CONTRACT_TESTS` array in
//! this file AND record the rename in the story's Completion Notes so a
//! future retrospective reader can trace the name shift.

use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Each entry maps an architecture-named contract surface to a concrete test
/// (or test module) declared in the workspace's test suite. Story 4.4 ACs
/// #3 + #3a are the rationale; the architecture.md "required contract tests"
/// section is the authoritative list.
struct Required {
    /// File path relative to workspace root.
    file: &'static str,
    /// Name of the `fn`, `async fn`, or `mod` that pins this contract.
    /// Module-level entries pin a whole test cluster (e.g. `story_2_4_dropped`
    /// covers multiple Dropped-frame scenarios).
    name: &'static str,
    /// Architecture-doc surface label — appears in failure messages so a
    /// reader who didn't author the rename can find their bearings.
    surface: &'static str,
}

const REQUIRED_CONTRACT_TESTS: &[Required] = &[
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "story_2_4_dropped",
        surface: "(1) WS dropped-frame behavior — Lagged consumer emits Dropped",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "pragmas_on_every_writer_checkout",
        surface: "(2a) PRAGMA invariants — writer pool checkout",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "pragmas_on_every_reader_checkout",
        surface: "(2b) PRAGMA invariants — reader pool checkout",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "connection_factory_policy_lint_passes",
        surface: "(3) Connection factory lint enforcement",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "state_plus_event_atomicity_rollback",
        surface: "(4a) state+event INSERT atomicity — rollback path",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "state_plus_event_atomicity_under_sigkill_during_load",
        surface: "(4b) state+event INSERT atomicity — SIGKILL during load (AC #3a deadlock fix)",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "story_2_5_shutdown",
        surface: "(5) Graceful shutdown — close-frame + drain + WAL checkpoint",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "events_list_oldest_available_after_purge",
        surface: "(6) Cursor-gap detection — oldest_available_event_id post-purge",
    },
    Required {
        file: "crates/adapter-claude/tests/contract_install.rs",
        // Task 3.1 marks this entry as ALL (module-level) because the file
        // exercises every settings.json atomic-install invariant; locking
        // to a single fn name would fragilize the inventory against
        // internal-test refactors. Match on a stable doc-comment landmark
        // — the file's module docstring names "atomic-on-interrupt
        // invariants (AC #3)" as the cross-cutting contract it covers.
        name: "atomic-on-interrupt invariants",
        surface: "(7) Atomic settings.json install — module docstring landmark",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "hook_unreliability_tolerance_pretooluse_without_posttooluse",
        surface: "(8) Hook unreliability tolerance — PreToolUse without PostToolUse",
    },
    Required {
        file: "crates/protocol/tests/contract_protocol.rs",
        name: "outbound_type_accepts_unknown_fields",
        surface: "(9) Outbound envelope additive-compat — unknown fields accepted",
    },
    Required {
        file: "crates/daemon/tests/contract_daemon.rs",
        name: "source_session_id_collision_safety",
        surface: "(10) (source, session_id) collision safety",
    },
];

fn body_contains_test_name(body: &str, name: &str) -> bool {
    // Match `fn <name>(`, `async fn <name>(`, or `mod <name> {`/`mod <name>{`.
    // Hand-rolled scan; checking for the literal name token preceded by
    // `fn ` / `async fn ` / `mod ` and followed by `(` / `{` / whitespace.
    let needles = [
        format!("fn {name}("),
        format!("async fn {name}("),
        format!("mod {name} "),
        format!("mod {name}{{"),
    ];
    needles.iter().any(|n| body.contains(n))
}

/// Module-level entry (`name = "atomic settings.json install"`) is a
/// stable doc-comment landmark rather than a single fn name — the install
/// contract test file is dense and prone to internal refactor. We pin a
/// human-readable phrase that the file's module docstring carries.
fn body_contains_landmark(body: &str, landmark: &str) -> bool {
    body.contains(landmark)
}

#[test]
fn all_ten_required_contract_tests_present() {
    let mut missing: Vec<&Required> = Vec::new();
    let mut bodies: std::collections::HashMap<&str, String> = Default::default();

    for r in REQUIRED_CONTRACT_TESTS {
        let body = bodies.entry(r.file).or_insert_with(|| {
            let p = workspace_root().join(r.file);
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        });
        let present = if r.name.contains(' ') {
            // Module-level landmark (e.g. the contract_install.rs file).
            body_contains_landmark(body, r.name)
        } else {
            body_contains_test_name(body, r.name)
        };
        if !present {
            missing.push(r);
        }
    }

    assert!(
        missing.is_empty(),
        "Required contract tests not found — a rename or deletion broke the inventory:\n{}\n\n\
         If the rename was intentional, update `REQUIRED_CONTRACT_TESTS` in \
         tests/contract_test_inventory.rs AND record the change in the story's \
         Completion Notes. See Story 4.4 AC #3.",
        missing
            .iter()
            .map(|r| format!("  - {} :: {} (surface: {})", r.file, r.name, r.surface))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn contract_test_inventory_size_is_at_least_ten() {
    // AC #3 specifies "10 required contract tests"; the inventory may grow as
    // future stories add new contract surfaces (e.g. multi-source disambiguation
    // when a second adapter ships), but it MUST NOT shrink below 10 — that would
    // silently drop a v1.x guarantee. A future deletion would have to also
    // delete this assertion, which is a louder PR signal than a quiet array
    // edit.
    assert!(
        REQUIRED_CONTRACT_TESTS.len() >= 10,
        "REQUIRED_CONTRACT_TESTS shrank below 10 entries — Story 4.4 AC #3 floor violated"
    );
}
