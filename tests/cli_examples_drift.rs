//! Hermetic doc-drift guardrails for the three reference examples (Story 4.2).
//!
//! No daemon, no Node, fast. Asserts each example has its required files,
//! cookbook-anchor markers are present, architecture.md reflects the
//! shipped TypeScript shape (not the prior Rust draft), `examples/README.md`
//! carries the reconciliation note, and the root `Cargo.toml`'s
//! `[workspace] members` deliberately excludes `examples/`.
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

const EXAMPLES: &[(&str, &str)] = &[
    ("multi-session-router", "state-session-fanout"),
    ("event-log-viewer", "rest-cursor-pagination"),
    ("reconnect-recovery", "dropped-frame-recovery"),
];

#[test]
fn each_example_has_required_files() {
    for (name, _) in EXAMPLES {
        for rel in &["src/index.ts", "README.md", "package.json", "tsconfig.json"] {
            let p = workspace_root().join("examples").join(name).join(rel);
            assert!(
                p.is_file(),
                "Story 4.2: examples/{name}/{rel} missing — required for the \
                 reference example to be runnable + documented"
            );
        }
    }
}

#[test]
fn each_example_package_json_declares_node_22_6_engine() {
    let re = regex_lite_match;
    for (name, _) in EXAMPLES {
        let body = read_workspace_file(&format!("examples/{name}/package.json"));
        // Cheap structural check: parse JSON, walk to engines.node, assert
        // it satisfies the >=22.6 floor.
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("examples/{name}/package.json invalid JSON: {e}"));
        let engines = parsed
            .get("engines")
            .and_then(|v| v.get("node"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("examples/{name}/package.json missing engines.node string"));
        assert!(
            re(engines),
            "examples/{name}/package.json engines.node = {engines:?} \
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
fn each_example_source_carries_cookbook_anchors() {
    for (name, anchor) in EXAMPLES {
        let body = read_workspace_file(&format!("examples/{name}/src/index.ts"));
        let begin = format!("cookbook-begin:{anchor}");
        let end = format!("cookbook-end:{anchor}");
        assert!(
            body.contains(&begin),
            "examples/{name}/src/index.ts missing `{begin}` — Story 4.3's \
             cookbook authorship will block on this marker. The marker is \
             a pure comment; runtime is unaffected."
        );
        assert!(
            body.contains(&end),
            "examples/{name}/src/index.ts missing `{end}` — the begin marker \
             is present but the closing marker is not, so the inline-region \
             extraction would be unbounded."
        );
    }
}

#[test]
fn architecture_md_describes_examples_as_typescript_not_cargo() {
    let arch = read_workspace_file("docs/bmad/planning-artifacts/architecture.md");
    // The §Project Structure tree's examples block should mention
    // package.json (TypeScript shape) and NOT examples/*/Cargo.toml or
    // examples/*/src/main.rs (Rust shape from the prior draft).
    assert!(
        arch.contains("package.json"),
        "architecture.md must mention examples/*/package.json — the §Project \
         Structure tree describes the TypeScript shape Story 4.2 ships"
    );
    assert!(
        !arch.contains("examples/*/Cargo.toml"),
        "architecture.md still references `examples/*/Cargo.toml` — the \
         prior Rust draft must be replaced (Story 4.2 Task 6.1)"
    );
    assert!(
        !arch.contains("examples/*/src/main.rs"),
        "architecture.md still references `examples/*/src/main.rs` — the \
         prior Rust draft must be replaced (Story 4.2 Task 6.1)"
    );
}

#[test]
fn examples_readme_reconciliation_note_present() {
    let body = read_workspace_file("examples/README.md");
    // The reconciliation paragraph must name TypeScript and link to
    // project-context.md as the source of truth for the decision.
    for needle in ["TypeScript", "project-context.md"] {
        assert!(
            body.contains(needle),
            "examples/README.md missing reconciliation marker `{needle}` — \
             Story 4.2 Task 5.1 requires a paragraph naming the decision \
             source (project-context.md §Example presenters) and the \
             language choice (TypeScript)"
        );
    }
}

#[test]
fn examples_not_in_root_cargo_toml_members() {
    let body = read_workspace_file("Cargo.toml");
    // The root manifest's `[workspace] members` array must exclude
    // `examples/*`. Story 4.2 Task 1.2's invariant.
    assert!(
        body.contains("members = [\"crates/*\"]"),
        "root Cargo.toml must keep `members = [\"crates/*\"]` only — \
         adding `examples/*` would make the examples Cargo workspace \
         members, contradicting Story 4.2's TypeScript-on-Node decision"
    );
    assert!(
        !body.contains("\"examples/"),
        "root Cargo.toml references `examples/` as a workspace member — \
         Story 4.2's invariant is that examples/ is a Node project zone, \
         not a Cargo zone"
    );
}
