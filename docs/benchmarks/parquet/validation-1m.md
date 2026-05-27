# Validation — 1M row `hits` slice

**Date:** 2026-05-27  
**Data:** `data/hits-1m.parquet` (streamed from ClickHouse public dataset)  
**Command:** `./scripts/validate-parquet.sh data/hits-1m.parquet`

## Summary

| Result | Count |
|--------|------:|
| PASS | 30 |
| FAIL | 6 |
| SKIP (DuckDB SQL) | 7 |

## Fixes landed for this run

- Parquet load: `EventTime` Int64 seconds → micros, `EventDate` UInt16, unsigned ints, null → default
- Q8: `AdvEngineID` GROUP BY uses hash map (not demo-only `[0;8]` buckets)
- Q9: int GROUP BY prints numeric keys (not empty utf8)

## Known mismatches (investigate)

| Q | Issue |
|---|--------|
| 18 | No `ORDER BY` — top-10 groups are implementation-defined |
| 28 | `AVG(length(URL))` differs (~2%) — likely byte vs char `length` |
| 31–33 | Tie-heavy `ORDER BY c DESC` with many count=1 pairs |
| 40 | CASE/`Src` alias path — needs engine work |

## DuckDB skips (dashboard)

Q37–Q39, Q41–Q43: DuckDB rejects or errors on the exact ClickBench SQL in this CLI version; coldrun may still run them on demo. Re-check with a pinned DuckDB version or alternate reference.

## Hot timing (serve, Q1–39)

Full bench stops at Q40 today. Partial hot snapshot: [`../parquet-hits-1m/serve-hot.md`](../parquet-hits-1m/serve-hot.md).

Slowest hot @ 1M: Q19 ~1.07s, Q35 ~0.48s, Q23 ~0.30s (vs demo @100k all &lt;0.02s).

## Regenerate

```bash
./scripts/sample-parquet.sh https://datasets.clickhouse.com/hits_compatible/hits.parquet 1000000 data/hits-1m.parquet
./scripts/validate-parquet.sh data/hits-1m.parquet
./scripts/measure-parquet.sh data/hits-1m.parquet --bench-only --skip-validate
```
