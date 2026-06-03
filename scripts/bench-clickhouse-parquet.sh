#!/bin/bash
# Time ClickHouse on the same Parquet slice + queries as coldrun bench-serve.
#
# Usage:
#   ./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet
#   ./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot
#   ./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --compare
#   ./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --from 1 --to 10
#   ./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --queries 23,41
#
# Output: clickbench/coldrun/clickhouse-result.csv, logs/benchmarks/clickhouse-last.log
# Snapshots: docs/benchmarks/<slug>/clickhouse-hot.md, compare-hot.md
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# shellcheck source=scripts/lib/clickhouse-local.sh
. "$ROOT/scripts/lib/clickhouse-local.sh"
# shellcheck source=scripts/lib/bench-clickhouse-report.sh
. "$ROOT/scripts/lib/bench-clickhouse-report.sh"

PARQUET=""
WRITE_SNAPSHOT=0
COMPARE=0
CH_BENCH_FROM=1
CH_BENCH_TO=999
CH_BENCH_LIST=""
CH_BENCH_TRIES=3

while [ $# -gt 0 ]; do
  case "$1" in
    --write-snapshot) WRITE_SNAPSHOT=1; shift ;;
    --compare) COMPARE=1; shift ;;
    --from) CH_BENCH_FROM="${2:?}"; shift 2 ;;
    --to) CH_BENCH_TO="${2:?}"; shift 2 ;;
    --queries)
      CH_BENCH_LIST="$(echo "${2:?}" | tr ',' ' ')"
      shift 2
      ;;
    --help|-h)
      sed -n '2,16p' "$0"
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
  echo "Usage: $0 path.parquet [--write-snapshot] [--compare]" >&2
  exit 1
}

CH="$(clickhouse_local_bin "$ROOT" || true)"
if [ -z "$CH" ] || [ ! -x "$CH" ]; then
  echo "ClickHouse binary required — run: ./scripts/install-clickhouse-local.sh" >&2
  exit 1
fi

PARQUET="$(cd "$(dirname "$PARQUET")" && pwd)/$(basename "$PARQUET")"
PARQUET_BYTES=$(stat -f%z "$PARQUET" 2>/dev/null || stat -c%s "$PARQUET")
CH_BENCH_QUERIES="$ROOT/clickbench/coldrun/queries.sql"
CH_BENCH_CSV="$ROOT/clickbench/coldrun/clickhouse-result.csv"

case "$(basename "$PARQUET" .parquet)" in
  hits-1m) CH_BENCH_SNAPSHOT_SLUG="parquet-hits-1m" ;;
  *)
    CH_BENCH_SNAPSHOT_SLUG="parquet-$(basename "$PARQUET" .parquet | tr -c '[:alnum:]_-' '_')"
    ;;
esac

export CH_BENCH_CSV CH_BENCH_QUERIES CH_BENCH_FROM CH_BENCH_TO CH_BENCH_LIST
export CH_BENCH_SNAPSHOT_SLUG CH_BENCH_TRIES

SNAP_DIR="$ROOT/docs/benchmarks/${CH_BENCH_SNAPSHOT_SLUG}"
SERVE_MD="$SNAP_DIR/serve-hot.md"
CH_MD="$SNAP_DIR/clickhouse-hot.md"
COMPARE_MD="$SNAP_DIR/compare-hot.md"

mkdir -p "$ROOT/logs/benchmarks" "$ROOT/clickbench/coldrun"
: >"$CH_BENCH_CSV"
echo "num,try,seconds" >>"$CH_BENCH_CSV"

echo "ClickHouse: $("$CH" --version 2>/dev/null | head -1)" >&2
echo "Parquet: $PARQUET ($PARQUET_BYTES bytes)" >&2

query_num=0
ran=0
start_t=$(date +%s.%N)
while IFS= read -r q || [ -n "$q" ]; do
  [ -z "$q" ] && continue
  query_num=$((query_num + 1))
  bench_ch_should_run_query "$query_num" || continue
  ran=$((ran + 1))
  chq=$(clickhouse_reference_sql "$query_num" "$q")
  echo "Q${query_num}/${CH_BENCH_TO}" >&2
  for try in $(seq 1 "$CH_BENCH_TRIES"); do
    timing=$(clickhouse_run_timed "$CH" "$PARQUET" "$chq" || true)
    [ -z "$timing" ] && timing="null"
    echo "${query_num},${try},${timing}" >>"$CH_BENCH_CSV"
  done
  hot=$(bench_ch_hot_for_query "$query_num" 2>/dev/null || true)
  [ -n "$hot" ] && echo "  -> hot ${hot}s" >&2
done < "$CH_BENCH_QUERIES"

end_t=$(date +%s.%N)
wall=$(awk -v s="$start_t" -v e="$end_t" 'BEGIN { printf "%.2f", e - s }')
echo "Queries completed: $ran, wall ${wall}s" >&2
bench_ch_print_summary

if [ "$WRITE_SNAPSHOT" = "1" ]; then
  git_ref=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || true)
  bench_ch_write_snapshot "$CH_MD" "$PARQUET" "$PARQUET_BYTES" "$git_ref"
fi

if [ "$COMPARE" = "1" ]; then
  if [ ! -f "$SERVE_MD" ]; then
    echo "bench: missing $SERVE_MD (run bench-serve --write-snapshot first)" >&2
    exit 1
  fi
  if [ ! -f "$CH_MD" ]; then
    echo "bench: missing $CH_MD (re-run with --write-snapshot)" >&2
    exit 1
  fi
  bench_ch_write_compare "$COMPARE_MD" "$SERVE_MD" "$CH_MD"
fi

tee "$ROOT/logs/benchmarks/clickhouse-last.log" <"$CH_BENCH_CSV" >/dev/null
