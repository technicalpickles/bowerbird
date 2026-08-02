//! Hermetic doc-drift guardrails for the cookbook reference entries (Story
//! 4.2; consolidated into `docs/cookbook/<name>/` by Story 5.13; the entry
//! list glob-derived by Story 6-session-glance).
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
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Is `p` a directory, FOLLOWING symlinks?
///
/// `DirEntry::file_type()` does NOT follow symlinks (on Unix it is readdir's
/// `d_type`), so a symlinked entry directory reads as neither file nor
/// directory and every listing built on it silently drops the entry. CI's
/// `for d in docs/cookbook/*/` glob does the opposite: the shell's `*/`
/// resolves the link and matches, so CI would `npm ci && npm run typecheck`
/// a directory that no guard in this file has ever seen. `fs::metadata`
/// resolves, which puts the two back in agreement.
///
/// Duplicated in all three `cookbook_entry_dirs()` copies deliberately: each
/// `tests/*.rs` file is its own crate. It landed in THIS one alone the first
/// time, which left `cli_examples.rs` and `cli_docs_drift.rs` still dropping a
/// symlinked entry -- a fix in one of three copies is not a fix (taskwarrior
/// `4238d5ea` tracks the shared-helper cleanup).
fn is_dir_following_symlinks(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

fn read_workspace_file(rel: &str) -> String {
    let p = workspace_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Floor on how many cookbook entries the glob must find. See the twin
/// constant in `tests/cli_docs_drift.rs` for the rationale (deliberately
/// below the current count; it exists to catch an empty derivation, not to
/// track the real number).
const MIN_EXPECTED_ENTRIES: usize = 3;

/// One entry name that must always be in the derived list. The smallest
/// hardcoded fact that proves the glob is reading the right directory.
const ANCHOR_ENTRY: &str = "rest-cursor-pagination";

/// Every cookbook entry directory name, derived from the `docs/cookbook/*/`
/// glob (Story 6-session-glance, AC 4, extended to this file because the
/// hardcoded `ENTRIES` const here was the exact twin of `cli_docs_drift.rs`'s
/// `REQUIRED_COOKBOOK_ENTRIES`, and globbing one while leaving the other
/// literal just relocates the divergence).
///
/// Mirrors CI's own entry filter (`.github/workflows/ci.yml`): a subdir with
/// no `package.json` is skipped, exactly as CI skips it. The assertion that
/// no such subdir currently exists is
/// [`every_cookbook_subdirectory_is_a_typechecked_entry`] below.
///
/// Duplicated (not shared) with `cli_docs_drift.rs`: each `tests/*.rs` file is
/// its own crate, and this file already duplicates `workspace_root` /
/// `read_workspace_file` for the same reason.
///
/// **A13 positive companion.** Asserts non-empty, above the floor, and
/// containing a known anchor BEFORE any caller asserts the derived entries
/// conform. An empty derivation would make every downstream assertion pass
/// vacuously.
fn cookbook_entry_dirs() -> Vec<String> {
    let cookbook = workspace_root().join("docs/cookbook");
    let mut dirs: Vec<String> = fs::read_dir(&cookbook)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", cookbook.display()))
        .filter_map(|entry| {
            let entry = entry.expect("dir entry");
            if !is_dir_following_symlinks(&entry.path()) {
                return None;
            }
            if !entry.path().join("package.json").is_file() {
                return None;
            }
            Some(entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    dirs.sort();

    assert!(
        dirs.len() >= MIN_EXPECTED_ENTRIES,
        "docs/cookbook/*/ derivation found {} entries ({dirs:?}), fewer than \
         the floor of {MIN_EXPECTED_ENTRIES}. Every guard in this file asserts \
         over this list, so an under-derived list would make them all pass \
         vacuously.",
        dirs.len(),
    );
    assert!(
        dirs.iter().any(|d| d == ANCHOR_ENTRY),
        "docs/cookbook/*/ derivation ({dirs:?}) is missing the anchor entry \
         `{ANCHOR_ENTRY}`; the glob is not reading the cookbook directory."
    );
    dirs
}

/// Successor to `entries_const_matches_directory_listing` (and to
/// `cli_docs_drift.rs::cookbook_entry_consts_match_directory_listing`), both
/// of which compared a hardcoded const against this same glob and go
/// tautological once the const is derived from it.
///
/// Two gaps glob-derivation cannot close on its own:
///
///  1. A `docs/cookbook/` subdirectory with NO `package.json`. Both CI's
///     typecheck loop and [`cookbook_entry_dirs`] skip those by design (a
///     docs tree is allowed non-entry subdirs like `images/`), which means
///     such a directory would get neither typecheck nor shape coverage while
///     still looking like an entry to a reader.
///  2. A SYMLINKED entry directory. This one runs the other way: CI's
///     `for d in docs/cookbook/*/` resolves the link and typechecks it, so
///     the entry exists as far as CI is concerned. The guards here follow it
///     too now (see [`is_dir_following_symlinks`]) -- but a symlinked entry
///     still means the real files live outside `docs/cookbook/`, where
///     `check-file-list.py`, the prose-only scan, and the required-files
///     shape all point at a path that is not the source of truth. So it is
///     rejected outright rather than accommodated.
///
/// Today neither exists, so introducing one is a deliberate act that edits
/// this test rather than a silent coverage hole.
#[test]
fn every_cookbook_subdirectory_is_a_typechecked_entry() {
    let cookbook = workspace_root().join("docs/cookbook");
    let mut all_dirs: Vec<String> = fs::read_dir(&cookbook)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", cookbook.display()))
        .filter_map(|entry| {
            let entry = entry.expect("dir entry");
            is_dir_following_symlinks(&entry.path())
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    all_dirs.sort();

    // Positive companion: the listing found directories at all, so the
    // set-equality below is comparing two real sets, not two empty ones.
    assert!(
        !all_dirs.is_empty(),
        "docs/cookbook/ has no subdirectories; the listing is reading the \
         wrong path"
    );

    for name in &all_dirs {
        let p = cookbook.join(name);
        let meta = fs::symlink_metadata(&p)
            .unwrap_or_else(|e| panic!("symlink_metadata {}: {e}", p.display()));
        assert!(
            !meta.is_symlink(),
            "docs/cookbook/{name} is a symlink to a directory. CI's \
             `for d in docs/cookbook/*/` loop follows it and typechecks it, so it \
             reads as a real entry -- but its actual files live outside \
             docs/cookbook/, where the story File List audit, the required-files \
             shape guard, and the prose-only scan all address the wrong path. \
             Move the entry into docs/cookbook/ for real, or amend this test."
        );
    }

    assert_eq!(
        all_dirs,
        cookbook_entry_dirs(),
        "a docs/cookbook/ subdirectory has no package.json, so CI's typecheck \
         loop (.github/workflows/ci.yml) and every glob-derived guard skip it \
         while it still reads as an entry. Either give it a package.json (making \
         it a real, guarded entry) or amend this test to name it as a deliberate \
         non-entry docs subdirectory."
    );
}

#[test]
fn each_entry_has_required_files() {
    for name in cookbook_entry_dirs() {
        for rel in &[
            "src/index.ts",
            "README.md",
            "package.json",
            // npm ci (the CI typecheck loop) hard-fails without a lockfile.
            "package-lock.json",
            "tsconfig.json",
        ] {
            let p = workspace_root().join("docs/cookbook").join(&name).join(rel);
            assert!(
                p.is_file(),
                "Story 5.13: docs/cookbook/{name}/{rel} missing; required for the \
                 cookbook entry to be runnable + documented (invariant from Story 4.2)"
            );
        }
    }
}

/// Every `*.test.ts` FILE under `dir`, recursively.
///
/// Recursive because the npm script's glob is `tests/**/*.test.ts`: a
/// non-recursive `read_dir` reports "no tests here" for an entry whose suite
/// lives one directory down, and would then fail an entry that CI runs
/// perfectly well.
///
/// `is_file()` because a name check alone counts things `npm test` cannot run.
/// The review called out a DIRECTORY named `foo.test.ts`; measured, the
/// recursion branch above already claims that case (it is a directory, so it
/// is descended into and contributes nothing), so `is_file()` is not what
/// closes it. What `is_file()` does close is everything the directory branch
/// declines: a DANGLING SYMLINK named `foo.test.ts` is neither a directory nor
/// a file, and without this check it counted as coverage while `npm test`
/// reported green over an empty suite. Verified both ways.
fn test_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_dir_following_symlinks(&path) {
            found.extend(test_files_under(&path));
        } else if path.is_file() && path.to_string_lossy().ends_with(".test.ts") {
            found.push(path);
        }
    }
    found
}

/// An entry's `tests/` sidecar, its `scripts.test`, and CI's invocation of
/// that script must exist together or not at all.
///
/// CI's cookbook loop runs `npm test --if-present`, which is silent when an
/// entry declares no test script. That is deliberate (two entries genuinely
/// have no unit tests) and it is also exactly how a test suite becomes dead
/// weight: `session-glance/tests/glance.test.ts` -- the ONLY executable
/// statement of the canonical `deriveRepo` contract, and the only coverage of
/// the worktree-`.git`-as-a-FILE branch -- ran nowhere in CI until the
/// `--if-present` line landed, because that job used to run typecheck alone
/// and the Rust smoke never invokes npm.
///
/// Four things have to hold, and the guard originally checked one of them:
///
///  1. **CI actually runs the script.** This is the finding the guard is named
///     for and the one it did not check: it never read `ci.yml` at all, so
///     deleting `&& npm test --if-present` from the workflow left every
///     assertion here green while `glance.test.ts` went straight back to
///     running nowhere. A guard that cannot see the regression it was written
///     for is decoration.
///  2. **`tests/` present iff `scripts.test` present.** Tests without a script
///     are a suite CI skips; a script without tests is a command that matches
///     no files.
///  3. **The script actually runs the sidecar.** Key PRESENCE was the whole
///     check, so `"test": "true"` passed: a script that exits 0 without
///     reading a single test file is worse than no script, because it reports
///     green.
///  4. **The sidecar contains runnable test FILES**, found the way the npm
///     glob finds them. See [`test_files_under`].
///
/// Deliberately NOT a flat "every entry must have tests/": `rest-cursor-
/// pagination` and `state-session-fanout` ship none, and manufacturing test
/// files to satisfy a guard is how a suite gets filler. What is enforced is
/// that tests an entry DOES ship are wired to the command CI runs.
#[test]
fn entry_tests_are_wired_to_npm_test() {
    // (1) The workflow line itself. Scoped to the cookbook loop's own command
    // so an unrelated `npm test` elsewhere in the file cannot satisfy it.
    let ci = read_workspace_file(".github/workflows/ci.yml");
    let loop_line = ci
        .lines()
        .find(|l| l.contains("cd \"$d\"") && l.contains("npm ci"))
        .unwrap_or_else(|| {
            panic!(
                "no cookbook loop line in .github/workflows/ci.yml (looked for one running \
                 `cd \"$d\"` and `npm ci`). Either the loop moved or the guard below is \
                 asserting over a workflow that no longer exists."
            )
        });
    assert!(
        loop_line.contains("npm test"),
        "the cookbook loop in .github/workflows/ci.yml does not run `npm test`, so every \
         entry's tests/ sidecar runs NOWHERE in CI: the Rust smoke never invokes npm and \
         the typecheck job only runs tsc. That is the exact regression the assertions \
         below exist to prevent, and without this line they all stay green through it. \
         Got: `{loop_line}`"
    );
    assert!(
        loop_line.contains("npm run typecheck"),
        "the cookbook loop no longer runs `npm run typecheck`; got: `{loop_line}`"
    );

    let mut with_tests = 0usize;
    for name in cookbook_entry_dirs() {
        let dir = workspace_root().join("docs/cookbook").join(&name);
        let tests_dir = dir.join("tests");
        let has_tests_dir = is_dir_following_symlinks(&tests_dir);
        let body = read_workspace_file(&format!("docs/cookbook/{name}/package.json"));
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("docs/cookbook/{name}/package.json invalid JSON: {e}"));
        let test_script = parsed
            .get("scripts")
            .and_then(|v| v.get("test"))
            .and_then(|v| v.as_str());

        // (2) The biconditional.
        assert_eq!(
            has_tests_dir,
            test_script.is_some(),
            "docs/cookbook/{name}: tests/ dir present = {has_tests_dir}, package.json \
             scripts.test present = {}. They must agree. CI runs `npm test --if-present` \
             per entry, so a tests/ dir with no script is a suite nothing executes, and a \
             script with no tests/ dir is a command that matches no files.",
            test_script.is_some(),
        );

        if let Some(script) = test_script {
            // (3) The script has to READ the sidecar. Presence of the key was
            // the entire check, so `"test": "true"` passed it.
            assert!(
                script.contains("tests/"),
                "docs/cookbook/{name}: scripts.test = {script:?} never mentions `tests/`, so \
                 `npm test` can exit 0 without running a single file in the sidecar. A test \
                 script that reports green over nothing is worse than no test script."
            );
        }

        if has_tests_dir {
            with_tests += 1;
            // (4) Runnable files, found the way the npm glob finds them.
            let found = test_files_under(&tests_dir);
            assert!(
                !found.is_empty(),
                "docs/cookbook/{name}/tests/ has no *.test.ts FILES (searched recursively, \
                 the way the `tests/**/*.test.ts` glob does), so `npm test` runs an empty \
                 suite and reports green"
            );
        }
    }
    // A13 positive companion: at least one entry actually exercises the
    // has-tests branch above. If every entry lost its tests/ dir, the
    // biconditional would hold vacuously for all of them and this guard would
    // report green over zero coverage.
    assert!(
        with_tests > 0,
        "no cookbook entry has a tests/ directory, so the wiring assertions above all \
         passed on the empty branch"
    );
}

#[test]
fn each_entry_package_json_declares_node_22_6_engine() {
    let re = regex_lite_match;
    for name in cookbook_entry_dirs() {
        let name = name.as_str();
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
    // Positive tree-shape assertions: the diagram must actually show a
    // representative entry with its Node project files, not just the
    // workspace-manifest comment (review 2026-08-01, Blind Hunter finding 3).
    for needle in ["state-session-fanout/", "package.json", "tsconfig.json"] {
        assert!(
            arch.contains(needle),
            "architecture.md no longer mentions `{needle}`; the §Project \
             Structure tree must show the per-entry cookbook shape \
             (README.md + package.json + tsconfig.json + src/index.ts)"
        );
    }
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
    // forward through the Story 5.13 move. Scoped to the members line so a
    // legitimate `exclude = [...]` or a comment naming the path elsewhere in
    // the manifest cannot false-fail (review 2026-08-01).
    let members_line = body
        .lines()
        .find(|l| l.trim_start().starts_with("members"))
        .expect("root Cargo.toml has a [workspace] members line");
    assert!(
        members_line.contains("members = [\"crates/*\"]"),
        "root Cargo.toml must keep `members = [\"crates/*\"]` only; adding \
         cookbook entries would make them Cargo workspace members, \
         contradicting the TypeScript-on-Node decision (got: `{members_line}`)"
    );
    for banned in ["examples/", "docs/cookbook"] {
        assert!(
            !members_line.contains(banned),
            "root Cargo.toml lists `{banned}` in the workspace members; the \
             cookbook entries are a Node project zone, not a Cargo zone \
             (Story 4.2 invariant, Story 5.13 location)"
        );
    }
}
