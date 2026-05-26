# ClickBench harness (coldrun)

Scripts for the official [ClickBench](https://github.com/ClickHouse/ClickBench) cold-run on a cloud VM with full `hits.parquet`.

## Prerequisites

- Ubuntu 22.04+ (or similar), `c6a.4xlarge` or equivalent
- `hits.parquet` at `/data/hits.parquet` (or set `HITS_PARQUET`)
- Rust toolchain (`install` script can install via rustup)

## Layout

| Script | Role |
|--------|------|
| `install` | `cargo build --release`, install `coldrun` binary |
| `load` | Import Parquet into `.coldrun/` column store |
| `benchmark.sh` | Run all queries from `queries.sql` |
| `start` / `stop` / `check` | ClickBench lifecycle hooks |
| `query` | Single-query helper for debugging |

## Environment

```bash
export COLDRUN_ROOT=/path/to/coldrun   # repo root (auto-detected from script path)
export HITS_PARQUET=/data/hits.parquet # default in load script
export COLDRUN_DATA=/data/coldrun      # on-disk column store
```

## Quick repro (from repo root)

```bash
./clickbench/coldrun/install
./clickbench/coldrun/load
./clickbench/coldrun/benchmark.sh
```

## Local demo (no full dataset)

On a laptop, use synthetic data instead:

```bash
./scripts/smoke-all.sh
./scripts/bench-all.sh 100000
./scripts/bench-compare.sh 100000   # before/after regression
```

See [`docs/PERF.md`](../../docs/PERF.md) for optimization notes and committed overnight baselines.
