// Node-built-in `--test` runner. Run with:
//
//     npm test        # from docs/cookbook/session-glance/
//     node --experimental-strip-types --test 'tests/**/*.test.ts'
//
// Covers the pure branches of the entry: the canonical repo derivation, the
// age formatter, the grouped-text renderer, and the flag parser. The
// end-to-end behavior (real daemon, real REST filter, the machine modes)
// lives in the Rust smoke at `tests/cli_examples.rs`; this file exercises the
// branches that a fixture-driven smoke cannot reach, notably the derivation's
// worktree (`.git` as a FILE) and unreadable-path fallbacks.

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  deriveRepo,
  formatAge,
  ageSeconds,
  parseArgs,
  renderText,
  toRow,
  type GlanceRow,
} from "../src/index.ts";

function withTempTree(fn: (root: string) => void): void {
  const root = mkdtempSync(join(tmpdir(), "session-glance-test-"));
  try {
    fn(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function row(overrides: Partial<GlanceRow>): GlanceRow {
  return {
    repo: "repo",
    source: "claude",
    session_id: "s",
    current_state: "Idle",
    age: "1s",
    age_seconds: 1,
    started_at: 0,
    cwd: null,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// deriveRepo: the canonical FR44 derivation. Rule numbers refer to the doc
// comment on the function.
// ---------------------------------------------------------------------------

test("deriveRepo rule 1: null cwd lands in the named bucket, never dropped", () => {
  assert.equal(deriveRepo(null), "(unknown repo)");
  assert.equal(deriveRepo(""), "(unknown repo)");
});

test("deriveRepo rule 2: cwd below the repo root resolves to the repo", () => {
  withTempTree((root) => {
    const repo = join(root, "my-repo");
    mkdirSync(join(repo, ".git"), { recursive: true });
    mkdirSync(join(repo, "crates", "daemon", "src"), { recursive: true });
    assert.equal(deriveRepo(join(repo, "crates", "daemon", "src")), "my-repo");
    // And the repo root itself.
    assert.equal(deriveRepo(repo), "my-repo");
  });
});

test("deriveRepo rule 2: a worktree's `.git` FILE stops the walk", () => {
  // The load-bearing case for "existence, not isDirectory()". A worktree
  // nested inside a real repo would resolve to the OUTER repo under an
  // isDirectory() check, which is the bug this rule exists to prevent.
  withTempTree((root) => {
    const main = join(root, "main-repo");
    mkdirSync(join(main, ".git"), { recursive: true });
    const worktree = join(main, "worktrees", "feature-branch");
    mkdirSync(worktree, { recursive: true });
    writeFileSync(join(worktree, ".git"), "gitdir: /somewhere/.git/worktrees/feature-branch\n");

    assert.equal(deriveRepo(worktree), "feature-branch");
    // Positive companion: the outer repo IS reachable by walking up, so the
    // assertion above is proving the walk stopped, not that it never started.
    assert.equal(deriveRepo(join(main, "src")), "main-repo");
  });
});

test("deriveRepo rule 3: no `.git` ancestor falls back to basename(cwd)", () => {
  withTempTree((root) => {
    const plain = join(root, "just-a-dir");
    mkdirSync(plain, { recursive: true });
    assert.equal(deriveRepo(plain), "just-a-dir");
  });
});

test("deriveRepo rules 3+4: an unreadable path is a bucket, not a crash", () => {
  // A cwd recorded on another machine, or on a since-deleted directory.
  assert.equal(deriveRepo("/definitely/not/a/real/path/some-project"), "some-project");
  assert.equal(deriveRepo("/"), "/");
});

// ---------------------------------------------------------------------------
// Age formatting.
// ---------------------------------------------------------------------------

test("formatAge renders two units and never NaN", () => {
  const now = 1_000_000_000_000;
  assert.equal(formatAge(now, now), "0s");
  assert.equal(formatAge(now - 37_000, now), "37s");
  assert.equal(formatAge(now - (4 * 60 + 12) * 1000, now), "4m12s");
  assert.equal(formatAge(now - (83 * 60) * 1000, now), "1h23m");
  assert.equal(formatAge(now - (3 * 24 + 4) * 3600 * 1000, now), "3d4h");
});

test("formatAge names the null case instead of printing NaN or 1970", () => {
  const rendered = formatAge(null, 1_000_000_000_000);
  assert.equal(rendered, "age unknown");
  assert.ok(!rendered.includes("NaN"), "must not render NaN");
  assert.ok(!rendered.includes("1970"), "must not render a 1970 timestamp");
  assert.equal(ageSeconds(null, 1_000_000_000_000), null);
});

test("formatAge clamps a future started_at instead of going negative", () => {
  const now = 1_000_000_000_000;
  assert.equal(formatAge(now + 60_000, now), "0s");
  assert.equal(ageSeconds(now + 60_000, now), 0);
});

// ---------------------------------------------------------------------------
// Grouped text rendering.
// ---------------------------------------------------------------------------

test("renderText groups by repo, sorts deterministically, indents sessions", () => {
  const lines = renderText(
    [
      row({ repo: "beta", session_id: "two", current_state: "Working", age: "5s" }),
      row({ repo: "alpha", session_id: "b", current_state: "Idle", age: "1m0s" }),
      row({ repo: "alpha", session_id: "a", current_state: "WaitingInput", age: "9s" }),
    ],
    "idle,working",
  );
  assert.deepEqual(lines, [
    "alpha",
    "  claude/a  WaitingInput  9s",
    "  claude/b  Idle  1m0s",
    "beta",
    "  claude/two  Working  5s",
  ]);
});

test("renderText prints a clear line for zero live sessions, never blank", () => {
  const lines = renderText([], "idle,working,waitinginput,unknown");
  assert.equal(lines.length, 1);
  assert.ok(lines[0].includes("no live sessions"), `got: ${lines[0]}`);
  assert.ok(lines[0].trim().length > 0, "an empty glance must not print an empty line");
});

test("toRow keeps current_state verbatim in its PascalCase wire spelling", () => {
  // The trap: the ?state= filter tokens are lowercase, the rendered field is
  // PascalCase. Any re-spelling here silently empties a group.
  const rendered = toRow(
    {
      source: "claude",
      session_id: "s",
      current_state: "WaitingInput",
      last_event_kind: "Notification",
      last_event_at_ms: 0,
      updated_at: 0,
      last_pid: null,
      cwd: null,
      started_at: null,
    },
    1_000_000_000_000,
  );
  assert.equal(rendered.current_state, "WaitingInput");
  assert.equal(rendered.repo, "(unknown repo)");
  assert.equal(rendered.age, "age unknown");
});

// ---------------------------------------------------------------------------
// Flag parsing: the machine-mode contract's front door.
// ---------------------------------------------------------------------------

test("parseArgs defaults to text mode over the four non-Ended states", () => {
  const opts = parseArgs([]);
  assert.deepEqual(opts, {
    count: false,
    format: "text",
    states: "idle,working,waitinginput,unknown",
  });
  // `unknown` is in the default set on purpose: it is the decode-only
  // catch-all for future additive states, and dropping it would make a future
  // daemon's new state vanish from the glance.
  assert.ok(opts.states.split(",").includes("unknown"));
  assert.ok(!opts.states.split(",").includes("ended"));
});

test("parseArgs accepts the documented flags in any order", () => {
  assert.deepEqual(parseArgs(["--format=json", "--state=Working", "--count"]), {
    count: true,
    format: "json",
    states: "working",
  });
});

test("parseArgs normalizes state tokens the way the daemon does", () => {
  assert.equal(parseArgs(["--state= WaitingInput , IDLE "]).states, "waitinginput,idle");
});

test("parseArgs rejects an unknown flag by name, listing the accepted set", () => {
  assert.throws(
    () => parseArgs(["--fromat=json"]),
    (e: Error) => {
      assert.ok(e.message.includes("--fromat=json"), `must name the bad input: ${e.message}`);
      assert.ok(e.message.includes("--count"), `must list the accepted set: ${e.message}`);
      return true;
    },
  );
  // Positive companion: the near-miss above is rejected because the flag is
  // unknown, not because parsing rejects everything.
  assert.equal(parseArgs(["--format=json"]).format, "json");
});

test("parseArgs rejects an invalid state token with the daemon's vocabulary", () => {
  assert.throws(
    () => parseArgs(["--state=running"]),
    (e: Error) => {
      assert.ok(e.message.includes("running"), `must name the bad token: ${e.message}`);
      assert.ok(
        e.message.includes("idle, working, waitinginput, ended, unknown"),
        `must list the accepted set exactly as crates/daemon/src/api/filter.rs does: ${e.message}`,
      );
      return true;
    },
  );
  // A trailing comma yields an empty token: corrupt input, not "no filter".
  // Same call the daemon's parse_state_filter makes.
  assert.throws(() => parseArgs(["--state=working,"]));
  assert.throws(() => parseArgs(["--state="]));
  // Positive companion: every accepted token really is accepted, so the
  // rejections above are about the input, not a blanket refusal.
  assert.equal(
    parseArgs(["--state=idle,working,waitinginput,ended,unknown"]).states,
    "idle,working,waitinginput,ended,unknown",
  );
});
