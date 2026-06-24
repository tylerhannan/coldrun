# Serve hot — 100M rows (c6a.4xlarge)

**VM:** AWS `c6a.4xlarge` (32 GiB), `/data/coldrun` (~100M rows, V2 blockized reload)  
**Command:** `./scripts/bench-serve.sh 100000000 --skip-load --write-snapshot`  
**Protocol:** warm `serve`, 3 tries/query, hot = min(try 2, try 3)  
**Commit:** `5288c02`  
**Log:** `/data/bench-v2-warm-q36fast.log`

| Q | hot (s) | Notes |
|---|---------|-------|
| 1 | 0.000 | |
| 2 | 0.000 | |
| 3 | 0.234 | |
| 4 | 0.152 | |
| 5 | 2.748 | |
| 6 | 1.131 | |
| 7 | 0.227 | |
| 8 | 0.249 | |
| 9 | 1.905 | |
| 10 | 4.434 | |
| 11 | 0.747 | |
| 12 | 0.777 | |
| 13 | 3.366 | |
| 14 | 9.327 | |
| 15 | 0.786 | |
| 16 | 3.152 | |
| 17 | 3.223 | |
| 18 | 1.215 | |
| 19 | 3.466 | |
| 20 | 0.053 | |
| 21 | 4.482 | |
| 22 | 4.897 | |
| 23 | 55.158 | tries [62.356, 55.158, 55.165] |
| 24 | 49.663 | tries [72.733, 49.720, 49.663] |
| 25 | 0.008 | |
| 26 | 0.277 | |
| 27 | 0.658 | |
| 28 | 1.773 | |
| 29 | 7.824 | |
| 30 | 0.972 | |
| 31 | 2.607 | |
| 32 | 2.815 | |
| 33 | 17.130 | |
| 34 | 14.958 | |
| 35 | 14.890 | |
| 36 | 0.527 | tries [2.367, 0.528, 0.527] |
| 37 | 3.800 | |
| 38 | 3.905 | |
| 39 | 3.019 | |
| 40 | 1.179 | |
| 41 | 8.078 | |
| 42 | 1.606 | |
| 43 | 1.282 | |

**Hot sums**

| Scope | Coldrun | ClickHouse ([c6a.4xlarge](https://github.com/ClickHouse/ClickBench/blob/main/clickhouse/results/20260516/c6a.4xlarge.json)) |
|-------|---------|----------------------------------------------------------------------------------------------------------------------------------|
| Q1–22 | 46.57s | ~9.6s |
| Q24–43 | 136.97s | ~22.8s |
| Q1–22 + Q24–43 (42 queries) | **183.54s** | **~32.4s** |
| All 43 | **238.698s** | **~32.4s** |

Not ClickBench Combined (no cold protocol, no `drop_caches` per query). See [`compare-hot.md`](compare-hot.md).

Targeted Q23/Q24-only rerun at `c107ad4` remains useful as an earlier diagnostic check, but this full 43-query run is now the canonical warm snapshot.
