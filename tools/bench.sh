#!/usr/bin/env bash
# PyRust benchmark runner — uses hyperfine for timing.
#
# Usage: bash tools/bench.sh [options]
#
# Options:
#   --release              use target/release/pyrust  (default: debug)
#   --base-bin PATH        also time a base-branch binary (PR-vs-base comparison)
#   --top N                rows shown in each table (default: 15)
#   --measure-memory       also measure peak RSS via /usr/bin/time -v (Linux only)
#   --config PATH          bench.toml path (default: tools/bench.toml)
#   --pr-comment-out PATH  write Markdown PR comment to PATH
#   --benchmark-action-out PATH
#   --svg-out PATH
#   --markdown-out PATH
#   --json-out PATH
#
# Requires: hyperfine  (cargo install hyperfine  or  apt install hyperfine)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPORT_PY="$SCRIPT_DIR/bench_report.py"

# ── argument defaults ──────────────────────────────────────────────────────────
RELEASE=0
BASE_BIN=""
TOP=15
CONFIG="$SCRIPT_DIR/bench.toml"
PR_COMMENT_OUT=""
MEMORY_COMMENT_OUT=""
BENCHMARK_ACTION_OUT=""
SVG_OUT=""
MARKDOWN_OUT=""
JSON_OUT=""
MEASURE_MEMORY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)              RELEASE=1 ;;
    --base-bin)             BASE_BIN="$2";              shift ;;
    --top)                  TOP="$2";                   shift ;;
    --config)               CONFIG="$2";                shift ;;
    --pr-comment-out)       PR_COMMENT_OUT="$2";        shift ;;
    --memory-comment-out)   MEMORY_COMMENT_OUT="$2";    shift ;;
    --benchmark-action-out) BENCHMARK_ACTION_OUT="$2";  shift ;;
    --svg-out)              SVG_OUT="$2";               shift ;;
    --markdown-out)         MARKDOWN_OUT="$2";          shift ;;
    --json-out)             JSON_OUT="$2";              shift ;;
    --measure-memory)       MEASURE_MEMORY=1 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
  shift
done

# ── resolve binaries ───────────────────────────────────────────────────────────
PYTHON="${PYRUST_PY_BIN:-python3}"

if [[ -n "${PYRUST_BIN:-}" ]]; then
  PYRUST="$PYRUST_BIN"
elif [[ "$RELEASE" -eq 1 ]]; then
  PYRUST="$ROOT/target/release/pyrust"
else
  PYRUST="$ROOT/target/debug/pyrust"
fi

[[ -x "$PYRUST" ]] || {
  echo "error: PyRust binary not found: $PYRUST" >&2
  [[ "$RELEASE" -eq 1 ]] \
    && echo "       run: cargo build --release" >&2 \
    || echo "       run: cargo build" >&2
  exit 2
}

if [[ -n "$BASE_BIN" ]] && [[ ! -x "$BASE_BIN" ]]; then
  echo "error: base binary not found: $BASE_BIN" >&2
  exit 2
fi

command -v hyperfine >/dev/null 2>&1 || {
  echo "error: hyperfine not found" >&2
  echo "       install: cargo install hyperfine" >&2
  exit 2
}

command -v jq >/dev/null 2>&1 || {
  echo "error: jq not found" >&2
  echo "       install: apt install jq  or  brew install jq" >&2
  exit 2
}

# ── load per-script configs (one python call → JSON) ──────────────────────────
ALL_CONFIGS=$("$PYTHON" "$REPORT_PY" --dump-configs --config "$CONFIG")

