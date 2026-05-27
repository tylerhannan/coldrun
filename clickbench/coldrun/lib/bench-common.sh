#!/bin/bash
# Shared ClickBench-format driver for coldrun (repo-local; no upstream tree required).
set -euo pipefail

: "${BENCH_RESTARTABLE:=yes}"
: "${BENCH_DURABLE:=yes}"
: "${BENCH_TRIES:=3}"
: "${BENCH_QUERIES_FILE:=queries.sql}"
: "${BENCH_CHECK_TIMEOUT:=120}"
: "${BENCH_QUERY_FROM:=1}"
: "${BENCH_QUERY_TO:=999}"
: "${BENCH_PROGRESS:=0}"
: "${BENCH_PRINT_HOT_SUMMARY:=0}"

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

bench_run_query() {
  local query="$1"
  local query_num="$2"
  local i raw timing exit_code
  local results=()

  if [ "$BENCH_PROGRESS" = "1" ]; then
    echo "Q${query_num}: ${query:0:72}..." >&2
  fi

  if [ "$BENCH_RESTARTABLE" = "yes" ]; then
    ./stop >/dev/null 2>&1 || true
    bench_wait_stopped
    bench_flush_caches
    ./start >/dev/null 2>&1 || true
    bench_check_loop
  else
    bench_flush_caches
  fi

  for i in $(seq 1 "$BENCH_TRIES"); do
    errf=$(mktemp)
    printf '%s\n' "$query" | ./query >/dev/null 2>"$errf" && exit_code=0 || exit_code=$?
    if [ "$exit_code" -eq 0 ]; then
      timing=$(tr '\r' '\n' <"$errf" | grep -E '^[0-9]+(\.[0-9]+)?$' | tail -n1)
      [ -z "$timing" ] && timing="null"
    else
      timing="null"
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
}

bench_hot_seconds() {
  # ClickBench hot = min of 2nd and 3rd try (when BENCH_TRIES=3).
  awk -F, -v n="$1" '
    $1 == n && $2 >= 2 && $3 != "null" && $3 != "" { print $3 }
  ' result.csv | sort -n | head -n1
}

bench_print_hot_summary() {
  local query_num hot sum
  sum=0
  echo "=== hot (min of tries 2–3, seconds) ===" >&2
  query_num=0
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_should_run_query "$query_num" || continue
    hot=$(bench_hot_seconds "$query_num")
    [ -z "$hot" ] && hot="null"
    if [ "$hot" != "null" ]; then
      sum=$(awk -v a="$sum" -v b="$hot" 'BEGIN { printf "%.6f", a + b }')
    fi
    printf "Q%-3s %s\n" "$query_num" "$hot" >&2
  done < "$BENCH_QUERIES_FILE"
  echo "hot sum: $sum" >&2
}

bench_main() {
  local dir query_num q ran
  dir=$(bench_dir)
  cd "$dir"
  chmod +x ./*.sh ./query ./lib/*.sh 2>/dev/null || true

  : > result.csv
  echo "num,try,seconds" >> result.csv

  query_num=0
  ran=0
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_should_run_query "$query_num" || continue
    bench_run_query "$q" "$query_num"
    ran=$((ran + 1))
  done < "$BENCH_QUERIES_FILE"

  echo "Queries completed: $ran (of $query_num in ${BENCH_QUERIES_FILE})"
  if [ "$BENCH_PRINT_HOT_SUMMARY" = "1" ]; then
    bench_print_hot_summary
  fi
}
