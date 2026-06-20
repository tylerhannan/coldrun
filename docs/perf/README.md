# Per-query performance notes

Implementation notes for ClickBench queries with **non-obvious fused or sort-based paths**. Timings live in [`docs/benchmarks/`](../benchmarks/) and [`PERF.md`](../PERF.md).

**Current 100M strategy (Jun 2026):** sort + run-length aggregation in `crates/coldrun-core/src/exec/agg_sort.rs` — see [`PERF.md`](../PERF.md#sort-based-aggregation-agg_sortrs-jun-2026). **What to do next:** [`NEXT.md`](../NEXT.md).

## Queries with maintained notes

| Query | Doc | Fast path |
|-------|-----|-----------|
| Q9 | [`q-09.md`](q-09.md) | Sort distinct `(RegionID, UserID)` |
| Q14 | [`q-14.md`](q-14.md) | Sort `(phrase_hash, UserID)` distinct |
| Q17 | [`q-17.md`](q-17.md) | Sort `(UserID, phrase_hash)` top-K |
| Q18 | [`q-18.md`](q-18.md) | First-LIMIT distinct groups |
| Q19 | [`q-19.md`](q-19.md) | Sort `(UserID, minute, phrase_hash)` |
| Q23 | [`q-23.md`](q-23.md) | Disk-stream two-phase agg + batched top-10 |
| Q24 | [`q-24.md`](q-24.md) | Disk URL/EventTime top-K + sequential `SELECT *` |
| Q36 | [`q-36.md`](q-36.md) | Sort ClientIP u32 |
| Q41 | [`q-41.md`](q-41.md) | Zone scan + sort packed keys |

Other `q-*.md` files are historical one-pass notes from early optimization rounds. Prefer **PERF.md** and **bench snapshots** for current status.

Regenerate historical commit docs (optional):

```bash
./scripts/perf-q-commits.sh
```
