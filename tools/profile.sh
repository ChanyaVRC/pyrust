#!/usr/bin/env bash
# PyRust profiler harness — build an optimized-with-symbols binary and run it
# under a sampling / instrumentation profiler on a target script.
#
# The `profiling` Cargo profile (see the workspace Cargo.toml) is codegen-
# identical to `release` — same LTO and codegen-units, so the hot paths inline
# exactly as they ship — but keeps DWARF debug info and skips symbol stripping,
# so the profiler can name real functions and source lines.
#
# Usage:
#   tools/profile.sh [options] <script.py> [script-args...]
#
# Options:
#   --tool perf|callgrind|cachegrind|flamegraph   profiler to use (default: perf)
#   --no-build                                     reuse target/profiling/pyrust as-is
#   --out DIR                                      output directory (default: target/profile-out)
#   --freq HZ                                      perf sampling frequency (default: 4000)
#   -h, --help                                     show this help
#
# Examples:
#   tools/profile.sh crates/pyrust/tests/cases/performance/test_while_cmp.py
#   tools/profile.sh --tool callgrind path/to/script.py
#   tools/profile.sh --tool flamegraph crates/pyrust/tests/cases/performance/test_range_loop.py
#
# Profiler install hints:
#   perf        : linux-tools (Debian/Ubuntu: `apt install linux-tools-common linux-tools-$(uname -r)`)
#   valgrind    : `apt install valgrind`  (callgrind/cachegrind are valgrind tools)
#   flamegraph  : `cargo install flamegraph`  (wraps perf; needs perf installed too)
#
# WSL note: `perf` is frequently unavailable or unprivileged under WSL2. If perf
# fails, prefer `--tool callgrind` (valgrind works in WSL) — it is deterministic
# (instruction counts, not wall-clock) so it is also immune to the NTFS timing
# noise documented in CLAUDE.md.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TOOL="perf"
BUILD=1
OUT="$ROOT/target/profile-out"
FREQ=4000

die() { echo "profile.sh: $*" >&2; exit 1; }

# ── parse arguments ─────────────────────────────────────────────────────────
POSITIONAL=()
while [ $# -gt 0 ]; do
  case "$1" in
    --tool)       TOOL="${2:?--tool needs a value}"; shift 2 ;;
    --no-build)   BUILD=0; shift ;;
    --out)        OUT="${2:?--out needs a value}"; shift 2 ;;
    --freq)       FREQ="${2:?--freq needs a value}"; shift 2 ;;
    -h|--help)    sed -n '2,45p' "$0"; exit 0 ;;
    --)           shift; while [ $# -gt 0 ]; do POSITIONAL+=("$1"); shift; done ;;
    -*)           die "unknown option: $1 (see --help)" ;;
    *)            POSITIONAL+=("$1"); shift ;;
  esac
done

case "$TOOL" in
  perf|callgrind|cachegrind|flamegraph) ;;
  *) die "unknown --tool '$TOOL' (expected: perf | callgrind | cachegrind | flamegraph)" ;;
esac

[ "${#POSITIONAL[@]}" -ge 1 ] || die "no target script given (see --help)"
SCRIPT="${POSITIONAL[0]}"
SCRIPT_ARGS=("${POSITIONAL[@]:1}")
[ -f "$SCRIPT" ] || die "script not found: $SCRIPT"

need() { command -v "$1" >/dev/null 2>&1 || die "'$1' not found on PATH. $2"; }

BIN="$ROOT/target/profiling/pyrust"

# ── build the profiling binary ──────────────────────────────────────────────
if [ "$BUILD" -eq 1 ]; then
  echo "==> cargo build --profile profiling" >&2
  ( cd "$ROOT" && cargo build --profile profiling --bin pyrust )
fi
[ -x "$BIN" ] || die "profiling binary missing: $BIN (drop --no-build to build it)"

mkdir -p "$OUT"
STEM="$(basename "$SCRIPT" .py)"

echo "==> tool=$TOOL  bin=$BIN  script=$SCRIPT  out=$OUT" >&2

case "$TOOL" in
  perf)
    need perf "Install linux-tools (see header). Under WSL try --tool callgrind."
    DATA="$OUT/perf-$STEM.data"
    perf record -F "$FREQ" --call-graph dwarf -o "$DATA" -- "$BIN" "$SCRIPT" "${SCRIPT_ARGS[@]}"
    echo "==> perf report (top functions):" >&2
    perf report -i "$DATA" --stdio --percent-limit 1 | sed -n '1,40p'
    echo "==> full data: $DATA  (interactive: perf report -i $DATA)" >&2
    ;;

  flamegraph)
    need flamegraph "Install with: cargo install flamegraph (also needs perf)."
    SVG="$OUT/flamegraph-$STEM.svg"
    ( cd "$ROOT" && flamegraph -o "$SVG" --freq "$FREQ" -- "$BIN" "$SCRIPT" "${SCRIPT_ARGS[@]}" )
    echo "==> flamegraph: $SVG" >&2
    ;;

  callgrind)
    need valgrind "Install with: apt install valgrind."
    DATA="$OUT/callgrind-$STEM.out"
    valgrind --tool=callgrind --callgrind-out-file="$DATA" \
      "$BIN" "$SCRIPT" "${SCRIPT_ARGS[@]}"
    echo "==> callgrind data: $DATA" >&2
    if command -v callgrind_annotate >/dev/null 2>&1; then
      echo "==> callgrind_annotate (top by instruction count):" >&2
      callgrind_annotate "$DATA" | sed -n '1,40p'
    else
      echo "==> view with: callgrind_annotate $DATA   (or kcachegrind $DATA)" >&2
    fi
    ;;

  cachegrind)
    need valgrind "Install with: apt install valgrind."
    DATA="$OUT/cachegrind-$STEM.out"
    valgrind --tool=cachegrind --cachegrind-out-file="$DATA" \
      "$BIN" "$SCRIPT" "${SCRIPT_ARGS[@]}"
    echo "==> cachegrind data: $DATA" >&2
    command -v cg_annotate >/dev/null 2>&1 && cg_annotate "$DATA" | sed -n '1,30p' || true
    ;;

  *)
    die "unknown --tool '$TOOL' (expected: perf | callgrind | cachegrind | flamegraph)"
    ;;
esac

echo "==> done." >&2
