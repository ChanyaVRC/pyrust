#!/usr/bin/env python3
"""bench_report.py — PyRust benchmark report generator.

Called by tools/bench.sh; does no timing itself.

Modes
-----
--dump-configs          Print JSON of all resolved per-script configs and exit.
                        Used by bench.sh to feed iteration/warmup counts to hyperfine.
(default)               Read hyperfine JSON from --results-dir and generate reports.
"""

import argparse
import html
import json
import math
import os
import statistics
import sys
from dataclasses import dataclass, field
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    tomllib = None  # type: ignore[assignment]

ROOT = Path(__file__).resolve().parent.parent
CASES_DIR = ROOT / "crates" / "pyrust" / "tests" / "cases"
DEFAULT_CONFIG = Path(__file__).resolve().parent / "bench.toml"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class ScriptConfig:
    iterations: int
    warmup: int
    trim: float


@dataclass(frozen=True)
class BenchConfig:
    default: ScriptConfig
    overrides: dict[str, ScriptConfig]

    def resolve(self, rel: str) -> ScriptConfig:
        return self.overrides.get(rel, self.default)


def load_config(path: Path, fallback: ScriptConfig) -> BenchConfig:
    if tomllib is None or not path.exists():
        return BenchConfig(default=fallback, overrides={})
    with open(path, "rb") as fh:
        raw = tomllib.load(fh)
    defs = raw.get("defaults", {})
    merged = ScriptConfig(
        iterations=defs.get("iterations", fallback.iterations),
        warmup=defs.get("warmup", fallback.warmup),
        trim=float(defs.get("trim", fallback.trim)),
    )
    overrides: dict[str, ScriptConfig] = {}
    for rel, cfg in raw.get("script", {}).items():
        overrides[rel] = ScriptConfig(
            iterations=cfg.get("iterations", merged.iterations),
            warmup=cfg.get("warmup", merged.warmup),
            trim=float(cfg.get("trim", merged.trim)),
        )
    return BenchConfig(default=merged, overrides=overrides)


def dump_configs(config: BenchConfig) -> None:
    """Print JSON mapping every test script to its resolved config.

    All scripts are included so bench.sh can look up any key without a fallback.
    """
    scripts = sorted(CASES_DIR.rglob("test_*.py"))
    out: dict = {}
    for script in scripts:
        rel = script.relative_to(CASES_DIR).as_posix()
        cfg = config.resolve(rel)
        out[rel] = {"iterations": cfg.iterations, "warmup": cfg.warmup, "trim": cfg.trim}
    print(json.dumps(out))


# ---------------------------------------------------------------------------
# Hyperfine result parsing
# ---------------------------------------------------------------------------

def _trimmed_mean(samples: list[float], pct: float) -> float:
    n = len(samples)
    k = round(n * pct / 100)
    if k == 0 or n - 2 * k <= 0:
        return statistics.fmean(samples)
    s = sorted(samples)
    return statistics.fmean(s[k : n - k])


@dataclass
class ScriptResult:
    rel: str          # relative to CASES_DIR (posix)
    cfg: ScriptConfig
    py_times: list[float]
    rs_times: list[float]
    base_times: list[float] = field(default_factory=list)

    @property
    def py_avg_ms(self) -> float:
        return _trimmed_mean(self.py_times, self.cfg.trim) * 1000.0

    @property
    def rs_avg_ms(self) -> float:
        return _trimmed_mean(self.rs_times, self.cfg.trim) * 1000.0

    @property
    def base_avg_ms(self) -> float:
        return _trimmed_mean(self.base_times, self.cfg.trim) * 1000.0

    @property
    def py_std_ms(self) -> float:
        return statistics.stdev(self.py_times) * 1000.0 if len(self.py_times) >= 2 else 0.0

    @property
    def rs_std_ms(self) -> float:
        return statistics.stdev(self.rs_times) * 1000.0 if len(self.rs_times) >= 2 else 0.0

    @property
    def has_base(self) -> bool:
        return bool(self.base_times)

    @property
    def ratio(self) -> float:
        return self.rs_avg_ms / self.py_avg_ms if self.py_avg_ms > 0 else math.inf

    @property
    def diff_pct(self) -> float:
        return (self.ratio - 1.0) * 100.0 if math.isfinite(self.ratio) else math.inf

    @property
    def pr_change_pct(self) -> float:
        b = self.base_avg_ms
        return (self.rs_avg_ms / b - 1.0) * 100.0 if b > 0 else math.inf

    @property
    def pr_change_ms(self) -> float:
        return self.rs_avg_ms - self.base_avg_ms