# ── collect test scripts ───────────────────────────────────────────────────────
CASES_DIR="$ROOT/crates/pyrust/tests/cases"
mapfile -t SCRIPTS < <(find "$CASES_DIR" -name "test_*.py" | sort)
[[ ${#SCRIPTS[@]} -gt 0 ]] || {
  echo "error: no test_*.py found under $CASES_DIR" >&2
  exit 2
}

# ── run hyperfine per script ───────────────────────────────────────────────────
RESULTS_DIR=$(mktemp -d)
trap 'rm -rf "$RESULTS_DIR"' EXIT

echo "PyRust benchmark"
echo "  pyrust : $PYRUST"
echo "  python : $PYTHON"
[[ -n "$BASE_BIN" ]] && echo "  base   : $BASE_BIN"
echo "  scripts: ${#SCRIPTS[@]}"
echo ""

for script in "${SCRIPTS[@]}"; do
  rel="${script#"$CASES_DIR/"}"
  name="${rel//\//__}"     # "language/test_foo.py" → "language__test_foo.py"
  name="${name%.py}"

  iters=$(  jq -r --arg r "$rel" '.[$r].iterations' <<< "$ALL_CONFIGS")
  warmup=$( jq -r --arg r "$rel" '.[$r].warmup'     <<< "$ALL_CONFIGS")

  CMD_NAMES=(
    --command-name "python:$rel"
    --command-name "pyrust:$rel"
  )
  CMDS=("$PYTHON $script" "$PYRUST $script")

  # Probe the base binary before benchmarking — skip it for scripts that
  # exercise features not yet on the base branch (e.g. tests added in this PR).
  if [[ -n "$BASE_BIN" ]]; then
    if "$BASE_BIN" "$script" >/dev/null 2>&1; then
      CMD_NAMES+=(--command-name "base:$rel")
      CMDS+=("$BASE_BIN $script")
    else
      echo "  [skip base] $rel (base binary returned non-zero)"
    fi
  fi

  hyperfine \
    --warmup   "$warmup" \
    --runs     "$iters"  \
    --style    full      \
    --export-json "$RESULTS_DIR/$name.json" \
    "${CMD_NAMES[@]}"   \
    "${CMDS[@]}"
done

# ── measure peak RSS (Linux only, requires /usr/bin/time -v) ──────────────────
MEMORY_DIR=""
if [[ "$MEASURE_MEMORY" -eq 1 ]]; then
  if ! /usr/bin/time --version 2>&1 | grep -q "GNU"; then
    echo "  [memory] /usr/bin/time -v not available (GNU time required); skipping" >&2
    MEASURE_MEMORY=0
  else
    MEMORY_DIR=$(mktemp -d)
    echo "Measuring peak RSS..."
    for script in "${SCRIPTS[@]}"; do
      rel="${script#"$CASES_DIR/"}"
      name="${rel//\//__}"
      name="${name%.py}"

      # Extract "Maximum resident set size (kbytes): N" from GNU time stderr.
      # Redirect only stdout to /dev/null; stderr (time's report + any script
      # error output) goes through the pipe and grep filters to just the RSS line.
      # Do NOT use "2>/dev/null" inside the block — that would also suppress
      # /usr/bin/time's own output which it writes to its stderr.
      _mem_kb() {
        local bin="$1"
        local kb
        kb=$( { /usr/bin/time -v "$bin" "$script" >/dev/null; } 2>&1 \
                | grep -i "Maximum resident" | awk '{print $NF}' )
        printf '%s' "${kb:-null}"
      }

      py_kb=$(_mem_kb "$PYTHON")
      rs_kb=$(_mem_kb "$PYRUST")
      base_kb="null"
      if [[ -n "$BASE_BIN" ]] && [[ -x "$BASE_BIN" ]]; then
        base_kb=$(_mem_kb "$BASE_BIN")
      fi

      printf '{"rel":"%s","py_kb":%s,"rs_kb":%s,"base_kb":%s}\n' \
        "$rel" "$py_kb" "$rs_kb" "$base_kb" \
        > "$MEMORY_DIR/$name.json"
    done
    echo ""
  fi
fi

# ── generate reports ───────────────────────────────────────────────────────────
REPORT_ARGS=(
  --results-dir "$RESULTS_DIR"
  --config      "$CONFIG"
  --top         "$TOP"
)
[[ -n "$MEMORY_DIR" ]] && REPORT_ARGS+=(--memory-dir "$MEMORY_DIR")
[[ -n "$PR_COMMENT_OUT"       ]] && REPORT_ARGS+=(--pr-comment-out       "$PR_COMMENT_OUT")
[[ -n "$MEMORY_COMMENT_OUT"   ]] && REPORT_ARGS+=(--memory-comment-out   "$MEMORY_COMMENT_OUT")
[[ -n "$BENCHMARK_ACTION_OUT" ]] && REPORT_ARGS+=(--benchmark-action-out "$BENCHMARK_ACTION_OUT")
[[ -n "$SVG_OUT"              ]] && REPORT_ARGS+=(--svg-out              "$SVG_OUT")
[[ -n "$MARKDOWN_OUT"         ]] && REPORT_ARGS+=(--markdown-out         "$MARKDOWN_OUT")
[[ -n "$JSON_OUT"             ]] && REPORT_ARGS+=(--json-out             "$JSON_OUT")

"$PYTHON" "$REPORT_PY" "${REPORT_ARGS[@]}"
