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
  requestTimeoutMs,
  sanitizeHeading,
  sanitizeTextField,
  toRow,
  type GlanceRow,
} from "../src/index.ts";

/** One `GET /sessions` element, with only the interesting field overridden. */
function wire(overrides: Record<string, unknown> = {}): Parameters<typeof toRow>[0] {
  return {
    source: "claude",
    session_id: "s",
    current_state: "Idle",
    last_event_kind: "Stop",
    last_event_at_ms: 0,
    updated_at: 0,
    last_pid: null,
    cwd: null,
    started_at: null,
    ...overrides,
  } as Parameters<typeof toRow>[0];
}

const NOW = 1_700_000_000_000;

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

test("deriveRepo rule 1+5: junk off the wire is a bucket, never a crash", () => {
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

test("deriveRepo rule 3: cwd below the repo root resolves to the repo", () => {
  withTempTree((root) => {
    const repo = join(root, "my-repo");
    mkdirSync(join(repo, ".git"), { recursive: true });
    mkdirSync(join(repo, "crates", "daemon", "src"), { recursive: true });
    assert.equal(deriveRepo(join(repo, "crates", "daemon", "src")), "my-repo");
    // And the repo root itself.
    assert.equal(deriveRepo(repo), "my-repo");
  });
});

test("deriveRepo rule 3: a worktree's `.git` FILE stops the walk", () => {
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

test("deriveRepo rule 4: no `.git` ancestor falls back to basename(cwd)", () => {
  withTempTree((root) => {
    const plain = join(root, "just-a-dir");
    mkdirSync(plain, { recursive: true });
    assert.equal(deriveRepo(plain), "just-a-dir");
  });
});

test("deriveRepo rule 4: a path that does not exist falls back to basename(cwd)", () => {
  // A cwd recorded on another machine, or on a since-deleted directory. Note
  // this is the SAME branch as the test above (no `.git` anywhere up the
  // walk); it is not the unreadable-directory case, which has its own test.
  assert.equal(deriveRepo("/definitely/not/a/real/path/some-project"), "some-project");
  assert.equal(deriveRepo("/"), "/");
});

test("deriveRepo rule 4: an UNREADABLE directory is treated as having no `.git`", (t) => {
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
        // `t.skip()`, not a bare `return`. A bare return reports this test as
        // PASSED, which is a lie in the one environment where it does not run:
        // the reader of a green suite has no way to tell the EACCES branch was
        // never exercised. node:test prints it as `# SKIP` with the reason.
        t.skip("running as root: chmod 000 does not bind, so there is no EACCES to observe");
        return;
      }
      assert.equal(deriveRepo(locked), "outer-repo");
    } finally {
      chmodSync(locked, 0o700);
    }
  });
});

