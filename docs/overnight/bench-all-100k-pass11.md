# Bench all 43 queries @ 100k (pass 11)

`./scripts/bench-all.sh 100000` after pass 11.

| Q | pass 10 | pass 11 | Notes |
|---|---------|---------|-------|
| 6 | ~7ms | **~4ms** | COUNT DISTINCT SearchPhrase via ahash (no utf8 intern) |
| 23 | ~11ms | **~10ms** | `contains` filters instead of LIKE interpreter |
| 27 | ~4ms | **~3ms** | monotonic EventTime early scan + small sort |
| 25 | ~3ms | ~3ms | forward scan when EventTime monotonic in zones |

**Total ~0.16s** (pass 10 ~0.16s). 43/43 smoke pass.

## Pass 11 levers

1. **Zone-guided EventTime top-K** — skip zones when `min_event_time` exceeds heap worst (v2 index)
2. **Monotonic EventTime fast path** — O(limit) forward scan when zone bounds prove row/time order
3. **Q6 ahash DISTINCT** — hash string bytes; track empty phrase separately
4. **Q23 inline filters** — `contains("Google")` / `!.contains(".google.")` in near-unique + fused paths
5. **Q40 empty Src intern** — reuse one intern id when CASE yields `''`
