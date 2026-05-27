#!/bin/bash
# ClickBench-format benchmark locally (serve + 3 tries per query). No cloud VM required.
#
# Usage:
#   ./scripts/bench-clickbench.sh --demo 100000     # synthetic hits (default)
#   ./scripts/bench-clickbench.sh --skip-load       # reuse COLDRUN_DATA
#   ./scripts/bench-clickbench.sh hits.parquet      # full parquet load (slow/large)
#
# Output: Load time, Data size, 43 lines of [t1,t2,t3], plus result.csv in clickbench/coldrun/
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export PATH="${HOME}/.cargo/bin:${PATH}"
export COLDRUN_ROOT="$ROOT"
export COLDRUN_BIN="$ROOT/target/release/coldrun"
export COLDRUN_DATA="${COLDRUN_DATA:-$ROOT/clickbench/coldrun/.clickbench}"
export BENCH_RESTARTABLE="${BENCH_RESTARTABLE:-yes}"

BENCH="$ROOT/clickbench/coldrun"
chmod +x "$BENCH"/*.sh "$BENCH"/query "$BENCH"/lib/*.sh 2>/dev/null || true

cargo build --release -p coldrun-cli -q

COLDRUN_SKIP_LOAD=0
COLDRUN_SKIP_BUILD=1
DEMO_ROWS=""

while [ $# -gt 0 ]; do
  case "$1" in
    --demo)
      DEMO_ROWS="${2:-100000}"
      shift 2
      ;;
    --skip-load)
      COLDRUN_SKIP_LOAD=1
      shift
      ;;
    --embedded)
      export BENCH_RESTARTABLE=no
      shift
      ;;
    *)
      if [ -f "$1" ]; then
        export HITS_PARQUET="$1"
        rm -rf "$COLDRUN_DATA"
      else
        echo "Unknown argument: $1" >&2
        exit 1
      fi
      shift
      ;;
  esac
done

if [ "$COLDRUN_SKIP_LOAD" = "0" ]; then
  rm -rf "$COLDRUN_DATA"
  if [ -n "$DEMO_ROWS" ]; then
    "$COLDRUN_BIN" --data-dir "$COLDRUN_DATA" local --demo "$DEMO_ROWS" >/dev/null
    echo "loaded demo $DEMO_ROWS rows into hits" >&2
  elif [ -n "${HITS_PARQUET:-}" ]; then
    HITS_PARQUET="$HITS_PARQUET" "$BENCH/load"
  else
    "$COLDRUN_BIN" --data-dir "$COLDRUN_DATA" local --demo 100000 >/dev/null
    echo "loaded demo 100000 rows (use --demo N or path.parquet)" >&2
  fi
fi

mkdir -p "${COLDRUN_BENCH_LOG_DIR:-$ROOT/logs/benchmarks}"
export COLDRUN_SKIP_LOAD=1
"$BENCH/benchmark-local.sh" | tee "${COLDRUN_BENCH_LOG:-$ROOT/logs/benchmarks/clickbench-last.log}"
