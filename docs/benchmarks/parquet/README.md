# Parquet measurement (no AWS)

Validate and time coldrun on real `hits` Parquet **on your machine**. You need the file locally (full ~15 GB or a slice).

## 1. Get data

```bash
# Full file (large download)
curl -LO https://datasets.clickhouse.com/hits_compatible/hits.parquet

# Or slice if you already have the full file (needs DuckDB)
./scripts/sample-parquet.sh hits.parquet 1000000 hits-1m.parquet
```

## 2. Correctness vs DuckDB

```bash
brew install duckdb   # once
./scripts/validate-parquet.sh hits-1m.parquet --from 1 --to 15
./scripts/validate-parquet.sh hits-1m.parquet   # all 43
```

Logs: `logs/benchmarks/validate-*.log`

## 3. Hot-shaped timing on parquet load

```bash
./scripts/measure-parquet.sh data/hits-1m.parquet
```

**Latest @ 1M rows:** [`../parquet-hits-1m/serve-hot.md`](../parquet-hits-1m/serve-hot.md) (Q1–39; Q40+ blocked on engine)  
**Validation log:** [`validation-1m.md`](validation-1m.md) — 30/43 pass vs DuckDB on same slice.

Stream a slice without the full 15 GB download:

```bash
./scripts/sample-parquet.sh https://datasets.clickhouse.com/hits_compatible/hits.parquet 1000000 data/hits-1m.parquet
```

## What this is not

- Not ClickBench Combined (no `c6a.4xlarge`, no published JSON)
- Not a substitute for full 100M validation — start with 100k–1M row slices

See also [`../MEASUREMENT.md`](../MEASUREMENT.md) for demo workflows.
