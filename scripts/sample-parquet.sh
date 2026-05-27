#!/bin/bash
# Slice the first N rows of hits.parquet to a smaller file (needs DuckDB CLI).
#
# Usage:
#   ./scripts/sample-parquet.sh hits.parquet 1000000 hits-1m.parquet
#   ./scripts/sample-parquet.sh hits.parquet 100000   # writes hits-100k.parquet
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:?source parquet path or https URL}"
ROWS="${2:?row count}"
OUT="${3:-}"

if ! command -v duckdb >/dev/null 2>&1; then
  echo "duckdb CLI required (brew install duckdb)" >&2
  exit 1
fi

if [ -z "$OUT" ]; then
  if [[ "$SRC" == https://* ]]; then
    OUT="hits-${ROWS}.parquet"
  else
    OUT="${SRC%.parquet}-${ROWS}.parquet"
  fi
fi

if [[ "$SRC" == https://* ]]; then
  PARQUET_SRC="$SRC"
else
  [ -f "$SRC" ] || { echo "missing: $SRC" >&2; exit 1; }
  PARQUET_SRC="$(cd "$(dirname "$SRC")" && pwd)/$(basename "$SRC")"
fi

echo "sampling $ROWS rows from $SRC -> $OUT" >&2
duckdb -batch <<SQL
COPY (
  SELECT * FROM read_parquet('$PARQUET_SRC') LIMIT $ROWS
) TO '$OUT' (FORMAT PARQUET, COMPRESSION ZSTD);
SQL

du -sh "$OUT" | awk '{print "wrote", $2, "->", "'"$OUT"'"}' >&2
