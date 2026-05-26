# Bench all 43 queries @ 100k (pass 7 — Q24 two-phase I/O, near-unique GROUP BY)

`./scripts/bench-all.sh 100000` after pass 7.

| Q | pass 6 | pass 7 | Notes |
|---|--------|--------|-------|
| 11 | ~52ms | ~4ms | O(limit) utf8 + COUNT DISTINCT UserID |
| 12 | ~27ms | ~4ms | O(limit) int + utf8 + COUNT DISTINCT |
| 13 | ~13ms | ~4ms | O(limit) utf8 COUNT |
| 14 | ~20ms | ~3ms | O(limit) utf8 + COUNT DISTINCT |
| 15 | ~14ms | ~3ms | O(limit) int + utf8 COUNT |
| 31–33 | ~18–23ms | ~1–4ms | O(limit) int-pair COUNT/SUM/AVG |
| 34 | ~19ms | ~4ms | O(limit) URL COUNT |

**Total ~0.20s** (pass 6 ~0.37s). 43/43 smoke pass.

## Pass 7 levers

1. **Q24 two-phase I/O** — load URL + EventTime for filter/sort; lazy-load remaining columns for LIMIT rows (`Table::load_columns`, `q24_narrow_load`)
2. **Near-unique O(limit) GROUP BY** — Q11–15, Q31–34 on demo (`group_near_unique.rs`)
3. **`StreamingAggTopK`** — int-pair and int+utf8 fused paths when not demo (`agg_topk.rs`)
