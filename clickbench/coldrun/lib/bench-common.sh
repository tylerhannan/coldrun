#!/bin/bash
# Shared ClickBench-format driver for coldrun (repo-local; no upstream tree required).
set -Eeuo pipefail
trap 'echo "bench: failed at ${BASH_SOURCE[0]}:${LINENO}: ${BASH_COMMAND}" >&2' ERR

: "${BENCH_RESTARTABLE:=yes}"
: "${BENCH_DURABLE:=yes}"
: "${BENCH_TRIES:=3}"
: "${BENCH_QUERIES_FILE:=queries.sql}"
: "${BENCH_CHECK_TIMEOUT:=120}"
: "${BENCH_QUERY_FROM:=1}"
: "${BENCH_QUERY_TO:=999}"
: "${BENCH_PROGRESS:=0}"
: "${BENCH_PRINT_HOT_SUMMARY:=0}"
: "${BENCH_PRINT_TOP_SLOW:=0}"
: "${BENCH_COMPARE_LATEST:=}"
: "${BENCH_WRITE_SNAPSHOT:=}"

bench_dir() {
  cd "$(dirname "${BASH_SOURCE[0]}")/.."
  pwd
}

bench_flush_caches() {
  if [ "$(uname -s)" != "Linux" ] || [ ! -w /proc/sys/vm/drop_caches ] 2>/dev/null; then
    return 0
  fi
  if ! command -v sudo >/dev/null 2>&1; then
    return 0
  fi
  sync
  sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches' 2>/dev/null || true
}

bench_check_loop() {
  local i last_err
  for i in $(seq 1 "$BENCH_CHECK_TIMEOUT"); do
    if last_err=$(./check 2>&1 >/dev/null); then
      return 0
    fi
    sleep 1
  done
  echo "bench: ./check did not succeed within ${BENCH_CHECK_TIMEOUT}s" >&2
  [ -n "$last_err" ] && printf '%s\n' "$last_err" | sed 's/^/    /' >&2
  return 1
}

bench_wait_stopped() {
  local i
  for i in $(seq 1 60); do
    if ! ./check >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "bench: server did not stop within 60s; proceeding" >&2
}

bench_should_run_query() {
  local query_num="$1"
  if [ "$query_num" -lt "$BENCH_QUERY_FROM" ] || [ "$query_num" -gt "$BENCH_QUERY_TO" ]; then
    return 1
  fi
  if [ -n "${BENCH_QUERY_LIST:-}" ]; then
    case " $BENCH_QUERY_LIST " in
      *" $query_num "*) return 0 ;;
      *) return 1 ;;
    esac
  fi
  return 0
}

# shellcheck source=lib/bench-report.sh
source "$(dirname "${BASH_SOURCE[0]}")/bench-report.sh"

bench_run_query() {
  local query="$1"
  local query_num="$2"
  local i raw timing exit_code
  local results=()

  if [ "$BENCH_PROGRESS" = "1" ]; then
    echo "Q${query_num}/${BENCH_TOTAL:-?}" >&2
  fi

  if [ "$BENCH_RESTARTABLE" = "yes" ]; then
    ./stop >/dev/null 2>&1 || true
    bench_wait_stopped
    bench_flush_caches
    ./start >/dev/null 2>&1 || true
    bench_check_loop
  else
    if ! ./check >/dev/null 2>&1; then
      ./start >/dev/null 2>&1 || true
      bench_check_loop
    fi
    bench_flush_caches
  fi

  for i in $(seq 1 "$BENCH_TRIES"); do
    errf=$(mktemp)
    printf '%s\n' "$query" | ./query >/dev/null 2>"$errf" && exit_code=0 || exit_code=$?
    if [ "$exit_code" -eq 0 ]; then
      # Some failures return 0 but omit numeric timing; do not abort the whole bench.
      timing=$(tr '\r' '\n' <"$errf" | grep -E '^[0-9]+(\.[0-9]+)?$' | tail -n1 || true)
      if [ -z "$timing" ]; then
        timing="null"
        echo "bench: missing numeric timing for Q${query_num} try ${i} (exit 0)" >&2
        if [ -s "$errf" ]; then
          sed 's/^/    /' "$errf" >&2
        fi
      fi
    else
      timing="null"
      echo "bench: query failed for Q${query_num} try ${i} (exit ${exit_code})" >&2
      cat "$errf" >&2
    fi
    rm -f "$errf"
    results+=("$timing")
    echo "${query_num},${i},${timing}" >> result.csv
  done

  local out="["
  local j
  for j in "${!results[@]}"; do
    out+="${results[$j]}"
    [ "$j" -lt $((${#results[@]} - 1)) ] && out+=","
  done
  out+="],"
  echo "$out"
  if [ "$BENCH_PROGRESS" = "1" ]; then
    hot=$(bench_hot_seconds "$query_num" 2>/dev/null || true)
    [ -n "$hot" ] && echo "  -> hot ${hot}s" >&2
  fi
}

bench_count_queries() {
  local n=0
  local q
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    n=$((n + 1))
  done < "$BENCH_QUERIES_FILE"
  echo "$n"
}

bench_hot_seconds() {
  bench_hot_for_query "$1"
}

bench_print_hot_summary() {
  local query_num hot sum nulls
  sum=$(bench_hot_sum)
  nulls=$(bench_count_null_timings)
  echo "=== hot (min of tries 2–3, seconds) ===" >&2
  query_num=0
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_should_run_query "$query_num" || continue
    hot=$(bench_hot_for_query "$query_num")
    [ -z "$hot" ] && hot="null"
    printf "Q%-3s %s\n" "$query_num" "$hot" >&2
  done < "$BENCH_QUERIES_FILE"
  echo "hot sum: $sum" >&2
  [ "$nulls" -gt 0 ] && echo "WARNING: $nulls failed timings in result.csv" >&2
  if [ "$BENCH_PRINT_TOP_SLOW" = "1" ]; then
    bench_print_top_slow 8
  fi
  if [ -n "$BENCH_COMPARE_LATEST" ]; then
    bench_compare_latest_md "$BENCH_COMPARE_LATEST"
  fi
  if [ -n "$BENCH_WRITE_SNAPSHOT" ]; then
    bench_write_hot_snapshot "$BENCH_WRITE_SNAPSHOT" "${BENCH_SNAPSHOT_ROWS:-?}" \
      "${BENCH_SNAPSHOT_BYTES:-0}" "${BENCH_SNAPSHOT_COMMIT:-}"
  fi
}

bench_main() {
  local dir query_num q ran total
  dir=$(bench_dir)
  cd "$dir"
  chmod +x ./*.sh ./query ./lib/*.sh 2>/dev/null || true

  total=$(bench_count_queries)
  export BENCH_TOTAL="$total"

  : > result.csv
  echo "num,try,seconds" >> result.csv

  query_num=0
  ran=0
  local start_t
  start_t=$(date +%s.%N)
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_should_run_query "$query_num" || continue
    bench_run_query "$q" "$query_num"
    ran=$((ran + 1))
  done < "$BENCH_QUERIES_FILE"
  local end_t wall
  end_t=$(date +%s.%N)
  wall=$(awk -v s="$start_t" -v e="$end_t" 'BEGIN { printf "%.2f", e - s }')

  echo "Queries completed: $ran (of $total in ${BENCH_QUERIES_FILE}), wall ${wall}s"
  if [ "$BENCH_PRINT_HOT_SUMMARY" = "1" ]; then
    bench_print_hot_summary
  fi
}
