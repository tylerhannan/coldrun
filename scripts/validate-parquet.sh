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
PRAGMA threads=1;
$(duckdb_hits_view_sql)
$q
SQL
}

normalize_result() {
  # Drop header rows; normalize numbers (incl. scientific) and separators for diff.
  python3 -c '
import re, sys
from datetime import datetime, timedelta, timezone

num = re.compile(r"^[0-9eE+.\-,\t]+$")
ts = re.compile(r"^\d{4}-\d{2}-\d{2}")

def norm_ts(s):
    m = re.match(
        r"(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})(?:([+-]\d{2})(?::(\d{2}))?)?$",
        s.strip(),
    )
    if not m:
        return s
    base, off_h, off_m = m.group(1), m.group(2), m.group(3)
    naive = datetime.strptime(base, "%Y-%m-%d %H:%M:%S")
    if off_h:
        sign = 1 if off_h[0] == "+" else -1
        tz_off = sign * (int(off_h[1:]) * 60 + int(off_m or 0))
        utc = naive - timedelta(minutes=tz_off)
    else:
        # coldrun DATE_TRUNC labels are PDT wall time (UTC-7)
        utc = naive + timedelta(hours=7)
    return str(int(utc.replace(tzinfo=timezone.utc).timestamp()) // 60)

for line in sys.stdin:
    line = line.strip()
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
        if re.match(r"^\d{4}-\d{2}-\d{2} ", p):
            out.append(norm_ts(p))
            continue
        if p.lstrip("-").isdigit():
            if len(p.lstrip("-")) > 15:
                out.append(f"{int(p):.6e}")
            else:
                out.append(p)
            continue
        try:
            f = float(p)
            if f == int(f) and abs(f) < 1e15:
                out.append(str(int(f)))
            elif abs(f) >= 1e15:
                out.append(f"{f:.6e}")
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

validate_sql() {
  python3 -c '
import sys
n = int(sys.argv[1])
q = sys.argv[2]
u = q.upper()
if n == 18 and "ORDER BY" not in u:
    q = q.replace("LIMIT", "ORDER BY UserID, SearchPhrase LIMIT", 1)
elif n == 31:
    q = q.replace("ORDER BY c DESC", "ORDER BY c DESC, SearchEngineID, ClientIP", 1)
elif n in (32, 33):
    q = q.replace("ORDER BY c DESC", "ORDER BY c DESC, WatchID, ClientIP", 1)
elif n == 41:
    q = q.replace("ORDER BY PageViews DESC", "ORDER BY PageViews DESC, URLHash, EventDate", 1)
print(q)
' "$1" "$2"
}

i=1
while IFS= read -r q || [ -n "$q" ]; do
  [ -z "$q" ] && continue
  if ! should_run "$i"; then
    i=$((i + 1))
    continue
  fi

  vq=$(validate_sql "$i" "$q")

  crf=$(mktemp)
  dkf=$(mktemp)
  if ! coldrun_out "$vq" >"$crf" 2>>"$LOG"; then
    echo "FAIL Q$i coldrun error" | tee -a "$LOG"
    FAIL=$((FAIL + 1))
    i=$((i + 1))
    rm -f "$crf" "$dkf"
    continue
  fi
  if ! duckdb_out "$vq" >"$dkf" 2>>"$LOG"; then
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
    echo "  query: $vq" >>"$LOG"
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
