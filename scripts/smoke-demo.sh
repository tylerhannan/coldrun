#!/bin/bash
# MVP smoke: queries 1–10 on synthetic data (no Parquet download).
# See docs/SMOKE-DEMO.md
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo build --release -p coldrun-cli

BIN="$ROOT/target/release/coldrun"
DATA="${COLDRUN_DATA:-$ROOT/.coldrun-demo}"
ROWS="${1:-10000}"

rm -rf "$DATA"
"$BIN" --data-dir "$DATA" local --demo "$ROWS"

echo "=== ClickBench queries 1–15 (demo, ${ROWS} rows) ==="
i=1
while IFS= read -r q; do
  [ "$i" -gt 15 ] && break
  echo ">> Q$i: $q"
  printf '%s\n' "$q" | "$BIN" --data-dir "$DATA" local 2>&1 | tail -n 3
  i=$((i + 1))
done < "$ROOT/clickbench/coldrun/queries.sql"