def _parse_hyperfine_json(path: Path, config: BenchConfig) -> "ScriptResult | None":
    """Parse one per-script hyperfine JSON file.

    Command names written by bench.sh follow the convention:
        python:<rel>   pyrust:<rel>   base:<rel>
    where <rel> is the posix path relative to tests/cases/.
    """
    data = json.loads(path.read_text())
    py_times = rs_times = base_times = None
    rel: "str | None" = None

    for entry in data.get("results", []):
        cmd: str = entry["command"]
        # Use raw times when available; fall back to the scalar mean hyperfine reports.
        times: list[float] = entry.get("times", [entry["mean"]])

        if cmd.startswith("python:"):
            rel = cmd[len("python:"):]
            py_times = times
        elif cmd.startswith("pyrust:"):
            rs_times = times
        elif cmd.startswith("base:"):
            base_times = times

    if rel is None or py_times is None or rs_times is None:
        return None

    return ScriptResult(
        rel=rel,
        cfg=config.resolve(rel),
        py_times=py_times,
        rs_times=rs_times,
        base_times=base_times or [],
    )


def load_results(results_dir: Path, config: BenchConfig) -> list[ScriptResult]:
    rows: list[ScriptResult] = []
    for json_path in sorted(results_dir.glob("*.json")):
        result = _parse_hyperfine_json(json_path, config)
        if result is not None:
            rows.append(result)
    rows.sort(key=lambda r: r.rs_avg_ms, reverse=True)
    return rows


# ---------------------------------------------------------------------------
# Console output
# ---------------------------------------------------------------------------

def print_summary(rows: list[ScriptResult]) -> None:
    total_py = sum(r.py_avg_ms for r in rows)
    total_rs = sum(r.rs_avg_ms for r in rows)
    overall  = total_rs / total_py if total_py > 0 else math.inf
    ratios   = [r.ratio for r in rows if math.isfinite(r.ratio)]
    print("\nOverall Summary")
    print(f"  Python  total : {total_py:.3f} ms")
    print(f"  PyRust  total : {total_rs:.3f} ms")
    print(f"  ratio (mean)  : {overall:.3f}x")
    if ratios:
        print(f"  ratio (median): {statistics.median(ratios):.3f}x")
    print()


def print_per_script(rows: list[ScriptResult], top: int) -> None:
    n = min(top, len(rows))
    print(f"Top {n} by PyRust avg")
    print(f"{'script':<56} {'iters':>5} {'warmup':>6} {'py_ms':>9} {'rs_ms':>9} {'ratio':>7} {'diff%':>8}")
    print("-" * 108)
    for r in rows[:n]:
        print(
            f"{r.rel[:56]:<56} {r.cfg.iterations:>5} {r.cfg.warmup:>6}"
            f" {r.py_avg_ms:>9.3f} {r.rs_avg_ms:>9.3f}"
            f" {r.ratio:>7.3f}x {r.diff_pct:>+7.2f}%"
        )
    print()


def print_pr_vs_base(rows: list[ScriptResult], top: int) -> None:
    base_rows = [r for r in rows if r.has_base]
    if not base_rows:
        return
    total_base = sum(r.base_avg_ms for r in base_rows)
    total_pr   = sum(r.rs_avg_ms   for r in base_rows)
    change     = (total_pr / total_base - 1.0) * 100.0 if total_base > 0 else math.inf
    print("PR vs Base")
    print(f"  base total: {total_base:.3f} ms  PR total: {total_pr:.3f} ms  change: {change:+.2f}%\n")
    n = min(top, len(base_rows))
    print(f"{'script':<56} {'base_ms':>9} {'pr_ms':>9} {'change%':>9}")
    print("-" * 90)
    for r in sorted(base_rows, key=lambda r: r.rs_avg_ms, reverse=True)[:n]:
        print(f"{r.rel[:56]:<56} {r.base_avg_ms:>9.3f} {r.rs_avg_ms:>9.3f} {r.pr_change_pct:>+8.2f}%")
    print()


# ---------------------------------------------------------------------------
# GitHub integration
# ---------------------------------------------------------------------------

