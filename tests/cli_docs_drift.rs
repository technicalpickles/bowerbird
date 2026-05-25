//! Hermetic doc-drift guardrails for the Story 4.3 documentation suite.
//!
//! No daemon, no Node, no network. Asserts the five required docs exist,
//! cookbook entries follow the canonical four-section shape, cookbook code
//! blocks are byte-identical to their anchored example regions, the
//! cookbook-anchor consumption is bidirectional (every anchor in
//! `examples/*` is consumed by exactly one cookbook entry; every directive
//! resolves to an anchor), internal markdown links in the docs resolve to
//! files on disk, and architecture.md's `docs/` tree matches the shipped
//! surface (not the stale `docs/architecture/` + `docs/api/` placeholders).
//!
//! Mirrors `tests/cli_examples_drift.rs` shape (Story 4.2). The
//! example-side anchor-existence test lives there; this crate is the
//! consumer-side counterpart.

use std::collections::{HashMap, HashSet};
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
    "docs/cookbook/state-session-fanout.md",
    "docs/cookbook/rest-cursor-pagination.md",
    "docs/cookbook/dropped-frame-recovery.md",
];

const EXAMPLE_SOURCES: &[&str] = &[
    "examples/multi-session-router/src/index.ts",
    "examples/event-log-viewer/src/index.ts",
    "examples/reconnect-recovery/src/index.ts",
];

/// The set of markdown files allowed to consume cookbook anchors. The
/// presenter-authoring guide is included so future Task 2.9-style usage
/// works without a test-side change.
fn cookbook_consumer_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut files: Vec<PathBuf> = Vec::new();
    let cookbook_dir = root.join("docs/cookbook");
    for entry in fs::read_dir(&cookbook_dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", cookbook_dir.display()))
    {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) == Some("md")
            && path.file_name().and_then(|s| s.to_str()) != Some("README.md")
        {
            files.push(path);
        }
    }
    files.push(root.join("docs/presenter-authoring.md"));
    files
}

#[test]
fn required_docs_exist() {
    for rel in REQUIRED_DOCS.iter().chain(REQUIRED_COOKBOOK_ENTRIES.iter()) {
        let p = workspace_root().join(rel);
        assert!(
            p.is_file(),
            "Story 4.3 required doc missing: {} — Task 1-5 must land all \
             five top-level docs plus the three cookbook entries before \
             this test passes",
            p.display(),
        );
    }
}

#[test]
fn every_cookbook_entry_has_canonical_four_sections() {
    // The project-context.md §Cookbook discipline shape: four level-2
    // headings in this order. README.md is exempt — it has its own table
    // shape, not a recipe shape.
    const SECTIONS: &[&str] = &["## Problem", "## Approach", "## Code", "## Variants"];

    for rel in REQUIRED_COOKBOOK_ENTRIES {
        let body = read_workspace_file(rel);
        let mut seen = 0usize;
        for line in body.lines() {
            if seen < SECTIONS.len() && line.trim_end() == SECTIONS[seen] {
                seen += 1;
            }
        }
        assert_eq!(
            seen,
            SECTIONS.len(),
            "{rel} missing canonical section ordering. Expected (in order): \
             {SECTIONS:?}. Found {seen} of {} sections in the right order — \
             the next missing one is `{}`.",
            SECTIONS.len(),
            SECTIONS.get(seen).copied().unwrap_or("(none)"),
        );
    }
}

/// A cookbook-include directive found in a markdown file. The owning file
/// is tracked by the caller (the directive is meaningful only relative to
/// that file's parent dir).
#[derive(Debug, Clone)]
struct Directive {
    /// Relative path the directive references (verbatim, as written).
    referenced_path: String,
    /// Anchor name (the `<anchor>` in `cookbook-begin:<anchor>`).
    anchor: String,
    /// Byte offset in the markdown body immediately after the directive line.
    after_directive_offset: usize,
}

/// Find every `<!-- cookbook-include: <path> cookbook-begin:<anchor> -->`
/// directive in the markdown body. Hand-rolled scan; no regex dep.
fn find_directives(body: &str) -> Vec<Directive> {
    const START: &str = "<!-- cookbook-include: ";
    const MID: &str = " cookbook-begin:";
    const END: &str = " -->";

    let mut directives = Vec::new();
    let mut cursor = 0usize;
    while let Some(start_rel) = body[cursor..].find(START) {
        let start = cursor + start_rel;
        let after_start = start + START.len();
        let Some(mid_rel) = body[after_start..].find(MID) else {
            cursor = after_start;
            continue;
        };
        let mid = after_start + mid_rel;
        let referenced_path = body[after_start..mid].trim().to_string();

        let after_mid = mid + MID.len();
        let Some(end_rel) = body[after_mid..].find(END) else {
            cursor = after_mid;
            continue;
        };
        let end = after_mid + end_rel;
        let anchor = body[after_mid..end].trim().to_string();

        // Advance to the end of the directive line so the next-fenced-block
        // search starts after the directive.
        let after_directive = end + END.len();
        let after_directive_offset = body[after_directive..]
            .find('\n')
            .map(|i| after_directive + i + 1)
            .unwrap_or(body.len());

        directives.push(Directive {
            referenced_path,
            anchor,
            after_directive_offset,
        });

        cursor = after_directive_offset;
    }
    directives
}

