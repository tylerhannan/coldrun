# Parquet measurement (no AWS)

Validate and time coldrun on real `hits` Parquet **on your machine**. You need the file locally (full ~15 GB or a slice).

## 0. ClickHouse reference (once)

Validation compares coldrun to **ClickHouse** on the same Parquet slice (not DuckDB):

```bash
./scripts/install-clickhouse-local.sh   # latest from https://clickhouse.com → clickhouse-local/clickhouse
```

## 1. Get data

```bash
# Full file (large download)
curl -LO https://datasets.clickhouse.com/hits_compatible/hits.parquet

# Or slice (ClickHouse local)
./scripts/sample-parquet.sh hits.parquet 1000000 hits-1m.parquet
```

## 2. Correctness vs ClickHouse

```bash
./scripts/validate-parquet.sh hits-1m.parquet --from 1 --to 15
./scripts/validate-parquet.sh hits-1m.parquet   # all 43
```

Logs: `logs/benchmarks/validate-*.log`

## 3. Hot-shaped timing on parquet load

```bash
./scripts/measure-parquet.sh data/hits-1m.parquet
```

**Latest @ 1M rows:** [`../parquet-hits-1m/compare-hot.md`](../parquet-hits-1m/compare-hot.md) — coldrun hot **0.84s**, ClickHouse **1.34s** (0.62×)  
**Validation:** [`validation-1m.md`](validation-1m.md) — **43/43** vs ClickHouse on same slice.

### Hot sum vs ClickHouse (same 1M slice, laptop)

| Engine | Protocol | Sum Q1–43 |
|--------|----------|-----------|
| **coldrun** | warm `serve`, hot = min(try 2, 3) | **0.84s** ([`serve-hot.md`](../parquet-hits-1m/serve-hot.md)) |
| **ClickHouse** | `clickhouse local --time`, `file()` Parquet, hot = min(try 2, 3) | **1.34s** ([`clickhouse-hot.md`](../parquet-hits-1m/clickhouse-hot.md)) |

Regenerate side-by-side:

```bash
COLDRUN_DATA=.coldrun-validate-hits-1m_ ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot
./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot --compare
```

Not ClickBench Combined — use for relative tuning only. Largest coldrun gaps vs CH: Q41, Q36, Q29.

Stream a slice without the full 15 GB download:

```bash
./scripts/sample-parquet.sh https://datasets.clickhouse.com/hits_compatible/hits.parquet 1000000 data/hits-1m.parquet
```

## What this is not

- Not ClickBench Combined (no `c6a.4xlarge`, no published JSON)
- Not a substitute for full 100M validation — start with 100k–1M row slices

See also [`../MEASUREMENT.md`](../MEASUREMENT.md) for demo workflows.
