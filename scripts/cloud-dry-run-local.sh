#!/bin/bash
# Local ClickBench harness dry-run on 1M parquet (no AWS). Logs to logs/benchmarks/cloud-dry-run.log
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
LOG="$ROOT/logs/benchmarks/cloud-dry-run.log"
mkdir -p "$ROOT/logs/benchmarks"

exec > >(tee -a "$LOG") 2>&1
echo "=== cloud dry-run local $(date -u +%Y-%m-%dT%H:%M:%SZ) ==="

cargo build --release -p coldrun-cli -q

export COLDRUN_ROOT="$ROOT"
export COLDRUN_BIN="$ROOT/target/release/coldrun"
export COLDRUN_DATA="$ROOT/.coldrun-cloud-dry"
PARQUET="$ROOT/data/hits-1m.parquet"

[ -f "$PARQUET" ] || { echo "missing $PARQUET"; exit 1; }

echo "=== load 1M ==="
start_t=$(date +%s.%N)
HITS_PARQUET="$PARQUET" "$ROOT/clickbench/coldrun/load"
end_t=$(date +%s.%N)
load_s=$(awk -v s="$start_t" -v e="$end_t" 'BEGIN { printf "%.3f", e - s }')
size=$(COLDRUN_DATA="$COLDRUN_DATA" "$ROOT/clickbench/coldrun/data-size")
echo "Load time: ${load_s}s"
echo "Data size: ${size} bytes"

BENCH="$ROOT/clickbench/coldrun"
chmod +x "$BENCH"/*.sh "$BENCH"/query "$BENCH"/lib/*.sh 2>/dev/null || true

echo "=== start serve ==="
COLDRUN_DATA="$COLDRUN_DATA" "$BENCH/start"
sleep 1
COLDRUN_DATA="$COLDRUN_DATA" "$BENCH/check" && echo "check: ok"

echo "=== bench-clickbench embedded (43 queries, warm) ==="
COLDRUN_DATA="$COLDRUN_DATA" \
  COLDRUN_SKIP_LOAD=1 \
  COLDRUN_SKIP_BUILD=1 \
  BENCH_RESTARTABLE=no \
  BENCH_PRINT_HOT_SUMMARY=1 \
  "$ROOT/scripts/bench-clickbench.sh" --skip-load --embedded 2>&1 | tail -60

echo "=== stop serve ==="
COLDRUN_DATA="$COLDRUN_DATA" "$BENCH/stop" 2>/dev/null || true

echo "=== done $(date -u +%Y-%m-%dT%H:%M:%SZ) ==="