/// Extract the body of the next fenced code block starting at or after
/// `from_offset`. Permissive about the language tag — accepts ```ts or ```
/// or anything else (the directive identifies the snippet, not the lang
/// hint). Returns the inner body trimmed of leading/trailing blank lines,
/// or None if no fence is found.
fn next_fenced_block_body(body: &str, from_offset: usize) -> Option<String> {
    let tail = &body[from_offset..];
    // Find an opening fence on its own line (allow whitespace prefix).
    let mut search_from = 0usize;
    loop {
        let fence_rel = tail[search_from..].find("```")?;
        let fence_pos = search_from + fence_rel;
        // Confirm the fence is at line start (or only whitespace before it
        // on the line).
        let line_start = tail[..fence_pos].rfind('\n').map_or(0, |i| i + 1);
        let pre_fence = &tail[line_start..fence_pos];
        if !pre_fence.chars().all(char::is_whitespace) {
            search_from = fence_pos + 3;
            continue;
        }
        // Skip to end of opening-fence line (consumes the language tag).
        let after_open_line = tail[fence_pos..].find('\n').map(|i| fence_pos + i + 1)?;
        // Find the closing ``` on its own line.
        let mut close_search_from = after_open_line;
        loop {
            let close_rel = tail[close_search_from..].find("```")?;
            let close_pos = close_search_from + close_rel;
            let close_line_start = tail[..close_pos].rfind('\n').map_or(0, |i| i + 1);
            let close_pre_fence = &tail[close_line_start..close_pos];
            if !close_pre_fence.chars().all(char::is_whitespace) {
                close_search_from = close_pos + 3;
                continue;
            }
            return Some(tail[after_open_line..close_line_start].to_string());
        }
    }
}

/// Extract the body between `// cookbook-begin:<anchor>` and
/// `// cookbook-end:<anchor>` in a TypeScript source file. Returns None if
/// either marker is missing or the begin marker is after the end marker.
fn extract_anchored_region(ts_body: &str, anchor: &str) -> Option<String> {
    let begin_marker = format!("// cookbook-begin:{anchor}");
    let end_marker = format!("// cookbook-end:{anchor}");
    let begin_line_start = ts_body.find(&begin_marker)?;
    let after_begin_line = ts_body[begin_line_start..]
        .find('\n')
        .map(|i| begin_line_start + i + 1)?;
    let end_line_start = ts_body[after_begin_line..].find(&end_marker)? + after_begin_line;
    let end_line_real_start = ts_body[..end_line_start].rfind('\n').map_or(0, |i| i + 1);
    if end_line_real_start <= after_begin_line {
        return None;
    }
    Some(ts_body[after_begin_line..end_line_real_start].to_string())
}

#[test]
fn cookbook_include_directives_match_example_anchors() {
    use pretty_assertions::assert_eq;

    let mut total_directives = 0usize;
    for md_path in cookbook_consumer_files() {
        let md_body = match fs::read_to_string(&md_path) {
            Ok(s) => s,
            Err(_) => continue, // presenter-authoring.md may not exist in early dev; skip silently
        };
        for directive in find_directives(&md_body) {
            total_directives += 1;
            let ts_path = md_path
                .parent()
                .expect("markdown parent")
                .join(&directive.referenced_path);
            let ts_canon = ts_path.canonicalize().unwrap_or_else(|e| {
                panic!(
                    "directive in {} references {} which cannot be resolved: {e}",
                    md_path.display(),
                    ts_path.display(),
                )
            });
            let ts_body = fs::read_to_string(&ts_canon)
                .unwrap_or_else(|e| panic!("read {}: {e}", ts_canon.display()));
            let anchored = extract_anchored_region(&ts_body, &directive.anchor)
                .unwrap_or_else(|| {
                    panic!(
                        "no `cookbook-begin:{}` / `cookbook-end:{}` pair in {} (directive lives in {})",
                        directive.anchor,
                        directive.anchor,
                        ts_canon.display(),
                        md_path.display(),
                    )
                });
            let fenced = next_fenced_block_body(&md_body, directive.after_directive_offset)
                .unwrap_or_else(|| {
                    panic!(
                        "no fenced code block found after `cookbook-include` directive for \
                         anchor `{}` in {} — every cookbook-include must be followed by a \
                         ```ts (or ```) fenced block",
                        directive.anchor,
                        md_path.display(),
                    )
                });

            // Normalize leading/trailing whitespace per AC #6c. Internal
            // whitespace and newlines are preserved.
            let lhs = anchored.trim();
            let rhs = fenced.trim();
            assert_eq!(
                lhs,
                rhs,
                "cookbook drift detected: anchor `{}` in {} disagrees with fenced block in {}. \
                 The fenced block MUST be byte-identical to the example's anchored region \
                 (leading/trailing whitespace normalized). Either update the cookbook entry \
                 to match the example, or update the example's anchored region and re-run \
                 this test.",
                directive.anchor,
                ts_canon.display(),
                md_path.display(),
            );
        }
    }
    assert!(
        total_directives > 0,
        "no cookbook-include directives found across any consumer file — \
         the cookbook entries must use the directive to enforce drift-detection",
    );
}

