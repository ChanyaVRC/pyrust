#!/usr/bin/env python3
import argparse
import html
import json
import math
import os
import platform
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path


def find_pyrust_binary(root: Path) -> Path:
    override = os.environ.get("PYRUST_BIN")
    if override:
        return Path(override)

    if platform.system().lower().startswith("win"):
        return root / "target" / "debug" / "pyrust.exe"
    return root / "target" / "debug" / "pyrust"


def collect_scripts(root: Path) -> list[Path]:
    cases = root / "tests" / "cases"
    scripts = sorted(cases.rglob("test_*.py"))
    if not scripts:
        raise RuntimeError("no test_*.py found under tests/cases")
    return scripts


def run_once(program: Path, script: Path) -> float:
    start = time.perf_counter()
    completed = subprocess.run(
        [str(program), str(script)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    elapsed = time.perf_counter() - start
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed: {program} {script}\n"
            f"exit={completed.returncode}\n"
            f"stdout:\n{completed.stdout}\n"
            f"stderr:\n{completed.stderr}"
        )
    return elapsed


@dataclass
class ScriptStats:
    path: str
    category: str
    py_samples: list[float]
    rs_samples: list[float]
    base_samples: list[float] = field(default_factory=list)

    @property
    def has_base(self) -> bool:
        return bool(self.base_samples)

    @property
    def py_avg_ms(self) -> float:
        return statistics.fmean(self.py_samples) * 1000.0

    @property
    def rs_avg_ms(self) -> float:
        return statistics.fmean(self.rs_samples) * 1000.0

    @property
    def base_avg_ms(self) -> float:
        return statistics.fmean(self.base_samples) * 1000.0

    @property
    def py_min_ms(self) -> float:
        return min(self.py_samples) * 1000.0

    @property
    def rs_min_ms(self) -> float:
        return min(self.rs_samples) * 1000.0

    @property
    def py_max_ms(self) -> float:
        return max(self.py_samples) * 1000.0

    @property
    def rs_max_ms(self) -> float:
        return max(self.rs_samples) * 1000.0

    @property
    def py_std_ms(self) -> float:
        if len(self.py_samples) < 2:
            return 0.0
        return statistics.stdev(self.py_samples) * 1000.0

    @property
    def rs_std_ms(self) -> float:
        if len(self.rs_samples) < 2:
            return 0.0
        return statistics.stdev(self.rs_samples) * 1000.0

    @property
    def ratio(self) -> float:
        py_ms = self.py_avg_ms
        if py_ms <= 0:
            return math.inf
        return self.rs_avg_ms / py_ms

    @property
    def diff_ms(self) -> float:
        return self.rs_avg_ms - self.py_avg_ms

    @property
    def diff_pct(self) -> float:
        if self.py_avg_ms <= 0:
            return math.inf
        return (self.ratio - 1.0) * 100.0

    @property
    def pr_change_ms(self) -> float:
        return self.rs_avg_ms - self.base_avg_ms

    @property
    def pr_change_pct(self) -> float:
        base = self.base_avg_ms
        if base <= 0:
            return math.inf
        return (self.rs_avg_ms / base - 1.0) * 100.0


def get_category(root: Path, script: Path) -> str:
    rel = script.relative_to(root).parts
    if len(rel) >= 3:
        return rel[2]
    return "unknown"


def benchmark(
    root: Path,
    python_bin: Path,
    pyrust_bin: Path,
    scripts: list[Path],
    iterations: int,
    warmup: bool,
    base_bin: "Path | None" = None,
) -> list[ScriptStats]:
    stats: dict[Path, ScriptStats] = {}

    for script in scripts:
        rel = script.relative_to(root).as_posix()
        stats[script] = ScriptStats(
            path=rel,
            category=get_category(root, script),
            py_samples=[],
            rs_samples=[],
        )

    if warmup:
        for script in scripts:
            run_once(python_bin, script)
            run_once(pyrust_bin, script)
            if base_bin:
                run_once(base_bin, script)

    for _ in range(iterations):
        for script in scripts:
            stats[script].py_samples.append(run_once(python_bin, script))
            stats[script].rs_samples.append(run_once(pyrust_bin, script))
            if base_bin:
                stats[script].base_samples.append(run_once(base_bin, script))

    rows = [stats[script] for script in scripts]
    rows.sort(key=lambda row: row.rs_avg_ms, reverse=True)
    return rows


def print_environment(
    python_bin: Path,
    pyrust_bin: Path,
    base_bin: "Path | None",
    rows: list[ScriptStats],
    iterations: int,
    warmup: bool,
):
    print("Speed Comparison: Python vs PyRust")
    print(f"Python executable: {python_bin}")
    print(f"Python version:    {platform.python_version()}")
    print(f"PyRust binary:     {pyrust_bin}")
    if base_bin:
        print(f"Base binary:       {base_bin}")
    print(f"Platform:          {platform.platform()}")
    print(f"CPU count:         {os.cpu_count()}")
    print(f"Scripts:           {len(rows)}")
    print(f"Iterations:        {iterations}")
    print(f"Warmup:            {'enabled' if warmup else 'disabled'}")
    print("")


def print_overall_summary(rows: list[ScriptStats]) -> None:
    total_py = sum(row.py_avg_ms for row in rows)
    total_rs = sum(row.rs_avg_ms for row in rows)
    overall_ratio = (total_rs / total_py) if total_py > 0 else math.inf
    overall_diff = total_rs - total_py

    ratios = [row.ratio for row in rows if math.isfinite(row.ratio)]
    median_ratio = statistics.median(ratios) if ratios else math.inf

    print("Overall Summary")
    print(f"  Python total avg: {total_py:.3f} ms")
    print(f"  PyRust total avg: {total_rs:.3f} ms")
    print(f"  Absolute diff:    {overall_diff:+.3f} ms")
    print(f"  Overall ratio:    {overall_ratio:.2f}x (PyRust/Python)")
    print(f"  Median ratio:     {median_ratio:.2f}x")
    print("")


def print_per_script(rows: list[ScriptStats], top: int) -> None:
    print(f"Top {min(top, len(rows))} by PyRust avg time")
    print(
        f"{'script':60} {'py_avg':>10} {'rs_avg':>10} {'ratio':>8} {'diff_ms':>10} {'diff_%':>9}"
    )
    print("-" * 118)
    for row in rows[:top]:
        print(
            f"{row.path[:60]:60} "
            f"{row.py_avg_ms:10.3f} {row.rs_avg_ms:10.3f} "
            f"{row.ratio:8.2f}x {row.diff_ms:+10.3f} {row.diff_pct:+8.2f}%"
        )
    print("")


def print_variability(rows: list[ScriptStats], top: int) -> None:
    ranked = sorted(rows, key=lambda row: row.rs_std_ms, reverse=True)
    print(f"Top {min(top, len(ranked))} PyRust variability (stddev)")
    print(f"{'script':60} {'rs_std_ms':>12} {'rs_min_ms':>12} {'rs_max_ms':>12}")
    print("-" * 102)
    for row in ranked[:top]:
        print(
            f"{row.path[:60]:60} {row.rs_std_ms:12.3f} {row.rs_min_ms:12.3f} {row.rs_max_ms:12.3f}"
        )
    print("")


def print_winners(rows: list[ScriptStats], top: int) -> None:
    regressions = sorted(rows, key=lambda row: row.ratio, reverse=True)
    improvements = sorted(rows, key=lambda row: row.ratio)

    print(f"Top {min(top, len(rows))} regressions (higher ratio is worse)")
    for row in regressions[:top]:
        print(f"  {row.path} -> {row.ratio:.2f}x ({row.diff_pct:+.2f}%)")
    print("")

    print(f"Top {min(top, len(rows))} improvements (lower ratio is better)")
    for row in improvements[:top]:
        print(f"  {row.path} -> {row.ratio:.2f}x ({row.diff_pct:+.2f}%)")
    print("")


def print_pr_vs_base(rows: list[ScriptStats], top: int) -> None:
    base_rows = [row for row in rows if row.has_base]
    if not base_rows:
        return

    total_base = sum(row.base_avg_ms for row in base_rows)
    total_pr = sum(row.rs_avg_ms for row in base_rows)
    overall_change_pct = (total_pr / total_base - 1.0) * 100.0 if total_base > 0 else math.inf

    print("PR vs Base")
    print(f"  Base total avg: {total_base:.3f} ms")
    print(f"  PR total avg:   {total_pr:.3f} ms")
    print(f"  Overall change: {overall_change_pct:+.2f}%")
    print("")

    print(f"Top {min(top, len(base_rows))} by PR avg time")
    print(
        f"{'script':60} {'base_avg':>10} {'pr_avg':>10} {'change_ms':>10} {'change_%':>9}"
    )
    print("-" * 108)
    for row in base_rows[:top]:
        print(
            f"{row.path[:60]:60} "
            f"{row.base_avg_ms:10.3f} {row.rs_avg_ms:10.3f} "
            f"{row.pr_change_ms:+10.3f} {row.pr_change_pct:+8.2f}%"
        )
    print("")

    regressions = sorted(base_rows, key=lambda r: r.pr_change_pct, reverse=True)
    improvements = sorted(base_rows, key=lambda r: r.pr_change_pct)

    print(f"Top {min(top, len(base_rows))} regressions (PR vs Base)")
    for row in regressions[:top]:
        print(f"  {row.path} -> {row.pr_change_pct:+.2f}% ({row.pr_change_ms:+.3f} ms)")
    print("")

    print(f"Top {min(top, len(base_rows))} improvements (PR vs Base)")
    for row in improvements[:top]:
        print(f"  {row.path} -> {row.pr_change_pct:+.2f}% ({row.pr_change_ms:+.3f} ms)")
    print("")


def print_category_summary(rows: list[ScriptStats]) -> None:
    grouped: dict[str, list[ScriptStats]] = {}
    for row in rows:
        grouped.setdefault(row.category, []).append(row)

    print("Category Summary")
    print(f"{'category':20} {'scripts':>8} {'py_total_ms':>14} {'rs_total_ms':>14} {'ratio':>10}")
    print("-" * 72)
    for category in sorted(grouped.keys()):
        items = grouped[category]
        py_total = sum(item.py_avg_ms for item in items)
        rs_total = sum(item.rs_avg_ms for item in items)
        ratio = rs_total / py_total if py_total > 0 else math.inf
        print(f"{category:20} {len(items):8} {py_total:14.3f} {rs_total:14.3f} {ratio:10.2f}x")
    print("")


def _change_indicator(pct: float) -> str:
    if pct < -0.5:
        return "✅"
    if pct > 2.0:
        return "⚠️"
    return "➡️"


def _build_summary_lines(rows: list[ScriptStats]) -> list[str]:
    lines: list[str] = []
    base_rows = [row for row in rows if row.has_base]

    if base_rows:
        total_base = sum(row.base_avg_ms for row in base_rows)
        total_pr = sum(row.rs_avg_ms for row in base_rows)
        overall_change = (total_pr / total_base - 1.0) * 100.0 if total_base > 0 else math.inf
        indicator = _change_indicator(overall_change)
        lines += [
            "## Benchmark: PR vs Base",
            "",
            f"- Base total avg: {total_base:.3f} ms",
            f"- PR total avg:   {total_pr:.3f} ms",
            f"- Overall change: {overall_change:+.2f}% {indicator}",
            "",
            "| Script | Base (ms) | PR (ms) | Change |",
            "|---|---:|---:|---:|",
        ]
        for row in base_rows[:20]:
            ind = _change_indicator(row.pr_change_pct)
            lines.append(
                f"| {row.path} | {row.base_avg_ms:.3f} | {row.rs_avg_ms:.3f}"
                f" | {row.pr_change_pct:+.2f}% {ind} |"
            )
        lines += ["", "---", ""]

    total_py = sum(row.py_avg_ms for row in rows)
    total_rs = sum(row.rs_avg_ms for row in rows)
    overall_ratio = (total_rs / total_py) if total_py > 0 else math.inf
    lines += [
        "## Speed Comparison (Python vs PyRust)",
        "",
        f"- Scripts: {len(rows)}",
        f"- Python total avg: {total_py:.3f} ms",
        f"- PyRust total avg: {total_rs:.3f} ms",
        f"- Overall ratio (PyRust/Python): {overall_ratio:.2f}x",
        "",
        "| Script | Python avg (ms) | PyRust avg (ms) | Ratio |",
        "|---|---:|---:|---:|",
    ]
    for row in rows[:20]:
        lines.append(
            f"| {row.path} | {row.py_avg_ms:.3f} | {row.rs_avg_ms:.3f} | {row.ratio:.2f}x |"
        )

    return lines


def write_github_step_summary(rows: list[ScriptStats]) -> None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return
    lines = _build_summary_lines(rows)
    Path(summary_path).write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_pr_comment(rows: list[ScriptStats], out_path: str) -> None:
    """Write a standalone Markdown PR comment (PR-vs-base + Python-vs-PyRust)."""
    lines = _build_summary_lines(rows)
    Path(out_path).write_text("\n".join(lines) + "\n", encoding="utf-8")


def build_json_snapshot(
    rows: list[ScriptStats],
    iterations: int,
    warmup: bool,
    benchmark_elapsed_ms: float,
) -> dict:
    timestamp = os.environ.get("BENCHMARK_TIMESTAMP_UTC", "")
    commit = os.environ.get("GITHUB_SHA", "local")
    commit_short = commit[:12] if len(commit) > 12 else commit
    total_py = sum(row.py_avg_ms for row in rows)
    total_rs = sum(row.rs_avg_ms for row in rows)
    return {
        "commit": commit,
        "commit_short": commit_short,
        "timestamp": timestamp,
        "iterations": iterations,
        "warmup": warmup,
        "elapsed_ms": benchmark_elapsed_ms,
        "total_py_ms": total_py,
        "total_rs_ms": total_rs,
        "overall_ratio": (total_rs / total_py) if total_py > 0 else None,
        "scripts": [
            {
                "path": row.path,
                "category": row.category,
                "py_avg_ms": row.py_avg_ms,
                "rs_avg_ms": row.rs_avg_ms,
                "ratio": row.ratio if math.isfinite(row.ratio) else None,
            }
            for row in rows
        ],
    }


def build_benchmark_action_output(rows: list[ScriptStats]) -> list[dict]:
    """
    Produce output for benchmark-action/github-action-benchmark
    (tool: customSmallerIsBetter).  Each script emits two entries:
    one for PyRust and one for Python so both trend lines appear on the chart.
    """
    entries = []
    for row in rows:
        name = row.path.split("/")[-1].removesuffix(".py")
        entries.append({
            "name": f"{name} [PyRust]",
            "value": round(row.rs_avg_ms, 3),
            "unit": "ms",
            "range": f"±{round(row.rs_std_ms, 3)} ms",
        })
        entries.append({
            "name": f"{name} [Python]",
            "value": round(row.py_avg_ms, 3),
            "unit": "ms",
            "range": f"±{round(row.py_std_ms, 3)} ms",
        })
    return entries


def build_markdown_snapshot(
    rows: list[ScriptStats],
    iterations: int,
    warmup: bool,
    benchmark_elapsed_ms: float,
    top: int,
) -> str:
    total_py = sum(row.py_avg_ms for row in rows)
    total_rs = sum(row.rs_avg_ms for row in rows)
    overall_ratio = (total_rs / total_py) if total_py > 0 else math.inf
    overall_diff = total_rs - total_py

    timestamp = os.environ.get("BENCHMARK_TIMESTAMP_UTC", "(unknown)")
    commit = os.environ.get("GITHUB_SHA", "(local)")
    if len(commit) > 12:
        commit = commit[:12]

    lines = [
        f"- Updated (UTC): `{timestamp}`",
        f"- Commit: `{commit}`",
        f"- Scripts: `{len(rows)}`",
        f"- Iterations: `{iterations}`",
        f"- Warmup: `{'enabled' if warmup else 'disabled'}`",
        f"- Python total avg: `{total_py:.3f} ms`",
        f"- PyRust total avg: `{total_rs:.3f} ms`",
        f"- Overall ratio (PyRust/Python): `{overall_ratio:.2f}x`",
        f"- Absolute diff: `{overall_diff:+.3f} ms`",
        f"- Benchmark wall-clock: `{benchmark_elapsed_ms:.3f} ms`",
        "",
        "| Script | Python avg (ms) | PyRust avg (ms) | Ratio | Diff (ms) | Diff (%) |",
        "|---|---:|---:|---:|---:|---:|",
    ]

    for row in rows[:top]:
        lines.append(
            "| "
            f"{row.path} | {row.py_avg_ms:.3f} | {row.rs_avg_ms:.3f} | "
            f"{row.ratio:.2f}x | {row.diff_ms:+.3f} | {row.diff_pct:+.2f}% |"
        )

    return "\n".join(lines) + "\n"


def build_svg_snapshot(
    rows: list[ScriptStats],
    iterations: int,
    warmup: bool,
    benchmark_elapsed_ms: float,
    top: int,
) -> str:
    total_py = sum(row.py_avg_ms for row in rows)
    total_rs = sum(row.rs_avg_ms for row in rows)
    overall_ratio = (total_rs / total_py) if total_py > 0 else math.inf
    overall_diff = total_rs - total_py

    timestamp = os.environ.get("BENCHMARK_TIMESTAMP_UTC", "(unknown)")
    commit = os.environ.get("GITHUB_SHA", "(local)")
    if len(commit) > 12:
        commit = commit[:12]

    top_rows = rows[:top]
    width = 1200
    row_height = 28
    header_height = 210
    footer_height = 28
    height = header_height + len(top_rows) * row_height + footer_height

    lines = [
        f"<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}' viewBox='0 0 {width} {height}'>",
        "<style>",
        "  .bg { fill: #0f172a; }",
        "  .card { fill: #111827; stroke: #334155; stroke-width: 1; }",
        "  .title { fill: #e2e8f0; font: 700 28px ui-sans-serif, -apple-system, Segoe UI, Roboto, Helvetica, Arial; }",
        "  .meta { fill: #93c5fd; font: 500 14px ui-sans-serif, -apple-system, Segoe UI, Roboto, Helvetica, Arial; }",
        "  .text { fill: #e5e7eb; font: 500 14px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }",
        "  .head { fill: #60a5fa; font: 700 14px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }",
        "  .good { fill: #34d399; font: 600 14px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }",
        "  .bad { fill: #fca5a5; font: 600 14px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }",
        "</style>",
        "<rect class='bg' x='0' y='0' width='100%' height='100%' />",
        "<rect class='card' x='20' y='20' rx='12' ry='12' width='1160' height='{}' />".format(height - 40),
        "<text class='title' x='40' y='58'>PyRust Benchmark Snapshot (main)</text>",
        "<text class='meta' x='40' y='86'>Updated (UTC): {}</text>".format(html.escape(timestamp)),
        "<text class='meta' x='420' y='86'>Commit: {}</text>".format(html.escape(commit)),
        "<text class='meta' x='620' y='86'>Scripts: {}</text>".format(len(rows)),
        "<text class='meta' x='760' y='86'>Iterations: {}</text>".format(iterations),
        "<text class='meta' x='910' y='86'>Warmup: {}</text>".format("enabled" if warmup else "disabled"),
        "<text class='text' x='40' y='118'>Python total avg: {:.3f} ms</text>".format(total_py),
        "<text class='text' x='320' y='118'>PyRust total avg: {:.3f} ms</text>".format(total_rs),
        "<text class='text' x='620' y='118'>Overall ratio (PyRust/Python): {:.2f}x</text>".format(overall_ratio),
        "<text class='text' x='980' y='118'>Diff: {:+.3f} ms</text>".format(overall_diff),
        "<text class='text' x='40' y='140'>Benchmark wall-clock: {:.3f} ms</text>".format(benchmark_elapsed_ms),
        "<text class='head' x='40' y='178'>Script</text>",
        "<text class='head' x='730' y='178'>Python(ms)</text>",
        "<text class='head' x='860' y='178'>PyRust(ms)</text>",
        "<text class='head' x='980' y='178'>Ratio</text>",
        "<text class='head' x='1060' y='178'>Diff%</text>",
    ]

    y = 202
    for row in top_rows:
        diff_cls = "good" if row.ratio <= 1.0 else "bad"
        lines.extend(
            [
                "<text class='text' x='40' y='{y}'>{path}</text>".format(
                    y=y,
                    path=html.escape(row.path[:80]),
                ),
                "<text class='text' x='730' y='{y}'>{v:.3f}</text>".format(y=y, v=row.py_avg_ms),
                "<text class='text' x='860' y='{y}'>{v:.3f}</text>".format(y=y, v=row.rs_avg_ms),
                "<text class='{cls}' x='980' y='{y}'>{v:.2f}x</text>".format(
                    cls=diff_cls,
                    y=y,
                    v=row.ratio,
                ),
                "<text class='{cls}' x='1060' y='{y}'>{v:+.2f}%</text>".format(
                    cls=diff_cls,
                    y=y,
                    v=row.diff_pct,
                ),
            ]
        )
        y += row_height

    lines.append("</svg>")
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compare execution speed between Python and PyRust over tests/cases"
    )
    parser.add_argument("--iterations", type=int, default=3)
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        help="number of scripts to show in top lists",
    )
    parser.add_argument(
        "--no-warmup",
        action="store_true",
        help="disable one warmup run per script before timed runs",
    )
    parser.add_argument(
        "--markdown-out",
        type=str,
        default="",
        help="optional output path for markdown snapshot",
    )
    parser.add_argument(
        "--svg-out",
        type=str,
        default="",
        help="optional output path for svg snapshot",
    )
    parser.add_argument(
        "--json-out",
        type=str,
        default="",
        help="optional output path for JSON snapshot (used by build_benchmark_pages.py)",
    )
    parser.add_argument(
        "--benchmark-action-out",
        type=str,
        default="",
        help="output path for benchmark-action/github-action-benchmark JSON (customSmallerIsBetter)",
    )
    parser.add_argument(
        "--base-bin",
        type=str,
        default="",
        help="path to the base branch PyRust binary for PR vs base comparison",
    )
    parser.add_argument(
        "--pr-comment-out",
        type=str,
        default="",
        help="output path for a Markdown PR comment (PR-vs-base + Python-vs-PyRust tables)",
    )
    args = parser.parse_args()

    if args.iterations <= 0:
        print("--iterations must be > 0", file=sys.stderr)
        return 2

    root = Path(__file__).resolve().parent.parent
    python_bin = Path(sys.executable)
    pyrust_bin = find_pyrust_binary(root)

    if not pyrust_bin.exists():
        print(f"PyRust binary not found: {pyrust_bin}", file=sys.stderr)
        print("Build first: cargo build", file=sys.stderr)
        return 2

    base_bin: "Path | None" = None
    if args.base_bin:
        base_bin = Path(args.base_bin)
        if not base_bin.exists():
            print(f"Base binary not found: {base_bin}", file=sys.stderr)
            return 2

    scripts = collect_scripts(root)
    benchmark_start = time.perf_counter()
    rows = benchmark(
        root=root,
        python_bin=python_bin,
        pyrust_bin=pyrust_bin,
        scripts=scripts,
        iterations=args.iterations,
        warmup=not args.no_warmup,
        base_bin=base_bin,
    )
    benchmark_elapsed = (time.perf_counter() - benchmark_start) * 1000.0

    print_environment(python_bin, pyrust_bin, base_bin, rows, args.iterations, warmup=not args.no_warmup)
    print_overall_summary(rows)
    print_per_script(rows, args.top)
    print_variability(rows, args.top)
    print_winners(rows, args.top)
    print_pr_vs_base(rows, args.top)
    print_category_summary(rows)
    print(f"Benchmark wall-clock: {benchmark_elapsed:.3f} ms")

    write_github_step_summary(rows)

    if args.pr_comment_out:
        write_pr_comment(rows, args.pr_comment_out)

    if args.markdown_out:
        snapshot = build_markdown_snapshot(
            rows=rows,
            iterations=args.iterations,
            warmup=not args.no_warmup,
            benchmark_elapsed_ms=benchmark_elapsed,
            top=args.top,
        )
        Path(args.markdown_out).write_text(snapshot, encoding="utf-8")

    if args.svg_out:
        snapshot_svg = build_svg_snapshot(
            rows=rows,
            iterations=args.iterations,
            warmup=not args.no_warmup,
            benchmark_elapsed_ms=benchmark_elapsed,
            top=args.top,
        )
        Path(args.svg_out).write_text(snapshot_svg, encoding="utf-8")

    if args.json_out:
        snapshot_json = build_json_snapshot(
            rows=rows,
            iterations=args.iterations,
            warmup=not args.no_warmup,
            benchmark_elapsed_ms=benchmark_elapsed,
        )
        Path(args.json_out).write_text(json.dumps(snapshot_json, indent=2), encoding="utf-8")

    if args.benchmark_action_out:
        ba_output = build_benchmark_action_output(rows)
        Path(args.benchmark_action_out).write_text(json.dumps(ba_output, indent=2), encoding="utf-8")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
