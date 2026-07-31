"""Tests for scripts/run-bench-gate.py (Story 5.18, AC #4).

Drives the wrapper end-to-end with stub bench/gate commands whose exit
sequence is scripted through a counter file, covering the full branch
matrix:

  (a) gate passes first try            -> exit 0, bench ran once
  (b) gate fails then passes           -> exit 0, bench ran twice,
                                          attempt-1 JSON preserved,
                                          step-summary note written
  (c) gate fails twice                 -> exit 1, bench ran twice
  (d) gate exits 2 (tooling)           -> exit 2, bench ran ONCE (no retry)
  (e) bench crashes                    -> its code propagates, gate never ran
  (f) gate exits 2 on attempt 2        -> exit 2

Every case asserts on BOTH the exit code and the number of bench
invocations: case (d) is the one that silently degrades into "retry
everything" if the exit-code branch is wrong, and only the invocation
count catches it (Story 5.16's pass-2 precedent: green tests, live bug).
"""

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

WRAPPER = Path(__file__).resolve().parent.parent / "run-bench-gate.py"

# Stub for both the bench and the gate command. Counts its invocations in a
# counter file, optionally writes a summary JSON recording which attempt
# produced it (bench behavior), and exits with the code scripted for this
# invocation (last code repeats if invoked more often than scripted).
STUB_SOURCE = """\
import sys
from pathlib import Path

counter = Path(sys.argv[1])
count = (int(counter.read_text()) if counter.exists() else 0) + 1
counter.write_text(str(count))

summary = sys.argv[2]
if summary != "-":
    Path(summary).write_text('{"attempt": %d}' % count)

codes = [int(c) for c in sys.argv[3:]]
sys.exit(codes[min(count, len(codes)) - 1])
"""


class RunBenchGateTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

        self.stub = self.tmp / "stub.py"
        self.stub.write_text(STUB_SOURCE)
        self.bench_counter = self.tmp / "bench-count"
        self.gate_counter = self.tmp / "gate-count"
        self.summary = self.tmp / "bench-summary.json"
        self.attempt1 = self.tmp / "bench-summary.attempt1.json"
        self.step_summary = self.tmp / "step-summary.md"

    def bench_cmd(self, *codes):
        codes = codes or (0,)
        return (
            f"{sys.executable} {self.stub} {self.bench_counter} "
            f"{self.summary} " + " ".join(str(c) for c in codes)
        )

    def gate_cmd(self, *codes):
        return (
            f"{sys.executable} {self.stub} {self.gate_counter} - "
            + " ".join(str(c) for c in codes)
        )

    def invocations(self, counter):
        return int(counter.read_text()) if counter.exists() else 0

    def run_wrapper(self, bench, gate):
        env = dict(os.environ)
        env["GITHUB_STEP_SUMMARY"] = str(self.step_summary)
        return subprocess.run(
            [
                sys.executable,
                str(WRAPPER),
                "--bench",
                bench,
                "--gate",
                gate,
                "--summary",
                str(self.summary),
            ],
            env=env,
            capture_output=True,
            text=True,
        )

    def test_a_gate_passes_first_try(self):
        result = self.run_wrapper(self.bench_cmd(), self.gate_cmd(0))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.invocations(self.bench_counter), 1)
        self.assertEqual(self.invocations(self.gate_counter), 1)
        self.assertFalse(self.attempt1.exists())
        self.assertFalse(self.step_summary.exists())

    def test_b_gate_fails_then_passes(self):
        result = self.run_wrapper(self.bench_cmd(), self.gate_cmd(1, 0))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.invocations(self.bench_counter), 2)
        self.assertEqual(self.invocations(self.gate_counter), 2)
        # Attempt-1 summary preserved alongside the final one (AC #1).
        self.assertEqual(self.attempt1.read_text(), '{"attempt": 1}')
        self.assertEqual(self.summary.read_text(), '{"attempt": 2}')
        # Retry is named in the log (AC #2: noise stays countable).
        self.assertIn("::warning::", result.stderr)
        # Step-summary note carries both attempts' summary contents (AC #2).
        note = self.step_summary.read_text()
        self.assertIn('{"attempt": 1}', note)
        self.assertIn('{"attempt": 2}', note)

    def test_c_gate_fails_twice(self):
        result = self.run_wrapper(self.bench_cmd(), self.gate_cmd(1, 1))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.invocations(self.bench_counter), 2)
        self.assertEqual(self.invocations(self.gate_counter), 2)
        self.assertIn("::error::", result.stderr)
        # Attempt-1 summary still preserved for the artifact upload.
        self.assertEqual(self.attempt1.read_text(), '{"attempt": 1}')

    def test_d_gate_tooling_failure_is_not_retried(self):
        result = self.run_wrapper(self.bench_cmd(), self.gate_cmd(2))
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.invocations(self.bench_counter), 1)
        self.assertEqual(self.invocations(self.gate_counter), 1)

    def test_e_bench_crash_propagates_and_gate_never_runs(self):
        result = self.run_wrapper(self.bench_cmd(3), self.gate_cmd(0))
        self.assertEqual(result.returncode, 3)
        self.assertEqual(self.invocations(self.bench_counter), 1)
        self.assertEqual(self.invocations(self.gate_counter), 0)

    def test_f_gate_tooling_failure_on_attempt_2(self):
        result = self.run_wrapper(self.bench_cmd(), self.gate_cmd(1, 2))
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.invocations(self.bench_counter), 2)
        self.assertEqual(self.invocations(self.gate_counter), 2)

    def test_bench_crash_on_attempt_2_propagates(self):
        # Not in the story's lettered list, but the same "breakage, not
        # noise" rule applies on the retry path: a bench that crashes on
        # attempt 2 must propagate its code, not be reported as a gate
        # verdict.
        result = self.run_wrapper(self.bench_cmd(0, 4), self.gate_cmd(1, 0))
        self.assertEqual(result.returncode, 4)
        self.assertEqual(self.invocations(self.bench_counter), 2)
        self.assertEqual(self.invocations(self.gate_counter), 1)


if __name__ == "__main__":
    unittest.main()
