#!/usr/bin/env bash
# PyRust benchmark runner — uses hyperfine for timing.
#
# Usage: bash tools/bench.sh [options]
#
# Options:
#   --release              use target/release/pyrust  (default: debug)
#   --base-bin PATH        also time a base-branch binary (PR-vs-base comparison)
#   --top N                rows shown in each PR-comment table (default: 15)
#   --measure-memory       also measure peak RSS via /usr/bin/time -v (Linux only)
#   --config PATH          bench.toml path (default: tools/bench.toml)
#   --pr-comment-out PATH  write Markdown PR comment (timing, top-N) to PATH
#   --pr-full-out PATH     write the complete (untruncated) timing report to PATH
#   --memory-comment-out PATH  write Markdown memory comment (top-N) to PATH
#   --memory-full-out PATH     write the complete (untruncated) memory report to PATH
#   --benchmark-action-out PATH
#   --svg-out PATH
#   --markdown-out PATH
#   --json-out PATH
#
# Environment:
#   PYRUST_BENCH_MEM_MB     address-space cap for every child (default 4096;
#                           0 disables)
#   PYRUST_BENCH_TIMEOUT_S  wall-clock cap per benchmark case (default 300;
#                           0 disables)
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
PR_FULL_OUT=""
MEMORY_COMMENT_OUT=""
MEMORY_FULL_OUT=""
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
    --pr-full-out)          PR_FULL_OUT="$2";           shift ;;
    --memory-comment-out)   MEMORY_COMMENT_OUT="$2";    shift ;;
    --memory-full-out)      MEMORY_FULL_OUT="$2";       shift ;;
    --benchmark-action-out) BENCHMARK_ACTION_OUT="$2";  shift ;;
    --svg-out)              SVG_OUT="$2";               shift ;;
    --markdown-out)         MARKDOWN_OUT="$2";          shift ;;
    --json-out)             JSON_OUT="$2";              shift ;;
    --measure-memory)       MEASURE_MEMORY=1 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
  shift
done

# ── resource caps ──────────────────────────────────────────────────────────────
# A runaway interpreter (allocation storm, unbounded native recursion) can eat
# memory faster than the host can reclaim it and take the whole box down — this
# has crashed the WSL2 development VM.  tools/run-limited.sh caps every ad-hoc
# invocation and the parity harness caps every fixture child; benchmark children
# were the remaining uncapped path.
#
# Benches must stay low-overhead, so we do NOT wrap each timed command (that
# would put an extra process in every measurement).  Instead the address-space
# cap is applied once to this shell: rlimits are inherited across fork/exec, so
# hyperfine, /usr/bin/time and every interpreter they spawn run under it at zero
# measurement cost.  The wall-clock cap goes around each hyperfine invocation,
# outside the timing loop.
BENCH_MEM_MB="${PYRUST_BENCH_MEM_MB:-4096}"
BENCH_TIMEOUT_S="${PYRUST_BENCH_TIMEOUT_S:-300}"

# Reject junk loudly: a typo'd override must not silently leave children
# uncapped, which is the exact failure these limits exist to prevent.
[[ "$BENCH_MEM_MB" =~ ^[0-9]+$ ]] || {
  echo "error: PYRUST_BENCH_MEM_MB must be a non-negative integer (got '$BENCH_MEM_MB')" >&2
  exit 2
}
[[ "$BENCH_TIMEOUT_S" =~ ^[0-9]+$ ]] || {
  echo "error: PYRUST_BENCH_TIMEOUT_S must be a non-negative integer (got '$BENCH_TIMEOUT_S')" >&2
  exit 2
}

CAP_DESC="disabled"
if [[ "$BENCH_MEM_MB" -gt 0 ]]; then
  BENCH_MEM_KB=$((BENCH_MEM_MB * 1024))
  # -v (RLIMIT_AS) and -d (RLIMIT_DATA) mirror tools/run-limited.sh.  Both are
  # attempted independently so a platform that only honours one still gets it.
  # A lower inherited limit (bench.sh run under run-limited.sh, say) cannot be
  # raised and must not be reported as a failure — read back what is actually in
  # force and describe the tighter of the two.
  ulimit -v "$BENCH_MEM_KB" 2>/dev/null || true
  ulimit -d "$BENCH_MEM_KB" 2>/dev/null || true
  EFFECTIVE_KB=""
  for cur in "$(ulimit -v)" "$(ulimit -d)"; do
    [[ "$cur" =~ ^[0-9]+$ ]] || continue          # "unlimited"
    if [[ -z "$EFFECTIVE_KB" ]] || [[ "$cur" -lt "$EFFECTIVE_KB" ]]; then
      EFFECTIVE_KB="$cur"
    fi
  done
  if [[ -n "$EFFECTIVE_KB" ]]; then
    CAP_DESC="$((EFFECTIVE_KB / 1024)) MiB"
  else
    echo "warning: could not lower the address-space rlimit; bench children run uncapped" >&2
  fi
