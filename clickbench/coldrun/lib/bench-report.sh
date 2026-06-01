#!/bin/bash
# Reporting helpers for bench-common / bench-serve (hot timings, snapshots, diffs).
set -euo pipefail

bench_rows_slug() {
  case "$1" in
    100000) echo "demo-100k" ;;
    500000) echo "demo-500k" ;;
    *) echo "demo-${1}" ;;
  esac
}

bench_hot_for_query() {
  awk -F, -v n="$1" '
    $1 == n && $2 >= 2 && $3 != "null" && $3 != "" { print $3 }
  ' result.csv | sort -n | head -n1
}

bench_cold_for_query() {
  awk -F, -v n="$1" '
    $1 == n && $2 == 1 && $3 != "null" && $3 != "" { print $3; exit }
  ' result.csv
}

bench_hot_sum() {
  local query_num hot sum=0
  query_num=0
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_should_run_query "$query_num" || continue
    hot=$(bench_hot_for_query "$query_num")
    [ -z "$hot" ] && continue
    sum=$(awk -v a="$sum" -v b="$hot" 'BEGIN { printf "%.6f", a + b }')
  done < "$BENCH_QUERIES_FILE"
  echo "$sum"
}

bench_print_top_slow() {
  local n="${1:-5}"
  echo "=== slowest (hot, seconds) ===" >&2
  awk -F, '
    NR > 1 && $2 >= 2 && $3 != "null" && $3 != "" {
      q = $1; t = $3 + 0
      if (!(q in best) || t < best[q]) best[q] = t
    }
    END {
      for (q in best) print best[q], q
    }
  ' result.csv | sort -rn | head -n "$n" | while read -r t q; do
    printf "Q%-3s %s\n" "$q" "$t" >&2
  done
}

bench_count_null_timings() {
  awk -F, 'NR > 1 && ($3 == "null" || $3 == "") { c++ } END { print c+0 }' result.csv
}

bench_compare_latest_md() {
  local latest="${1:?}"
  [ -f "$latest" ] || {
    echo "bench: no baseline at $latest (skip compare)" >&2
    return 0
  }
  echo "=== vs bench-all ($(basename "$latest")) ===" >&2
  printf "%-4s %8s %8s %8s\n" "Q" "hot" "all" "delta%" >&2
  local query_num hot all
  query_num=0
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_should_run_query "$query_num" || continue
    hot=$(bench_hot_for_query "$query_num")
    all=$(awk -F'|' -v n="$query_num" '
      NF >= 3 {
        q = $2 + 0
        if (q == n) { gsub(/^ +| +$/, "", $3); print $3; exit }
      }
    ' "$latest" 2>/dev/null || true)
    if [ -z "$hot" ] || [ -z "$all" ]; then
      printf "%-4s %8s %8s %8s\n" "$query_num" "${hot:--}" "${all:--}" "-" >&2
      continue
    fi
    awk -v q="$query_num" -v h="$hot" -v a="$all" 'BEGIN {
      if (a + 0 == 0) pct = "-"
      else pct = sprintf("%.0f%%", (h - a) / a * 100)
      printf "%-4s %8.3f %8.3f %8s\n", q, h, a, pct
    }' >&2
  done < "$BENCH_QUERIES_FILE"
}

bench_write_hot_snapshot() {
  local out="$1" rows="$2" data_bytes="$3" git_ref="${4:-}"
  local query_num hot sum slug="${BENCH_SNAPSHOT_SLUG:-}"
  mkdir -p "$(dirname "$out")"
  sum=$(bench_hot_sum)
  {
    if [ "$slug" = "parquet-hits-1m" ]; then
      echo "# Serve hot — 1M parquet \`hits\` slice"
      echo
      echo "**Command:** \`COLDRUN_DATA=.coldrun-validate-hits-1m_ ./scripts/bench-serve.sh 1000000 --skip-load --no-compare\`  "
      echo "**Protocol:** warm \`serve\`, 3 tries/query, hot = min(try 2, try 3)  "
    else
      echo "# Serve hot — ${rows} demo rows"
      echo
      echo "**Command:** \`./scripts/bench-serve.sh ${rows}\`"
      echo "**Protocol:** warm \`serve\`, 3 tries/query, hot = min(try 2, try 3)"
    fi
    [ -n "$git_ref" ] && echo "**Commit:** \`${git_ref}\`"
    echo "**Data size:** ${data_bytes} bytes"
    echo
    echo "| Q | hot (s) | cold (try 1) |"
    echo "|---|---------|----------------|"
    query_num=0
    while IFS= read -r q || [ -n "$q" ]; do
      [ -z "$q" ] && continue
      query_num=$((query_num + 1))
      bench_should_run_query "$query_num" || continue
      hot=$(bench_hot_for_query "$query_num")
      cold=$(bench_cold_for_query "$query_num")
      [ -z "$hot" ] && hot="null"
      [ -z "$cold" ] && cold="—"
      echo "| $query_num | $hot | $cold |"
    done < "$BENCH_QUERIES_FILE"
    echo
    if [ "$slug" = "parquet-hits-1m" ]; then
      echo "**Hot sum (Q1–43):** ${sum}s"
      echo
      echo "Regenerate:"
      echo
      echo '```bash'
      echo 'COLDRUN_DATA="$PWD/.coldrun-validate-hits-1m_" BENCH_SNAPSHOT_SLUG=parquet-hits-1m \'
      echo '  env -u BENCH_QUERY_TO -u BENCH_QUERY_FROM \'
      echo '  ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot'
      echo '```'
    else
      echo "**Hot sum:** ${sum}s — comparable shape to ClickBench hot; not leaderboard-valid without 100M + VM."
      echo
      echo "Regenerate: \`./scripts/bench-serve.sh ${rows} --write-snapshot\`"
    fi
  } >"$out"
  echo "wrote $out" >&2
}
