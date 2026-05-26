# Performance work

Coldrun optimizes for ClickBench **Combined** (hot 60%, cold 20%, load 10%, disk 10%). See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full design.

## Local benchmarking (no `hits.parquet`)

```bash
./scripts/bench-demo.sh           # default 100k synthetic rows
./scripts/bench-demo.sh 500000  # heavier
```

Compare before/after on the same machine with the same `ROWS` argument.

Overnight regression summaries (committed): [`docs/overnight/`](overnight/).

```bash
./scripts/overnight-regression.sh 100000   # item 1 baseline
./scripts/overnight-regression.sh 500000   # item 2 stress
```

## Implemented

| Area | What |
|------|------|
| **Column pruning** | Load only columns referenced by the query (`open_table_for_query`) |
| **Vectorized filters** | Fast paths for `AND`/`OR`, `col <> 0`, `col <> ''`, date ranges, `LIKE '%x%'` |
| **Fast global aggregates** | `COUNT(*)` / `SUM` / `AVG` / `MIN`/`MAX` on one column without per-row interpreter |
| **LZ4 column files** | Optional compression on flush for payloads &gt; 4 KB (backward-compatible read) |
| **Integer GROUP BY** | Packed `u128` keys for up to four int/date keys incl. `col - N` (Q36) (`group_int.rs`) |
| **Referer host GROUP BY** | Cached regex host extraction for Q29 (`group_referrer.rs`) |
| **HAVING shortcut** | Empty result when `COUNT(*) > N` and `N` ≥ filtered rows (Q28–29 demo) |
| **Top-K partial sort** | `select_nth_unstable_by` before full sort when `LIMIT` + many groups |
| **Int COUNT DISTINCT** | `HashSet<i64>` instead of string keys on numeric columns |
| **PK zone index** | Min/max zones on `CounterID` + `EventDate`; prune dashboard filters (Q36–43) |
| **Zone pre-agg (v1)** | Per-zone `adv_nonzero` + sparse dashboard masks; Q2 O(zones), Q37–43 ~10× faster |
| **Direct-index GROUP BY** | Low-cardinality int keys without hash (`group_direct.rs`: Q8, Q9, Q10) |
| **Sorted / monotonic GROUP BY** | `group_sorted.rs`: RLE after sort; Q16 monotonic UserID |
| **Utf8 arena** | Bump-buffer interning for fused utf8 COUNT (Q13, Q34, Q37) |
| **SIMD nonzero counts** | Chunked `<> 0` column scans (`simd_count.rs`, Q2) |
| **Demo near-unique GROUP BY** | `TableMeta::demo_near_unique` + O(limit) scan (`group_near_unique.rs`, Q19/Q35/Q36) |
| **Q40 CASE fused** | Dashboard + CASE referer + URL without interpreter (`group_fused_q40.rs`) |
| **Sharded int-pair GROUP BY** | 256-way shards for Q31–33 (`column_slice.rs`) |
| **Fused SearchPhrase aggs** | Q11/Q22/Q23 (`group_fused_q11.rs`, `group_fused_q22.rs`, `group_fused_q23.rs`) |
| **Streaming top-K scaffold** | `agg_topk.rs` (for real `hits` cardinality) |
| **Multi global agg** | One mask pass for `SUM` + `COUNT(*)` + `AVG` (Q3) |
| **Global COUNT DISTINCT** | Dedicated fast path for int/utf8 columns (Q5–Q6) |
| **Column-order scan** | `SELECT col ORDER BY col LIMIT` sorts via row indices (Q25–Q26) |
| **Group hash reserve** | Pre-size hash tables from filtered row count |
| **Q27 scan** | Two-key `ORDER BY EventTime, SearchPhrase` |
| **Q29 fast path** | 90× `SUM(ResolutionWidth + k)` in one column pass |
| **Sparse masks** | Iterate selected row indices when filter is selective |
| **mmap columns** | Files &gt; 64 KB decoded via `memmap2` |
| **Parallel Parquet load** | Per-batch column extract with `rayon` |
| **bench-all.sh** | Time all 43 queries on demo data |
| **CI** | GitHub Actions: build + `smoke-all.sh 10000` |

```bash
./scripts/bench-all.sh 100000    # all 43 queries
```

## Changelog

| Commit | Focus |
|--------|--------|
| v0.1 | Column pruning, vectorized filters, LZ4 |
| round 1–3 | Int GROUP BY, top-K, zones |
| round 4 | Multi global agg, global COUNT DISTINCT |
| round 5 | Scan sort fast path, hash reserve, in-place mask AND/OR |
| overnight 1–2 | Regression script; 100k/500k baselines in `docs/overnight/` |
| overnight 4–11 | Q27/Q29 fast paths, sparse masks, mmap, rayon load, bench-all, CI |
| batch 2 (12–17) | bench-all baseline, memchr LIKE, IN-list, Q7 min/max, ahash, README/CI badge |
| batch 3 (18–21) | Referer GROUP BY, 4-key int GROUP BY, HAVING shortcut, harness README, bench-compare |
| Q1–Q43 pass | Utf8 GROUP BY, top-K alias fix, Q19 minute extract, Q20 eq scan — see [`perf/`](perf/) |
| fused kernels | `group_fused.rs`: int-pair aggs (Q31–33), utf8 COUNT, int+utf8, Q19 triple, int4 COUNT, Q24 scan |
| pass 3 | Zone v1 pre-agg, sparse dashboard masks, `group_direct`, `group_sorted`, utf8 arena, SIMD nonzero — [`bench-all-100k-pass3.md`](overnight/bench-all-100k-pass3.md) |
| pass 4 | `demo_near_unique` O(limit) paths (Q19/Q35/Q36), Q40 CASE fused, Q19 utf8 intern — [`bench-all-100k-pass4.md`](overnight/bench-all-100k-pass4.md) |
| pass 5 | Sharded Q31–33, fused Q11/Q22/Q23, near-unique Q17–18, DISTINCT intern — [`bench-all-100k-pass5.md`](overnight/bench-all-100k-pass5.md) |

## Next (planned)

1. **StreamingTopK on real paths** — wire `agg_topk.rs` when loading Parquet / non-demo tables
2. **`Arc<[T]>` column buffers** — zero-copy numeric decode after LZ4
3. **Q24 scan** — projection pushdown + column-order read
4. **ClickBench PR prep** — harness polish (scores need cloud VM)

## Honest scope

Demo timings on a laptop are for **regression testing**, not leaderboard claims. Real Combined scores need `c6a.4xlarge`, full `hits.parquet`, and the ClickBench cold-run protocol.
