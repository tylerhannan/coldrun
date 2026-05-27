#!/bin/bash
# Slice the first N rows of hits.parquet to a smaller file (needs DuckDB CLI).
#
# Usage:
#   ./scripts/sample-parquet.sh hits.parquet 1000000 hits-1m.parquet
#   ./scripts/sample-parquet.sh hits.parquet 100000   # writes hits-100k.parquet
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:?source parquet}"
ROWS="${2:?row count}"
OUT="${3:-${SRC%.parquet}-${ROWS}.parquet}"

if ! command -v duckdb >/dev/null 2>&1; then
  echo "duckdb CLI required (brew install duckdb)" >&2
  exit 1
fi
[ -f "$SRC" ] || { echo "missing: $SRC" >&2; exit 1; }

echo "sampling $ROWS rows from $SRC -> $OUT" >&2
duckdb -batch <<SQL
COPY (
  SELECT * FROM read_parquet('$SRC') LIMIT $ROWS
) TO '$OUT' (FORMAT PARQUET, COMPRESSION ZSTD);
SQL

du -sh "$OUT" | awk '{print "wrote", $2, "->", "'"$OUT"'"}' >&2
