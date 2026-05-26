#!/bin/bash
# Full repro: build, load Parquet, run queries 1–5.
# Usage:
#   ./scripts/repro-local.sh              # requires ./hits.parquet
#   ./scripts/repro-local.sh path.parquet
#   ./scripts/smoke-demo.sh               # no Parquet; synthetic data only
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo build --release -p coldrun-cli
export COLDRUN_BIN="$ROOT/target/release/coldrun"

BENCH="$ROOT/clickbench/coldrun"
chmod +x "$BENCH"/* 2>/dev/null || true

PARQUET="${1:-hits.parquet}"
if [ ! -f "$PARQUET" ]; then
  echo "Usage: $0 [path/to/hits.parquet]"
  echo "Download: curl -LO https://datasets.clickhouse.com/hits_compatible/hits.parquet"
  echo "Or run:   ./scripts/smoke-demo.sh"
  exit 1
fi

export COLDRUN_DATA="$BENCH/.clickbench"
HITS_PARQUET="$PARQUET" "$BENCH/load"

echo "=== queries 1–5 ==="
head -n 5 "$BENCH/queries.sql" | while IFS= read -r q; do
  echo ">> $q"
  printf '%s\n' "$q" | COLDRUN_DATA="$COLDRUN_DATA" "$BENCH/query" 2>&1 | tail -n1
done

echo "Data size: $(COLDRUN_DATA="$COLDRUN_DATA" "$BENCH/data-size") bytes"
