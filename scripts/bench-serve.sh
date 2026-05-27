#!/bin/bash
# Warm-server timing: one coldrun serve process, 3 tries per query (ClickBench hot shape).
# No per-query restart — fast local measurement; not the full cold-run protocol.
#
# Usage:
#   ./scripts/bench-serve.sh 100000
#   ./scripts/bench-serve.sh 100000 --from 1 --to 10
#   ./scripts/bench-serve.sh 100000 --queries 1,6,23,40
#   ./scripts/bench-serve.sh 100000 --compare-only    # Q1 CLI vs serve, then exit
#
# Output: Load time, Data size, [t1,t2,t3], lines + hot summary on stderr.
# Artifacts: clickbench/coldrun/result.csv, logs/benchmarks/serve-last.log
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export PATH="${HOME}/.cargo/bin:${PATH}"
export COLDRUN_ROOT="$ROOT"
export COLDRUN_BIN="$ROOT/target/release/coldrun"
export COLDRUN_DATA="${COLDRUN_DATA:-$ROOT/clickbench/coldrun/.clickbench-serve}"

export BENCH_RESTARTABLE=no
export BENCH_DURABLE=no
export BENCH_TRIES=3
export BENCH_PROGRESS=1
export BENCH_PRINT_HOT_SUMMARY=1

BENCH="$ROOT/clickbench/coldrun"
chmod +x "$BENCH"/*.sh "$BENCH"/query "$BENCH"/lib/*.sh 2>/dev/null || true

cargo build --release -p coldrun-cli -q

ROWS=100000
SKIP_LOAD=0
COMPARE_LOCAL=0
COMPARE_ONLY=0

while [ $# -gt 0 ]; do
  case "$1" in
    --from)
      export BENCH_QUERY_FROM="${2:?}"
      shift 2
      ;;
    --to)
      export BENCH_QUERY_TO="${2:?}"
      shift 2
      ;;
    --queries)
      list=$(echo "${2:?}" | tr ',' ' ')
      export BENCH_QUERY_LIST="$list"
      shift 2
      ;;
    --skip-load)
      SKIP_LOAD=1
      shift
      ;;
    --compare-only)
      COMPARE_LOCAL=1
      COMPARE_ONLY=1
      shift
      ;;
    --help|-h)
      sed -n '2,14p' "$0"
      exit 0
      ;;
    *)
      if [[ "$1" =~ ^[0-9]+$ ]]; then
        ROWS="$1"
      else
        echo "Unknown argument: $1" >&2
        exit 1
      fi
      shift
      ;;
  esac
done

if [ "$SKIP_LOAD" = "0" ]; then
  rm -rf "$COLDRUN_DATA"
  "$COLDRUN_BIN" --data-dir "$COLDRUN_DATA" local --demo "$ROWS" >/dev/null
  echo "loaded demo $ROWS rows into hits" >&2
fi

if [ "$COMPARE_LOCAL" = "1" ]; then
  q='SELECT COUNT(*) FROM hits;'
  errf=$(mktemp)
  printf '%s\n' "$q" | COLDRUN_BENCH=1 "$COLDRUN_BIN" --data-dir "$COLDRUN_DATA" local >/dev/null 2>"$errf"
  local_t=$(tr '\n' '\n' <"$errf" | grep -E '^[0-9]+(\.[0-9]+)?$' | tail -n1)
  rm -f "$errf"
  cd "$BENCH"
  ./start >/dev/null 2>&1
  sleep 0.3
  errf=$(mktemp)
  printf '%s\n' "$q" | ./query >/dev/null 2>"$errf"
  serve_t=$(tr '\n' '\n' <"$errf" | grep -E '^[0-9]+(\.[0-9]+)?$' | tail -n1)
  rm -f "$errf"
  ./stop >/dev/null 2>&1 || true
  cd "$ROOT"
  echo "=== Q1 overhead (fresh CLI vs warm serve, seconds) ===" >&2
  echo "local (new process): $local_t" >&2
  echo "serve (warm):        $serve_t" >&2
  if [ -n "$local_t" ] && [ -n "$serve_t" ] && awk "BEGIN { exit !($local_t > 0) }"; then
    awk -v l="$local_t" -v s="$serve_t" 'BEGIN {
      printf "serve is %.1fx faster than local for Q1\n", l / s
    }' >&2
  fi
  if [ "$COMPARE_ONLY" = "1" ]; then
    exit 0
  fi
fi

mkdir -p "${COLDRUN_BENCH_LOG_DIR:-$ROOT/logs/benchmarks}"
export COLDRUN_SKIP_LOAD=1
export COLDRUN_SKIP_BUILD=1
"$BENCH/benchmark-local.sh" | tee "${COLDRUN_BENCH_LOG:-$ROOT/logs/benchmarks/serve-last.log}"