def _indicator(pct: float) -> str:
    if pct < -0.5:
        return "✅"
    if pct > 2.0:
        return "⚠️"
    return "➡️"


def _fmt_kb(kb: "int | None") -> str:
    if kb is None:
        return "—"
    if kb >= 1024:
        return f"{kb / 1024:.1f} MB"
    return f"{kb} KB"


def _details_summary(shown: int, total: int, label: str) -> str:
    """`<details>` heading; flags truncation so readers know to grab the full report."""
    if shown >= total:
        return f"All {total} scripts — {label}"
    return f"Top {shown} of {total} scripts — {label} (full report attached)"


def _summary_md(rows: list[ScriptResult], top: "int | None" = None) -> str:
    """Timing-only PR comment. Memory is a separate comment.

    `top` caps each `<details>` table to that many (slowest) scripts so the
    posted comment stays small; pass `None` for the complete, attachable report.
    """
    lines: list[str] = []
    base_rows = [r for r in rows if r.has_base]

    # ── PR vs Base ────────────────────────────────────────────────────────────
    if base_rows:
        total_base = sum(r.base_avg_ms for r in base_rows)
        total_pr   = sum(r.rs_avg_ms   for r in base_rows)
        overall    = (total_pr / total_base - 1.0) * 100.0 if total_base > 0 else math.inf
        base_shown = base_rows if top is None else base_rows[:top]
        lines += [
            "### ⏱ PR vs Base",
            "",
            f"| | Time |",
            "|---|---|",
            f"| Base total | {total_base:.1f} ms |",
            f"| PR total   | {total_pr:.1f} ms |",
            f"| Change     | {overall:+.2f}% {_indicator(overall)} |",
            "",
            f"<details><summary>{_details_summary(len(base_shown), len(base_rows), 'PR vs Base')}</summary>",
            "",
            "| Script | iters | Base (ms) | PR (ms) | Change |",
            "|---|---:|---:|---:|---:|",
        ]
        for r in base_shown:
            lines.append(
                f"| `{r.rel}` | {r.cfg.iterations}"
                f" | {r.base_avg_ms:.3f} | {r.rs_avg_ms:.3f}"
                f" | {r.pr_change_pct:+.2f}% {_indicator(r.pr_change_pct)} |"
            )
        lines += ["", "</details>", "", "---", ""]

    # ── Speed: Python vs PyRust ───────────────────────────────────────────────
    total_py = sum(r.py_avg_ms for r in rows)
    total_rs = sum(r.rs_avg_ms for r in rows)
    overall_ratio = total_rs / total_py if total_py > 0 else math.inf
    rows_shown = rows if top is None else rows[:top]
    lines += [
        "### 🚀 Speed: Python vs PyRust",
        "",
        f"| | |",
        "|---|---|",
        f"| Scripts       | {len(rows)} |",
        f"| Python total  | {total_py:.1f} ms |",
        f"| PyRust total  | {total_rs:.1f} ms |",
        f"| Overall ratio | {overall_ratio:.3f}x |",
        "",
        f"<details><summary>{_details_summary(len(rows_shown), len(rows), 'timing')}</summary>",
        "",
        "| Script | iters | Python (ms) | PyRust (ms) | Ratio |",
        "|---|---:|---:|---:|---:|",
    ]
    for r in rows_shown:
        lines.append(
            f"| `{r.rel}` | {r.cfg.iterations}"
            f" | {r.py_avg_ms:.3f} | {r.rs_avg_ms:.3f} | {r.ratio:.3f}x |"
        )
    lines += ["", "</details>"]
    return "\n".join(lines) + "\n"


