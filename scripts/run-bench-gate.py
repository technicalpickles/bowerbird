#!/usr/bin/env python3
"""
Best-of-2 bench-gate wrapper (Story 5.18, AC #1-#2).

Runs a bench command and then its gate command, re-measuring exactly once
when (and only when) the gate reports a POLICY failure, which is gate exit
code 1 and nothing else. Two spurious reds in one session (2026-07-29/30)
came from hosted-runner noise on diffs that could not have caused them; a
deterministic regression fails both attempts, so best-of-2 masks noise
without masking real regressions. The attempt count is fixed at two by
design: a configurable retry count is a knob for making red builds go away.

Usage:
  run-bench-gate.py --bench <command> --gate <command> --summary <path>

Commands are split with shlex and run WITHOUT a shell. The gate's policy
itself (budgets, ratios) lives in the per-platform baseline files; this
wrapper only decides whether a failure earns one re-measure.

Exit codes:
  0  Gate passed (on the first or the second attempt).
  1  Gate reported a policy failure (exit 1) on BOTH attempts.
  2  Tooling breakage, never retried: the gate exited 2, the gate exited
     with any code other than 0/1/2 (a crash or signal death is breakage,
     not a policy verdict), a command could not be spawned, or the bench
     claimed success without writing the summary file.
  N  The bench itself exited N != 0. Propagated immediately, the gate
     never runs. A bench crash is breakage, not noise.

Retry bookkeeping (AC #2: the noise stays countable, because a silent
retry is the reflex-rerunning failure mode wearing a costume):
  - Before re-measuring, the attempt-1 summary is copied to a sibling
    `<summary-stem>.attempt1.json` so both JSONs ship in the job artifacts.
  - The retry is announced with a `::warning::` annotation.
  - When attempt 2 passes, both attempts' summary contents are appended to
    `$GITHUB_STEP_SUMMARY` (stdout when unset, e.g. locally). A double
    failure is loud on its own: the red job plus the `::error::` carry it.

Any stale attempt-1 file from a previous run is removed at startup so a
cached `target/` can never ship yesterday's numbers in today's artifacts.

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


class SpawnFailure(Exception):
    """A bench or gate command could not be spawned at all."""


def gh_error(msg: str) -> None:
    print(f"::error::{msg}", file=sys.stderr)


def gh_warning(msg: str) -> None:
    print(f"::warning::{msg}", file=sys.stderr)


def gh_notice(msg: str) -> None:
    print(f"::notice::{msg}")


def run(cmd: list[str], label: str) -> int:
    print(f"run-bench-gate: running {label}: {shlex.join(cmd)}", flush=True)
    try:
        return subprocess.run(cmd).returncode
    except OSError as exc:
        raise SpawnFailure(f"could not spawn {label} ({cmd[0]!r}): {exc}") from exc


def read_or_placeholder(path: Path) -> str:
    try:
        return path.read_text().strip()
    except OSError as exc:
        return f"(unreadable: {exc})"


def append_step_summary(text: str) -> None:
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if not step_summary:
        print(text)
        return
    try:
        with open(step_summary, "a") as fh:
            fh.write(text)
    except OSError as exc:
        # Bookkeeping must not flip a passing run red; the annotation and the
        # job log still carry both attempts' numbers.
        gh_warning(f"could not append to GITHUB_STEP_SUMMARY: {exc}")


def run_bench(bench_cmd: list[str], summary: Path, attempt: int) -> int:
    """Run the bench once. Returns 0 only if it exited 0 AND wrote the
    summary; any other outcome has already been annotated."""
    bench_rc = run(bench_cmd, f"bench (attempt {attempt})")
    if bench_rc != 0:
        gh_error(
            f"bench exited {bench_rc} on attempt {attempt}; propagating "
            "(a bench crash is breakage, not noise)."
        )
        return bench_rc
    if not summary.exists():
        gh_error(
            f"bench exited 0 on attempt {attempt} but wrote no summary at "
            f"{summary}; refusing to gate a phantom run (check that --summary "
            "matches the path the bench writes)."
        )
        return -1  # sentinel: caller maps to tooling exit 2
    return 0


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
    if not bench_cmd or not gate_cmd:
        gh_error("--bench and --gate must be non-empty commands.")
        return 2

    attempt1_path = args.summary.with_name(args.summary.stem + ".attempt1.json")
    attempt1_path.unlink(missing_ok=True)

    # Attempt 1: bench.
    bench_rc = run_bench(bench_cmd, args.summary, attempt=1)
    if bench_rc != 0:
        return 2 if bench_rc == -1 else bench_rc

    # Attempt 1: gate. Only exit 1 is a policy verdict that earns the
    # re-measure (AC #1); 0 passes, and everything else is breakage.
    gate_rc = run(gate_cmd, "gate (attempt 1)")
    if gate_rc == 0:
        return 0
    if gate_rc != 1:
        gh_error(
            f"gate exited {gate_rc} on attempt 1; that is tooling breakage, "
            "not a policy failure, so there is no re-measure."
        )
        return 2

    # Policy failure: preserve attempt 1's numbers, then re-measure once.
    try:
        shutil.copyfile(args.summary, attempt1_path)
    except OSError as exc:
        gh_warning(f"could not preserve attempt-1 summary: {exc}")
    gh_warning(
        f"gate failed (exit {gate_rc}) on attempt 1; re-measuring once. "
        f"Attempt-1 summary preserved at {attempt1_path}."
    )

    bench_rc = run_bench(bench_cmd, args.summary, attempt=2)
    if bench_rc != 0:
        return 2 if bench_rc == -1 else bench_rc

    gate_rc = run(gate_cmd, "gate (attempt 2)")
    if gate_rc == 0:
        gh_notice(
            "gate passed on attempt 2; attempt 1 is counted as runner noise. "
            "Both attempts' numbers are in the step summary."
        )
        append_step_summary(
            "## Bench gate re-measure (best-of-2)\n\n"
            "The gate failed on attempt 1 and passed on attempt 2. Counting "
            "this as runner noise; if these notes pile up, the noise rate "
            "is the finding.\n\n"
            f"Attempt 1 (`{attempt1_path.name}`):\n\n"
            f"```json\n{read_or_placeholder(attempt1_path)}\n```\n\n"
            f"Attempt 2 (`{args.summary.name}`):\n\n"
            f"```json\n{read_or_placeholder(args.summary)}\n```\n"
        )
        return 0
    if gate_rc != 1:
        gh_error(
            f"gate exited {gate_rc} on attempt 2; that is tooling breakage, "
            "not a policy failure. Propagating as exit 2."
        )
        return 2

    gh_error(
        "gate failed on BOTH attempts (exit 1 each time). This is not a "
        "single-outlier flake; treat it as a real policy failure. "
        f"Attempt-1 summary: {attempt1_path}."
    )
    return 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except SpawnFailure as exc:
        gh_error(f"{exc} (tooling breakage, exit 2)")
        sys.exit(2)
