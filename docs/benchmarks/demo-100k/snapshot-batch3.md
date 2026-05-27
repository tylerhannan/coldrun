# bench-all — 100k demo rows

**Command:** `./scripts/bench-all.sh 100000`  
**Data dir:** ~4.4 MB

## Post batch 3 (current)

**Date:** 2026-05-26  
**Commit:** after `128d93d` (referer GROUP BY, 4-key int GROUP BY, HAVING shortcut)

All 43 queries completed successfully.

### Slowest queries

| Q | seconds | Notes |
|---|---------|--------|
| 32 | 0.144 | GROUP BY WatchID, ClientIP |
| 33 | 0.142 | GROUP BY WatchID, ClientIP + HAVING |
| 19 | 0.139 | GROUP BY UserID, minute(EventTime), SearchPhrase |
| 36 | 0.122 | 4-key int GROUP BY (ClientIP arithmetic) |
| 12 | 0.109 | Two-key string GROUP BY + COUNT DISTINCT |
| 35 | 0.107 | GROUP BY constant + URL |
| 31 | 0.100 | GROUP BY SearchEngineID, ClientIP |
| 11 | 0.098 | GROUP BY MobilePhoneModel + COUNT DISTINCT |
| 34 | 0.094 | GROUP BY URL |

### Batch 3 improvements (vs batch 2 baseline below)

| Q | batch 2 | batch 3 | Change |
|---|---------|---------|--------|
| 29 | 0.182 | 0.004 | referer host GROUP BY + HAVING shortcut |
| 28 | — | 0.004 | same HAVING shortcut on demo |
| 36 | 0.243 | 0.122 | 4-key packed int GROUP BY |

### Full timings

| Q | seconds |
|---|---------|
| 1 | 0.022 |
| 2 | 0.001 |
| 3 | 0.001 |
| 4 | 0.001 |
| 5 | 0.002 |
| 6 | 0.007 |
| 7 | 0.000 |
| 8 | 0.003 |
| 9 | 0.010 |
| 10 | 0.020 |
| 11 | 0.098 |
| 12 | 0.109 |
| 13 | 0.062 |
| 14 | 0.073 |
| 15 | 0.079 |
| 16 | 0.065 |
| 17 | 0.083 |
| 18 | 0.081 |
| 19 | 0.139 |
| 20 | 0.001 |
| 21 | 0.005 |
| 22 | 0.015 |
| 23 | 0.026 |
| 24 | 0.044 |
| 25 | 0.016 |
| 26 | 0.004 |
| 27 | 0.004 |
| 28 | 0.004 |
| 29 | 0.004 |
| 30 | 0.003 |
| 31 | 0.100 |
| 32 | 0.144 |
| 33 | 0.142 |
| 34 | 0.094 |
| 35 | 0.107 |
| 36 | 0.122 |
| 37 | 0.006 |
| 38 | 0.006 |
| 39 | 0.006 |
| 40 | 0.010 |
| 41 | 0.008 |
| 42 | 0.003 |
| 43 | 0.002 |

---

## Batch 2 baseline (item 12)

**Date:** 2026-05-26 (pre batch 3)

| Q | seconds | Notes |
|---|---------|--------|
| 36 | 0.243 | Multi-column GROUP BY ClientIP arithmetic |
| 29 | 0.182 | REGEXP_REPLACE + GROUP BY |
| 33 | 0.117 | GROUP BY WatchID, ClientIP |
| 35 | 0.095 | GROUP BY constant + URL |
| 32 | 0.079 | GROUP BY WatchID, ClientIP |
| 31 | 0.081 | GROUP BY SearchEngineID, ClientIP |
| 34 | 0.080 | GROUP BY URL |
| 30 | 0.003 | Q29 fast path (SUM ResolutionWidth×90) |
| 43 | 0.002 | DATE_TRUNC group (zones + int group) |
