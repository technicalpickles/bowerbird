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
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, chmodSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  deriveRepo,
  formatAge,
  ageSeconds,
  parseArgs,
  renderText,
  sanitizeHeading,
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

test("deriveRepo rule 1+4: junk off the wire is a bucket, never a crash", () => {
  // `cwd` reaches deriveRepo through an unchecked `as SessionListItem[]` cast
  // of the response body, so its declared `string | null` is a claim about the
  // daemon, not a guarantee about the value. `undefined` in particular is the
  // one a `=== null` check misses, and a throw here takes down the WHOLE run
  // (every session, not one row) because the map is not per-item guarded.
  for (const junk of [undefined, 42, [], {}, true] as unknown[]) {
    assert.equal(
      deriveRepo(junk as string | null),
      "(unknown repo)",
      `deriveRepo(${JSON.stringify(junk) ?? "undefined"}) must bucket, not throw or leak`,
    );
  }
  // Positive companion: a real string still derives normally, so the bucketing
  // above is about the junk and not a blanket fallback.
  assert.equal(deriveRepo("/tmp/some-project"), "some-project");
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

test("deriveRepo rule 3: a path that does not exist falls back to basename(cwd)", () => {
  // A cwd recorded on another machine, or on a since-deleted directory. Note
  // this is the SAME branch as the test above (no `.git` anywhere up the
  // walk); it is not the unreadable-directory case, which has its own test.
  assert.equal(deriveRepo("/definitely/not/a/real/path/some-project"), "some-project");
  assert.equal(deriveRepo("/"), "/");
});

test("deriveRepo rule 3: an UNREADABLE directory is treated as having no `.git`", () => {
  // The documented behavior, verified rather than assumed: `existsSync`
  // returns false on EACCES instead of throwing, so the walk does not stop at
  // an unreadable directory -- it keeps going up and finds the enclosing repo.
  // (An earlier draft of the rule text claimed the walk aborted to
  // `basename(cwd)` here. It never did.)
  withTempTree((root) => {
    const repo = join(root, "outer-repo");
    mkdirSync(join(repo, ".git"), { recursive: true });
    const locked = join(repo, "locked");
    mkdirSync(locked, { recursive: true });
    chmodSync(locked, 0o000);
    try {
      // Precondition: the directory really is unreadable FOR THIS PROCESS.
      // Running as root (some CI containers) defeats the mode bits entirely,
      // and asserting EACCES semantics there would assert a falsehood.
      let unreadable = true;
      try {
        readdirSync(locked);
        unreadable = false;
      } catch {
        // expected
      }
      if (!unreadable) {
        return;
      }
      assert.equal(deriveRepo(locked), "outer-repo");
    } finally {
      chmodSync(locked, 0o700);
    }
  });
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
  // The two negatives come FIRST on purpose. Behind an `assert.equal(rendered,
  // "age unknown")` they can never be the assertion that fires: any break that
  // makes the output contain `NaN` or `1970` also breaks the equality, so the
  // negatives would be permanently unobservable-red (A13). In this order a
  // regression in the placeholder is caught BY them.
  assert.ok(!rendered.includes("NaN"), `must not render NaN; got: ${rendered}`);
  assert.ok(!rendered.includes("1970"), `must not render a 1970 timestamp; got: ${rendered}`);
  assert.equal(rendered, "age unknown");
  assert.equal(ageSeconds(null, 1_000_000_000_000), null);
});

test("formatAge clamps a future started_at instead of going negative", () => {
  const now = 1_000_000_000_000;
  assert.equal(formatAge(now + 60_000, now), "0s");
  assert.equal(ageSeconds(now + 60_000, now), 0);
});

test("formatAge keeps the two-unit shape for absurd started_at values", () => {
  // A finite-but-insane `started_at` is not a hypothetical: it is one bad
  // wire value away. `-1e30` passes a `Number.isFinite` guard, and the day
  // count then formats in SCIENTIFIC NOTATION (`1e+22d`), which escapes the
  // documented two-unit shape a consumer parses.
  const now = 1_000_000_000_000;
  for (const absurd of [-1e30, 1e30, Number.MAX_VALUE, 1.5, Number.NaN, Infinity]) {
    const rendered = formatAge(absurd, now);
    assert.ok(
      !rendered.includes("e+"),
      `started_at ${absurd} must not render in scientific notation; got: ${rendered}`,
    );
    assert.equal(rendered, "age unknown");
    assert.equal(ageSeconds(absurd, now), null);
  }
  // Positive companion: a real epoch-ms value at the same scale of elapsed
  // time still renders normally, so the rejections above are about the value
  // being unusable, not about large ages.
  assert.equal(formatAge(now - 400 * 24 * 3600 * 1000, now), "400d0h");
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

test("renderText headings cannot be mistaken for session rows", () => {
  // The text format's only structural discriminator is the two-space indent,
  // and `6-tmux-ambient` parses exactly that. `cwd` is verbatim off the wire,
  // and both a newline and a leading space are legal POSIX path components:
  // one would split a heading across two lines and manufacture a phantom
  // repo, the other would make a heading shape-identical to a session row.
  const lines = renderText(
    [row({ repo: "evil\nrepo", session_id: "a" }), row({ repo: "  indented", session_id: "b" })],
    "idle",
  );
  const headings = lines.filter((l) => !l.startsWith("  "));
  assert.equal(headings.length, 2, `exactly two headings; got ${JSON.stringify(lines)}`);
  for (const line of lines) {
    assert.ok(!line.includes("\n"), `no line may embed a newline; got ${JSON.stringify(line)}`);
  }
  // Positive companion: an ordinary name passes through untouched, so the
  // rewriting above is targeted rather than a blanket mangle.
  assert.equal(sanitizeHeading("bowerbird"), "bowerbird");
  assert.equal(sanitizeHeading("my.repo-2"), "my.repo-2");
  // A name that is nothing but whitespace collapses to the named bucket
  // instead of printing a blank line.
  assert.equal(sanitizeHeading("   "), "(unknown repo)");
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
  assert.deepEqual(parseArgs([]), {
    count: false,
    format: "text",
    states: "idle,working,waitinginput,unknown",
    help: false,
  });
});

// Split out of the deepEqual above deliberately. Behind it, neither assertion
// below could ever be the one that fires: any break to DEFAULT_STATES breaks
// the deepEqual first, leaving both negatives unobservable-red (A13). On their
// own they are the failure a widened default set produces.
test("the default state set carries `unknown` and excludes `ended`", () => {
  const states = parseArgs([]).states.split(",");
  // `unknown` is the decode-only catch-all for future additive states;
  // dropping it would make a future daemon's new state vanish from the glance.
  assert.ok(states.includes("unknown"), `default set must carry unknown; got ${states}`);
  // `ended` in the default set would put finished sessions in an attention
  // surface whose whole premise is "what is live right now".
  assert.ok(!states.includes("ended"), `default set must exclude ended; got ${states}`);
});

test("parseArgs accepts the documented flags in any order", () => {
  assert.deepEqual(parseArgs(["--format=json", "--state=Working", "--count"]), {
    count: true,
    format: "json",
    states: "working",
    help: false,
  });
  // Same flags, reversed. "Order-independent" is a claim the test makes, not
  // one the doc comment makes alone.
  assert.deepEqual(parseArgs(["--count", "--state=Working", "--format=json"]), {
    count: true,
    format: "json",
    states: "working",
    help: false,
  });
});

test("parseArgs rejects a repeated --format/--state instead of resolving last-wins", () => {
  // Last-wins IS order dependence, which is the one property the doc comment
  // above parseArgs promises the flags do not have.
  for (const argv of [
    ["--format=json", "--format=text"],
    ["--format=text", "--format=json"],
    ["--state=idle", "--state=working"],
    ["--state=idle", "--state=idle"],
  ]) {
    assert.throws(
      () => parseArgs(argv),
      (e: Error) => {
        assert.ok(e.message.includes("twice"), `must say what went wrong: ${e.message}`);
        assert.ok(e.message.includes("order"), `must say why it matters: ${e.message}`);
        return true;
      },
      `${argv.join(" ")} must be rejected`,
    );
  }
  // Positive companion: repeating `--count` is fine (a boolean has no second
  // value to disagree with), so the rejections above are about conflicting
  // VALUES, not about repetition itself.
  assert.equal(parseArgs(["--count", "--count"]).count, true);
});

test("parseArgs answers --help and -h instead of calling them unrecognized", () => {
  for (const flag of ["--help", "-h"]) {
    assert.equal(parseArgs([flag]).help, true, `${flag} must set help`);
  }
  // Positive companion: `--help` is recognized specifically, not by a blanket
  // acceptance of anything starting with `-`.
  assert.throws(() => parseArgs(["--halp"]));
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
