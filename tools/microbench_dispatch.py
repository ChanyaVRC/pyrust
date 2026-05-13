#!/usr/bin/env python3
"""Microbenchmark harness for the typed-dispatch / legacy-dispatch
comparison gating hot-path builtin migrations (issue #399).

Each bench script under ``bench/typed_dispatch/`` runs a tight
``for _ in range(N): builtin(...)`` loop.  Two ``loop_noop_*.py``
scripts measure the bare loop overhead at matching ``N``s; this script
subtracts that overhead and divides by ``N`` to produce a per-call
nanosecond figure.

The harness shells out to ``hyperfine`` for sub-millisecond timing
precision (the rest of the repo's bench infrastructure uses it too —
see ``tools/bench.sh``).

Usage:

    cargo build --release
    python3 tools/microbench_dispatch.py [--runs 7] [--warmup 2]

Outputs a Markdown table on stdout; suitable for pasting into a PR
description.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


# ── bench definitions ────────────────────────────────────────────────────
# (label, script, n, dispatch-form, paired-noop)
#
# `dispatch-form` is "typed" or "legacy" — labels how the builtin's body
# is declared in `builtin_modules/bodies/builtins.rs` today.  Loop-overhead
# baselines have form "noop".
@dataclass(frozen=True)
class Bench:
    label: str
    script: str
    n: int
    form: str  # "typed", "legacy", or "noop"
    noop: str  # which noop file to subtract


BENCHES = [
    # 1-arg typed (overload-dispatched).
    Bench("abs(int)",          "test_abs_int.py",     1_000_000, "typed",  "loop_noop.py"),
    Bench("abs(complex)",      "test_abs_complex.py", 1_000_000, "typed",  "loop_noop.py"),
    # 2-arg typed (single-body) — body-bound (file I/O).
    Bench("open(path, mode)",  "test_open_file.py",      10_000, "typed",  "loop_noop_open.py"),
    # Legacy `(args)`-form benches (candidates for migration).
    Bench("len(list)",         "test_len_list.py",    1_000_000, "legacy", "loop_noop.py"),
    Bench("getattr(obj, str)", "test_getattr_obj.py", 1_000_000, "legacy", "loop_noop.py"),
    # Legacy-form reference baselines (1-arg `(args)` shape).
    Bench("id(int) [legacy]",  "test_id_int.py",      1_000_000, "legacy", "loop_noop.py"),
]


def find_pyrust(root: Path) -> Path:
    """Resolve the pyrust release binary.  ``PYRUST_BIN`` overrides."""
    override = os.environ.get("PYRUST_BIN")
    if override:
        return Path(override)
    cand = root / "target" / "release" / "pyrust"
    if cand.exists():
        return cand
    cand = root / "target" / "debug" / "pyrust"
    if cand.exists():
        return cand
    raise SystemExit("error: pyrust binary not found; build with `cargo build --release` first")


def hyperfine_run(binary: Path, script: Path, runs: int, warmup: int) -> dict:
    """Run a single script under hyperfine and return the parsed result."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        out = Path(tmp.name)
    try:
        subprocess.run(
            [
                "hyperfine",
                "--shell=none",
                "--warmup", str(warmup),
                "--runs", str(runs),
                "--time-unit", "microsecond",
                "--export-json", str(out),
                f"{binary} {script}",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        return json.loads(out.read_text())["results"][0]
    finally:
        out.unlink(missing_ok=True)


def measure_all(binary: Path, bench_dir: Path, runs: int, warmup: int) -> dict[str, dict]:
    """Time every distinct script (benches + noops) once.  Returns
    {filename: hyperfine-result-dict}."""
    scripts: set[str] = set()
    for b in BENCHES:
        scripts.add(b.script)
        scripts.add(b.noop)

    timings: dict[str, dict] = {}
    for name in sorted(scripts):
        script = bench_dir / name
        if not script.exists():
            raise SystemExit(f"error: bench script missing: {script}")
        print(f"  timing {name}", file=sys.stderr)
        timings[name] = hyperfine_run(binary, script, runs=runs, warmup=warmup)
    return timings


def per_call_ns(bench: Bench, timings: dict[str, dict]) -> tuple[float, float]:
    """Return (mean ns/call, stddev ns/call) for the given bench."""
    bench_mean_s = timings[bench.script]["mean"]
    noop_mean_s = timings[bench.noop]["mean"]
    per_call_s = (bench_mean_s - noop_mean_s) / bench.n
    # Combine stddevs by quadrature, then per-call.
    bench_std_s = timings[bench.script]["stddev"]
    noop_std_s = timings[bench.noop]["stddev"]
    combined_std_s = (bench_std_s ** 2 + noop_std_s ** 2) ** 0.5
    per_call_std_s = combined_std_s / bench.n
    return per_call_s * 1e9, per_call_std_s * 1e9


def render_table(rows: list[tuple[Bench, float, float]]) -> str:
    """Pretty-print the bench results as a Markdown table."""
    by_label = {b.label: (mean, std) for b, mean, std in rows}

    # Legacy-form reference: `id(int) [legacy]` is the 1-arg `(args)` shape
    # with the cheapest possible body.  Use it as the legacy comparand for
    # the typed 1-arg builtins.
    legacy_1arg = by_label.get("id(int) [legacy]", (float("nan"), 0.0))[0]

    # For 2-arg typed (`open`), the closest legacy comparand among the
    # measured benches is `getattr(obj, str)` — also two positional args
    # with a kwarg-reject prelude.
    legacy_2arg = by_label.get("getattr(obj, str)", (float("nan"), 0.0))[0]

    def fmt(v: float) -> str:
        return f"{v:.1f}" if v == v else "n/a"

    lines = [
        "| Bench                  | Form   | Legacy (ns/call) | Typed (ns/call) | Ratio  | Notes |",
        "|------------------------|--------|-----------------:|----------------:|-------:|-------|",
    ]
    for bench, mean, std in rows:
        note = ""
        if bench.label == "id(int) [legacy]":
            # This is the legacy reference — no typed counterpart to show.
            legacy = mean
            typed = float("nan")
            ratio = float("nan")
            note = "legacy 1-arg reference"
        elif bench.form == "typed":
            typed = mean
            # Pick the right legacy comparand by arg count.
            if bench.label == "open(path, mode)":
                legacy = legacy_2arg
                note = "body-bound (file I/O); dispatch cost not isolatable"
            else:
                legacy = legacy_1arg
                if bench.label == "abs(int)":
                    note = "first-overload match (best case)"
                elif bench.label == "abs(complex)":
                    note = "3 misses + catch-all (worst case)"
            ratio = (typed / legacy) if legacy and legacy == legacy else float("nan")
        else:
            # Legacy-form bench measured as-is; no typed counterpart yet.
            legacy = mean
            typed = float("nan")
            ratio = float("nan")
            note = "migration candidate (still `(args)` form)"
        ratio_s = f"{ratio:.2f}x" if ratio == ratio else "n/a"
        lines.append(
            f"| {bench.label:22} | {bench.form:6} | "
            f"{fmt(legacy):>16} | {fmt(typed):>15} | {ratio_s:>6} | {note} |"
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=7,
                        help="hyperfine runs per script (default: 7)")
    parser.add_argument("--warmup", type=int, default=2,
                        help="hyperfine warmup runs per script (default: 2)")
    parser.add_argument("--json-out", type=str, default="",
                        help="optional path to write raw timings as JSON")
    args = parser.parse_args()

    if shutil.which("hyperfine") is None:
        print("error: hyperfine not found; install via `cargo install hyperfine`", file=sys.stderr)
        return 2

    root = Path(__file__).resolve().parent.parent
    bench_dir = root / "bench" / "typed_dispatch"
    pyrust = find_pyrust(root)

    print(f"# Typed-dispatch microbench (issue #399)", file=sys.stderr)
    print(f"# binary: {pyrust}", file=sys.stderr)
    print(f"# runs/warmup: {args.runs}/{args.warmup}", file=sys.stderr)

    timings = measure_all(pyrust, bench_dir, runs=args.runs, warmup=args.warmup)

    rows: list[tuple[Bench, float, float]] = []
    for bench in BENCHES:
        mean, std = per_call_ns(bench, timings)
        rows.append((bench, mean, std))

    print()
    print("Per-call cost (ns), loop overhead subtracted")
    print("=" * 80)
    print(render_table(rows))

    if args.json_out:
        out = {
            "binary": str(pyrust),
            "runs": args.runs,
            "warmup": args.warmup,
            "results": [
                {
                    "label": b.label,
                    "script": b.script,
                    "n": b.n,
                    "form": b.form,
                    "ns_per_call_mean": mean,
                    "ns_per_call_stddev": std,
                    "script_mean_s": timings[b.script]["mean"],
                    "noop_mean_s": timings[b.noop]["mean"],
                }
                for b, mean, std in rows
            ],
        }
        Path(args.json_out).write_text(json.dumps(out, indent=2))
        print(f"\nwrote {args.json_out}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
