#!/usr/bin/env python3
"""Unit tests for tools/bench_report.py partial/timeout reporting (issue #3021)."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS_DIR))
import bench_report  # noqa: E402


def _make_row(rel: str, py_ms: float = 10.0, rs_ms: float = 5.0, base_ms: "float | None" = None):
    cfg = bench_report.ScriptConfig(iterations=20, warmup=3, trim=10.0)
    return bench_report.ScriptResult(
        rel=rel,
        cfg=cfg,
        py_times=[py_ms / 1000.0] * 5,
        rs_times=[rs_ms / 1000.0] * 5,
        base_times=[base_ms / 1000.0] * 5 if base_ms is not None else [],
    )


def _write_hyperfine_v1(path: Path, rel: str, py_s: float = 0.01, rs_s: float = 0.005, base_s: "float | None" = None):
    results = [
        {"command": f"python:{rel}", "mean": py_s, "times": [py_s, py_s, py_s]},
        {"command": f"pyrust:{rel}", "mean": rs_s, "times": [rs_s, rs_s, rs_s]},
    ]
    if base_s is not None:
        results.append({"command": f"base:{rel}", "mean": base_s, "times": [base_s, base_s, base_s]})
    path.write_text(json.dumps({"results": results}), encoding="utf-8")


class BenchReportTimeoutTests(unittest.TestCase):
    def test_successful_run_has_no_warning_or_timeout_rows(self):
        rows = [_make_row("performance/test_fast.py", py_ms=10.0, rs_ms=5.0, base_ms=5.5)]
        md = bench_report._summary_md(rows, top=15)
        self.assertNotIn("[!WARNING]", md)
        self.assertNotIn("Partial benchmark run", md)
        self.assertNotIn("timed out", md)
        self.assertIn("| Scripts       | 1 |", md)
        self.assertIn("`performance/test_fast.py`", md)

    def test_partial_run_marks_partial_and_names_timed_out_cases(self):
        cfg = bench_report.BenchConfig(
            default=bench_report.ScriptConfig(iterations=20, warmup=3, trim=10.0),
            overrides={"performance/test_hang.py": bench_report.ScriptConfig(iterations=7, warmup=1, trim=10.0)},
        )
        rows = [_make_row("performance/test_fast.py", py_ms=10.0, rs_ms=5.0, base_ms=5.5)]
        md = bench_report._summary_md(
            rows,
            top=15,
            timed_out=["performance/test_hang.py", "performance/test_hang.py", "performance/test_slow.py"],
            timeout_s=300,
            config=cfg,
        )
        self.assertIn("> [!WARNING]", md)
        self.assertIn("**Partial benchmark run**: 2 case(s) exceeded the 300s timeout cap", md)
        self.assertIn("Timed out: `performance/test_hang.py`, `performance/test_slow.py`", md)
        self.assertIn("| Scripts       | 1 completed (2 timed out) |", md)
        # Explicit timed-out rows appear in both PR vs Base and Speed tables.
        self.assertEqual(md.count("| `performance/test_hang.py` | 7 | — | — | ⏱ timed out (>300s) |"), 2)
        self.assertEqual(md.count("| `performance/test_slow.py` | 20 | — | — | ⏱ timed out (>300s) |"), 2)

    def test_all_cases_timed_out_still_writes_comment_and_full_report(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            results_dir = tmp_path / "results"
            results_dir.mkdir()
            comment_out = tmp_path / "comment.md"
            full_out = tmp_path / "full.md"
            proc = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS_DIR / "bench_report.py"),
                    "--results-dir",
                    str(results_dir),
                    "--pr-comment-out",
                    str(comment_out),
                    "--pr-full-out",
                    str(full_out),
                    "--timeout-s",
                    "15",
                    "--timed-out",
                    "performance/test_hang.py",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            comment = comment_out.read_text(encoding="utf-8")
            full = full_out.read_text(encoding="utf-8")
            self.assertEqual(comment, full)
            self.assertIn("**Partial benchmark run**: 1 case(s) exceeded the 15s timeout cap", comment)
            self.assertIn("### ⏱ Timed-out Cases", comment)
            self.assertIn("`performance/test_hang.py`", comment)
            self.assertIn("⏱ timed out (>15s)", comment)

    def test_partial_cli_invocation_writes_reports_with_completed_and_timed_out(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            results_dir = tmp_path / "results"
            results_dir.mkdir()
            _write_hyperfine_v1(results_dir / "performance__test_ok.json", "performance/test_ok.py", base_s=0.006)
            comment_out = tmp_path / "comment.md"
            full_out = tmp_path / "full.md"
            proc = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS_DIR / "bench_report.py"),
                    "--results-dir",
                    str(results_dir),
                    "--pr-comment-out",
                    str(comment_out),
                    "--pr-full-out",
                    str(full_out),
                    "--timeout-s",
                    "30",
                    "--timed-out",
                    "performance/test_hang.py",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            comment = comment_out.read_text(encoding="utf-8")
            self.assertIn("**Partial benchmark run**", comment)
            self.assertIn("`performance/test_ok.py`", comment)
            self.assertIn("`performance/test_hang.py`", comment)
            self.assertIn("⏱ timed out (>30s)", comment)

    def test_empty_results_without_timeouts_still_exits_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            results_dir = Path(tmp) / "results"
            results_dir.mkdir()
            proc = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS_DIR / "bench_report.py"),
                    "--results-dir",
                    str(results_dir),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 2)


if __name__ == "__main__":
    unittest.main()