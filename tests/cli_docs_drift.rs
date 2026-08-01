//! Hermetic doc-drift guardrails for the Story 4.3 documentation suite,
//! updated for Story 5.13's cookbook consolidation.
//!
//! No daemon, no Node, no network. Asserts the required docs exist,
//! cookbook entry READMEs follow the canonical five-section shape (prose
//! only: code lives in the colocated `src/index.ts`, so a TypeScript
//! fenced block in a README is drift), internal markdown links in the
//! docs resolve to files on disk, and architecture.md's `docs/` tree
//! matches the shipped surface (not the stale `docs/architecture/` +
//! `docs/api/` placeholders).
//!
//! The pre-5.13 byte-identity drift-check between cookbook code blocks and
//! example anchor regions is gone: there is no duplicated code to check
//! anymore. `tests/cli_examples_drift.rs` remains the entry-side
//! counterpart (required files, engines floor, Cargo-zone boundary).

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_workspace_file(rel: &str) -> String {
    let p = workspace_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

const REQUIRED_DOCS: &[&str] = &[
    "docs/quickstart.md",
    "docs/presenter-authoring.md",
    "docs/protocol.md",
    "docs/no-list.md",
    "docs/cookbook/README.md",
];

const REQUIRED_COOKBOOK_ENTRIES: &[&str] = &[
    "docs/cookbook/state-session-fanout/README.md",
    "docs/cookbook/rest-cursor-pagination/README.md",
    "docs/cookbook/dropped-frame-recovery/README.md",
];

#[test]
fn required_docs_exist() {
    for rel in REQUIRED_DOCS.iter().chain(REQUIRED_COOKBOOK_ENTRIES.iter()) {
        let p = workspace_root().join(rel);
        assert!(
            p.is_file(),
            "Story 4.3/5.13 required doc missing: {}; the five top-level \
             docs plus the three per-entry cookbook READMEs must all exist",
            p.display(),
        );
    }
}

/// Walk a markdown body with fence awareness: opening-fence info strings are
/// checked against the prose-safe allowlist, fenced content is skipped for
/// heading matching, and level-2 headings are matched against `sections` in
/// order. Returns how many of `sections` were seen in order.
///
/// The allowlist (not a code-language blacklist) is the load-bearing choice:
/// the pre-review guard banned only the literal ```ts / ```typescript
/// spellings, so ```js, ```tsx, tilde fences, and bare fences all smuggled
/// code back into prose (review 2026-08-01, Blind Hunter finding 1).
fn scan_prose_only_markdown(rel: &str, body: &str, sections: &[&str]) -> usize {
    const ALLOWED_FENCE_LANGS: &[&str] = &["", "sh"];

    let mut seen = 0usize;
    let mut in_fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            if !in_fence {
                let info = t.trim_start_matches(['`', '~']).trim().to_ascii_lowercase();
                assert!(
                    ALLOWED_FENCE_LANGS.contains(&info.as_str()),
                    "{rel} opens a fenced block with language `{info}`; cookbook \
                     prose allows only plain or `sh` fences. Code lives in the \
                     colocated src/index.ts, never embedded in the README \
                     (Story 5.13 AC 3, consolidation contract)"
                );
            }
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if seen < sections.len() && line.trim_end() == sections[seen] {
            seen += 1;
        }
    }
    seen
}

#[test]
fn every_cookbook_entry_has_canonical_five_sections() {
    // The post-5.13 project-context.md §Cookbook discipline shape: five
    // level-2 headings in this order, prose only. Headings inside fences do
    // not count (a shell comment reading `## Run it` cannot spoof the scan).
    const SECTIONS: &[&str] = &[
        "## What this is",
        "## Run it",
        "## How it works",
        "## How to apply it",
        "## Files",
    ];

    for rel in REQUIRED_COOKBOOK_ENTRIES {
        let body = read_workspace_file(rel);
        let seen = scan_prose_only_markdown(rel, &body, SECTIONS);
        assert_eq!(
            seen,
            SECTIONS.len(),
            "{rel} missing canonical section ordering. Expected (in order): \
             {SECTIONS:?}. Found {seen} of {} sections in the right order; \
             the next missing one is `{}`.",
            SECTIONS.len(),
            SECTIONS.get(seen).copied().unwrap_or("(none)"),
        );
    }
}

#[test]
fn cookbook_index_readme_is_prose_only() {
    // The index is exempt from the five-section recipe shape (it has a table
    // shape) but NOT from the prose-only fence rule: the landing page is the
    // most likely place an illustrative code block sneaks back in.
    let body = read_workspace_file("docs/cookbook/README.md");
    scan_prose_only_markdown("docs/cookbook/README.md", &body, &[]);
}

