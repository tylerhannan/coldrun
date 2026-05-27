# Overnight run status (items 1–11)

**Completed:** 2026-05-26  
**Branch:** `main` @ `9904054`  
**Smoke:** 43/43 PASS on 100k demo rows after all changes

| # | Item | Status | Commit |
|---|------|--------|--------|
| 1 | Regression script + 100k baseline | Done | `193eb9c` |
| 2 | 500k stress run | Done | `707dfd9` |
| 3 | PERF.md links to benchmark docs | Done | `a968055` |
| 4 | Q27 two-column ORDER BY scan | Done | `b0183cd` |
| 5 | Q29 wide SUM fast path | Done | `4d92a87` |
| 6 | Sparse mask iteration (group) | Done | `3dfd120` |
| 7 | Dual global COUNT DISTINCT | Done | `e5a4d61` |
| 8 | mmap column read (&gt;64KB) | Done | `4560237` |
| 9 | Parallel Parquet column decode | Done | `a360bfb` |
| 10 | `bench-all.sh` | Done | `c94f4ca` |
| 11 | GitHub Actions CI | Done | `9904054` |

## Quick commands

```bash
./scripts/bench-regression.sh 100000
./scripts/bench-all.sh 100000
./scripts/smoke-all.sh 100000
```

## Notes

- No `hits.parquet` downloaded (laptop-friendly).
- CI runs on GitHub with 10k rows; local stress used 100k/500k.
- Item 7 fast path applies when a query has exactly two `COUNT(DISTINCT …)` projections (not a single combined ClickBench query today).
