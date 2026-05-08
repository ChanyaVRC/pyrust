#!/usr/bin/env python3
"""
Build the benchmark GitHub Pages site.

Appends a new snapshot to history.json (in place), then generates
index.html with a Chart.js performance-over-time graph.

Usage:
    python tools/build_benchmark_pages.py \
        --snapshot  benchmark-snapshot.json \
        --history   /path/to/history.json \
        --svg       benchmark.svg \
        --out-html  dist/index.html \
        --max-entries 200
"""
import argparse
import json
import math
from pathlib import Path


def load_history(path: Path) -> list:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return []


def append_snapshot(history: list, snapshot: dict, max_entries: int) -> list:
    commit = snapshot.get("commit", "")
    history = [e for e in history if e.get("commit") != commit]
    history.append(snapshot)
    if max_entries > 0:
        history = history[-max_entries:]
    return history


def _hsl(index: int, total: int) -> str:
    h = int(360 * index / max(total, 1))
    return f"hsl({h}, 70%, 60%)"


def build_html(history: list, has_svg: bool) -> str:
    if not history:
        return (
            "<!doctype html><html><body style='background:#0b1220;color:#e2e8f0'>"
            "<p style='padding:2rem'>No benchmark data yet.</p></body></html>\n"
        )

    labels = [e.get("commit_short", e.get("commit", "")[:12]) for e in history]
    timestamps = [e.get("timestamp", "") for e in history]
    total_py = [e.get("total_py_ms") for e in history]
    total_rs = [e.get("total_rs_ms") for e in history]

    # Collect script paths in order of first appearance
    seen: set[str] = set()
    script_paths: list[str] = []
    for entry in history:
        for s in entry.get("scripts", []):
            p = s["path"]
            if p not in seen:
                seen.add(p)
                script_paths.append(p)

    per_script: dict[str, list] = {
        p: [
            next((s["rs_avg_ms"] for s in e.get("scripts", []) if s["path"] == p), None)
            for e in history
        ]
        for p in script_paths
    }

    overall_datasets = json.dumps([
        {
            "label": "Python total avg (ms)",
            "data": total_py,
            "borderColor": "#60a5fa",
            "backgroundColor": "rgba(96,165,250,0.08)",
            "tension": 0.3,
            "pointRadius": 3,
            "fill": True,
        },
        {
            "label": "PyRust total avg (ms)",
            "data": total_rs,
            "borderColor": "#34d399",
            "backgroundColor": "rgba(52,211,153,0.08)",
            "tension": 0.3,
            "pointRadius": 3,
            "fill": True,
        },
    ])

    script_datasets = json.dumps([
        {
            "label": p.split("/")[-1],
            "data": per_script[p],
            "borderColor": _hsl(i, len(script_paths)),
            "backgroundColor": "transparent",
            "tension": 0.3,
            "pointRadius": 3,
            "hidden": True,
        }
        for i, p in enumerate(script_paths)
    ])

    svg_section = (
        '\n  <section class="card">\n'
        '    <h2>Latest Snapshot</h2>\n'
        '    <img src="./benchmark.svg" alt="latest benchmark" '
        'style="width:100%;height:auto;border-radius:8px;" />\n'
        "  </section>"
        if has_svg else ""
    )

    n = len(history)
    return f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>PyRust Benchmark History</title>
  <script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.3/dist/chart.umd.min.js"></script>
  <style>
    *{{box-sizing:border-box;margin:0;padding:0}}
    body{{background:#0b1220;color:#e2e8f0;font-family:ui-sans-serif,-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif;padding:24px}}
    h1{{font-size:1.75rem;font-weight:700;margin-bottom:4px;color:#f1f5f9}}
    h2{{font-size:1rem;font-weight:600;color:#94a3b8;margin-bottom:16px}}
    .sub{{color:#64748b;margin-bottom:24px;font-size:.875rem}}
    .wrap{{max-width:1280px;margin:0 auto}}
    .card{{background:#111827;border:1px solid #1e293b;border-radius:12px;padding:24px;margin-bottom:24px}}
    .chart-wrap{{position:relative;height:320px}}
  </style>
</head>
<body>
<div class="wrap">
  <h1>PyRust Benchmark History</h1>
  <p class="sub">{n} data point{'s' if n != 1 else ''} &middot; auto-updated on every push to master</p>

  <section class="card">
    <h2>Overall: Python vs PyRust (total avg ms)</h2>
    <div class="chart-wrap"><canvas id="overall"></canvas></div>
  </section>

  <section class="card">
    <h2>Per-script PyRust avg (ms) &mdash; click legend to toggle</h2>
    <div class="chart-wrap"><canvas id="per-script"></canvas></div>
  </section>
{svg_section}
</div>
<script>
const labels = {json.dumps(labels)};
const timestamps = {json.dumps(timestamps)};

const base = {{
  responsive: true,
  maintainAspectRatio: false,
  interaction: {{mode:'index', intersect:false}},
  plugins: {{
    legend: {{labels: {{color:'#94a3b8', boxWidth:14}}}},
    tooltip: {{
      callbacks: {{
        title(items) {{
          const i = items[0].dataIndex;
          return `${{labels[i]}}  ${{timestamps[i]}}`;
        }}
      }}
    }}
  }},
  scales: {{
    x: {{ticks:{{color:'#64748b', maxRotation:45, autoSkip:true, maxTicksLimit:24}}, grid:{{color:'#1e293b'}}}},
    y: {{ticks:{{color:'#64748b'}}, grid:{{color:'#1e293b'}}, beginAtZero:true}}
  }}
}};

new Chart(document.getElementById('overall'), {{
  type:'line', data:{{labels, datasets:{overall_datasets}}}, options:base
}});
new Chart(document.getElementById('per-script'), {{
  type:'line', data:{{labels, datasets:{script_datasets}}}, options:base
}});
</script>
</body>
</html>
"""


def main() -> int:
    parser = argparse.ArgumentParser(description="Build benchmark Pages site from history")
    parser.add_argument("--snapshot", required=True, help="path to benchmark-snapshot.json")
    parser.add_argument("--history", required=True, help="path to history.json (updated in place)")
    parser.add_argument("--svg", default="", help="path to benchmark.svg (referenced in page)")
    parser.add_argument("--out-html", required=True, help="output path for index.html")
    parser.add_argument("--max-entries", type=int, default=200, help="max history entries to keep")
    args = parser.parse_args()

    snapshot = json.loads(Path(args.snapshot).read_text(encoding="utf-8"))
    history_path = Path(args.history)
    history = load_history(history_path)
    history = append_snapshot(history, snapshot, args.max_entries)
    history_path.write_text(json.dumps(history, indent=2), encoding="utf-8")

    has_svg = bool(args.svg) and Path(args.svg).exists()
    html = build_html(history, has_svg)
    out = Path(args.out_html)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    print(f"Written {out}  ({len(history)} history entries)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
