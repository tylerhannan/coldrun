#!/bin/bash
# Capture regression logs for overnight runs (not committed; see docs/overnight/).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export PATH="${HOME}/.cargo/bin:${PATH}"

ROWS="${1:-100000}"
LOGDIR="${LOGDIR:-$ROOT/logs/overnight}"
mkdir -p "$LOGDIR"

cargo build --release -p coldrun-cli -q

echo "=== smoke-all ${ROWS} rows ===" | tee "$LOGDIR/smoke-${ROWS}.log"
./scripts/smoke-all.sh "$ROWS" 2>&1 | tee -a "$LOGDIR/smoke-${ROWS}.log"

echo "=== bench-demo ${ROWS} rows ===" | tee "$LOGDIR/bench-${ROWS}.log"
./scripts/bench-demo.sh "$ROWS" 2>&1 | tee -a "$LOGDIR/bench-${ROWS}.log"

echo "Logs: $LOGDIR"
