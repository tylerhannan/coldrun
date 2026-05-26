# Baseline — 100k demo rows (overnight item 1)

**Date:** 2026-05-26  
**Command:** `./scripts/overnight-regression.sh 100000`

## smoke-all

- **Result:** 43/43 PASS
- **Row count:** 100,000 synthetic `hits`

## bench-demo (Q1–10)

| Q | seconds |
|---|---------|
| 1 | 0.023 |
| 2 | 0.000 |
| 3 | 0.001 |
| 4 | 0.001 |
| 5 | 0.004 |
| 6 | 0.010 |
| 7 | 0.008 |
| 8 | 0.002 |
| 9 | 0.016 |
| 10 | 0.027 |

**Data dir size:** ~4.4 MB (LZ4 columns)
