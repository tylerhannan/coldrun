#!/bin/bash
# Resolve bundled ClickHouse binary (clickhouse-local/clickhouse).
# Source from other scripts: . "$ROOT/scripts/lib/clickhouse-local.sh"

clickhouse_local_bin() {
  local root="${1:?}"
  if [ -x "$root/clickhouse-local/clickhouse" ]; then
    echo "$root/clickhouse-local/clickhouse"
    return 0
  fi
  if command -v clickhouse >/dev/null 2>&1; then
    command -v clickhouse
    return 0
  fi
  return 1
}

clickhouse_hits_cte() {
  local parquet="${1:?}"
  cat <<SQL
WITH hits AS (
  SELECT * REPLACE (
    toDate('1970-01-01') + toInt32(EventDate) AS EventDate,
    toDateTime(EventTime) AS EventTime
  )
  FROM file('${parquet}', Parquet)
)
SQL
}

# Reference SQL for ClickHouse (may differ from coldrun when parquet/file() quirks apply).
clickhouse_reference_sql() {
  local n="$1"
  local q="$2"
  case "$n" in
    4)
      # Int64 sum overflows on parquet file(); Float64 avg matches coldrun semantics.
      echo "$q" | sed 's/AVG(UserID)/avg(toFloat64(UserID))/'
      ;;
    *)
      echo "$q"
      ;;
  esac
}

clickhouse_out() {
  local bin="$1"
  local parquet="$2"
  local q="$3"
  "$bin" local --format TabSeparatedWithNames --max_threads 1 --query "
SET max_threads = 1;
SET session_timezone = 'UTC';
$(clickhouse_hits_cte "$parquet")
$q
" 2>/dev/null
}

# Build a full local query (CTE + body) for timing or execution.
clickhouse_build_query() {
  local parquet="${1:?}"
  local sql="${2:?}"
  cat <<SQL
SET max_threads = 1;
SET session_timezone = 'UTC';
$(clickhouse_hits_cte "$parquet")
$sql
SQL
}

# Run one query via clickhouse-local; print elapsed seconds (last --time line).
clickhouse_run_timed() {
  local bin="${1:?}"
  local parquet="${2:?}"
  local sql="${3:?}"
  local out timing
  out=$("$bin" local --time --max_threads 1 --query "$(clickhouse_build_query "$parquet" "${sql%;}")" 2>&1) || true
  timing=$(printf '%s\n' "$out" | grep -E '^[0-9]+\.[0-9]+$' | tail -n1)
  if [ -z "$timing" ]; then
    echo "null"
    return 1
  fi
  echo "$timing"
}
