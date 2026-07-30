#!/usr/bin/env bash
# Run a command under a wall-clock timeout and an address-space cap.
#
# A runaway interpreter build (unbounded native recursion, allocation storms
# from adversarial repros) can consume memory faster than the WSL2 VM can
# reclaim it and take down the whole VM — this has crashed the development box
# twice. Every ad-hoc pyrust/python invocation in agent workflows and repro
# scripts must go through this wrapper (the parity harness enforces the same
# limits internally).
#
# Usage: tools/run-limited.sh [-t SECONDS] [-m MEM_MB] -- command args...
set -euo pipefail

timeout_s=120
mem_mb=4096
while [ $# -gt 0 ]; do
    case "$1" in
        -t) timeout_s=$2; shift 2 ;;
        -m) mem_mb=$2; shift 2 ;;
        --) shift; break ;;
        *) break ;;
    esac
done
if [ $# -eq 0 ]; then
    echo "usage: run-limited.sh [-t SECONDS] [-m MEM_MB] -- command args..." >&2
    exit 2
fi

mem_kb=$((mem_mb * 1024))
exec timeout -k 5 "$timeout_s" bash -c '
    ulimit -v '"$mem_kb"' -d '"$mem_kb"' 2>/dev/null || true
    exec "$@"
' _ "$@"
