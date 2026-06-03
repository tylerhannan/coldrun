# Validation — 1M row `hits` slice

**Date:** 2026-06-01  
**Data:** `data/hits-1m.parquet` (streamed from ClickHouse public dataset)  
**Reference:** ClickHouse local (`clickhouse-local/clickhouse`, install via `./scripts/install-clickhouse-local.sh`)  
**Command:** `./scripts/validate-parquet.sh data/hits-1m.parquet --skip-load`

## Summary

| Result | Count |
|--------|------:|
| PASS | 43 |
| FAIL | 0 |
| SKIP | 0 |

## Validation protocol

- ClickHouse reads the same Parquet via `file(..., Parquet)` with `EventDate` / `EventTime` cast to ClickBench types.
- `max_threads = 1` for deterministic tie order.
- Tie-heavy queries (Q18, Q29–31, Q41) compare against SQL extended with explicit `ORDER BY` group-key columns.
- `LENGTH()` uses UTF-8 byte count (ClickHouse semantics).
- Q4 `AVG(UserID)`: reference uses `avg(toFloat64(UserID))` (Int64 sum overflows on parquet `file()`).
- Large `AVG(UserID)` (Q4) normalized to `%.6e` before diff.

## Regenerate

```bash
./scripts/install-clickhouse-local.sh
./scripts/sample-parquet.sh https://datasets.clickhouse.com/hits_compatible/hits.parquet 1000000 data/hits-1m.parquet
./scripts/validate-parquet.sh data/hits-1m.parquet
COLDRUN_DATA="$PWD/.coldrun-validate-hits-1m_" BENCH_SNAPSHOT_SLUG=parquet-hits-1m \
  env -u BENCH_QUERY_TO -u BENCH_QUERY_FROM \
  ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot
```

## Hot timing (serve, Q1–43)

Snapshot: [`../parquet-hits-1m/serve-hot.md`](../parquet-hits-1m/serve-hot.md) — hot sum **1.32s** @ 1M rows (~**0.63×** ClickHouse ~2.1s on same slice).
