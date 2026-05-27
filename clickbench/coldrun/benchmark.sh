#!/bin/bash
# ClickBench entry: use upstream driver when present, else repo-local benchmark-local.sh.
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
export BENCH_DOWNLOAD_SCRIPT="${BENCH_DOWNLOAD_SCRIPT-}"
export BENCH_DURABLE="${BENCH_DURABLE:-yes}"
export BENCH_RESTARTABLE="${BENCH_RESTARTABLE:-yes}"

if [ -f "$DIR/../lib/benchmark-common.sh" ]; then
  exec "$DIR/../lib/benchmark-common.sh"
fi

exec "$DIR/benchmark-local.sh"
