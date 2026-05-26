#!/bin/bash
# Time ClickBench queries 1–10 on demo data (no Parquet download).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo build --release -p coldrun-cli -q

BIN="$ROOT/target/release/coldrun"
DATA="${COLDRUN_DATA:-$ROOT/.coldrun-bench}"
ROWS="${1:-100000}"

rm -rf "$DATA"
"$BIN" --data-dir "$DATA" local --demo "$ROWS" >/dev/null

echo "=== bench demo (${ROWS} rows, queries 1–10) ==="
printf "%-4s %10s  %s\n" "Q" "seconds" "query"
i=1
while IFS= read -r q; do
  [ "$i" -gt 10 ] && break
  out=$(printf '%s\n' "$q" | "$BIN" --data-dir "$DATA" local 2>&1)
  timing=$(printf '%s\n' "$out" | grep -E '^[0-9]+(\.[0-9]+)?$' | tail -n1 || echo "?")
  short=$(echo "$q" | cut -c1-60)
  printf "%-4s %10s  %s\n" "$i" "$timing" "$short"
  i=$((i + 1))
done < "$ROOT/clickbench/coldrun/queries.sql"

du -sh "$DATA" 2>/dev/null | awk '{print "data dir:", $1}'
