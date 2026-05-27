# bench-all @ 100k — fused kernels pass

**Command:** `./scripts/bench-all.sh 100000`  
**Change:** `group_fused.rs` + Q24 index scan

## vs post–batch 3

| Q | batch 3 | fused | Δ |
|---|---------|-------|---|
| 31 | 0.100 | 0.022 | ~4.5× |
| 32 | 0.091 | 0.022 | ~4.1× |
| 33 | 0.128 | 0.030 | ~4.3× |
| 11 | 0.083 | 0.022 | ~3.8× |
| 17 | 0.077 | 0.020 | ~3.9× |
| 24 | 0.042 | 0.023 | ~1.8× |

## Still asking “why not 1ms?”

| Q | seconds | Why |
|---|---------|-----|
| 19 | 0.126 | 3-key groups (UserID×minute×phrase); ~100k hash inserts |
| 36 | 0.117 | 4-key packed groups; nearly unique keys on demo |
| 12 | 0.094 | 2×utf8 + COUNT DISTINCT int64 per group |
| 35 | 0.088 | URL cardinality + string hash |

Next: streaming top-K during aggregation (don’t build all groups), simd filters, real `hits` on VM.
