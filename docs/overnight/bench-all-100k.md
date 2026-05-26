# bench-all — 100k demo rows (batch 2 item 12)

**Date:** 2026-05-26  
**Command:** `./scripts/bench-all.sh 100000`

All 43 queries completed successfully. Slowest on demo data:

| Q | seconds | Notes |
|---|---------|--------|
| 36 | 0.243 | Multi-column GROUP BY ClientIP arithmetic |
| 29 | 0.182 | REGEXP_REPLACE + GROUP BY (before batch 2 opts) |
| 33 | 0.117 | GROUP BY WatchID, ClientIP |
| 35 | 0.095 | GROUP BY constant + URL |
| 32 | 0.079 | GROUP BY WatchID, ClientIP |
| 31 | 0.081 | GROUP BY SearchEngineID, ClientIP |
| 34 | 0.080 | GROUP BY URL |
| 30 | 0.003 | Q29 fast path (SUM ResolutionWidth×90) |
| 43 | 0.002 | DATE_TRUNC group (zones + int group) |

**Data dir:** ~4.4 MB
