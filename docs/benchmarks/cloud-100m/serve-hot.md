# Serve hot — 100M rows (c6a.4xlarge)

**VM:** AWS `c6a.4xlarge` (32 GiB), `/data/coldrun` (~100M rows, V2 blockized reload)  
**Command:** `./scripts/bench-serve.sh 100000000 --skip-load --write-snapshot`  
**Protocol:** warm `serve`, 3 tries/query, hot = min(try 2, try 3)  
**Commit:** `51a8b61`  
**Log:** `/data/bench-v2-warm-q36check.log`

| Q | hot (s) | Notes |
|---|---------|-------|
| 1 | 0.000 | |
| 2 | 0.000 | |
| 3 | 0.232 | |
| 4 | 0.149 | |
| 5 | 2.698 | |
| 6 | 1.134 | |
| 7 | 0.226 | |
| 8 | 0.259 | |
| 9 | 1.908 | |
| 10 | 4.374 | |
| 11 | 0.770 | |
| 12 | 0.789 | |
| 13 | 3.305 | |
| 14 | 9.313 | |
| 15 | 0.794 | |
| 16 | 3.120 | |
| 17 | 3.220 | |
| 18 | 1.192 | |
| 19 | 3.435 | |
| 20 | 0.053 | |
| 21 | 4.499 | |
| 22 | 4.907 | |
| 23 | 53.716 | tries [56.406, 53.753, 53.716] |
| 24 | 49.746 | tries [73.982, 49.805, 49.746] |
| 25 | 0.008 | |
| 26 | 0.276 | |
| 27 | 0.652 | |
| 28 | 1.778 | |
| 29 | 7.779 | |
| 30 | 0.973 | |
| 31 | 2.583 | |
| 32 | 2.792 | |
| 33 | 17.045 | |
| 34 | 14.835 | |
| 35 | 14.782 | |
| 36 | 80.815 | tries [85.529, 84.527, 80.815] |
| 37 | 3.788 | |
| 38 | 3.892 | |
| 39 | 3.034 | |
| 40 | 1.196 | |
| 41 | 7.530 | |
| 42 | 1.612 | |
| 43 | 1.313 | |

**Hot sums**

| Scope | Coldrun | ClickHouse ([c6a.4xlarge](https://github.com/ClickHouse/ClickBench/blob/main/clickhouse/results/20260516/c6a.4xlarge.json)) |
|-------|---------|----------------------------------------------------------------------------------------------------------------------------------|
| Q1–22 | 46.377s | ~9.6s |
| Q24–43 | 216.422s | ~22.8s |
| Q1–22 + Q24–43 (42 queries) | **262.799s** | **~32.4s** |
| All 43 | **316.515s** | **~32.4s** |

Not ClickBench Combined (no cold protocol, no `drop_caches` per query). See [`compare-hot.md`](compare-hot.md).

Targeted Q23/Q24-only rerun at `c107ad4` remains useful as an earlier diagnostic check, but this full 43-query run is now the canonical warm snapshot.
