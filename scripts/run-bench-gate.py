#!/usr/bin/env python3
"""
Best-of-2 bench-gate wrapper (Story 5.18, AC #1-#2).

Runs a bench command and then its gate command, re-measuring exactly once
when (and only when) the gate reports a POLICY failure. Two spurious reds
in one session (2026-07-29/30) came from hosted-runner noise on diffs that
could not have caused them; a deterministic regression fails both attempts,
so best-of-2 masks noise without masking real regressions. The attempt
count is fixed at two by design: a configurable retry count is a knob for
making red builds go away.

Usage:
  run-bench-gate.py --bench <command> --gate <command> --summary <path>

Commands are split with shlex and run WITHOUT a shell. The gate's policy
itself (budgets, ratios) lives in the per-platform baseline files; this
wrapper only decides whether a failure earns one re-measure.

Exit codes:
  0  Gate passed (on the first or the second attempt).
  1  Gate failed on BOTH attempts (policy failure held).
  2  Gate tooling failure (bad args / unreadable input) — propagated
     immediately, never retried.
  N  The bench itself exited N != 0 — propagated immediately, the gate
     never runs. A bench crash is breakage, not noise.

Retry bookkeeping (AC #2: the noise stays countable — a silent retry is
the reflex-rerunning failure mode wearing a costume):
  - Before re-measuring, the attempt-1 summary is copied to a sibling
    `<summary-stem>.attempt1.json` so both JSONs ship in the job artifacts.
  - The retry is announced with a `::warning::` annotation.
  - When attempt 2 passes, both attempts' summary contents are appended to
    `$GITHUB_STEP_SUMMARY` (stdout when unset, e.g. locally).

This wrapper never writes a baseline: a retry changes which run the
current-summary numbers come from, and nothing else.
"""

from __future__ import annotations

import argparse
import shlex
import shutil
import subprocess
import sys
import os
from pathlib import Path


def gh_error(msg: str) -> None:
    print(f"::error::{msg}", file=sys.stderr)


def gh_warning(msg: str) -> None:
    print(f"::warning::{msg}", file=sys.stderr)


def gh_notice(msg: str) -> None:
    print(f"::notice::{msg}")


def run(cmd: list[str], label: str) -> int:
    print(f"run-bench-gate: running {label}: {shlex.join(cmd)}", flush=True)
    return subprocess.run(cmd).returncode


def read_or_placeholder(path: Path) -> str:
    try:
        return path.read_text().strip()
    except OSError as exc:
        return f"(unreadable: {exc})"


def append_step_summary(text: str) -> None:
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as fh:
            fh.write(text)
    else:
        print(text)


def main() -> int:
    parser = argparse.ArgumentParser(description="Best-of-2 bench gate wrapper")
    parser.add_argument(
        "--bench",
        required=True,
        help="Bench command producing the summary JSON (shlex-split, no shell)",
    )
    parser.add_argument(
        "--gate",
        required=True,
        help="Gate command judging the summary JSON (shlex-split, no shell)",
    )
    parser.add_argument(
        "--summary",
        required=True,
        type=Path,
        help="Summary JSON path the bench writes and the gate reads",
    )
    args = parser.parse_args()

    bench_cmd = shlex.split(args.bench)
    gate_cmd = shlex.split(args.gate)

    # Attempt 1: bench. A crash is breakage, not noise — propagate, no gate.
    bench_rc = run(bench_cmd, "bench (attempt 1)")
    if bench_rc != 0:
        gh_error(
            f"bench exited {bench_rc} on attempt 1; propagating without "
            "running the gate (a bench crash is breakage, not noise)."
        )
        return bench_rc

    # Attempt 1: gate.
    gate_rc = run(gate_cmd, "gate (attempt 1)")
    if gate_rc == 0:
        return 0
    if gate_rc == 2:
        gh_error(
            "gate reported a tooling failure (exit 2) on attempt 1; "
            "propagating without a re-measure."
        )
        return 2

    # Policy failure: preserve attempt 1's numbers, then re-measure once.
    attempt1_path = args.summary.with_name(args.summary.stem + ".attempt1.json")
    try:
        shutil.copyfile(args.summary, attempt1_path)
    except OSError as exc:
        gh_warning(f"could not preserve attempt-1 summary: {exc}")
    gh_warning(
        f"gate failed (exit {gate_rc}) on attempt 1; re-measuring once. "
        f"Attempt-1 summary preserved at {attempt1_path}."
    )

    bench_rc = run(bench_cmd, "bench (attempt 2)")
    if bench_rc != 0:
        gh_error(
            f"bench exited {bench_rc} on attempt 2; propagating "
            "(a bench crash is breakage, not noise)."
        )
        return bench_rc

    gate_rc = run(gate_cmd, "gate (attempt 2)")
    if gate_rc == 0:
        gh_notice(
            "gate passed on attempt 2; attempt 1 is counted as runner noise. "
            "Both attempts' numbers are in the step summary."
        )
        append_step_summary(
            "## Bench gate re-measure (best-of-2)\n\n"
            "The gate failed on attempt 1 and passed on attempt 2. Counting "
            "this as runner noise — if these notes pile up, the noise rate "
            "is the finding.\n\n"
            f"Attempt 1 (`{attempt1_path.name}`):\n\n"
            f"```json\n{read_or_placeholder(attempt1_path)}\n```\n\n"
            f"Attempt 2 (`{args.summary.name}`):\n\n"
            f"```json\n{read_or_placeholder(args.summary)}\n```\n"
        )
        return 0
    if gate_rc == 2:
        gh_error(
            "gate reported a tooling failure (exit 2) on attempt 2; propagating."
        )
        return 2

    gh_error(
        f"gate failed on BOTH attempts (exit {gate_rc} on attempt 2). "
        "This is not a single-outlier flake; treat it as a real policy "
        f"failure. Attempt-1 summary: {attempt1_path}."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
