# ClickHouse hot — 1M parquet `hits` slice

**Command:** `./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot`  
**Protocol:** `clickhouse local --time`, `file()` Parquet, 3 tries/query, hot = min(try 2, try 3), `max_threads=1`  
**Note:** new `clickhouse local` process per try (Parquet OS cache warm after try 1); coldrun uses warm `serve` — see [`compare-hot.md`](compare-hot.md).
**Commit:** `e2f1527`
**Parquet size:** 70131061 bytes

| Q | hot (s) | cold (try 1) |
|---|---------|----------------|
| 1 | 0.006 | 0.026 |
| 2 | 0.006 | 0.016 |
| 3 | 0.007 | 0.010 |
| 4 | 0.007 | 0.010 |
| 5 | 0.010 | 0.011 |
| 6 | 0.018 | 0.019 |
| 7 | 0.007 | 0.010 |
| 8 | 0.007 | 0.013 |
| 9 | 0.015 | 0.018 |
| 10 | 0.024 | 0.023 |
| 11 | 0.009 | 0.010 |
| 12 | 0.009 | 0.009 |
| 13 | 0.018 | 0.018 |
| 14 | 0.013 | 0.013 |
| 15 | 0.014 | 0.014 |
| 16 | 0.012 | 0.012 |
| 17 | 0.049 | 0.050 |
| 18 | 0.044 | 0.043 |
| 19 | 0.091 | 0.093 |
| 20 | 0.005 | 0.006 |
| 21 | 0.032 | 0.037 |
| 22 | 0.030 | 0.031 |
| 23 | 0.041 | 0.048 |
| 24 | 0.074 | 0.101 |
| 25 | 0.012 | 0.012 |
| 26 | 0.017 | 0.018 |
| 27 | 0.011 | 0.012 |
| 28 | 0.035 | 0.099 |
| 29 | 0.708 | 0.712 |
| 30 | 0.089 | 0.088 |
| 31 | 0.014 | 0.014 |
| 32 | 0.019 | 0.019 |
| 33 | 0.162 | 0.156 |
| 34 | 0.112 | 0.113 |
| 35 | 0.105 | 0.112 |
| 36 | 0.012 | 0.014 |
| 37 | 0.074 | 0.075 |
| 38 | 0.034 | 0.027 |
| 39 | 0.027 | 0.032 |
| 40 | 0.179 | 0.185 |
| 41 | 0.017 | 0.021 |
| 42 | 0.016 | 0.016 |
| 43 | 0.014 | 0.015 |

**Hot sum (Q1–43):** 2.205000s

Regenerate:

```bash
./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot
```
