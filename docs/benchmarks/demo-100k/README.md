# Demo bench — 100k rows

**Command:** `./scripts/bench-all.sh 100000`  
**Queries:** [`clickbench/coldrun/queries.sql`](../../../clickbench/coldrun/queries.sql) (43)

## Latest full run

See [`latest.md`](latest.md) for all 43 query timings (pass 11 era, ~0.15s total).

## Pass changelog (highlights)

Each pass doc captures what changed and a few representative timings. Earlier passes with full tables: [`pass-03.md`](pass-03.md), [`snapshot-batch3.md`](snapshot-batch3.md).

| Pass | Doc | Focus |
|------|-----|--------|
| 11 | [`pass-11.md`](pass-11.md) | Zone EventTime top-K, Q6 ahash DISTINCT, Q23/Q27 filters |
| 10 | [`pass-10.md`](pass-10.md) | Utf8 `.col.idx`, parallel `project_rows`, streaming top-K |
| 9 | [`pass-09.md`](pass-09.md) | Row-indexed Q24, fused AND, zone v2 EventTime |
| 8 | [`pass-08.md`](pass-08.md) | Metadata-only I/O, near-unique Q16/Q22/Q23 |
| 7 | [`pass-07.md`](pass-07.md) | Q24 two-phase I/O, near-unique O(limit) |
| 6 | [`pass-06.md`](pass-06.md) | PodStorage, StreamingTopK, Q24 partial sort |
| 5 | [`pass-05.md`](pass-05.md) | Fused Q11/Q22/Q23, sharded pair GROUP BY |
| 4 | [`pass-04.md`](pass-04.md) | `demo_near_unique`, Q40 CASE fused |
| 3 | [`pass-03.md`](pass-03.md) | Zones v1, direct index, utf8 arena, SIMD |
| 2 | [`pass-02.md`](pass-02.md) | Early fused kernels |
| — | [`fused-kernels.md`](fused-kernels.md) | Pre-pass fused bench snapshot |
| — | [`snapshot-batch3.md`](snapshot-batch3.md) | Batch 3 era full table (historical) |
