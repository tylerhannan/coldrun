#!/bin/bash
# Local real-data measurement without AWS: sample (optional) → load → validate → bench-serve.
#
# Usage:
#   ./scripts/measure-parquet.sh hits.parquet
#   ./scripts/measure-parquet.sh hits.parquet --sample 1000000   # 1M rows first
#   ./scripts/measure-parquet.sh hits-1m.parquet --validate-only
#   ./scripts/measure-parquet.sh hits-1m.parquet --bench-only --skip-validate
#
# Needs: ClickHouse in clickhouse-local/, coldrun built, Parquet on disk.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PARQUET=""
SAMPLE_ROWS=""
VALIDATE_ONLY=0
BENCH_ONLY=0
SKIP_VALIDATE=0
QUERY_FROM=1
QUERY_TO=43

while [ $# -gt 0 ]; do
  case "$1" in
    --sample) SAMPLE_ROWS="${2:?}"; shift 2 ;;
    --validate-only) VALIDATE_ONLY=1; shift ;;
    --bench-only) BENCH_ONLY=1; shift ;;
    --skip-validate) SKIP_VALIDATE=1; shift ;;
    --from) QUERY_FROM="${2:?}"; shift 2 ;;
    --to) QUERY_TO="${2:?}"; shift 2 ;;
    --help|-h)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *)
      if [ -z "$PARQUET" ] && [ -f "$1" ]; then
        PARQUET="$1"
        shift
      else
        echo "Unknown argument: $1" >&2
        exit 1
      fi
      ;;
  esac
done

[ -n "$PARQUET" ] || {
  echo "Usage: $0 path.parquet [--sample N]" >&2
  echo "Get data: curl -LO https://datasets.clickhouse.com/hits_compatible/hits.parquet" >&2
  echo "Or slice:  ./scripts/sample-parquet.sh hits.parquet 1000000 hits-1m.parquet" >&2
  exit 1
}

WORK="$PARQUET"
if [ -n "$SAMPLE_ROWS" ]; then
  base="${PARQUET%.parquet}"
  WORK="${base}-${SAMPLE_ROWS}.parquet"
  if [ ! -f "$WORK" ]; then
    "$ROOT/scripts/sample-parquet.sh" "$PARQUET" "$SAMPLE_ROWS" "$WORK"
  else
    echo "reusing sample $WORK" >&2
  fi
fi

slug=$(basename "$WORK" .parquet | tr -c '[:alnum:]_-' '_')
export COLDRUN_DATA="${COLDRUN_DATA:-$ROOT/.coldrun-measure-$slug}"
export COLDRUN_BENCH_LOG="$ROOT/logs/benchmarks/measure-$slug.log"

if [ "$BENCH_ONLY" = "0" ] && [ "$SKIP_VALIDATE" = "0" ]; then
  echo "=== validate coldrun vs ClickHouse ===" >&2
  "$ROOT/scripts/validate-parquet.sh" "$WORK" --from "$QUERY_FROM" --to "$QUERY_TO"
fi

[ "$VALIDATE_ONLY" = "1" ] && exit 0

echo "=== bench-serve (warm hot timing) ===" >&2
export COLDRUN_SKIP_LOAD=1
# Load once for bench-serve
rm -rf "$COLDRUN_DATA"
"$ROOT/target/release/coldrun" --data-dir "$COLDRUN_DATA" local --load "$WORK" >/dev/null 2>&1
rows=$("$ROOT/target/release/coldrun" --data-dir "$COLDRUN_DATA" local --sql "SELECT COUNT(*) FROM hits" 2>/dev/null | head -1 || echo "?")
echo "loaded $rows rows from $WORK" >&2

export BENCH_COMPARE_LATEST=""
export BENCH_SNAPSHOT_SLUG="parquet-$slug"
export BENCH_SNAPSHOT_ROWS="$rows"
export BENCH_SNAPSHOT_BYTES="$(COLDRUN_DATA="$COLDRUN_DATA" "$ROOT/clickbench/coldrun/data-size" 2>/dev/null || echo 0)"

"$ROOT/scripts/bench-serve.sh" 100000 --skip-load --no-compare
