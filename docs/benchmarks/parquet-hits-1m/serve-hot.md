# Serve hot — 1M parquet `hits` slice

**Command:** `COLDRUN_DATA=.coldrun-validate-hits-1m_ ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot`  
**Data:** `data/hits-1m.parquet` loaded into `.coldrun-validate-hits-1m_` (~107 MB on disk)  
**Protocol:** warm `serve`, 3 tries/query, hot = min(try 2, try 3)  
**Commit:** `8630a04`

| Q | hot (s) | cold (try 1) |
|---|---------|----------------|
| 1 | 0.000 | 0.001 |
| 2 | 0.002 | 0.002 |
| 3 | 0.005 | 0.006 |
| 4 | 0.006 | 0.009 |
| 5 | 0.010 | 0.010 |
| 6 | 0.014 | 0.021 |
| 7 | 0.004 | 0.005 |
| 8 | 0.003 | 0.003 |
| 9 | 0.077 | 0.081 |
| 10 | 0.023 | 0.024 |
| 11 | 0.012 | 0.014 |
| 12 | 0.014 | 0.016 |
| 13 | 0.018 | 0.019 |
| 14 | 0.026 | 0.028 |
| 15 | 0.018 | 0.020 |
| 16 | 0.024 | 0.025 |
| 17 | 0.035 | 0.035 |
| 18 | 0.035 | 0.036 |
| 19 | 1.068 | 1.139 |
| 20 | 0.006 | 0.007 |
| 21 | 0.113 | 0.161 |
| 22 | 0.128 | 0.129 |
| 23 | 0.296 | 0.406 |
| 24 | 0.146 | 0.196 |
| 25 | 0.017 | 0.018 |
| 26 | 0.013 | 0.014 |
| 27 | 0.019 | 0.019 |
| 28 | 0.297 | 0.299 |
| 29 | 1.116 | 1.115 |
| 30 | 0.026 | 0.027 |
| 31 | 0.023 | 0.024 |
| 32 | 0.023 | 0.024 |
| 33 | 0.023 | 0.023 |
| 34 | 0.122 | 0.122 |
| 35 | 0.483 | 0.609 |
| 36 | 0.155 | 0.159 |
| 37 | 0.130 | 0.131 |
| 38 | 0.211 | 0.225 |
| 39 | 0.115 | 0.116 |
| 40 | 0.308 | 0.393 |
| 41 | 0.255 | 0.273 |
| 42 | 0.040 | 0.043 |
| 43 | 0.321 | 0.340 |

**Hot sum (Q1–43):** 5.78s — ~30× slower than demo @100k; not leaderboard-valid without 100M + VM.

Slowest hot: Q29 (1.12s), Q19 (1.07s), Q35 (0.48s), Q43 (0.32s), Q40 (0.31s).

Regenerate (use absolute `COLDRUN_DATA`; unset `BENCH_QUERY_TO` if set):

```bash
COLDRUN_DATA="$PWD/.coldrun-validate-hits-1m_" \
BENCH_SNAPSHOT_SLUG=parquet-hits-1m \
env -u BENCH_QUERY_TO -u BENCH_QUERY_FROM \
  ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot
```
