# Bench all 43 queries @ 100k (pass 8 — metadata-only I/O, scan/GROUP BY levers)

`./scripts/bench-all.sh 100000` after pass 8.

| Q | pass 7 | pass 8 | Notes |
|---|--------|--------|-------|
| 1 | ~23ms | **~0ms** | bare COUNT(*) loads zero column files |
| 16 | ~10ms | **~1ms** | near-unique UserID GROUP BY + LIMIT |
| 22/23 | ~9–17ms | **~8–13ms** | near-unique SearchPhrase fused aggs |
| 24 | ~23ms | **~13ms** | LIKE index list + parallel lazy column load |
| 25 | ~17ms | **~4ms** | ORDER BY EventTime ≠ SELECT col + partial sort |

**Total ~0.12s** (pass 7 ~0.20s). 43/43 smoke pass.

## Pass 8 levers

1. **Metadata-only column pruning** — empty `referenced_columns` set loads no `.col` files (Q1)
2. **Near-unique Q16/Q22/Q23** — O(limit) on demo instead of full hash / sort
3. **Q25 scan** — sort by EventTime, project SearchPhrase; partial top-K sort
4. **Q24** — direct LIKE index list; rayon parallel phase-2 column load
5. **Q21** — COUNT URL LIKE without allocating full filter mask
