#!/bin/bash
# Compare coldrun vs DuckDB on the same Parquet file (no cloud).
#
# Usage:
#   ./scripts/validate-parquet.sh hits-1m.parquet
#   ./scripts/validate-parquet.sh hits.parquet --from 1 --to 15
#   ./scripts/validate-parquet.sh hits.parquet --queries 1,2,3,5
#   ./scripts/validate-parquet.sh hits.parquet --skip-load   # reuse COLDRUN_DATA
#
# Requires: duckdb CLI, coldrun built, Parquet on disk (use sample-parquet.sh to shrink).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export PATH="${HOME}/.cargo/bin:${PATH}"

BIN="$ROOT/target/release/coldrun"
DIFF="${DIFF:-/usr/bin/diff}"
[ -x "$DIFF" ] || DIFF="diff"
QUERIES="$ROOT/clickbench/coldrun/queries.sql"
PARQUET=""
SKIP_LOAD=0
QUERY_FROM=1
QUERY_TO=999
QUERY_LIST=""

while [ $# -gt 0 ]; do
  case "$1" in
    --from) QUERY_FROM="${2:?}"; shift 2 ;;
    --to) QUERY_TO="${2:?}"; shift 2 ;;
    --queries) QUERY_LIST="$(echo "${2:?}" | tr ',' ' ')"; shift 2 ;;
    --skip-load) SKIP_LOAD=1; shift ;;
    --help|-h)
      sed -n '2,12p' "$0"
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
  echo "Usage: $0 path.parquet [--from N] [--to N] [--queries 1,2,3]" >&2
  exit 1
}
[ -f "$PARQUET" ] || { echo "missing parquet: $PARQUET" >&2; exit 1; }

if ! command -v duckdb >/dev/null 2>&1; then
  echo "duckdb CLI required (brew install duckdb)" >&2
  exit 1
fi

cargo build --release -p coldrun-cli -q

PARQUET="$(cd "$(dirname "$PARQUET")" && pwd)/$(basename "$PARQUET")"
slug=$(basename "$PARQUET" .parquet | tr -c '[:alnum:]_-' '_')
DATA="${COLDRUN_DATA:-$ROOT/.coldrun-validate-$slug}"

should_run() {
  local n="$1"
  [ "$n" -ge "$QUERY_FROM" ] && [ "$n" -le "$QUERY_TO" ] || return 1
  [ -z "$QUERY_LIST" ] && return 0
  case " $QUERY_LIST " in *" $n "*) return 0 ;; *) return 1 ;; esac
}

coldrun_out() {
  local q="$1"
  local errf
  errf=$(mktemp)
  if ! printf '%s\n' "$q" | "$BIN" --data-dir "$DATA" local 2>"$errf"; then
    cat "$errf" >&2
    rm -f "$errf"
    return 1
  fi
  rm -f "$errf"
}

duckdb_hits_view_sql() {
  cat <<SQL
CREATE OR REPLACE TEMP VIEW hits AS
SELECT * REPLACE (
  date_add(DATE '1970-01-01', CAST(EventDate AS INTEGER)) AS EventDate,
  to_timestamp(CAST(EventTime AS BIGINT)) AS EventTime
)
FROM read_parquet('$PARQUET');
SQL
}

duckdb_out() {
  local q="$1"
  duckdb -batch -csv -noheader 2>/dev/null <<SQL
$(duckdb_hits_view_sql)
$q
SQL
}

normalize_result() {
  # Drop header rows; normalize numbers (incl. scientific) and separators for diff.
  python3 -c '
import re, sys
num = re.compile(r"^[0-9eE+.\-,\t]+$")
ts = re.compile(r"^\d{4}-\d{2}-\d{2}")
for line in sys.stdin:
    line = line.strip()
    line = re.sub(
        r"(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})(?:[+-]\d{2}(?::\d{2})?)?",
        r"\1",
        line,
    )
    if not line:
        continue
    if not num.match(line.replace(" ", "")) and not ts.search(line):
        continue
    parts = re.split(r"[\t,]", line)
    out = []
    for p in parts:
        p = p.strip()
        if p == "":
            out.append("")
            continue
        if p.lstrip("-").isdigit():
            out.append(p)
            continue
        try:
            f = float(p)
            if f == int(f) and abs(f) < 1e15:
                out.append(str(int(f)))
            else:
                out.append(f"{f:.10g}")
        except ValueError:
            out.append(p)
    print(",".join(out))
' | LC_ALL=C sort
}

if [ "$SKIP_LOAD" = "0" ]; then
  echo "loading $PARQUET into $DATA ..." >&2
  rm -rf "$DATA"
  "$BIN" --data-dir "$DATA" local --load "$PARQUET" >/dev/null 2>&1
fi

PASS=0
FAIL=0
SKIP=0
LOG="${VALIDATE_LOG:-$ROOT/logs/benchmarks/validate-$slug.log}"
mkdir -p "$(dirname "$LOG")"
: >"$LOG"

i=1
while IFS= read -r q || [ -n "$q" ]; do
  [ -z "$q" ] && continue
  if ! should_run "$i"; then
    i=$((i + 1))
    continue
  fi

  crf=$(mktemp)
  dkf=$(mktemp)
  if ! coldrun_out "$q" >"$crf" 2>>"$LOG"; then
    echo "FAIL Q$i coldrun error" | tee -a "$LOG"
    FAIL=$((FAIL + 1))
    i=$((i + 1))
    rm -f "$crf" "$dkf"
    continue
  fi
  if ! duckdb_out "$q" >"$dkf" 2>>"$LOG"; then
    echo "SKIP Q$i duckdb error (unsupported SQL?)" | tee -a "$LOG"
    SKIP=$((SKIP + 1))
    i=$((i + 1))
    rm -f "$crf" "$dkf"
    continue
  fi

  if "$DIFF" -q <(normalize_result <"$crf") <(normalize_result <"$dkf") >/dev/null 2>&1; then
    echo "PASS Q$i" | tee -a "$LOG"
    PASS=$((PASS + 1))
  else
    echo "FAIL Q$i result mismatch" | tee -a "$LOG"
    echo "  query: $q" >>"$LOG"
    echo "  --- coldrun (first 5 lines) ---" >>"$LOG"
    head -5 "$crf" >>"$LOG"
    echo "  --- duckdb (first 5 lines) ---" >>"$LOG"
    head -5 "$dkf" >>"$LOG"
    FAIL=$((FAIL + 1))
  fi
  rm -f "$crf" "$dkf"
  i=$((i + 1))
done <"$QUERIES"

echo "=== validate $PARQUET: $PASS pass, $FAIL fail, $SKIP skip (log: $LOG) ===" | tee -a "$LOG"
[ "$FAIL" -eq 0 ]
