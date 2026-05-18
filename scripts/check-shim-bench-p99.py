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
DEFAULT_ABSOLUTE_BUDGET_NS = 5_000_000  # AC #1 default per platform; overridable per-baseline
DEFAULT_REGRESSION_THRESHOLD = 1.15  # +15% over committed baseline fails by default


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

    # The regression gate needs a baseline; the absolute gate needs the
    # per-platform budget that lives in the baseline. Load the baseline
    # first so both gates use consistent policy.
    if not args.baseline_json.exists():
        gh_error(
            f"No committed baseline at {args.baseline_json}. Both gates are "
            "unarmed without it. Download the shim-bench-<runner-os> artifact "
            "from this run, copy target/shim-bench-summary.json into "
            f"{args.baseline_json}, and commit. See crates/shim/benches/README.md."
        )
        return 1

    try:
        baseline = load_summary(args.baseline_json, "committed baseline")
    except Exception as exc:
        gh_error(f"failed to read committed baseline: {exc}")
        return 2

    # Per-platform policy lives in the baseline file (ADR 0003). Falling back
    # to the AC #1 defaults preserves behavior for baselines committed before
    # the policy fields existed.
    abs_budget_ns = int(baseline.get("absolute_budget_nanos") or DEFAULT_ABSOLUTE_BUDGET_NS)
    regression_max = baseline.get("regression_max_ratio", DEFAULT_REGRESSION_THRESHOLD)

    # Absolute gate
    if p99_nanos > abs_budget_ns:
        gh_error(
            f"absolute gate FAILED: current p99 {fmt_ms(p99_nanos)} > "
            f"per-platform budget {fmt_ms(abs_budget_ns)}. "
            "If this is intentional, update the baseline and the related ADR "
            "before merging."
        )
        failed = True
    else:
        gh_notice(
            f"absolute gate OK: current p99 {fmt_ms(p99_nanos)} <= "
            f"{fmt_ms(abs_budget_ns)} budget."
        )

    # Regression gate — skipped when the baseline explicitly opts out via
    # `regression_max_ratio: null` (ADR 0003: macos-latest noise floor is
    # wider than any meaningful percentage gate).
    baseline_p99 = int(baseline["p99_nanos"])
    delta_pct = (p99_nanos / baseline_p99 - 1.0) * 100.0 if baseline_p99 > 0 else 0.0
    print(
        f"Baseline: p99={fmt_ms(baseline_p99)}; current vs baseline delta: {delta_pct:+.2f}%"
    )

    if regression_max is None:
        gh_notice(
            "regression gate disabled by baseline policy (regression_max_ratio: null). "
            "See ADR 0003 for the per-platform rationale."
        )
    else:
        regression_max_f = float(regression_max)
        threshold = baseline_p99 * regression_max_f
        if p99_nanos > threshold:
            gh_error(
                f"p99 regression gate FAILED: current p99 {fmt_ms(p99_nanos)} > "
                f"baseline {fmt_ms(baseline_p99)} * {regression_max_f:.2f} "
                f"({fmt_ms(threshold)})."
            )
            failed = True
        else:
            gh_notice(
                f"p99 regression gate OK: current p99 {fmt_ms(p99_nanos)} within "
                f"{(regression_max_f - 1) * 100:+.0f}% of baseline {fmt_ms(baseline_p99)}."
            )

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
