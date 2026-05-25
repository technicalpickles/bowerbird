#!/usr/bin/env python3
"""
Hook→presenter daemon-bench p99 gate (Story 4.4 / AC #7).

Reads the bench summary produced by `cargo bench -p bowerbird-daemon --bench
hook_to_presenter` (see `crates/daemon/benches/hook_to_presenter.rs`) and
enforces two gates per shape (solo, fanout3, burst, steady):

  1. Absolute: current p99 must be <= absolute_budget_nanos (NFR2: 100ms).
  2. Regression: current p99 must be <= committed_p99 * regression_max_ratio.

Mirrors `scripts/check-shim-bench-p99.py` (same CLI shape, same JSON schema
floor, same per-platform policy in the baseline file) so reviewers don't have
to learn a second gate idiom.

Usage:
  check-daemon-bench-p99.py <current-summary.json> <committed-baseline.json>

Exit codes:
  0  All four shapes pass both gates.
  1  At least one shape failed, OR the committed baseline is missing.
  2  Bad arguments / unreadable input (tooling failure, not policy).

Schema (current summary is what gets committed as the baseline after a clean
run on each platform — baselines add per-platform policy fields):
  {
    "schema_version": 1,
    "solo_p99_nanos":    <int>,
    "fanout3_p99_nanos": <int>,
    "burst_p99_nanos":   <int>,
    "steady_p99_nanos":  <int>,
    "samples":           <int>,
    "absolute_budget_nanos":   <int>,   // baselines only
    "regression_max_ratio":    <float>  // baselines only
  }
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

SCHEMA_VERSION = 1
DEFAULT_ABSOLUTE_BUDGET_NS = 100_000_000  # NFR2: hook→presenter ≤100ms
DEFAULT_REGRESSION_THRESHOLD = 1.30  # +30% over committed baseline (looser than shim's 1.15 per Axiom 3)

SHAPES = ("solo", "fanout3", "burst", "steady")


def gh_error(msg: str) -> None:
    print(f"::error::{msg}", file=sys.stderr)


def gh_notice(msg: str) -> None:
    print(f"::notice::{msg}")


def fmt_ms(nanos: float) -> str:
    return f"{nanos / 1_000_000:.3f}ms"


def load_summary(path: Path, label: str) -> dict:
    if not path.exists():
        raise FileNotFoundError(f"{label} not found at {path}")
    with path.open() as fh:
        data = json.load(fh)
    schema = data.get("schema_version")
    if schema != SCHEMA_VERSION:
        raise ValueError(
            f"{label} ({path}) schema_version={schema!r} (expected {SCHEMA_VERSION})"
        )
    for shape in SHAPES:
        key = f"{shape}_p99_nanos"
        if key not in data:
            raise ValueError(f"{label} ({path}) missing '{key}'")
    return data


def main() -> int:
    parser = argparse.ArgumentParser(description="Daemon hook→presenter bench p99 gate")
    parser.add_argument(
        "current_json",
        type=Path,
        help="Current-run summary JSON (target/daemon-bench-summary.json)",
    )
    parser.add_argument(
        "baseline_json",
        type=Path,
        help="Committed baseline JSON (crates/daemon/benches/baselines/<platform>.json)",
    )
    args = parser.parse_args()

    try:
        current = load_summary(args.current_json, "current summary")
    except FileNotFoundError as exc:
        gh_error(str(exc))
        return 2
    except Exception as exc:
        gh_error(f"failed to read current summary: {exc}")
        return 2

    samples = int(current.get("samples", 0))
    print("Current run:")
    for shape in SHAPES:
        p99 = int(current[f"{shape}_p99_nanos"])
        print(f"  {shape}: p99={fmt_ms(p99)}")
    print(f"  samples={samples}")

    if not args.baseline_json.exists():
        gh_error(
            f"No committed baseline at {args.baseline_json}. Both gates are "
            "unarmed without it. Download the daemon-bench-<runner-os> artifact "
            "from this run, copy target/daemon-bench-summary.json into "
            f"{args.baseline_json}, and commit. See crates/daemon/benches/README.md."
        )
        return 1

    try:
        baseline = load_summary(args.baseline_json, "committed baseline")
    except Exception as exc:
        gh_error(f"failed to read committed baseline: {exc}")
        return 2

    abs_budget_ns = int(baseline.get("absolute_budget_nanos") or DEFAULT_ABSOLUTE_BUDGET_NS)
    regression_max = baseline.get("regression_max_ratio", DEFAULT_REGRESSION_THRESHOLD)

    failed = False
    for shape in SHAPES:
        key = f"{shape}_p99_nanos"
        current_p99 = int(current[key])
        baseline_p99 = int(baseline[key])

        # Absolute gate
        if current_p99 > abs_budget_ns:
            gh_error(
                f"{shape}: absolute gate FAILED: current p99 {fmt_ms(current_p99)} > "
                f"per-platform budget {fmt_ms(abs_budget_ns)}. NFR2 is hook→presenter ≤100ms."
            )
            failed = True
        else:
            gh_notice(
                f"{shape}: absolute gate OK: current p99 {fmt_ms(current_p99)} <= "
                f"{fmt_ms(abs_budget_ns)} budget."
            )

        # Regression gate
        if regression_max is None:
            gh_notice(
                f"{shape}: regression gate disabled by baseline policy (regression_max_ratio: null)."
            )
            continue

        regression_max_f = float(regression_max)
        if baseline_p99 <= 0:
            gh_notice(
                f"{shape}: regression gate skipped — baseline p99 is zero (uninitialized?)."
            )
            continue
        threshold = baseline_p99 * regression_max_f
        delta_pct = (current_p99 / baseline_p99 - 1.0) * 100.0
        if current_p99 > threshold:
            gh_error(
                f"{shape}: p99 regression gate FAILED: current p99 {fmt_ms(current_p99)} > "
                f"baseline {fmt_ms(baseline_p99)} * {regression_max_f:.2f} "
                f"({fmt_ms(threshold)}); delta {delta_pct:+.1f}%."
            )
            failed = True
        else:
            gh_notice(
                f"{shape}: p99 regression gate OK: current p99 {fmt_ms(current_p99)} within "
                f"{(regression_max_f - 1) * 100:+.0f}% of baseline {fmt_ms(baseline_p99)} "
                f"(delta {delta_pct:+.1f}%)."
            )

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
