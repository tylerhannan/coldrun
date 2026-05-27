# Latest bench — demo 100k (all 43 queries)

**Command:** `./scripts/bench-all.sh 100000`  
**Commit era:** pass 11 (`eccc19c`)  
**Data dir:** ~6.3 MB (utf8 `.col.idx` sidecars)

| Q | seconds |
|---|---------|
| 1 | 0.000 |
| 2 | 0.000 |
| 3 | 0.001 |
| 4 | 0.001 |
| 5 | 0.002 |
| 6 | 0.004 |
| 7 | 0.000 |
| 8 | 0.001 |
| 9 | 0.003 |
| 10 | 0.003 |
| 11 | 0.004 |
| 12 | 0.004 |
| 13 | 0.003 |
| 14 | 0.003 |
| 15 | 0.003 |
| 16 | 0.001 |
| 17 | 0.004 |
| 18 | 0.003 |
| 19 | 0.004 |
| 20 | 0.001 |
| 21 | 0.005 |
| 22 | 0.007 |
| 23 | 0.011 |
| 24 | 0.008 |
| 25 | 0.004 |
| 26 | 0.003 |
| 27 | 0.003 |
| 28 | 0.005 |
| 29 | 0.004 |
| 30 | 0.003 |
| 31 | 0.003 |
| 32 | 0.004 |
| 33 | 0.001 |
| 34 | 0.005 |
| 35 | 0.004 |
| 36 | 0.001 |
| 37 | 0.006 |
| 38 | 0.006 |
| 39 | 0.006 |
| 40 | 0.010 |
| 41 | 0.009 |
| 42 | 0.003 |
| 43 | 0.003 |

**Total (sum of per-query times):** ~0.15s — each query is a fresh CLI process; use for regression on one machine, not cross-hardware comparison.

Regenerate: `./scripts/bench-all.sh 100000 | tee /tmp/bench-all-100k.txt`
