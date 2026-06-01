#!/bin/bash
# Slice the first N rows of hits.parquet to a smaller file (ClickHouse local).
#
# Usage:
#   ./scripts/sample-parquet.sh hits.parquet 1000000 hits-1m.parquet
#   ./scripts/sample-parquet.sh hits.parquet 100000   # writes hits-100k.parquet
#   ./scripts/sample-parquet.sh https://datasets.clickhouse.com/.../hits.parquet 1000000 hits-1m.parquet
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/lib/clickhouse-local.sh
. "$ROOT/scripts/lib/clickhouse-local.sh"

SRC="${1:?source parquet path or https URL}"
ROWS="${2:?row count}"
OUT="${3:-}"

CH="$(clickhouse_local_bin "$ROOT" || true)"
if [ -z "$CH" ] || [ ! -x "$CH" ]; then
  echo "ClickHouse binary required — run: ./scripts/install-clickhouse-local.sh" >&2
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

OUT="$(cd "$(dirname "$OUT")" 2>/dev/null && pwd)/$(basename "$OUT")" || OUT="$(pwd)/$(basename "$OUT")"

echo "sampling $ROWS rows from $SRC -> $OUT" >&2
if [[ "$SRC" == https://* ]]; then
  FROM_EXPR="url('$PARQUET_SRC', Parquet)"
else
  FROM_EXPR="file('$PARQUET_SRC', Parquet)"
fi
"$CH" local --query "
INSERT INTO FUNCTION file('$OUT', Parquet)
SELECT * FROM $FROM_EXPR LIMIT $ROWS
"

du -sh "$OUT" | awk '{print "wrote", $1, "->", "'"$OUT"'"}' >&2
