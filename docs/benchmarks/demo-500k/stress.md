# Stress — 500k demo rows

**Date:** 2026-05-26  
**Command:** `./scripts/bench-regression.sh 500000`

## smoke-all

- **Result:** 43/43 PASS
- **Row count:** 500,000 synthetic `hits`
- **Slowest queries (from log):** Q29 ~0.05s+, grouped Q30–35 scale with row count

## bench-demo (Q1–10)

| Q | seconds |
|---|---------|
| 1 | 0.121 |
| 2 | 0.002 |
| 3 | 0.002 |
| 4 | 0.003 |
| 5 | 0.031 |
| 6 | 0.051 |
| 7 | 0.042 |
| 8 | 0.009 |
| 9 | 0.067 |
| 10 | 0.128 |

**Data dir size:** ~22 MB (LZ4 columns)
