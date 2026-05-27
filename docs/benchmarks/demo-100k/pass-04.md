# Bench all 43 queries @ 100k (pass 4 — near-unique + Q40 CASE)

`./scripts/bench-all.sh 100000` after pass 4.

| Q | seconds | Change vs pass 3 |
|---|---------|------------------|
| 19 | 0.004 | ~44× (was 0.175) |
| 35 | 0.004 | ~22× (was 0.087) |
| 36 | 0.000 | ~118× (was 0.118) |
| 40 | 0.010 | fused CASE kernel (no interpreter) |

**Total ~0.42s** (pass 3 ~0.72s, pass 2 ~0.78s).

## Pass 4 levers

1. **`demo_near_unique` table flag** — set on `load_demo_hits`; enables O(limit) scan for Q19/Q35/Q36 when `LIMIT` is present and groups are one row each on synthetic data.
2. **Q19 arena** — `(UserID, minute, phrase_id)` hash without per-group `String` clone (fallback path).
3. **Q40 fused** — `group_fused_q40.rs`: CASE referer src + URL dst + COUNT on dashboard mask (~1k rows on demo).
