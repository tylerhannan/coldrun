#!/bin/bash
# Run all 43 ClickBench queries on demo data; report pass/fail.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo build --release -p coldrun-cli 2>&1 | tail -3

BIN="$ROOT/target/release/coldrun"
DATA="${COLDRUN_DATA:-$ROOT/.coldrun-demo}"
ROWS="${1:-50000}"

rm -rf "$DATA"
"$BIN" --data-dir "$DATA" local --demo "$ROWS" >/dev/null

PASS=0
FAIL=0
LOG="$ROOT/smoke-all.log"
: > "$LOG"

i=1
while IFS= read -r q; do
  [ -z "$q" ] && continue
  if out=$(printf '%s\n' "$q" | "$BIN" --data-dir "$DATA" local 2>&1); then
    timing=$(printf '%s\n' "$out" | grep -E '^[0-9]+(\.[0-9]+)?$' | tail -n1 || echo "?")
    echo "PASS Q$i (${timing}s)" | tee -a "$LOG"
    PASS=$((PASS + 1))
  else
    echo "FAIL Q$i: $q" | tee -a "$LOG"
    printf '%s\n' "$out" | tail -5 | tee -a "$LOG"
    FAIL=$((FAIL + 1))
  fi
  i=$((i + 1))
done < "$ROOT/clickbench/coldrun/queries.sql"

echo "=== $PASS passed, $FAIL failed (see $LOG) ===" | tee -a "$LOG"
[ "$FAIL" -eq 0 ]
