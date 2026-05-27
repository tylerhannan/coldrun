#!/bin/bash
# ClickBench-format output from repo root (no upstream ClickBench checkout required).
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="${COLDRUN_ROOT:-$(cd "$DIR/../.." && pwd)}"
export COLDRUN_ROOT="$ROOT"
export COLDRUN_BIN="${COLDRUN_BIN:-$ROOT/target/release/coldrun}"
export COLDRUN_DATA="${COLDRUN_DATA:-$DIR/.clickbench}"
export COLDRUN_HOST="${COLDRUN_HOST:-127.0.0.1:9000}"
export BENCH_RESTARTABLE="${BENCH_RESTARTABLE:-yes}"
export BENCH_DURABLE="${BENCH_DURABLE:-yes}"
export BENCH_TRIES="${BENCH_TRIES:-3}"

cd "$DIR"
# shellcheck source=lib/bench-common.sh
source "$DIR/lib/bench-common.sh"

if [ "${COLDRUN_SKIP_BUILD:-0}" != "1" ]; then
  [ -x "$COLDRUN_BIN" ] || "$DIR/install"
fi

if [ "${COLDRUN_SKIP_LOAD:-0}" = "1" ]; then
  echo "Load time: 0.000"
else
  start_t=$(date +%s.%N)
  ./load >/dev/null
  sync 2>/dev/null || true
  end_t=$(date +%s.%N)
  awk -v s="$start_t" -v e="$end_t" 'BEGIN { printf "Load time: %.3f\n", e - s }'
fi

size=$(./data-size 2>/dev/null || echo 0)
echo "Data size: $size"

./start >/dev/null 2>&1 || true
bench_check_loop

bench_main

./stop >/dev/null 2>&1 || true
