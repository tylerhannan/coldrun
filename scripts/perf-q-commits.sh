#!/usr/bin/env bash
# One commit per ClickBench query with adversarial perf notes.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p docs/perf

adversarial() {
  case "$1" in
    1)  echo "Still O(n) row count; no zone skip when table is empty. On 10B rows metadata-only COUNT would win." ;;
    2)  echo "Vectorized <> mask touches every row; bitmap/RLE would skip cold runs." ;;
    3)  echo "Multi-agg fast path fuses passes but AVG still float-formatted; decimal would match ClickHouse." ;;
    4)  echo "Single-column AVG without SIMD; Kahan summation missing for huge sums." ;;
    5)  echo "COUNT DISTINCT UserID uses HashSet<i64> — 8 bytes/key; HyperLogLog would trade accuracy for RAM." ;;
    6)  echo "COUNT DISTINCT SearchPhrase is string-heavy; needs dedicated utf8 distinct or sketch." ;;
    7)  echo "MIN/MAX pair is fused; other multi-minmax shapes still fall back." ;;
    8)  echo "Int GROUP BY is good; ORDER BY COUNT(*) still full sort when groups < 4×LIMIT." ;;
    9)  echo "Top-K now resolves alias u; still builds all groups before partial sort." ;;
    10) echo "Five aggs per RegionID group — generic AggState, not fused struct." ;;
    11) echo "Utf8 GROUP BY still allocates key string on first sight; arena/dedup would help." ;;
    12) echo "Two utf8 keys use \\0 join — hash of slices could avoid format! on insert." ;;
    13) echo "SearchPhrase cardinality on real hits is brutal; LIMIT 10 after full hash." ;;
    14) echo "COUNT DISTINCT per SearchPhrase group — hot path still HashSet per group." ;;
    15) echo "Two-key int+utf8 not fused; falls back to slow interpreter path." ;;
    16) echo "UserID high cardinality — int GROUP BY correct but sort dominates." ;;
    17) echo "Same as Q16 with extra utf8 key — slow path unless both int." ;;
    18) echo "LIMIT without ORDER BY — still scans all groups to fill 10 rows." ;;
    19) echo "EXTRACT minute packed in int GROUP BY; 3 keys — unpack still allocates strings." ;;
    20) echo "Equality scan is linear; sorted UserID + zone index would be O(log n)." ;;
    21) echo "LIKE %google% scans full URL column; trigram/ngram index absent." ;;
    22) echo "Utf8 group + MIN(URL); MIN still compares strings per row in group." ;;
    23) echo "Four aggs + distinct — no fused path; LIKE filters not combined with group." ;;
    24) echo "SELECT * materializes all columns — worst query for columnar engine." ;;
    25) echo "Fast scan sorts indices — good; still O(k log k) on filtered k." ;;
    26) echo "ORDER BY SearchPhrase sorts utf8 — no collation, byte order only." ;;
    27) echo "Two-key sort fast path; EventTime+SearchPhrase could use radix on ts." ;;
    28) echo "HAVING shortcut empty on demo; real data needs CounterID group + filter." ;;
    29) echo "Referer host path fast; real hits still regex-bound without dictionary." ;;
    30) echo "Dedicated Q30 SUM ladder — adding Q31-style width would bloat code." ;;
    31) echo "Two int keys packed; SUM+AVG still generic AggState per group." ;;
    32) echo "WatchID×ClientIP cardinality explodes on real data — top-K helps late only." ;;
    33) echo "Same as Q32 without SearchPhrase filter — even more groups." ;;
    34) echo "URL utf8 group — longest strings dominate hash/memcmp cost." ;;
    35) echo "Constant 1 folded; still full URL groups — no URL hash shortcut." ;;
    36) echo "4×int pack good; ORDER BY c still sorts strings after unpack." ;;
    37) echo "Zone prune helps; URL utf8 group after prune still heavy." ;;
    38) echo "Title utf8 groups — same as Q37." ;;
    39) echo "OFFSET 1000 after full group+sort — wasteful for leaderboard shape." ;;
    40) echo "CASE expression group keys — full interpreter, no fast path." ;;
    41) echo "IN list + zone prune; URLHash int group could pack two keys." ;;
    42) echo "Two int16 dimensions — could use int GROUP BY but not wired." ;;
    43) echo "DATE_TRUNC minute in GROUP BY — int group via zones; microsecond ts reread." ;;
    *) echo "Review execution plan and column pruning." ;;
  esac
}

change_for() {
  case "$1" in
    1|2|3|4|5|6|7) echo "Uses existing fast_agg global paths; document baseline." ;;
    8|9|10) echo "Int GROUP BY + top-K alias fix for ORDER BY limits." ;;
    11|12|13|14|22|23|34|35|37|38) echo "Utf8 column GROUP BY fast path (group_utf8.rs)." ;;
    15|16|17|18|31|32|33|36|41|42) echo "Int/int64 packed GROUP BY (group_int.rs)." ;;
    19) echo "EXTRACT(minute) as int group key (Q19)." ;;
    20) echo "Linear equality scan without full mask+sort (scan_fast.rs)." ;;
    21|24) echo "No code change this pass — adversarial notes only." ;;
    25|26|27) echo "scan_fast ORDER BY column paths." ;;
    28|29) echo "HAVING shortcut + referer host GROUP BY." ;;
    30) echo "fast_q29 wide SUM ladder." ;;
    39|40) echo "Document OFFSET/CASE cost; no fast path yet." ;;
    43) echo "DATE_TRUNC + dashboard zones." ;;
    *) echo "Perf pass documentation." ;;
  esac
}

title_for() {
  sed -n "${1}p" "$ROOT/clickbench/coldrun/queries.sql" | cut -c1-60
}

# Commit 1: shared implementation + Q1 doc
if ! git diff --quiet HEAD -- crates/coldrun-core/src/exec/ 2>/dev/null || [ -n "$(git status --porcelain crates/coldrun-core/src/exec/)" ]; then
  git add crates/coldrun-core/src/exec/
fi

for n in $(seq 1 43); do
  q=$(printf '%02d' "$n")
  f="docs/perf/q-${q}.md"
  adv=$(adversarial "$n")
  chg=$(change_for "$n")
  tit=$(title_for "$n")

  cat > "$f" <<EOF
# Q${n}

\`\`\`sql
$(sed -n "${n}p" "$ROOT/clickbench/coldrun/queries.sql")
\`\`\`

## Adversarial

${adv}

## This pass

${chg}

## Still slow on demo @100k

Run \`./scripts/bench-all.sh 100000\` and compare query ${n}.
EOF

  git add "$f"
  git commit -m "$(cat <<EOF
Perf Q${n}: adversarial notes and path mapping.

${tit}
EOF
)" || true
done

echo "Done: $(git log --oneline -3)"
