# Serve hot — 1M parquet `hits` slice

**Command:** `COLDRUN_DATA=.coldrun-validate-hits-1m_ ./scripts/bench-serve.sh 1000000 --skip-load --no-compare`  
**Protocol:** warm `serve`, 3 tries/query, hot = min(try 2, try 3)  
**Commit:** `49b3a7f`
**Data size:** 107069440 bytes

| Q | hot (s) | cold (try 1) |
|---|---------|----------------|
| 1 | 0.000 | 0.001 |
| 2 | 0.000 | 0.003 |
| 3 | 0.002 | 0.007 |
| 4 | 0.001 | 0.009 |
| 5 | 0.006 | 0.005 |
| 6 | 0.003 | 0.012 |
| 7 | 0.001 | 0.004 |
| 8 | 0.001 | 0.001 |
| 9 | 0.069 | 0.074 |
| 10 | 0.013 | 0.013 |
| 11 | 0.003 | 0.011 |
| 12 | 0.003 | 0.005 |
| 13 | 0.006 | 0.007 |
| 14 | 0.010 | 0.011 |
| 15 | 0.011 | 0.012 |
| 16 | 0.020 | 0.026 |
| 17 | 0.038 | 0.083 |
| 18 | 0.038 | 0.046 |
| 19 | 0.105 | 0.118 |
| 20 | 0.001 | 0.001 |
| 21 | 0.035 | 0.166 |
| 22 | 0.038 | 0.036 |
| 23 | 0.007 | 0.131 |
| 24 | 0.058 | 0.106 |
| 25 | 0.001 | 0.001 |
| 26 | 0.001 | 0.001 |
| 27 | 0.003 | 0.003 |
| 28 | 0.006 | 0.009 |
| 29 | 0.036 | 0.079 |
| 30 | 0.006 | 0.006 |
| 31 | 0.004 | 0.008 |
| 32 | 0.005 | 0.007 |
| 33 | 0.033 | 0.051 |
| 34 | 0.061 | 0.119 |
| 35 | 0.053 | 0.053 |
| 36 | 0.199 | 0.226 |
| 37 | 0.057 | 0.058 |
| 38 | 0.042 | 0.047 |
| 39 | 0.034 | 0.036 |
| 40 | 0.045 | 0.049 |
| 41 | 0.227 | 0.243 |
| 42 | 0.018 | 0.021 |
| 43 | 0.017 | 0.017 |

**Hot sum (Q1–43):** 1.317000s

ClickHouse comparison: [`compare-hot.md`](compare-hot.md) · [`clickhouse-hot.md`](clickhouse-hot.md)

Regenerate:

```bash
COLDRUN_DATA="$PWD/.coldrun-validate-hits-1m_" BENCH_SNAPSHOT_SLUG=parquet-hits-1m \
  env -u BENCH_QUERY_TO -u BENCH_QUERY_FROM \
  ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot
```