def _memory_comment_md(rows: list[ScriptResult], memory: "dict[str, dict]",
                       top: "int | None" = None) -> str:
    """Standalone memory-usage comment (peak RSS per script).

    `top` caps the `<details>` table to that many (slowest) scripts; pass
    `None` for the complete, attachable report.
    """
    py_mems = [memory[r.rel]["py_kb"] for r in rows
               if r.rel in memory and isinstance(memory[r.rel].get("py_kb"), (int, float))]
    rs_mems = [memory[r.rel]["rs_kb"] for r in rows
               if r.rel in memory and isinstance(memory[r.rel].get("rs_kb"), (int, float))]
    py_med = sorted(py_mems)[len(py_mems) // 2] if py_mems else None
    rs_med = sorted(rs_mems)[len(rs_mems) // 2] if rs_mems else None
    has_base = any(
        isinstance(memory.get(r.rel, {}).get("base_kb"), (int, float)) for r in rows
    )

    lines: list[str] = [
        "### 🧠 Memory Usage (peak RSS)",
        "",
        "| | Median peak RSS |",
        "|---|---|",
        f"| Python | {_fmt_kb(py_med)} |",
        f"| PyRust | {_fmt_kb(rs_med)} |",
    ]
    if has_base:
        base_mems = [memory[r.rel]["base_kb"] for r in rows
                     if r.rel in memory and isinstance(memory[r.rel].get("base_kb"), (int, float))]
        base_med = sorted(base_mems)[len(base_mems) // 2] if base_mems else None
        lines.append(f"| Base   | {_fmt_kb(base_med)} |")
    rows_shown = rows if top is None else rows[:top]
    lines += [
        "",
        f"<details><summary>{_details_summary(len(rows_shown), len(rows), 'peak memory')}</summary>",
        "",
    ]
    if has_base:
        lines += ["| Script | Python | PyRust | Base |", "|---|---:|---:|---:|"]
        for r in rows_shown:
            m = memory.get(r.rel, {})
            lines.append(
                f"| `{r.rel}` | {_fmt_kb(m.get('py_kb'))}"
                f" | {_fmt_kb(m.get('rs_kb'))} | {_fmt_kb(m.get('base_kb'))} |"
            )
    else:
        lines += ["| Script | Python | PyRust |", "|---|---:|---:|"]
        for r in rows_shown:
            m = memory.get(r.rel, {})
            lines.append(
                f"| `{r.rel}` | {_fmt_kb(m.get('py_kb'))} | {_fmt_kb(m.get('rs_kb'))} |"
            )
    lines += ["", "</details>"]
    return "\n".join(lines) + "\n"


def load_memory(memory_dir: str) -> "dict[str, dict] | None":
    """Load per-script peak RSS data from JSON files written by bench.sh."""
    if not memory_dir:
        return None
    d = Path(memory_dir)
    if not d.is_dir():
        return None
    result: dict[str, dict] = {}
    for p in sorted(d.glob("*.json")):
        try:
            obj = json.loads(p.read_text())
            rel = obj.get("rel")
            if rel:
                result[rel] = obj
        except Exception:
            pass
    return result or None


def write_github_step_summary(rows: list[ScriptResult]) -> None:
    dest = os.environ.get("GITHUB_STEP_SUMMARY")
    if dest:
        Path(dest).write_text(_summary_md(rows), encoding="utf-8")


# ---------------------------------------------------------------------------
# File output builders
# ---------------------------------------------------------------------------

def build_benchmark_action(rows: list[ScriptResult]) -> list[dict]:
    """Format for benchmark-action/github-action-benchmark (customSmallerIsBetter)."""
    entries = []
    for r in rows:
        name = r.rel.split("/")[-1].removesuffix(".py")
        entries.append({
            "name": f"{name} [PyRust]",
            "value": round(r.rs_avg_ms, 3),
            "unit": "ms",
            "range": f"±{round(r.rs_std_ms, 3)} ms",
            "extra": f"iters={r.cfg.iterations} trim={r.cfg.trim:.0f}%",
        })
        entries.append({
            "name": f"{name} [Python]",
            "value": round(r.py_avg_ms, 3),
            "unit": "ms",
            "range": f"±{round(r.py_std_ms, 3)} ms",
            "extra": f"iters={r.cfg.iterations} trim={r.cfg.trim:.0f}%",
        })
    return entries


def build_json_snapshot(rows: list[ScriptResult]) -> dict:
    commit = os.environ.get("GITHUB_SHA", "local")
    total_py = sum(r.py_avg_ms for r in rows)
    total_rs = sum(r.rs_avg_ms for r in rows)
    return {
        "commit": commit,
        "commit_short": commit[:12],
        "timestamp": os.environ.get("BENCHMARK_TIMESTAMP_UTC", ""),
        "total_py_ms": total_py,
        "total_rs_ms": total_rs,
        "overall_ratio": (total_rs / total_py) if total_py > 0 else None,
        "scripts": [
            {
                "path": r.rel,
                "iterations": r.cfg.iterations,
                "warmup": r.cfg.warmup,
                "trim": r.cfg.trim,
                "py_avg_ms": r.py_avg_ms,
                "rs_avg_ms": r.rs_avg_ms,
                "py_std_ms": r.py_std_ms,
                "rs_std_ms": r.rs_std_ms,
                "ratio": r.ratio if math.isfinite(r.ratio) else None,
            }
            for r in rows
        ],
    }


def build_markdown_snapshot(rows: list[ScriptResult], top: int) -> str:
    commit    = os.environ.get("GITHUB_SHA", "(local)")[:12]
    ts        = os.environ.get("BENCHMARK_TIMESTAMP_UTC", "(unknown)")
    total_py  = sum(r.py_avg_ms for r in rows)
    total_rs  = sum(r.rs_avg_ms for r in rows)
    ratio     = total_rs / total_py if total_py > 0 else math.inf
    iters_set = sorted({r.cfg.iterations for r in rows})
    iters_str = "/".join(str(i) for i in iters_set) if len(iters_set) > 1 else str(iters_set[0])

    lines = [
        f"- Updated (UTC): `{ts}`",
        f"- Commit: `{commit}`",
        f"- Scripts: `{len(rows)}`",
        f"- Iterations: `{iters_str}` (per-script)",
        f"- Python total: `{total_py:.3f} ms`",
        f"- PyRust total: `{total_rs:.3f} ms`",
        f"- Overall ratio: `{ratio:.3f}x`",
        "",
        "| Script | iters | Python (ms) | PyRust (ms) | Ratio | Diff% |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for r in rows[:top]:
        lines.append(
            f"| {r.rel} | {r.cfg.iterations}"
            f" | {r.py_avg_ms:.3f} | {r.rs_avg_ms:.3f}"
            f" | {r.ratio:.3f}x | {r.diff_pct:+.2f}% |"
        )
    return "\n".join(lines) + "\n"


def build_svg_snapshot(rows: list[ScriptResult], top: int) -> str:
    total_py = sum(r.py_avg_ms for r in rows)
    total_rs = sum(r.rs_avg_ms for r in rows)
    ratio    = total_rs / total_py if total_py > 0 else math.inf
    commit   = os.environ.get("GITHUB_SHA", "(local)")[:12]
    ts       = os.environ.get("BENCHMARK_TIMESTAMP_UTC", "(unknown)")

    top_rows = rows[:top]
    W, row_h, hdr_h, ftr_h = 1280, 28, 220, 28
    H = hdr_h + len(top_rows) * row_h + ftr_h

    def e(s: str) -> str:
        return html.escape(str(s))

    lines = [
        f"<svg xmlns='http://www.w3.org/2000/svg' width='{W}' height='{H}' viewBox='0 0 {W} {H}'>",
        "<style>",
        "  .bg  {fill:#0f172a}",
        "  .card{fill:#111827;stroke:#334155;stroke-width:1}",
        "  .ttl {fill:#e2e8f0;font:700 26px ui-sans-serif,system-ui,sans-serif}",
        "  .meta{fill:#93c5fd;font:500 13px ui-sans-serif,system-ui,sans-serif}",
        "  .mono{fill:#e5e7eb;font:500 13px ui-monospace,monospace}",
        "  .hdr {fill:#60a5fa;font:700 13px ui-monospace,monospace}",
        "  .good{fill:#34d399;font:600 13px ui-monospace,monospace}",
        "  .bad {fill:#fca5a5;font:600 13px ui-monospace,monospace}",
        "</style>",
        f"<rect class='bg'   width='{W}' height='{H}'/>",
        f"<rect class='card' x='16' y='16' rx='10' width='{W-32}' height='{H-32}'/>",
        f"<text class='ttl'  x='36' y='54'>PyRust Benchmark (main)</text>",
        f"<text class='meta' x='36' y='76'>Updated: {e(ts)}  Commit: {e(commit)}</text>",
        f"<text class='meta' x='36' y='96'>Scripts: {len(rows)}</text>",
        f"<text class='mono' x='36'  y='118'>Python total: {total_py:.3f} ms</text>",
        f"<text class='mono' x='320' y='118'>PyRust total: {total_rs:.3f} ms</text>",
        f"<text class='mono' x='620' y='118'>Overall ratio: {ratio:.3f}x</text>",
        "<text class='hdr'  x='36'   y='158'>Script</text>",
        "<text class='hdr'  x='720'  y='158'>iters</text>",
        "<text class='hdr'  x='780'  y='158'>Python(ms)</text>",
        "<text class='hdr'  x='900'  y='158'>PyRust(ms)</text>",
        "<text class='hdr'  x='1020' y='158'>Ratio</text>",
        "<text class='hdr'  x='1110' y='158'>Diff%</text>",
        f"<line x1='36' y1='164' x2='{W-36}' y2='164' stroke='#334155'/>",
    ]

    y = 182
    for r in top_rows:
        cls = "good" if r.ratio <= 1.0 else "bad"
        lines += [
            f"<text class='mono' x='36'   y='{y}'>{e(r.rel[:80])}</text>",
            f"<text class='mono' x='720'  y='{y}'>{r.cfg.iterations}</text>",
            f"<text class='mono' x='780'  y='{y}'>{r.py_avg_ms:.3f}</text>",
            f"<text class='mono' x='900'  y='{y}'>{r.rs_avg_ms:.3f}</text>",
            f"<text class='{cls}' x='1020' y='{y}'>{r.ratio:.3f}x</text>",
            f"<text class='{cls}' x='1110' y='{y}'>{r.diff_pct:+.2f}%</text>",
        ]
        y += row_h

    lines.append("</svg>")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--config", default=str(DEFAULT_CONFIG), metavar="PATH")
    p.add_argument("--dump-configs", action="store_true",
                   help="print per-script config JSON and exit (used by bench.sh)")
    p.add_argument("--results-dir", default="", metavar="DIR",
                   help="directory containing hyperfine *.json result files")
    p.add_argument("--top", type=int, default=15)
    p.add_argument("--memory-dir",           default="", metavar="DIR",
                   help="directory with per-script peak-RSS JSON files from bench.sh")
    p.add_argument("--pr-comment-out",       default="", metavar="PATH")
    p.add_argument("--pr-full-out",          default="", metavar="PATH",
                   help="write the complete (untruncated) timing report to PATH for attachment")
    p.add_argument("--memory-comment-out",   default="", metavar="PATH",
                   help="write memory-usage comment to PATH (requires --memory-dir)")
    p.add_argument("--memory-full-out",      default="", metavar="PATH",
                   help="write the complete (untruncated) memory report to PATH for attachment")
    p.add_argument("--benchmark-action-out", default="", metavar="PATH")
    p.add_argument("--svg-out",              default="", metavar="PATH")
    p.add_argument("--markdown-out",         default="", metavar="PATH")
    p.add_argument("--json-out",             default="", metavar="PATH")
    args = p.parse_args()

    config = load_config(Path(args.config), ScriptConfig(iterations=20, warmup=3, trim=10.0))

    if args.dump_configs:
        dump_configs(config)
        return 0

    if not args.results_dir:
        print("error: --results-dir required", file=sys.stderr)
        return 2

    rows = load_results(Path(args.results_dir), config)
    if not rows:
        print("error: no results found in results-dir", file=sys.stderr)
        return 2

    memory = load_memory(args.memory_dir)

    print_summary(rows)
    print_per_script(rows, args.top)
    print_pr_vs_base(rows, args.top)
    write_github_step_summary(rows)

    if args.pr_comment_out:
        Path(args.pr_comment_out).write_text(_summary_md(rows, top=args.top), encoding="utf-8")
    if args.pr_full_out:
        Path(args.pr_full_out).write_text(_summary_md(rows), encoding="utf-8")
    if args.memory_comment_out and memory:
        Path(args.memory_comment_out).write_text(
            _memory_comment_md(rows, memory, top=args.top), encoding="utf-8"
        )
    if args.memory_full_out and memory:
        Path(args.memory_full_out).write_text(
            _memory_comment_md(rows, memory), encoding="utf-8"
        )
    if args.benchmark_action_out:
        Path(args.benchmark_action_out).write_text(
            json.dumps(build_benchmark_action(rows), indent=2), encoding="utf-8"
        )
    if args.markdown_out:
        Path(args.markdown_out).write_text(
            build_markdown_snapshot(rows, args.top), encoding="utf-8"
        )
    if args.svg_out:
        Path(args.svg_out).write_text(
            build_svg_snapshot(rows, args.top), encoding="utf-8"
        )
    if args.json_out:
        Path(args.json_out).write_text(
            json.dumps(build_json_snapshot(rows), indent=2), encoding="utf-8"
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
