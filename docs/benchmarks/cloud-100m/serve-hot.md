# Serve hot — 100M rows (c6a.4xlarge)

**VM:** AWS `c6a.4xlarge` (32 GiB), `/data/coldrun` (~100M rows, V2 blockized reload)  
**Command:** `./scripts/bench-serve.sh 100000000 --skip-load --write-snapshot`  
**Protocol:** warm `serve`, 3 tries/query, hot = min(try 2, try 3)  
**Commit:** `657f98d`  
**Log:** `/data/bench-v2-warm-full-rerun.log`

| Q | hot (s) | Notes |
|---|---------|-------|
| 1 | 0.000 | |
| 2 | 0.000 | |
| 3 | 0.234 | |
| 4 | 0.152 | |
| 5 | 2.773 | |
| 6 | 1.171 | |
| 7 | 0.226 | |
| 8 | 0.246 | |
| 9 | 1.916 | |
| 10 | 4.386 | |
| 11 | 0.743 | |
| 12 | 0.771 | |
| 13 | 3.343 | |
| 14 | 9.324 | |
| 15 | 0.824 | |
| 16 | 3.115 | |
| 17 | 3.226 | |
| 18 | 1.183 | |
| 19 | 3.478 | |
| 20 | 0.053 | |
| 21 | 4.557 | |
| 22 | 4.881 | |
| 23 | 52.715 | tries [60.154, 52.720, 52.715] |
| 24 | 49.808 | tries [70.488, 49.808, 49.813] |
| 25 | 0.007 | |
| 26 | 0.276 | |
| 27 | 0.652 | |
| 28 | 1.772 | |
| 29 | 7.910 | |
| 30 | 0.971 | |
| 31 | 2.541 | |
| 32 | 2.762 | |
| 33 | 17.111 | |
| 34 | 15.362 | |
| 35 | 14.741 | |
| 36 | 84.582 | tries [83.109, 84.675, 84.582] |
| 37 | 3.855 | |
| 38 | 3.952 | |
| 39 | 3.077 | |
| 40 | 1.186 | |
| 41 | 7.423 | |
| 42 | 1.616 | |
| 43 | 1.313 | |

**Hot sums**

| Scope | Coldrun | ClickHouse ([c6a.4xlarge](https://github.com/ClickHouse/ClickBench/blob/main/clickhouse/results/20260516/c6a.4xlarge.json)) |
|-------|---------|----------------------------------------------------------------------------------------------------------------------------------|
| Q1–22 | 46.602s | ~9.6s |
| Q24–43 | 220.917s | ~22.8s |
| Q1–22 + Q24–43 (42 queries) | **267.519s** | **~32.4s** |
| All 43 | **320.234s** | **~32.4s** |

Not ClickBench Combined (no cold protocol, no `drop_caches` per query). See [`compare-hot.md`](compare-hot.md).

Targeted Q23/Q24-only rerun at `c107ad4` remains useful as an earlier diagnostic check, but this full 43-query run is now the canonical warm snapshot.
