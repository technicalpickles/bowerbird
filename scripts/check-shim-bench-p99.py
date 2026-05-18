#!/usr/bin/env python3
"""
Shim hot-path p99 gate (Story 1.5, AC #1).

Reads the canonical bench summary produced by `cargo bench --bench hot_path`
(see `crates/shim/benches/hot_path.rs`) and enforces two gates:

  1. Absolute: current p99 must be <= 5,000,000 ns (5 ms).
  2. Regression: current p99 must be <= committed_p99 * 1.15.

Usage:
  check-shim-bench-p99.py <current-summary.json> <committed-baseline.json>

Exit codes:
  0  Both gates passed.
  1  At least one gate failed, OR the committed baseline is missing.
  2  Bad arguments / unreadable input (reserved for tooling failure, not policy).

Schema (both files share it; the current summary is what gets committed as
the baseline after a clean run on each platform):
  {"schema_version": 1, "p99_nanos": <int>, "mean_nanos": <int>, "samples": <int>}
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

SCHEMA_VERSION = 1
ABSOLUTE_BUDGET_NS = 5_000_000  # 5 ms per AC #1
REGRESSION_THRESHOLD = 1.15  # +15% over committed baseline fails


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
    if "p99_nanos" not in data:
        raise ValueError(f"{label} ({path}) missing 'p99_nanos'")
    schema = data.get("schema_version")
    if schema != SCHEMA_VERSION:
        raise ValueError(
            f"{label} ({path}) schema_version={schema!r} (expected {SCHEMA_VERSION})"
        )
    return data


def main() -> int:
    parser = argparse.ArgumentParser(description="Shim bench p99 gate")
    parser.add_argument(
        "current_json",
        type=Path,
        help="Current-run summary JSON (target/shim-bench-summary.json)",
    )
    parser.add_argument(
        "baseline_json",
        type=Path,
        help="Committed baseline JSON (crates/shim/benches/baselines/<platform>.json)",
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

    p99_nanos = int(current["p99_nanos"])
    mean_nanos = int(current["mean_nanos"])
    samples = int(current.get("samples", 0))
    print(
        f"Current run: p99={fmt_ms(p99_nanos)} mean={fmt_ms(mean_nanos)} samples={samples}"
    )

    failed = False

    # Absolute gate — current p99 must respect the AC #1 budget regardless of
    # whether a baseline exists.
    if p99_nanos > ABSOLUTE_BUDGET_NS:
        gh_error(
            f"AC #1 absolute gate FAILED: current p99 {fmt_ms(p99_nanos)} > 5ms budget. "
            "If this is intentional, file an ADR per PRD line 181 before merging."
        )
        failed = True
    else:
        gh_notice(
            f"AC #1 absolute gate OK: current p99 {fmt_ms(p99_nanos)} <= 5ms budget."
        )

    # Regression gate — requires a committed baseline. Missing baseline is a
    # hard fail (the gate is unarmed otherwise). To seed: download the
    # shim-bench-<runner-os> artifact from this run, copy
    # target/shim-bench-summary.json into baselines/<platform>.json, commit.
    if not args.baseline_json.exists():
        gh_error(
            f"No committed baseline at {args.baseline_json}. The regression gate "
            "is unarmed without it. Download the shim-bench-<runner-os> artifact "
            "from this run, copy target/shim-bench-summary.json into "
            f"{args.baseline_json}, and commit. See crates/shim/benches/README.md."
        )
        return 1

    try:
        baseline = load_summary(args.baseline_json, "committed baseline")
    except Exception as exc:
        gh_error(f"failed to read committed baseline: {exc}")
        return 2

    baseline_p99 = int(baseline["p99_nanos"])
    threshold = baseline_p99 * REGRESSION_THRESHOLD
    delta_pct = (p99_nanos / baseline_p99 - 1.0) * 100.0 if baseline_p99 > 0 else 0.0
    print(
        f"Baseline: p99={fmt_ms(baseline_p99)}; current vs baseline delta: {delta_pct:+.2f}%"
    )
    if p99_nanos > threshold:
        gh_error(
            f"p99 regression gate FAILED: current p99 {fmt_ms(p99_nanos)} > "
            f"baseline {fmt_ms(baseline_p99)} * 1.15 ({fmt_ms(threshold)})."
        )
        failed = True
    else:
        gh_notice(
            f"p99 regression gate OK: current p99 {fmt_ms(p99_nanos)} within "
            f"+15% of baseline {fmt_ms(baseline_p99)}."
        )

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
