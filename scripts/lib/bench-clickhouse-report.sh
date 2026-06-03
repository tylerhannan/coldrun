#!/bin/bash
# Snapshot + compare helpers for bench-clickhouse-parquet.sh
set -euo pipefail

bench_ch_hot_for_query() {
  awk -F, -v n="$1" '
    $1 == n && $2 >= 2 && $3 != "null" && $3 != "" { print $3 }
  ' "$CH_BENCH_CSV" | sort -n | head -n1
}

bench_ch_cold_for_query() {
  awk -F, -v n="$1" '
    $1 == n && $2 == 1 && $3 != "null" && $3 != "" { print $3; exit }
  ' "$CH_BENCH_CSV"
}

bench_ch_hot_sum() {
  local query_num hot sum=0
  query_num=0
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_ch_should_run_query "$query_num" || continue
    hot=$(bench_ch_hot_for_query "$query_num")
    [ -z "$hot" ] && continue
    sum=$(awk -v a="$sum" -v b="$hot" 'BEGIN { printf "%.6f", a + b }')
  done < "$CH_BENCH_QUERIES"
  echo "$sum"
}

bench_ch_should_run_query() {
  local query_num="$1"
  if [ "$query_num" -lt "$CH_BENCH_FROM" ] || [ "$query_num" -gt "$CH_BENCH_TO" ]; then
    return 1
  fi
  if [ -n "${CH_BENCH_LIST:-}" ]; then
    case " $CH_BENCH_LIST " in
      *" $query_num "*) return 0 ;;
      *) return 1 ;;
    esac
  fi
  return 0
}

bench_ch_parse_hot_md() {
  local md="$1" q="$2"
  awk -F'|' -v n="$q" '
    $2 ~ /^[[:space:]]*[0-9]+[[:space:]]*$/ {
      q = $2 + 0
      if (q == n) {
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", $3)
        print $3
        exit
      }
    }
  ' "$md"
}

bench_ch_write_snapshot() {
  local out="$1" parquet="$2" parquet_bytes="$3" git_ref="${4:-}"
  local query_num hot cold sum slug="${CH_BENCH_SNAPSHOT_SLUG:-}"
  sum=$(bench_ch_hot_sum)
  mkdir -p "$(dirname "$out")"
  {
    echo "# ClickHouse hot — 1M parquet \`hits\` slice"
    echo
    echo "**Command:** \`./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot\`  "
    echo "**Protocol:** \`clickhouse local --time\`, \`file()\` Parquet, 3 tries/query, hot = min(try 2, try 3), \`max_threads=1\`  "
    echo "**Note:** new \`clickhouse local\` process per try (Parquet OS cache warm after try 1); coldrun uses warm \`serve\` — see [\`compare-hot.md\`](compare-hot.md)."
    if [ -n "$git_ref" ]; then echo "**Commit:** \`${git_ref}\`"; fi
    echo "**Parquet size:** ${parquet_bytes} bytes"
    echo
    echo "| Q | hot (s) | cold (try 1) |"
    echo "|---|---------|----------------|"
    query_num=0
    while IFS= read -r q || [ -n "$q" ]; do
      [ -z "$q" ] && continue
      query_num=$((query_num + 1))
      bench_ch_should_run_query "$query_num" || continue
      hot=$(bench_ch_hot_for_query "$query_num")
      cold=$(bench_ch_cold_for_query "$query_num")
      [ -z "$hot" ] && hot="null"
      [ -z "$cold" ] && cold="—"
      echo "| $query_num | $hot | $cold |"
    done < "$CH_BENCH_QUERIES"
    echo
    echo "**Hot sum (Q1–43):** ${sum}s"
    echo
    echo "Regenerate:"
    echo
    echo '```bash'
    echo "./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot"
    echo '```'
  } >"$out"
  echo "wrote $out" >&2
}

bench_ch_write_compare() {
  local out="$1" serve_md="$2" ch_md="$3"
  local query_num ch_hot cr_hot ch_sum=0 cr_sum=0
  mkdir -p "$(dirname "$out")"
  {
    echo "# coldrun vs ClickHouse — 1M parquet hot sum"
    echo
    echo "Side-by-side **hot** timings (min of tries 2–3) on the same Parquet slice and query set."
    echo
    echo "| Engine | Protocol | Hot sum (Q1–43) |"
    echo "|--------|----------|-----------------|"
    cr_sum=$(grep -F '**Hot sum' "$serve_md" | grep -oE '[0-9]+\.[0-9]+' | head -1)
    ch_sum=$(grep -F '**Hot sum' "$ch_md" | grep -oE '[0-9]+\.[0-9]+' | head -1)
    echo "| **coldrun** | warm \`serve\` ([\`serve-hot.md\`](serve-hot.md)) | **${cr_sum:-?}s** |"
    echo "| **ClickHouse** | \`local --time\` ([\`clickhouse-hot.md\`](clickhouse-hot.md)) | **${ch_sum:-?}s** |"
    if [ -n "${cr_sum:-}" ] && [ -n "${ch_sum:-}" ] && awk "BEGIN { exit !($ch_sum > 0) }"; then
      ratio=$(awk -v c="$cr_sum" -v h="$ch_sum" 'BEGIN { printf "%.2f", c / h }')
      echo
      echo "**Ratio (coldrun / ClickHouse):** ${ratio}×"
    fi
    echo
    echo "| Q | coldrun hot | ClickHouse hot | Δ% |"
    echo "|---|-------------|----------------|-----|"
    query_num=0
    while IFS= read -r q || [ -n "$q" ]; do
      [ -z "$q" ] && continue
      query_num=$((query_num + 1))
      bench_ch_should_run_query "$query_num" || continue
      cr_hot=$(bench_ch_parse_hot_md "$serve_md" "$query_num")
      ch_hot=$(bench_ch_parse_hot_md "$ch_md" "$query_num")
      if [ -z "$cr_hot" ] || [ -z "$ch_hot" ]; then
        printf "| %s | %s | %s | — |\n" "$query_num" "${cr_hot:-"—"}" "${ch_hot:-"—"}"
        continue
      fi
      awk -v q="$query_num" -v c="$cr_hot" -v h="$ch_hot" 'BEGIN {
        if (h + 0 == 0) pct = "-"
        else pct = sprintf("%+.0f%%", (c - h) / h * 100)
        printf "| %s | %s | %s | %s |\n", q, c, h, pct
      }'
    done < "$CH_BENCH_QUERIES"
    echo
    echo "Regenerate:"
    echo
    echo '```bash'
    echo "./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot --compare"
    echo '```'
  } >"$out"
  echo "wrote $out" >&2
}

bench_ch_print_summary() {
  local query_num hot sum nulls=0
  sum=$(bench_ch_hot_sum)
  echo "=== ClickHouse hot (min of tries 2–3, seconds) ===" >&2
  query_num=0
  while IFS= read -r q || [ -n "$q" ]; do
    [ -z "$q" ] && continue
    query_num=$((query_num + 1))
    bench_ch_should_run_query "$query_num" || continue
    hot=$(bench_ch_hot_for_query "$query_num")
    [ -z "$hot" ] && { hot="null"; nulls=$((nulls + 1)); }
    printf "Q%-3s %s\n" "$query_num" "$hot" >&2
  done < "$CH_BENCH_QUERIES"
  echo "hot sum: $sum" >&2
  if [ "$nulls" -gt 0 ]; then echo "WARNING: $nulls failed timings" >&2; fi
}
