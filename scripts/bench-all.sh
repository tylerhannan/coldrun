#!/bin/bash
# Time all 43 ClickBench queries on demo data (no Parquet download).
# Commit snapshots: docs/benchmarks/demo-100k/latest.md
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export PATH="${HOME}/.cargo/bin:${PATH}"

cargo build --release -p coldrun-cli -q

BIN="$ROOT/target/release/coldrun"
DATA="${COLDRUN_DATA:-$ROOT/.coldrun-bench-all}"
ROWS="${1:-100000}"

rm -rf "$DATA"
"$BIN" --data-dir "$DATA" local --demo "$ROWS" >/dev/null

echo "=== bench all 43 queries (demo, ${ROWS} rows) ==="
printf "%-4s %10s  %s\n" "Q" "seconds" "query"

i=1
while IFS= read -r q; do
  [ -z "$q" ] && continue
  out=$(printf '%s\n' "$q" | "$BIN" --data-dir "$DATA" local 2>&1) || true
  timing=$(printf '%s\n' "$out" | grep -E '^[0-9]+(\.[0-9]+)?$' | tail -n1 || echo "ERR")
  short=$(echo "$q" | cut -c1-55)
  printf "%-4s %10s  %s\n" "$i" "$timing" "$short"
  i=$((i + 1))
done < "$ROOT/clickbench/coldrun/queries.sql"

du -sh "$DATA" 2>/dev/null | awk '{print "data dir:", $1}'
