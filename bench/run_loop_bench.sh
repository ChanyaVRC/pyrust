#!/usr/bin/env bash
# Loop benchmark driver — uses hyperfine for high-precision timing.
#
# Usage:   run_loop_bench.sh <pyrust-binary> [--export-json out.json]
# Each case under bench/cases/<name>.py is sized so its body runs for >=500ms
# under pyrust release; hyperfine reports mean ± σ and min in microseconds,
# with the noop case providing a startup baseline to subtract for the body time.

set -euo pipefail

BIN="${1:-}"
shift || true
if [ -z "$BIN" ]; then
    echo "usage: $0 <pyrust-binary> [hyperfine-extra-args...]" >&2
    exit 2
fi

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
CASES_DIR="$BENCH_DIR/cases"

CASES=(
    noop
    for_range
    for_range_const
    for_list_int
    for_tuple_int
    while
    nested
    enumerate
    dict_items
)

ARGS=()
for c in "${CASES[@]}"; do
    ARGS+=( --command-name "$c" "$BIN $CASES_DIR/$c.py" )
done

hyperfine \
    --warmup 1 \
    --min-runs 5 \
    --time-unit microsecond \
    --shell=none \
    "$@" \
    "${ARGS[@]}"
