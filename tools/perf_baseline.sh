#!/usr/bin/env bash
# Performance baseline harness (F-G-04 / NFR-06).
#
# Runs a command N times and reports median/min/max wall time in
# milliseconds. Numbers go to stdout, progress to stderr, so CI can
# capture the metrics cleanly.
#
# Usage:
#   tools/perf_baseline.sh <runs> <command> [args...]
#
# Current baseline target (M0): incremental no-op release build of the
# workspace, i.e. one warm-up build first, then timed rebuilds:
#   cargo build --release --workspace
#   tools/perf_baseline.sh 5 cargo build --release --workspace
# Once milestone M2 lands the run subcommand, the recorded baseline
# becomes a fixed sample configuration executed N times; the command
# surface of this script stays the same.
#
# The median is the lower median (element at position int((N+1)/2) of
# the sorted samples), which stays stable for even N.

set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <runs> <command> [args...]" >&2
    exit 2
fi

runs="$1"
shift

if ! [[ "$runs" =~ ^[0-9]+$ ]] || [ "$runs" -lt 1 ]; then
    echo "error: runs must be a positive integer, got '$runs'" >&2
    exit 2
fi

times_ms=()
for i in $(seq 1 "$runs"); do
    echo "run $i/$runs: $*" >&2
    start=$(date +%s%N)
    if ! "$@" >/dev/null 2>&1; then
        echo "error: baseline command failed: $*" >&2
        exit 1
    fi
    end=$(date +%s%N)
    times_ms+=( $(( (end - start) / 1000000 )) )
done

sorted=$(printf '%s\n' "${times_ms[@]}" | sort -n)
count=${#times_ms[@]}
median=$(echo "$sorted" | awk -v c="$count" 'NR == int((c + 1) / 2)')
min=$(echo "$sorted" | head -n 1)
max=$(echo "$sorted" | tail -n 1)

echo "command: $*"
echo "runs: $count"
echo "median_ms: $median"
echo "min_ms: $min"
echo "max_ms: $max"
