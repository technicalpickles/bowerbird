//! Story 4.4 / AC #1 / FR39 — protocol-changelog CI gate.
//!
//! Any PR that modifies `crates/protocol/src/*.rs` MUST add at least one entry
//! to `docs/protocol-changelog.md` under the active version section with a
//! `type:` header (`type: schema`, `type: behavioral`, or `type: security`).
//! The gate fires as a compiled workspace-root test that runs under
//! `cargo test --workspace -- --test-threads=1` (the standard CI lane) —
//! "compiled tests beat greps" per Epic 3 retro Team Agreement A7.
//!
//! Base-ref resolution chain (first that resolves wins):
//!   1. `BOWERBIRD_CHANGELOG_GATE_BASE` — local-developer override.
//!   2. `origin/$GITHUB_BASE_REF` — what GitHub Actions sets on `pull_request`.
//!   3. `origin/main` — the default branch.
//!
//! When none resolve (fresh clone, no `origin` remote, detached checkout in a
//! local sandbox), the test SKIPs with a `println!("SKIPPED: ...")` and
//! returns — CI always has the ref, so the gate fires on every PR; local
//! `cargo test` on a checkout without a remote does NOT fail spuriously.
//!
//! Hermetic — no daemon, no network, no Node. Shells out to `git` via
//! `std::process::Command` (workspace already uses this pattern in other
//! workspace-root tests; the rust-binding crates `git2` / `gix` would bloat
//! the dep tree for a single diff operation).

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Resolves the base ref to diff against. Returns `Ok(refname)` on success,
/// `Err(reason)` when no base ref is resolvable.
fn resolve_base_ref() -> Result<String, String> {
    // 1. Explicit developer override.
    if let Ok(base) = std::env::var("BOWERBIRD_CHANGELOG_GATE_BASE") {
        if ref_exists(&base) {
            return Ok(base);
        }
        return Err(format!(
            "BOWERBIRD_CHANGELOG_GATE_BASE={base} but ref does not resolve"
        ));
    }

    // 2. GitHub PR convention: GITHUB_BASE_REF holds the unqualified branch
    //    name; origin/<name> is what local diff needs.
    if let Ok(branch) = std::env::var("GITHUB_BASE_REF") {
        if !branch.is_empty() {
            let qualified = format!("origin/{branch}");
            if ref_exists(&qualified) {
                return Ok(qualified);
            }
        }
    }

    // 3. Default branch.
    if ref_exists("origin/main") {
        return Ok("origin/main".to_string());
    }

    Err("no base ref found (tried BOWERBIRD_CHANGELOG_GATE_BASE, origin/$GITHUB_BASE_REF, origin/main)".to_string())
}

fn ref_exists(name: &str) -> bool {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", name])
        .current_dir(workspace_root())
        .output();
    match out {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// Runs `git diff --name-only <base>...HEAD` and returns the list of changed paths.
fn changed_paths(base: &str) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["diff", "--name-only", &format!("{base}...HEAD")])
        .current_dir(workspace_root())
        .output()
        .map_err(|e| format!("git diff failed to spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff exited {:?}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.lines().map(|s| s.to_string()).collect())
}

/// Runs `git diff -U0 <base>...HEAD -- <path>` and returns the added-line bodies
/// (the diff lines that start with `+` but not `+++`, with the leading `+`
/// stripped). Unified-context = 0 so we only see actually-added lines, not
/// surrounding context.
fn added_lines(base: &str, path: &str) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["diff", "-U0", &format!("{base}...HEAD"), "--", path])
        .current_dir(workspace_root())
        .output()
        .map_err(|e| format!("git diff failed to spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff -U0 exited {:?}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines = stdout
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| l.trim_start_matches('+').to_string())
        .collect();
    Ok(lines)
}

const PROTOCOL_SRC_PREFIX: &str = "crates/protocol/src/";
const CHANGELOG_PATH: &str = "docs/protocol-changelog.md";
const ALLOWED_TYPES: &[&str] = &["type: schema", "type: behavioral", "type: security"];

#[test]
fn protocol_src_changes_require_changelog_entry() {
    let base = match resolve_base_ref() {
        Ok(b) => b,
        Err(reason) => {
            println!(
                "SKIPPED: protocol_changelog_gate: {reason}. CI always has a base ref; \
                 local `cargo test` without an `origin` remote skips cleanly."
            );
            return;
        }
    };

    let changed = match changed_paths(&base) {
        Ok(paths) => paths,
        Err(e) => {
            println!(
                "SKIPPED: protocol_changelog_gate: could not list changed paths against {base}: {e}"
            );
            return;
        }
    };

    let protocol_changes: Vec<&String> = changed
        .iter()
        .filter(|p| {
            p.starts_with(PROTOCOL_SRC_PREFIX)
                && Path::new(p).extension().and_then(|s| s.to_str()) == Some("rs")
        })
        .collect();

    if protocol_changes.is_empty() {
        // No protocol-crate changes; the gate has nothing to enforce.
        return;
    }

    let added = added_lines(&base, CHANGELOG_PATH).unwrap_or_else(|e| {
        panic!(
            "could not read added lines from {CHANGELOG_PATH} against {base}: {e}. \
             This is unexpected — `git diff` should succeed even if the file is unchanged \
             (it would simply return zero added lines)."
        )
    });

    let has_type_header = added
        .iter()
        .any(|line| ALLOWED_TYPES.iter().any(|t| line.contains(t)));

    assert!(
        has_type_header,
        "Protocol-crate changes require a corresponding changelog entry.\n\n\
         Changed files matching `{PROTOCOL_SRC_PREFIX}*.rs`:\n  {paths}\n\n\
         Expected at least one new entry in {CHANGELOG_PATH} with a `type:` header \
         from the allowed set ({allowed}). None were found in the diff against `{base}`.\n\n\
         How to fix: add an entry under the active version section of {CHANGELOG_PATH} \
         describing the wire-surface or behavioral impact of your change. See the preamble \
         of {CHANGELOG_PATH} for the gate's rules and Epic 3 retro Team Agreement A7 for \
         why this is enforced as a compiled test rather than a shell-script grep.",
        paths = protocol_changes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        allowed = ALLOWED_TYPES.join(", "),
        base = base,
    );
}
