#!/usr/bin/env bash
# Pass 2: update per-query perf notes (one commit per query).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p docs/perf

note() {
  case "$1" in
    1) echo "Q1: row_count metadata — no scan. Still 0 rows if table empty." ;;
    2) echo "Q2: nonzero int16 count without building bool mask." ;;
    3) echo "Q3: fused multi-agg one mask. AVG still float." ;;
    4) echo "Q4: single AVG column — could SIMD." ;;
    5) echo "Q5: global HashSet<i64> distinct — OK at 100k." ;;
    6) echo "Q6: utf8 distinct global — string clones in set." ;;
    7) echo "Q7: fused MIN/MAX date — optimal." ;;
    8) echo "Q8: int GROUP BY AdvEngineID — low cardinality." ;;
    9) echo "Q9: int distinct + top-K heap." ;;
    10) echo "Q10: fused RegionID multi-agg kernel." ;;
    11) echo "Q11: utf8+distinct int64 per group — heap top-10." ;;
    12) echo "Q12: int16+utf8 distinct — was 103ms, now ~26ms." ;;
    13) echo "Q13: utf8 COUNT hash map, dense mask iter." ;;
    14) echo "Q14: same as Q11 pattern." ;;
    15) echo "Q15: int+utf8 COUNT fused." ;;
    16) echo "Q16: Int64 COUNT map + top-K heap." ;;
    17) echo "Q17: int+utf8 COUNT fused." ;;
    18) echo "Q18: int+utf8 LIMIT without ORDER BY — still builds all groups." ;;
    19) echo "Q19: triple-key fused; 100k inserts — needs streaming or pre-agg." ;;
    20) echo "Q20: equality scan linear — index on UserID would be O(1)." ;;
    21) echo "Q21: LIKE scan — memchr helps; no trigram index." ;;
    22) echo "Q22: utf8 group+MIN — needs min fast path." ;;
    23) echo "Q23: multi MIN+COUNT — interpreter." ;;
    24) echo "Q24: index sort SELECT * — still materializes all cols." ;;
    25) echo "Q25–27: scan_fast column sort." ;;
    28) echo "Q28: HAVING impossible on demo — instant." ;;
    29) echo "Q29: referer host fused." ;;
    30) echo "Q30: 90× SUM ladder." ;;
    31) echo "Q31: int pair fused aggs." ;;
    32) echo "Q32: WatchID+ClientIP fused." ;;
    33) echo "Q33: same kernel as Q32." ;;
    34) echo "Q34: utf8 COUNT + heap." ;;
    35) echo "Q35: URL COUNT — string hash 100k×." ;;
    36) echo "Q36: int4 COUNT — ~unique keys on demo, heap top-10." ;;
    37) echo "Q37–39: zone prune + utf8 group." ;;
    40) echo "Q40: CASE in GROUP BY — interpreter only." ;;
    41) echo "Q41: dashboard zones + group." ;;
    42) echo "Q42: two int16 dims — could fuse." ;;
    43) echo "Q43: DATE_TRUNC minute int group + zones." ;;
    *) echo "Pass 2 notes." ;;
  esac
}

for n in $(seq 1 43); do
  q=$(printf '%02d' "$n")
  f="docs/perf/q-${q}.md"
  adv=$(note "$n")
  if [[ -f "$f" ]]; then
    if ! grep -q "Pass 2" "$f" 2>/dev/null; then
      cat >> "$f" <<EOF

## Pass 2 (faster)

${adv}
EOF
    fi
  fi
  git add "$f" 2>/dev/null || true
  git diff --cached --quiet || git commit -m "Perf Q${n} pass2: ${adv%% —*}" || true
done

echo "Done."
