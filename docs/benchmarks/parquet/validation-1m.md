# Validation — 1M row `hits` slice

**Date:** 2026-05-27  
**Data:** `data/hits-1m.parquet` (streamed from ClickHouse public dataset)  
**Command:** `./scripts/validate-parquet.sh data/hits-1m.parquet --skip-load`

## Summary

| Result | Count |
|--------|------:|
| PASS | 36 |
| FAIL | 7 |
| SKIP | 0 |

## Fixes in this round

- **Q40:** fused path accepts `Src`/`Dst` aliases and `CASE` in SELECT
- **DuckDB view:** cast `EventDate` / `EventTime` so dashboard Q37–39 compare (no DuckDB skips)
- **Output:** `EventDate` and min/max print as `YYYY-MM-DD`; `DATE_TRUNC` buckets as PDT wall time (Q7, Q43)
- **LENGTH:** byte length for DuckDB alignment

## Known mismatches

| Q | Issue |
|---|--------|
| 18 | No `ORDER BY` — top-10 groups are implementation-defined |
| 19 | `extract(minute FROM EventTime)` |
| 28 | `AVG(length(URL))` ~2% — residual byte/unicode or null handling |
| 31–33 | Tie-heavy `ORDER BY c DESC` with many count=1 pairs |
| 41 | Same PageViews tie band — different URLHash rows at OFFSET 100 |

## Hot timing (serve, Q1–43)

Snapshot: [`../parquet-hits-1m/serve-hot.md`](../parquet-hits-1m/serve-hot.md) — hot sum **5.78s** @ 1M rows (Q40 ~0.31s hot on fused path).

## Regenerate

```bash
./scripts/sample-parquet.sh https://datasets.clickhouse.com/hits_compatible/hits.parquet 1000000 data/hits-1m.parquet
./scripts/validate-parquet.sh data/hits-1m.parquet
COLDRUN_DATA="$PWD/.coldrun-validate-hits-1m_" BENCH_SNAPSHOT_SLUG=parquet-hits-1m \
  env -u BENCH_QUERY_TO -u BENCH_QUERY_FROM \
  ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot
```