#[test]
fn cookbook_entry_consts_match_directory_listing() {
    // The directory IS the surface (Story 5.13): CI's typecheck loop globs
    // docs/cookbook/*/, so a new entry directory gets typechecked while the
    // hardcoded guard lists here and in cli_examples_drift.rs silently skip
    // it. This sync check turns that gap into a red build.
    let cookbook = workspace_root().join("docs/cookbook");
    let mut dirs: Vec<String> = fs::read_dir(&cookbook)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", cookbook.display()))
        .filter_map(|entry| {
            let entry = entry.expect("dir entry");
            let is_dir = entry.file_type().expect("file type").is_dir();
            is_dir.then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    dirs.sort();

    let mut expected: Vec<String> = REQUIRED_COOKBOOK_ENTRIES
        .iter()
        .map(|rel| {
            rel.trim_start_matches("docs/cookbook/")
                .trim_end_matches("/README.md")
                .to_string()
        })
        .collect();
    expected.sort();

    assert_eq!(
        dirs, expected,
        "docs/cookbook/ entry directories and REQUIRED_COOKBOOK_ENTRIES \
         disagree. Every entry directory must be listed here (and in \
         cli_examples_drift.rs::ENTRIES) so the shape guards and the smoke \
         cover it; CI's typecheck glob alone is not structural coverage."
    );
}

#[test]
fn quickstart_internal_links_resolve() {
    // Permissive markdown link scan: find every `[text](path)` where path
    // is not http(s):// or mailto:. For each, resolve relative to the
    // markdown file's parent and assert the target exists on disk.
    // Anchor fragments (`path#fragment`) are split — verifying the
    // fragment resolves to an actual heading is over-engineering for V1.
    const DOCS_TO_CHECK: &[&str] = &[
        "README.md",
        "INSTALL.md",
        "docs/quickstart.md",
        "docs/presenter-authoring.md",
        "docs/protocol.md",
        "docs/cookbook/README.md",
        "docs/no-list.md",
        "docs/cookbook/state-session-fanout/README.md",
        "docs/cookbook/rest-cursor-pagination/README.md",
        "docs/cookbook/dropped-frame-recovery/README.md",
    ];

    let mut failures: Vec<String> = Vec::new();
    for rel in DOCS_TO_CHECK {
        let md_path = workspace_root().join(rel);
        let body = fs::read_to_string(&md_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", md_path.display()));

        // Hand-rolled scanner for `](...)` link targets. Skip auto-links
        // (`<http://...>`) and reference-style links.
        let bytes = body.as_bytes();
        let mut i = 0usize;
        while i + 2 < bytes.len() {
            if bytes[i] == b']' && bytes[i + 1] == b'(' {
                // Find matching close paren on the same line.
                let close_offset = bytes[i + 2..]
                    .iter()
                    .position(|&b| b == b')' || b == b'\n')
                    .unwrap_or(0);
                let close = i + 2 + close_offset;
                if close < bytes.len() && bytes[close] == b')' {
                    let target = &body[i + 2..close];
                    if !target.is_empty() {
                        check_link_target(rel, &md_path, target, &mut failures);
                    }
                    i = close + 1;
                    continue;
                }
            }
            i += 1;
        }
    }

    assert!(
        failures.is_empty(),
        "internal markdown links failed to resolve:\n  - {}",
        failures.join("\n  - "),
    );
}

fn check_link_target(doc_rel: &str, md_path: &Path, target: &str, failures: &mut Vec<String>) {
    let lc = target.trim();
    if lc.starts_with("http://")
        || lc.starts_with("https://")
        || lc.starts_with("mailto:")
        || lc.starts_with('#')
    {
        return;
    }
    // Strip `#fragment` and any trailing whitespace inside the parens.
    let path_part = lc.split('#').next().unwrap_or(lc).trim();
    if path_part.is_empty() {
        return;
    }
    let resolved = md_path.parent().expect("markdown parent").join(path_part);
    if !resolved.exists() {
        failures.push(format!(
            "{doc_rel}: link target `{target}` resolves to {} which does not exist",
            resolved.display(),
        ));
    }
}

#[test]
fn architecture_md_docs_tree_matches_shipped_surface() {
    let arch = read_workspace_file("docs/bmad/planning-artifacts/architecture.md");

    // (a) Must list every shipped doc surface. In the §Project structure
    //     tree block, files appear as bare basenames nested under `├── docs/`
    //     — accept either the bare form or the fully qualified `docs/<x>`.
    const REQUIRED: &[&[&str]] = &[
        &["docs/quickstart.md", "quickstart.md"],
        &["docs/presenter-authoring.md", "presenter-authoring.md"],
        &["docs/protocol.md", "protocol.md"],
        &["docs/cookbook/", "cookbook/"],
        &["docs/no-list.md", "no-list.md"],
    ];
    for forms in REQUIRED {
        assert!(
            forms.iter().any(|needle| arch.contains(needle)),
            "architecture.md missing reference to shipped doc surface (any of {forms:?}) — \
             Task 7.2 must reconcile the §Project structure tree's `docs/` block to \
             list the five Story 4.3 outputs",
        );
    }

    // (b) Must NOT contain stale placeholder paths from the original
    //     planning-artifact draft.
    const STALE: &[&str] = &["docs/architecture/", "docs/api/"];
    for needle in STALE {
        assert!(
            !arch.contains(needle),
            "architecture.md still references the stale path `{needle}` — \
             Task 7.2 must replace it. The real ADR location is `docs/decisions/` \
             (since story 3.1's ADR-0001); the real protocol-spec location is \
             `docs/protocol.md` (Story 4.3 ships it).",
        );
    }
}

// ---------------------------------------------------------------------------
// Story 4.3 content-drift guardrails. The tests above pin structural shape
// (files exist, sections exist, prose-only fences hold). The tests below
// pin LOAD-BEARING SUBSTANCE — the specific commands, route names, frame
// variants, scope-cut labels that an AC mandated. Pattern matches
// `release_pipeline_docs.rs::assert_contains_all` (Story 3.4).
//
// If a future doc edit silently paraphrases these markers, the AC contract
// breaks even though the doc still parses, still renders, and still passes
// the structural tests.
// ---------------------------------------------------------------------------

fn assert_contains_all(label: &str, body: &str, needles: &[&str]) {
    let missing: Vec<&&str> = needles.iter().filter(|n| !body.contains(**n)).collect();
    assert!(
        missing.is_empty(),
        "{label} missing required substrings: {missing:?}",
    );
}

#[test]
fn quickstart_carries_load_bearing_markers() {
    // AC #1: the five-step walkthrough commands, the Node 22.6+ floor, the
    // troubleshooting grep-target sentence, and the three forward pointers
    // to downstream docs. Each marker is a contract surface — paraphrasing
    // any of them silently breaks the AC.
    let body = read_workspace_file("docs/quickstart.md");
    assert_contains_all(
        "docs/quickstart.md AC #1 markers",
        &body,
        &[
            // Five-step walkthrough commands (AC #1 a–f).
            "bowerbird start",
            "bowerbird replay",
            "bowerbird auth token",
            "BOWERBIRD_TOKEN",
            "--experimental-strip-types",
            "bowerbird stop",
            // Node version floor (AC #1 "Node 22.6+ floor up-front").
            "22.6",
            // Troubleshooting grep-target. AC #1 names the literal line as
            // the failure-mode anchor — readers grep for the JSON-shape
            // hint when the example doesn't print anything.
            "should now see",
            "{event:\"state\"",
            "scrolling on stdout",
            // Three forward pointers (reader-path stack from
            // project-context.md:549-561).
            "docs/presenter-authoring.md",
            "docs/protocol.md",
            "docs/cookbook/",
        ],
    );
}

#[test]
fn presenter_authoring_carries_load_bearing_markers() {
    // AC #2: the six required sections (in order) and the seven
    // ServerMessage variants the doc must document.
    let body = read_workspace_file("docs/presenter-authoring.md");

    // Six sections in order — same state-machine pattern as
    // `every_cookbook_entry_has_canonical_five_sections`.
    const SECTIONS: &[&str] = &[
        "## The substrate model",
        "## Establishing a WebSocket connection",
        "## Sending a Subscribe message",
        "## Handling each ServerMessage frame",
        "## The dropped-frame recovery loop",
        "## Fetching a REST snapshot",
    ];
    let mut seen = 0usize;
    for line in body.lines() {
        if seen < SECTIONS.len() && line.trim_end() == SECTIONS[seen] {
            seen += 1;
        }
    }
    assert_eq!(
        seen,
        SECTIONS.len(),
        "docs/presenter-authoring.md missing AC #2 section ordering. \
         Expected (in order): {SECTIONS:?}. Found {seen} of {} in order; \
         the next missing one is `{}`.",
        SECTIONS.len(),
        SECTIONS.get(seen).copied().unwrap_or("(none)"),
    );

    // Seven ServerMessage variants + topic-grammar markers (AC #2 c, d).
    assert_contains_all(
        "docs/presenter-authoring.md AC #2 variant + topic markers",
        &body,
        &[
            // The seven ServerMessage variants the handler section covers.
            "`hello`",
            "`event`",
            "`state`",
            "`sync`",
            "`dropped`",
            "`close`",
            "`Unknown`",
            // Topic grammar — AC #2 mandates these six topics appear.
            "events.*",
            "events.<source>.*",
            "events.<source>.<session_id>",
            "state.session.*",
            "state.session.<id>",
            "state.session.<id>.current_state",
            // The Bearer-auth + bind_addr resolution markers.
            "Bearer",
            "server.json",
        ],
    );
}

#[test]
fn protocol_md_lists_eight_rest_routes() {
    // AC #3 b: the eight REST routes the daemon exposes. Match against the
    // declared paths so a rename in `crates/daemon/src/api/mod.rs` without a
    // doc update fails this test.
    let body = read_workspace_file("docs/protocol.md");
    assert_contains_all(
        "docs/protocol.md AC #3 b — eight REST routes",
        &body,
        &[
            "/healthz",
            "/readyz",
            "/status",
            "/sessions",
            "/sessions/{id}",
            "/sessions/{id}/events",
            "/sessions/{id}/stats",
            "/replay",
        ],
    );
}

#[test]
fn protocol_md_documents_wire_surface_variants() {
    // AC #3 d: two ClientMessage variants + seven ServerMessage variants,
    // each with its source-of-truth-derived shape. Mismatch here means the
    // wire reference is out of sync with `crates/protocol/src/ws.rs`.
    let body = read_workspace_file("docs/protocol.md");
    assert_contains_all(
        "docs/protocol.md AC #3 d — wire-surface variants",
        &body,
        &[
            // Two ClientMessage variants.
            "### `subscribe`",
            "### `unsubscribe`",
            // Seven ServerMessage variants.
            "### `hello`",
            "### `event`",
            "### `state`",
            "### `sync`",
            "### `dropped`",
            "### `close`",
            "### `Unknown`",
            // Per-frame type names — wire-surface anchors from
            // crates/protocol/src/ws.rs.
            "HelloFrame",
            "EventFrame",
            "StateFrame",
            "SyncFrame",
            "DroppedFrame",
            "CloseFrame",
        ],
    );
}

#[test]
fn protocol_md_covers_topic_grammar_wire_format_and_ingest_contract() {
    // AC #3 a + e + f: wire-format conventions (deny_unknown_fields, bearer
    // auth, protocol_version), the six topic-grammar entries, and the
    // ingest socket contract (path, mode, hook_kind requirement, NDJ
    // framing). These are the dense-reference surfaces a tool author looks
    // up; silent drift here means binding implementations break.
    let body = read_workspace_file("docs/protocol.md");
    assert_contains_all(
        "docs/protocol.md AC #3 a/e/f — conventions + topics + ingest",
        &body,
        &[
            // Wire-format conventions.
            "deny_unknown_fields",
            "protocol_version",
            "Bearer",
            // Topic grammar (six topics).
            "events.*",
            "events.<source>.*",
            "events.<source>.<session_id>",
            "state.session.*",
            "state.session.<id>",
            "state.session.<id>.current_state",
            // Ingest socket contract markers (Story 1.8 + ADR-0002).
            "ingest.sock",
            "hook_kind",
            "0600",
        ],
    );
}

#[test]
fn no_list_enumerates_thirteen_scope_cuts_with_intentional_framing() {
    // AC #5: the thirteen explicit scope cuts (a–m) plus the opening
    // "intentional non-targets" framing. The cuts are the contract surface
    // for "is this in scope?"; quietly dropping one would let a contributor
    // re-litigate a decision the doc was supposed to settle.
    let body = read_workspace_file("docs/no-list.md");
    assert_contains_all(
        "docs/no-list.md AC #5 — thirteen scope cuts + intentional framing",
        &body,
        &[
            // Opening framing — exact phrase from AC #5 mandate.
            "intentional",
            "non-targets",
            // Thirteen cuts (a–m) — match the AC #5 enumeration verbatim.
            "No Windows support",
            "No distro packaging",
            "No HITL",
            "No tool blocking",
            "No personas",
            "No LAN",
            "No daemon-side activity-rate",
            "No crates.io",
            "No `bowerbird gc`",
            "No musl",
            "No code signing",
            "No structured JSON logging",
            "No rate limiting",
        ],
    );
}

#[test]
fn cookbook_readme_lists_three_required_entries() {
    // AC #4 (Story 4.3), reshaped by Story 5.13: the cookbook's README is
    // the entry-surface index. It must link the three self-contained entry
    // directories so a reader sees the surface area at a glance. Drift
    // here = silent loss of discoverability for one of the three patterns.
    let body = read_workspace_file("docs/cookbook/README.md");
    assert_contains_all(
        "docs/cookbook/README.md index markers; three entry directory LINKS \
         (the `](name/)` form, so the quick-run command paths cannot satisfy \
         this test if the index table is deleted)",
        &body,
        &[
            // Three entry directories as markdown link targets.
            "](state-session-fanout/)",
            "](rest-cursor-pagination/)",
            "](dropped-frame-recovery/)",
        ],
    );
}