#[test]
fn every_cookbook_anchor_in_examples_has_a_cookbook_entry() {
    // Bidirectional integrity. Pairs with
    // tests/cli_examples_drift.rs::each_example_source_carries_cookbook_anchors
    // — that test asserts the markers EXIST; this asserts they are CONSUMED.

    // 1. Collect every anchor name from every example src/index.ts.
    let mut example_anchors: Vec<(String, PathBuf)> = Vec::new();
    for ex in EXAMPLE_SOURCES {
        let path = workspace_root().join(ex);
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for line in body.lines() {
            const BEGIN: &str = "// cookbook-begin:";
            if let Some(idx) = line.find(BEGIN) {
                let anchor = line[idx + BEGIN.len()..].trim().to_string();
                example_anchors.push((anchor, path.clone()));
            }
        }
    }

    // 2. Collect every directive across the consumer files (cookbook
    //    entries + presenter-authoring.md).
    let mut consumers: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for md_path in cookbook_consumer_files() {
        let body = match fs::read_to_string(&md_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for directive in find_directives(&body) {
            consumers
                .entry(directive.anchor)
                .or_default()
                .push(md_path.clone());
        }
    }

    // 3. Assert each example anchor has EXACTLY ONE consumer.
    let mut orphan_msgs: Vec<String> = Vec::new();
    for (anchor, ex_path) in &example_anchors {
        match consumers.get(anchor).map(Vec::as_slice).unwrap_or(&[]) {
            [] => orphan_msgs.push(format!(
                "anchor `{anchor}` in {} is not referenced by any cookbook entry \
                 (or `docs/presenter-authoring.md`) — either consume it via a \
                 `<!-- cookbook-include: ... cookbook-begin:{anchor} -->` directive \
                 or remove the anchor markers from the example",
                ex_path.display(),
            )),
            [_] => { /* exactly one consumer — good */ }
            many => orphan_msgs.push(format!(
                "anchor `{anchor}` in {} is consumed by {} files (expected exactly 1): {:?}",
                ex_path.display(),
                many.len(),
                many.iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>(),
            )),
        }
    }

    // 4. Assert no directive references an anchor that doesn't exist in
    //    examples (catches typos in directive anchor names).
    let example_anchor_names: HashSet<&str> =
        example_anchors.iter().map(|(a, _)| a.as_str()).collect();
    for (anchor, md_paths) in &consumers {
        if !example_anchor_names.contains(anchor.as_str()) {
            for md_path in md_paths {
                orphan_msgs.push(format!(
                    "directive in {} references anchor `{anchor}` which does NOT exist in \
                     any example src/index.ts — typo, or the anchor was removed",
                    md_path.display(),
                ));
            }
        }
    }

    assert!(
        orphan_msgs.is_empty(),
        "cookbook-anchor bidirectional integrity violations:\n  - {}",
        orphan_msgs.join("\n  - "),
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
        "docs/quickstart.md",
        "docs/presenter-authoring.md",
        "docs/protocol.md",
        "docs/cookbook/README.md",
        "docs/no-list.md",
        "docs/cookbook/state-session-fanout.md",
        "docs/cookbook/rest-cursor-pagination.md",
        "docs/cookbook/dropped-frame-recovery.md",
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
// (files exist, sections exist, cookbook anchors match). The tests below
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
    // `every_cookbook_entry_has_canonical_four_sections`.
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
fn cookbook_readme_lists_three_required_entries_paired_with_examples() {
    // AC #4: the cookbook's README is the entry-surface index. It must
    // table-list the three V1 entries paired with their example tools so a
    // reader sees the surface area at a glance. Drift here = silent loss
    // of discoverability for one of the three patterns.
    let body = read_workspace_file("docs/cookbook/README.md");
    assert_contains_all(
        "docs/cookbook/README.md AC #4 — three entries paired with examples",
        &body,
        &[
            // Three cookbook entries (markdown link targets).
            "state-session-fanout.md",
            "rest-cursor-pagination.md",
            "dropped-frame-recovery.md",
            // Three paired example directories.
            "multi-session-router",
            "event-log-viewer",
            "reconnect-recovery",
        ],
    );
}
