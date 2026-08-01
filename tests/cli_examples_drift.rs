//! Hermetic doc-drift guardrails for the three cookbook reference entries
//! (Story 4.2; consolidated into `docs/cookbook/<name>/` by Story 5.13).
//!
//! No daemon, no Node, fast. Asserts each entry has its required files,
//! architecture.md reflects the consolidated TypeScript shape (not the
//! prior Rust draft), `docs/cookbook/README.md` carries the Cargo-zone
//! reconciliation note, and the root `Cargo.toml`'s `[workspace] members`
//! deliberately excludes the cookbook directories.
//!
//! Mirrors `tests/release_pipeline_docs.rs` shape. Per Epic 3 retro Team
//! agreement A7: doc-drift verification as a compiled test, not a
//! verification-block grep.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_workspace_file(rel: &str) -> String {
    let p = workspace_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

const ENTRIES: &[&str] = &[
    "state-session-fanout",
    "rest-cursor-pagination",
    "dropped-frame-recovery",
];

#[test]
fn each_entry_has_required_files() {
    for name in ENTRIES {
        for rel in &["src/index.ts", "README.md", "package.json", "tsconfig.json"] {
            let p = workspace_root().join("docs/cookbook").join(name).join(rel);
            assert!(
                p.is_file(),
                "Story 5.13: docs/cookbook/{name}/{rel} missing; required for the \
                 cookbook entry to be runnable + documented (invariant from Story 4.2)"
            );
        }
    }
}

#[test]
fn each_entry_package_json_declares_node_22_6_engine() {
    let re = regex_lite_match;
    for name in ENTRIES {
        let body = read_workspace_file(&format!("docs/cookbook/{name}/package.json"));
        // Cheap structural check: parse JSON, walk to engines.node, assert
        // it satisfies the >=22.6 floor.
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("docs/cookbook/{name}/package.json invalid JSON: {e}"));
        let engines = parsed
            .get("engines")
            .and_then(|v| v.get("node"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                panic!("docs/cookbook/{name}/package.json missing engines.node string")
            });
        assert!(
            re(engines),
            "docs/cookbook/{name}/package.json engines.node = {engines:?} \
             does not satisfy Story 4.2's Node 22.6+ floor (must start with \
             >=22.[6-9] or >=23+ or >= a higher major)"
        );
    }
}

/// Hand-rolled matcher mirroring the regex pattern documented in Story 4.2:
/// `^>=22\.[6-9]|^>=2[3-9]|^>=[3-9]`. Returns true when the engines.node
/// string declares Node 22.6+ (or a higher major).
fn regex_lite_match(s: &str) -> bool {
    // Strip a leading `>=` if present; otherwise reject (other operators
    // like `^` or exact versions are not a tight enough floor for the
    // smoke test's --experimental-strip-types requirement).
    let Some(rest) = s.strip_prefix(">=") else {
        return false;
    };
    let mut parts = rest.split('.');
    let Some(major_s) = parts.next() else {
        return false;
    };
    let Ok(major) = major_s.parse::<u32>() else {
        return false;
    };
    if major > 22 {
        return true;
    }
    if major < 22 {
        return false;
    }
    // major == 22 → minor must be ≥ 6.
    let Some(minor_s) = parts.next() else {
        return false;
    };
    let Ok(minor) = minor_s.parse::<u32>() else {
        return false;
    };
    minor >= 6
}

#[test]
fn architecture_md_describes_cookbook_as_typescript_not_cargo() {
    let arch = read_workspace_file("docs/bmad/planning-artifacts/architecture.md");
    // The §Project Structure tree's cookbook block should describe the
    // consolidated Node zone (Story 5.13) and NOT the pre-4.2 Rust draft
    // shape (examples/*/Cargo.toml, examples/*/src/main.rs).
    assert!(
        arch.contains("docs/cookbook/*/ is a Node project zone"),
        "architecture.md must state that docs/cookbook/*/ is a Node project \
         zone; the §Project Structure tree describes the consolidated shape \
         Story 5.13 ships"
    );
    assert!(
        !arch.contains("examples/*/Cargo.toml"),
        "architecture.md still references `examples/*/Cargo.toml`; the \
         prior Rust draft must stay replaced (Story 4.2 Task 6.1)"
    );
    assert!(
        !arch.contains("examples/*/src/main.rs"),
        "architecture.md still references `examples/*/src/main.rs`; the \
         prior Rust draft must stay replaced (Story 4.2 Task 6.1)"
    );
}

#[test]
fn cookbook_readme_carries_cargo_zone_note() {
    let body = read_workspace_file("docs/cookbook/README.md");
    // The index must keep the reconciliation note (formerly in
    // examples/README.md, folded in by Story 5.13): the cookbook is a
    // TypeScript zone, the decision source is project-context.md, and the
    // Cargo workspace deliberately excludes it.
    for needle in [
        "Not a Cargo zone",
        "TypeScript",
        "project-context.md",
        "members = [\"crates/*\"]",
    ] {
        assert!(
            body.contains(needle),
            "docs/cookbook/README.md missing reconciliation marker `{needle}`; \
             Story 5.13 folded the Story 4.2 Task 5.1 note (decision source + \
             language choice + Cargo-zone boundary) into the cookbook index"
        );
    }
}

#[test]
fn cookbook_not_in_root_cargo_toml_members() {
    let body = read_workspace_file("Cargo.toml");
    // The root manifest's `[workspace] members` array must exclude the
    // cookbook entry directories. Story 4.2 Task 1.2's invariant, carried
    // forward through the Story 5.13 move.
    assert!(
        body.contains("members = [\"crates/*\"]"),
        "root Cargo.toml must keep `members = [\"crates/*\"]` only; adding \
         cookbook entries would make them Cargo workspace members, \
         contradicting the TypeScript-on-Node decision"
    );
    for banned in ["\"examples/", "\"docs/cookbook"] {
        assert!(
            !body.contains(banned),
            "root Cargo.toml references `{banned}` as a workspace member; \
             the cookbook entries are a Node project zone, not a Cargo zone \
             (Story 4.2 invariant, Story 5.13 location)"
        );
    }
}
