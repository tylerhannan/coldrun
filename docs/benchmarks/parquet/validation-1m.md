# Validation — 1M row `hits` slice

**Date:** 2026-06-01  
**Data:** `data/hits-1m.parquet` (streamed from ClickHouse public dataset)  
**Command:** `./scripts/validate-parquet.sh data/hits-1m.parquet --skip-load`

## Summary

| Result | Count |
|--------|------:|
| PASS | 43 |
| FAIL | 0 |
| SKIP | 0 |

## Validation protocol

- DuckDB runs with `PRAGMA threads=1` for deterministic tie order.
- Tie-heavy queries (Q18, Q31–33, Q41) compare against SQL extended with explicit `ORDER BY` group-key columns so both engines use the same deterministic sort.
- `LENGTH()` uses Unicode code-point count (DuckDB semantics).
- Large `AVG(UserID)` (Q4) and `DATE_TRUNC` timestamps (Q43) normalized before diff.

## Regenerate

```bash
./scripts/sample-parquet.sh https://datasets.clickhouse.com/hits_compatible/hits.parquet 1000000 data/hits-1m.parquet
./scripts/validate-parquet.sh data/hits-1m.parquet
COLDRUN_DATA="$PWD/.coldrun-validate-hits-1m_" BENCH_SNAPSHOT_SLUG=parquet-hits-1m \
  env -u BENCH_QUERY_TO -u BENCH_QUERY_FROM \
  ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot
```

## Hot timing (serve, Q1–43)

Snapshot: [`../parquet-hits-1m/serve-hot.md`](../parquet-hits-1m/serve-hot.md) — hot sum **5.44s** @ 1M rows (Q29/Q35/Q43 fused paths).