test("deriveRepo rule 2: a RELATIVE cwd never walks the reader's directory tree", () => {
  // The bug this rule closes: `existsSync(join("relative/sub", ".git"))`
  // resolves against THIS process's working directory, not the recorded
  // session's. Run the entry from a repo root and every relative `cwd`
  // collapses to a heading named `.`; run it from elsewhere and the same
  // daemon yields different headings. Two runs of one surface disagreeing is
  // exactly what AC 3 exists to prevent.
  //
  // Proved by running the derivation from two different working directories
  // and asserting they agree, which is the property itself rather than a
  // restatement of the implementation.
  withTempTree((root) => {
    const repo = join(root, "reader-repo");
    mkdirSync(join(repo, "sub", ".git"), { recursive: true });
    const original = process.cwd();
    const seen = new Set<string>();
    try {
      for (const from of [repo, tmpdir()]) {
        process.chdir(from);
        seen.add(deriveRepo("sub/deeper"));
        seen.add(deriveRepo("."));
      }
    } finally {
      process.chdir(original);
    }
    assert.deepEqual(
      [...seen].sort(),
      [".", "deeper"],
      `a relative cwd must derive the same name from every working directory; got ${[...seen]}`,
    );
    // Positive companion: the tree the walk WOULD have found is really there,
    // so the agreement above is the rule firing and not an empty directory.
    assert.equal(deriveRepo(join(repo, "sub", "deeper")), "sub");
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

test("formatAge rejects a non-positive started_at instead of aging from 1969", () => {
  // `-1` and `0` are SAFE INTEGERS, so the isSafeInteger guard passes them and
  // the arithmetic then reports a ~57-year age (`20667d21h`) off a value that
  // means "at or before the epoch". No session started in 1969; non-positive
  // is unusable, not old.
  const now = 1_700_000_000_000;
  for (const nonPositive of [-1, 0, -86_400_000]) {
    const rendered = formatAge(nonPositive, now);
    assert.ok(
      !/^\d{4,}d/.test(rendered),
      `started_at ${nonPositive} must not render a multi-decade age; got: ${rendered}`,
    );
    assert.equal(rendered, "age unknown");
    assert.equal(ageSeconds(nonPositive, now), null);
  }
  // Positive companion: 1ms past the epoch is still a (nonsensical but
  // usable) value, so the rejection is about the sign and not about small
  // numbers.
  assert.equal(ageSeconds(1, 1001), 1);
});

test("formatAge guards nowMs, not just started_at", () => {
  // The null branch exists so the age column never reads `NaN`. It was
  // reachable through the OTHER parameter anyway: `formatAge(1000, NaN)`
  // rendered `NaNdNaNh`. `nowMs` is a caller argument rather than a wire
  // value, which is precisely why nothing else was guarding it.
  for (const badNow of [Number.NaN, Infinity, -Infinity, 1.5, 1e30]) {
    const rendered = formatAge(1000, badNow);
    assert.ok(!rendered.includes("NaN"), `nowMs ${badNow} must not render NaN; got: ${rendered}`);
    assert.equal(rendered, "age unknown");
    assert.equal(ageSeconds(1000, badNow), null);
  }
  // Positive companion: a real `Date.now()`-shaped value still renders.
  assert.equal(formatAge(1_700_000_000_000 - 5000, 1_700_000_000_000), "5s");
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

test("headings cannot be mistaken for session rows", () => {
  // The text format's only structural discriminator is the two-space indent,
  // and `6-tmux-ambient` parses exactly that. `cwd` is verbatim off the wire,
  // and both a newline and a leading space are legal POSIX path components:
  // one would split a heading across two lines and manufacture a phantom
  // repo, the other would make a heading shape-identical to a session row.
  //
  // Driven through `toRow` from a hostile `cwd`, not by handing `renderText` a
  // pre-built row: sanitization lives at row construction now (one
  // representation, not two), so a fixture that skips `toRow` would assert
  // against a shape the entry never produces.
  const lines = renderText(
    [
      toRow(wire({ cwd: "/parent/evil\nrepo", session_id: "a" }), NOW),
      toRow(wire({ cwd: "/parent/  indented", session_id: "b" }), NOW),
    ],
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
  // instead of printing a blank line. ALL whitespace, not just the space: the
  // strip runs BEFORE the flatten precisely so a lone tab does not survive as
  // a one-character U+FFFD heading.
  for (const blank of ["   ", "\t", "\n", " \t\n ", "\u00a0", "\u2028"]) {
    assert.equal(
      sanitizeHeading(blank),
      "(unknown repo)",
      `a whitespace-only name must collapse to the bucket; ${JSON.stringify(blank)} did not`,
    );
  }
});

test("sanitizeTextField flattens every line terminator, not just the ASCII ones", () => {
  // U+0085 NEL is a line terminator to a terminal and is NOT in JavaScript's
  // `\s`, so no whitespace-based guard catches it; U+2028 / U+2029 are line
  // terminators in the ECMAScript grammar itself. All three are one bad `cwd`
  // or `session_id` away from forging a heading.
  for (const terminator of ["\u0085", "\u2028", "\u2029", "\n", "\r", "\u0000", "\u007f"]) {
    const flattened = sanitizeTextField(`a${terminator}b`);
    assert.ok(
      !flattened.includes(terminator),
      `${JSON.stringify(terminator)} must not survive into a text line; got ${JSON.stringify(flattened)}`,
    );
    assert.equal(flattened, "a\uFFFDb");
  }
  // Positive companion: ordinary text, including non-ASCII that is NOT a line
  // terminator, passes through untouched. The flatten is targeted.
  assert.equal(sanitizeTextField("my.repo-2 \u00e9\u4e2d"), "my.repo-2 \u00e9\u4e2d");
});

test("a session row cannot be forged through session_id, source or current_state", () => {
  // The heading was sanitized and the session line was not, so the SAME attack
  // worked one field over: `session_id` is as verbatim off the wire as `cwd`
  // is. A newline in any of the three printed wire fields would end the
  // indented line and start an unindented one, which is a repo heading by
  // definition.
  for (const field of ["session_id", "source", "current_state"] as const) {
    const forgery = "x\nEVIL-REPO\n  claude/forged  Working  0s";
    const lines = renderText([toRow(wire({ [field]: forgery }), NOW)], "idle");
    const headings = lines.filter((l) => !l.startsWith("  "));
    assert.deepEqual(
      headings,
      ["(unknown repo)"],
      `a forged ${field} produced extra headings: ${JSON.stringify(lines)}`,
    );
    for (const line of lines) {
      assert.ok(
        !line.includes("\n"),
        `no line may embed a newline (via ${field}); got ${JSON.stringify(line)}`,
      );
    }
  }
  // Positive companion: ordinary values render exactly as the line format
  // says, so the flatten above is targeted rather than a blanket mangle.
  assert.deepEqual(
    renderText([toRow(wire({ session_id: "sess-alpha", current_state: "WaitingInput" }), NOW)], "idle"),
    ["(unknown repo)", "  claude/sess-alpha  WaitingInput  age unknown"],
  );
});

test("the repo a row is GROUPED by is the repo that gets PRINTED", () => {
  // Sanitizing at print time gave one value two spellings: rows were grouped
  // and sorted on the raw `row.repo` and printed as `sanitizeHeading(repo)`,
  // so `/x/ foo` and `/x/foo` became two groups that both printed `foo` --
  // and the printed order came out `foo, aaa, foo`, contradicting the
  // README's "Repos sort by name". Sanitizing once, in `toRow`, is what makes
  // the group key, the sort key and the printed heading the same string.
  const rows = [
    toRow(wire({ cwd: "/x/ foo", session_id: "a" }), NOW),
    toRow(wire({ cwd: "/x/aaa", session_id: "b" }), NOW),
    toRow(wire({ cwd: "/x/foo", session_id: "c" }), NOW),
  ];
  const lines = renderText(rows, "idle");
  const headings = lines.filter((l) => !l.startsWith("  "));
  assert.deepEqual(
    headings,
    ["aaa", "foo"],
    `one heading per printed name, sorted by it; got ${JSON.stringify(lines)}`,
  );
  // And both sessions really did land in the one `foo` group, which is what
  // makes the heading count above a merge rather than a dropped row.
  assert.deepEqual(lines, [
    "aaa",
    "  claude/b  Idle  age unknown",
    "foo",
    "  claude/a  Idle  age unknown",
    "  claude/c  Idle  age unknown",
  ]);
  // Positive companion: `toRow` really did sanitize, so the merge above is
  // the one-representation property and not two paths that happen to agree.
  assert.equal(rows[0].repo, "foo");
});

test("--format=json carries the sanitized fields and the untouched cwd", () => {
  // One representation, stated as an assertion: the JSON row is the same
  // values text mode prints. `cwd` is the escape hatch, so a machine consumer
  // can always recover the raw path.
  const raw = "/x/ we\nird";
  const row = toRow(wire({ cwd: raw, session_id: "s\nid" }), NOW);
  assert.equal(row.repo, "we\uFFFDird");
  assert.equal(row.session_id, "s\uFFFDid");
  assert.equal(row.cwd, raw, "cwd stays verbatim: it is never printed as text-mode structure");
  assert.equal(row.started_at, null);
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

test("parseArgs answers --help even when another argument is bad", () => {
  // The rationale for answering `--help` at all is that a CLI which replies
  // "unrecognized argument --help" teaches the reader it has no discoverable
  // surface. A single-pass parser did exactly that one argument over: it
  // rejected the typo before it ever reached the request for help, which is
  // the moment a reader most needs the usage text.
  for (const argv of [
    ["--halp", "--help"],
    ["--help", "--halp"],
    ["--state=running", "-h"],
    ["--format=json", "--format=text", "--help"],
  ]) {
    assert.equal(parseArgs(argv).help, true, `${argv.join(" ")} must still answer help`);
  }
  // Positive companion: without the help flag every one of those arguments is
  // still rejected, so `--help` is winning rather than parsing being lax.
  assert.throws(() => parseArgs(["--halp"]));
  assert.throws(() => parseArgs(["--state=running"]));
  assert.throws(() => parseArgs(["--format=json", "--format=text"]));
});

// ---------------------------------------------------------------------------
// The request deadline and its env override.
// ---------------------------------------------------------------------------

test("requestTimeoutMs defaults to 5000 and takes a positive-integer override", () => {
  // Pure and env-injected rather than reading `process.env` directly: mutating
  // process env races every concurrent read in a parallel runner, which is the
  // same rule `clippy.toml` enforces on the Rust side.
  for (const unset of [{}, { BOWERBIRD_GLANCE_TIMEOUT_MS: "" }, { BOWERBIRD_GLANCE_TIMEOUT_MS: "  " }]) {
    assert.equal(requestTimeoutMs(unset), 5000, `unset/blank must be the default; ${JSON.stringify(unset)}`);
  }
  assert.equal(requestTimeoutMs({ BOWERBIRD_GLANCE_TIMEOUT_MS: "1000" }), 1000);
  assert.equal(requestTimeoutMs({ BOWERBIRD_GLANCE_TIMEOUT_MS: " 250 " }), 250);
});

test("requestTimeoutMs rejects a bad override instead of silently reverting", () => {
  // A typo'd `BOWERBIRD_GLANCE_TIMEOUT_MS=1s` that quietly falls back to 5000
  // is "I configured it and it did nothing", which is the failure the flag
  // parser already refuses to produce for arguments.
  for (const bad of ["1s", "0", "-1", "abc", "1.5", "1e30", "Infinity", "NaN"]) {
    assert.throws(
      () => requestTimeoutMs({ BOWERBIRD_GLANCE_TIMEOUT_MS: bad }),
      (e: Error) => {
        assert.ok(e.message.includes(bad), `must name the bad value: ${e.message}`);
        assert.ok(
          e.message.includes("milliseconds"),
          `must say what a good value looks like: ${e.message}`,
        );
        return true;
      },
      `${JSON.stringify(bad)} must be rejected`,
    );
  }
  // Positive companion: the neighbouring good value is accepted, so the
  // rejections are about the values and not a blanket refusal.
  assert.equal(requestTimeoutMs({ BOWERBIRD_GLANCE_TIMEOUT_MS: "1" }), 1);
});
