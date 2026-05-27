# Bench all 43 queries @ 100k (pass 9 — row-indexed Q24, fused AND, zone v2)

Includes pass 8 (metadata-only I/O, near-unique Q16/Q22/Q23, scan partial sort).

`./scripts/bench-all.sh 100000` after pass 9.

| Q | pass 7 | pass 9 | Notes |
|---|--------|--------|-------|
| 1 | ~23ms | **~0ms** | metadata-only column pruning |
| 16 | ~10ms | **~1ms** | near-unique UserID GROUP BY |
| 24 | ~23ms | **~10ms** | row-indexed `project_rows` (no full column decode) |
| 25 | ~17ms | **~4ms** | partial sort + skip bool mask for `<> ''` |

**Total ~0.10s** (pass 7 ~0.20s). 43/43 smoke pass.

## Pass 8 levers

1. Empty `referenced_columns` → load zero `.col` files (Q1)
2. Near-unique O(limit) for Q11–16, Q22–23, Q31–34
3. Q25 ORDER BY EventTime ≠ SELECT col + partial top-K sort
4. Q21 URL LIKE count without mask alloc

## Pass 9 levers

1. **`ColumnData::read_cells_at`** — decode only requested row indices from `.col` files
2. **`Table::project_rows`** — Q24 phase-2 without loading full columns into memory
3. **Fused AND filter** — single-pass mask for multi-LIKE / `<> ''` trees (Q23)
4. **Zone v2** — per-zone EventTime min/max in index (forward-compatible serde defaults)
5. **Scan filter shortcut** — `SearchPhrase <> ''` without 100k bool vector
