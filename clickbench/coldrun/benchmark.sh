#!/bin/bash
# When copied into ClickHouse/ClickBench, exec ../lib/benchmark-common.sh
# From this repo, use scripts/repro-local.sh for a minimal smoke run.
export BENCH_DOWNLOAD_SCRIPT=""
export BENCH_DURABLE=yes
export BENCH_RESTARTABLE=no
if [ -f ../lib/benchmark-common.sh ]; then
  exec ../lib/benchmark-common.sh
fi
echo "benchmark.sh: run from ClickBench tree or use scripts/repro-local.sh" >&2
exit 1