fi

HYPERFINE_CMD=(hyperfine)
PROBE_CMD=()
TIMEOUT_DESC="disabled"
if [[ "$BENCH_TIMEOUT_S" -gt 0 ]]; then
  if command -v timeout >/dev/null 2>&1; then
    HYPERFINE_CMD=(timeout -k 5 "$BENCH_TIMEOUT_S" hyperfine)
    PROBE_CMD=(timeout -k 5 "$BENCH_TIMEOUT_S")
    TIMEOUT_DESC="${BENCH_TIMEOUT_S}s per case"
  else
    echo "warning: 'timeout' not found; bench cases run without a wall-clock cap" >&2
  fi
fi

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
mapfile -t SCRIPTS < <(find "$CASES_DIR/performance" -name "test_*.py" | sort)
[[ ${#SCRIPTS[@]} -gt 0 ]] || {
  echo "error: no test_*.py found under $CASES_DIR/performance" >&2
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
echo "  caps   : address space $CAP_DESC, wall clock $TIMEOUT_DESC"
echo ""

TIMED_OUT=()

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
    if "${PROBE_CMD[@]}" "$BASE_BIN" "$script" >/dev/null 2>&1; then
      CMD_NAMES+=(--command-name "base:$rel")
      CMDS+=("$BASE_BIN $script")
    else
      echo "  [skip base] $rel (base binary returned non-zero)"
    fi
  fi

  status=0
  "${HYPERFINE_CMD[@]}" \
    --warmup   "$warmup" \
    --runs     "$iters"  \
    --style    full      \
    --export-json "$RESULTS_DIR/$name.json" \
    "${CMD_NAMES[@]}"   \
    "${CMDS[@]}" || status=$?

  if [[ "$status" -ne 0 ]]; then
    # 124 = `timeout` fired; 137 = the follow-up SIGKILL landed.
    if [[ "$status" -eq 124 ]] || [[ "$status" -eq 137 ]]; then
      echo "  [timeout] $rel exceeded ${BENCH_TIMEOUT_S}s and was killed" \
           "(raise PYRUST_BENCH_TIMEOUT_S if legitimate)" >&2
      # A killed hyperfine leaves a partial/absent export; drop it so the
      # report never averages a truncated run.
      rm -f "$RESULTS_DIR/$name.json"
      TIMED_OUT+=("$rel")
      continue
    fi
    echo "error: hyperfine failed for $rel (exit $status)" >&2
    exit "$status"
  fi
done

# Announce timeouts before the report runs, so the diagnosis survives even when
# the report itself bails out (e.g. every case timed out and there is nothing
# left to report on).
if [[ ${#TIMED_OUT[@]} -gt 0 ]]; then
  echo "" >&2
  echo "error: ${#TIMED_OUT[@]} case(s) exceeded the ${BENCH_TIMEOUT_S}s cap and are excluded from the report:" >&2
  printf '  %s\n' "${TIMED_OUT[@]}" >&2
  echo "" >&2
fi

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
        kb=$( { "${PROBE_CMD[@]}" /usr/bin/time -v "$bin" "$script" >/dev/null; } 2>&1 \
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
[[ -n "$PR_FULL_OUT"          ]] && REPORT_ARGS+=(--pr-full-out          "$PR_FULL_OUT")
[[ -n "$MEMORY_COMMENT_OUT"   ]] && REPORT_ARGS+=(--memory-comment-out   "$MEMORY_COMMENT_OUT")
[[ -n "$MEMORY_FULL_OUT"      ]] && REPORT_ARGS+=(--memory-full-out      "$MEMORY_FULL_OUT")
[[ -n "$BENCHMARK_ACTION_OUT" ]] && REPORT_ARGS+=(--benchmark-action-out "$BENCHMARK_ACTION_OUT")
[[ -n "$SVG_OUT"              ]] && REPORT_ARGS+=(--svg-out              "$SVG_OUT")
[[ -n "$MARKDOWN_OUT"         ]] && REPORT_ARGS+=(--markdown-out         "$MARKDOWN_OUT")
[[ -n "$JSON_OUT"             ]] && REPORT_ARGS+=(--json-out             "$JSON_OUT")

"$PYTHON" "$REPORT_PY" "${REPORT_ARGS[@]}"

# Reports are written first so a partial run still produces usable output; the
# timeout is then surfaced as a failure, because a case that outlives its cap is
# a bug and must not pass silently.
if [[ ${#TIMED_OUT[@]} -gt 0 ]]; then
  exit 1
fi
