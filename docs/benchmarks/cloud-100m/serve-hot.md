# Serve hot — 100M rows (c6a.4xlarge)

**VM:** AWS `c6a.4xlarge` (32 GiB), `/data/coldrun` (~100M rows, ~33 GiB on disk)  
**Command:** `./scripts/bench-serve.sh 100000000 --skip-load` (Q1–22 + Q24–43; Q23 smoke separate)  
**Protocol:** warm `serve`, 3 tries/query, hot = min(try 2, try 3)  
**Commit:** `eb414c9` (Q24 streaming + sequential `project_rows`)  
**Log:** `/data/bench-warm-full.log` on bench VM

| Q | hot (s) | Notes |
|---|---------|-------|
| 1 | 0.000 | |
| 2 | 0.000 | |
| 3 | 0.232 | |
| 4 | 0.149 | |
| 5 | 2.742 | |
| 6 | 1.096 | |
| 7 | 0.281 | |
| 8 | 0.244 | |
| 9 | 1.866 | |
| 10 | 4.397 | |
| 11 | 0.727 | |
| 12 | 0.758 | |
| 13 | 3.212 | |
| 14 | 9.198 | |
| 15 | 0.775 | |
| 16 | 3.118 | |
| 17 | 3.185 | |
| 18 | 1.195 | |
| 19 | 3.409 | |
| 20 | 0.062 | |
| 21 | 4.467 | |
| 22 | 4.907 | |
| 23 | 234.1 | smoke only (OOM fix verified; not in formal 3-try run) |
| 24 | 231.3 | |
| 25 | 0.008 | |
| 26 | 0.274 | |
| 27 | 0.641 | |
| 28 | 1.775 | |
| 29 | 7.909 | |
| 30 | 0.967 | |
| 31 | 2.559 | |
| 32 | 2.827 | |
| 33 | 17.221 | |
| 34 | 14.921 | |
| 35 | 14.936 | |
| 36 | 83.271 | |
| 37 | 3.837 | |
| 38 | 3.967 | |
| 39 | 3.101 | |
| 40 | 1.203 | |
| 41 | 7.496 | |
| 42 | 1.610 | |
| 43 | 1.312 | |

**Hot sums**

| Scope | Coldrun | ClickHouse ([c6a.4xlarge](https://github.com/ClickHouse/ClickBench/blob/main/clickhouse/results/20260516/c6a.4xlarge.json)) |
|-------|---------|----------------------------------------------------------------------------------------------------------------------------------|
| Q1–22 | 46.0s | ~9.6s |
| Q24–43 | 401.1s | ~22.8s |
| Q1–22 + Q24–43 (42 queries) | **447.1s** | **~32.4s** |
| All 43 (Q23 = smoke) | **681.2s** | **~32.4s** |

Not ClickBench Combined (no cold protocol, no `drop_caches` per query). See [`compare-hot.md`](compare-hot.md).
