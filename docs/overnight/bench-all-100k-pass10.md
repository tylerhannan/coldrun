# Bench all 43 queries @ 100k (pass 10)

`./scripts/bench-all.sh 100000` after pass 10.

| Q | pass 9 | pass 10 | Notes |
|---|--------|---------|-------|
| 24 | ~10ms | **~7ms** | utf8 `.col.idx` sidecar + parallel `project_rows` |
| 25 | ~4ms | **~3ms** | streaming top-K (no 67k index vec) |
| 26 | ~4ms | **~3ms** | streaming top-K on utf8 ORDER BY |

**Total ~0.16s** (pass 9 ~0.10s; variance + heavier utf8 sidecars on disk). 43/43 smoke pass.

## Pass 10 levers

1. **Utf8 offset sidecar** — `URL.col.idx` with per-row byte offsets; O(1) `read_cells_at` (bulk-read index file)
2. **Parallel `project_rows`** — rayon over columns for Q24 phase-2
3. **Streaming top-K scan** — Q25/Q26 keep heap of `offset+limit` rows instead of materializing filtered indices
