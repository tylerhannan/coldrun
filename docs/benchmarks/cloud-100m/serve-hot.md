# Serve hot — 100M rows (c6a.4xlarge)

**VM:** AWS `c6a.4xlarge` (32 GiB), `/data/coldrun` (~100M rows, V2 blockized reload)  
**Command:** `./scripts/bench-serve.sh 100000000 --skip-load --write-snapshot`  
**Protocol:** warm `serve`, 3 tries/query, hot = min(try 2, try 3)  
**Commit:** `80c09f0`  
**Log:** `/data/bench-v2-warm-full.log`

| Q | hot (s) | Notes |
|---|---------|-------|
| 1 | 0.000 | |
| 2 | 0.000 | |
| 3 | 0.232 | |
| 4 | 0.148 | |
| 5 | 2.727 | |
| 6 | 1.136 | |
| 7 | 0.282 | |
| 8 | 0.260 | |
| 9 | 1.910 | |
| 10 | 4.363 | |
| 11 | 0.749 | |
| 12 | 0.792 | |
| 13 | 3.354 | |
| 14 | 9.352 | |
| 15 | 0.812 | |
| 16 | 3.112 | |
| 17 | 3.205 | |
| 18 | 1.178 | |
| 19 | 3.478 | |
| 20 | 0.053 | |
| 21 | 4.517 | |
| 22 | 5.084 | |
| 23 | 56.151 | tries [56.839, 56.151, 56.275] |
| 24 | 49.990 | tries [84.866, 50.050, 49.990] |
| 25 | 0.007 | |
| 26 | 0.277 | |
| 27 | 0.657 | |
| 28 | 1.775 | |
| 29 | 7.953 | |
| 30 | 0.972 | |
| 31 | 2.594 | |
| 32 | 2.790 | |
| 33 | 17.108 | |
| 34 | 14.839 | |
| 35 | 14.769 | |
| 36 | 82.782 | |
| 37 | 3.790 | |
| 38 | 3.953 | |
| 39 | 3.026 | |
| 40 | 1.197 | |
| 41 | 7.536 | |
| 42 | 1.627 | |
| 43 | 1.306 | |

**Hot sums**

| Scope | Coldrun | ClickHouse ([c6a.4xlarge](https://github.com/ClickHouse/ClickBench/blob/main/clickhouse/results/20260516/c6a.4xlarge.json)) |
|-------|---------|----------------------------------------------------------------------------------------------------------------------------------|
| Q1–22 | 45.69s | ~9.6s |
| Q24–43 | 186.79s | ~22.8s |
| Q1–22 + Q24–43 (42 queries) | **232.48s** | **~32.4s** |
| All 43 | **321.843s** | **~32.4s** |

Not ClickBench Combined (no cold protocol, no `drop_caches` per query). See [`compare-hot.md`](compare-hot.md).

Targeted Q23/Q24-only rerun at `c107ad4` remains useful as an earlier diagnostic check, but this full 43-query run is now the canonical warm snapshot.
