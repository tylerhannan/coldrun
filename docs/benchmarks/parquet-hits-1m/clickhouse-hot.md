# ClickHouse hot — 1M parquet `hits` slice

**Command:** `./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot`  
**Protocol:** `clickhouse local --time`, `file()` Parquet, 3 tries/query, hot = min(try 2, try 3), `max_threads=1`  
**Note:** new `clickhouse local` process per try (Parquet OS cache warm after try 1); coldrun uses warm `serve` — see [`compare-hot.md`](compare-hot.md).
**Commit:** `f6e45f3`
**Parquet size:** 70131061 bytes

| Q | hot (s) | cold (try 1) |
|---|---------|----------------|
| 1 | 0.006 | 0.048 |
| 2 | 0.007 | 0.017 |
| 3 | 0.012 | 0.011 |
| 4 | 0.008 | 0.013 |
| 5 | 0.010 | 0.012 |
| 6 | 0.022 | 0.024 |
| 7 | 0.008 | 0.011 |
| 8 | 0.007 | 0.011 |
| 9 | 0.014 | 0.016 |
| 10 | 0.022 | 0.023 |
| 11 | 0.009 | 0.010 |
| 12 | 0.008 | 0.010 |
| 13 | 0.017 | 0.018 |
| 14 | 0.013 | 0.014 |
| 15 | 0.014 | 0.015 |
| 16 | 0.012 | 0.013 |
| 17 | 0.051 | 0.050 |
| 18 | 0.043 | 0.042 |
| 19 | 0.088 | 0.095 |
| 20 | 0.005 | 0.006 |
| 21 | 0.028 | 0.052 |
| 22 | 0.026 | 0.034 |
| 23 | 0.036 | 0.040 |
| 24 | 0.070 | 0.080 |
| 25 | 0.011 | 0.012 |
| 26 | 0.016 | 0.016 |
| 27 | 0.011 | 0.012 |
| 28 | 0.037 | 0.039 |
| 29 | 0.698 | 0.697 |
| 30 | 0.086 | 0.082 |
| 31 | 0.013 | 0.014 |
| 32 | 0.017 | 0.018 |
| 33 | 0.144 | 0.153 |
| 34 | 0.102 | 0.108 |
| 35 | 0.111 | 0.104 |
| 36 | 0.013 | 0.013 |
| 37 | 0.076 | 0.108 |
| 38 | 0.026 | 0.028 |
| 39 | 0.025 | 0.032 |
| 40 | 0.173 | 0.178 |
| 41 | 0.016 | 0.019 |
| 42 | 0.015 | 0.015 |
| 43 | 0.014 | 0.015 |

**Hot sum (Q1–43):** 2.140000s

Regenerate:

```bash
./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot
```
