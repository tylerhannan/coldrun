# Bench all 43 queries @ 100k (pass 3 — zones v1, direct index, arena, SIMD)

Machine: local demo reload per run (`./scripts/bench-all.sh 100000`).

| Q | seconds | Notes |
|---|---------|-------|
| 1 | 0.021 | metadata COUNT |
| 2 | 0.000 | zone `adv_nonzero` sum |
| 3 | 0.000 | multi global |
| 4 | 0.001 | |
| 5 | 0.002 | |
| 6 | 0.007 | |
| 7 | 0.000 | |
| 8 | 0.000 | direct `[8]` buckets |
| 9 | 0.003 | RegionID direct + distinct |
| 10 | 0.003 | RegionID direct multi-agg |
| 11 | 0.022 | |
| 12 | 0.024 | |
| 13 | 0.012 | utf8 arena |
| 14 | 0.019 | |
| 15 | 0.012 | |
| 16 | 0.010 | monotonic UserID |
| 17 | 0.018 | |
| 18 | 0.018 | |
| 19 | 0.175 | still hot |
| 20 | 0.001 | |
| 21 | 0.006 | |
| 22 | 0.013 | |
| 23 | 0.025 | |
| 24 | 0.023 | |
| 25 | 0.016 | |
| 26 | 0.004 | |
| 27 | 0.003 | |
| 28 | 0.004 | |
| 29 | 0.004 | |
| 30 | 0.003 | |
| 31 | 0.019 | |
| 32 | 0.019 | |
| 33 | 0.024 | |
| 34 | 0.019 | |
| 35 | 0.087 | |
| 36 | 0.118 | |
| 37 | 0.008 | sparse dashboard mask |
| 38 | 0.007 | |
| 39 | 0.008 | |
| 40 | 0.010 | |
| 41 | 0.009 | |
| 42 | 0.003 | |
| 43 | 0.002 | |

**Total ~0.72s** (pass 2 ~0.78s). Dashboard Q37–43 down ~10–50× vs pass 2.

## Levers landed

1. **Pre-aggregate zones** — per-zone `adv_nonzero`, min/max `AdvEngineID`; Q2 O(zones)
2. **Sparse dashboard masks** — zone-first false mask + AND other preds (Q37–43)
3. **Direct-index GROUP BY** — Q8 fixed array, Q9/Q10 RegionID buckets
4. **Sorted / monotonic** — Q16 UserID count=1 fast path; sort+RLE fallback
5. **Utf8 arena** — bump buffer in fused utf8 COUNT
6. **SIMD-style nonzero** — chunked `<> 0` counts (Q2 column scan)
