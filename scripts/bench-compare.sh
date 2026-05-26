#!/usr/bin/env bash
# Compare two bench-all runs (or capture a new baseline vs current).
#
# Usage:
#   ./scripts/bench-compare.sh 100000                    # save baseline, run again, diff
#   ./scripts/bench-compare.sh 100000 logs/a.tsv logs/b.tsv
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROWS="${1:-100000}"
BASE="${ROOT}/.coldrun-bench-all/baseline-${ROWS}.tsv"
shift || true

if [[ $# -ge 2 ]]; then
  A="$1"
  B="$2"
else
  mkdir -p "${ROOT}/.coldrun-bench-all"
  if [[ ! -f "$BASE" ]]; then
    echo "No baseline at $BASE — capturing first run..."
    "${ROOT}/scripts/bench-all.sh" "$ROWS" | tee "${BASE}.run.log" | awk '
      /^Q[0-9]+/ { print $1 "\t" $2 }
    ' > "$BASE"
    echo "Baseline saved to $BASE"
    echo "Re-run: ./scripts/bench-compare.sh $ROWS"
    exit 0
  fi
  A="$BASE"
  CUR="${ROOT}/.coldrun-bench-all/current-${ROWS}.tsv"
  "${ROOT}/scripts/bench-all.sh" "$ROWS" | tee "${CUR}.run.log" | awk '
    /^Q[0-9]+/ { print $1 "\t" $2 }
  ' > "$CUR"
  B="$CUR"
fi

echo "Compare: $A vs $B"
echo -e "query\tbefore\tafter\tdelta_pct"
join -t $'\t' "$A" "$B" | while IFS=$'\t' read -r q before after; do
  awk -v q="$q" -v b="$before" -v a="$after" 'BEGIN {
    if (b+0 == 0) { printf "%s\t%s\t%s\t-\n", q, b, a; exit }
    pct = ((a+0) - (b+0)) / (b+0) * 100
    printf "%s\t%s\t%s\t%.1f%%\n", q, b, a, pct
  }'
done | column -t -s $'\t' 2>/dev/null || join -t $'\t' "$A" "$B"
