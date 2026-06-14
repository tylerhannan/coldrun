# ClickHouse hot — 1M parquet `hits` slice

**Command:** `./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot`  
**Protocol:** `clickhouse local --time`, `file()` Parquet, 3 tries/query, hot = min(try 2, try 3), `max_threads=1`  
**Note:** new `clickhouse local` process per try (Parquet OS cache warm after try 1); coldrun uses warm `serve` — see [`compare-hot.md`](compare-hot.md).
**Commit:** `3eb4e05`
**Parquet size:** 86110992 bytes

| Q | hot (s) | cold (try 1) |
|---|---------|----------------|
| 1 | 0.005 | 0.005 |
| 2 | 0.003 | 0.003 |
| 3 | 0.003 | 0.003 |
| 4 | 0.004 | 0.004 |
| 5 | 0.006 | 0.007 |
| 6 | 0.010 | 0.010 |
| 7 | 0.003 | 0.003 |
| 8 | 0.003 | 0.003 |
| 9 | 0.007 | 0.007 |
| 10 | 0.014 | 0.014 |
| 11 | 0.006 | 0.006 |
| 12 | 0.006 | 0.006 |
| 13 | 0.007 | 0.008 |
| 14 | 0.009 | 0.010 |
| 15 | 0.009 | 0.009 |
| 16 | 0.007 | 0.007 |
| 17 | 0.023 | 0.023 |
| 18 | 0.022 | 0.023 |
| 19 | 0.047 | 0.051 |
| 20 | 0.002 | 0.005 |
| 21 | 0.029 | 0.029 |
| 22 | 0.035 | 0.035 |
| 23 | 0.079 | 0.080 |
| 24 | 0.061 | 0.063 |
| 25 | 0.009 | 0.010 |
| 26 | 0.006 | 0.007 |
| 27 | 0.009 | 0.009 |
| 28 | 0.034 | 0.037 |
| 29 | 0.410 | 0.411 |
| 30 | 0.009 | 0.009 |
| 31 | 0.011 | 0.011 |
| 32 | 0.013 | 0.013 |
| 33 | 0.063 | 0.064 |
| 34 | 0.064 | 0.063 |
| 35 | 0.063 | 0.064 |
| 36 | 0.007 | 0.007 |
| 37 | 0.048 | 0.051 |
| 38 | 0.027 | 0.030 |
| 39 | 0.029 | 0.031 |
| 40 | 0.093 | 0.094 |
| 41 | 0.018 | 0.018 |
| 42 | 0.015 | 0.021 |
| 43 | 0.011 | 0.011 |

**Hot sum (Q1–43):** 1.339000s

Regenerate:

```bash
./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot
```
