# Performance work

Coldrun optimizes for ClickBench **Combined** (hot 60%, cold 20%, load 10%, disk 10%). See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full design.

## Real Parquet @ 1M rows

Correctness and timing on a streamed slice of real `hits` (not synthetic demo):

| | Coldrun (warm serve, hot) | ClickHouse local (`file()` Parquet) |
|--|---------------------------|-------------------------------------|
| **Sum Q1–43** | **0.84s** | **1.34s** (3 tries/query, hot = min(2,3), `max_threads=1`) |
| **Ratio** | **0.62×** ClickHouse | — |
| **Correctness** | 43/43 vs ClickHouse | reference |

Snapshots: [`serve-hot.md`](benchmarks/parquet-hits-1m/serve-hot.md) · [`clickhouse-hot.md`](benchmarks/parquet-hits-1m/clickhouse-hot.md) · [`compare-hot.md`](benchmarks/parquet-hits-1m/compare-hot.md) · validation: [`parquet/validation-1m.md`](benchmarks/parquet/validation-1m.md).

```bash
./scripts/install-clickhouse-local.sh
./scripts/validate-parquet.sh data/hits-1m.parquet
COLDRUN_DATA=.coldrun-validate-hits-1m_ ./scripts/bench-serve.sh 1000000 --skip-load --no-compare --write-snapshot
./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot --compare
```

Laptop numbers only — not ClickBench Combined (no cold protocol, no 100M rows, no `c6a.4xlarge`). On this 1M slice coldrun is **~0.63×** ClickHouse hot sum. Largest remaining gaps vs CH on 1M: **Q36**, **Q41** (addressed on 100M via sort-based paths in `agg_sort.rs`).

## 100M cloud warm (c6a.4xlarge, Jun 2026)

Complete warm-serve run on AWS `c6a.4xlarge` (32 GiB), `/data/coldrun` ~100M rows. Snapshots: [`cloud-100m/serve-hot.md`](benchmarks/cloud-100m/serve-hot.md) · [`compare-hot.md`](benchmarks/cloud-100m/compare-hot.md).

| Metric | Coldrun (hot) | ClickHouse (official hot) |
|--------|---------------|---------------------------|
| **Q1–22 sum** | **46.602s** | ~9.6s |
| **Q24–43 sum** | **220.917s** | ~22.8s |
| **42 queries** (bench; Q23 skipped) | **267.519s** | **~32.4s** |
| **All 43** | **320.234s** | **~32.4s** |

**Commit:** `657f98d` · **Log:** `/data/bench-v2-warm-full-rerun.log`

| Query | CR | CH | Notes |
|-------|-----|-----|-------|
| Q14 | 9.2s | 0.75s | sort distinct still ~12× |
| Q17–19 | 3.2s / 1.2s / 3.4s | 1.6s / 0.94s / 2.7s | sort agg (was 12s / 21s / 27s pre-fix) |
| Q21–22 | 4.5s / 4.9s | 0.31s / 0.09s | string GROUP BY |
| Q23 | 52.7s | 0.61s | block-reader + V2 writer path (full warm @ `657f98d`) |
| Q24 | 49.8s | 0.10s | block-reader + V2 writer path (full warm @ `657f98d`) |
| Q25 | 0.008s | 0.038s | **CR wins** |
| Q29 | 7.9s | 9.6s | **CR wins** |
| Q33–35 | ~15–17s | ~3–4s | multi-column GROUP BY |
| Q36 | 84.6s | 0.25s | still dominant tail outlier; isolated Q36 run did not hold in full warm |
| Q41 | 7.4s | 0.013s | dashboard 5-col GROUP BY |

Runbook: use the cloud workflow in [`../README.md`](../README.md) ("Cloud dev box").

Q23 formal pre-V2 bench (Jun 2026): hot **222.341s** — tries [238.8, 229.9, 222.3] @ `dde9184`, log `/data/bench-q23-fix3.log`.

### V2 blockized reruns (Jun 2026)

After landing block-reader wiring + V2 writer, we reloaded 100M rows on the same VM and ran both targeted and full warm benches:

- **Load log:** `/data/load-v2.log` (`EXIT:0`, 27 `.col` + 27 `.blocks.json`)
- **Targeted bench log:** `/data/bench-v2-q23q24.log` (`c107ad4`)
- **Full warm bench logs:** `/data/bench-v2-warm-full.log` (`80c09f0`), `/data/bench-v2-warm-full-rerun.log` (`657f98d`)
- **Data size:** `14176149601` bytes reported by full warm bench (`~14.2 GB`)

| Query | Pre-V2 hot | V2 targeted (`c107ad4`) | V2 full warm (`657f98d`) |
|-------|------------|--------------------------|--------------------------|
| Q23 | 222.341s | **54.501s** | **52.715s** |
| Q24 | 231.3s | **48.673s** | **49.808s** |

Full all-43 hot sum moved from **669.4s** to **320.234s**.

## Sort-based aggregation (`agg_sort.rs`, Jun 2026)

For 100M rows, hash maps with millions of groups or per-group `AHashSet` OOM or stall. Replacement pattern:

1. **Collect** `(key, …)` pairs for filtered rows (parallel when ≥250k rows).
2. **Sort** + run-length counts; min-heap for top-K by count.
3. **Second pass** (when needed) only for top-K keys — e.g. Q23 `MIN(URL)` / `MIN(Title)`.

| Query | Module | Notes |
|-------|--------|-------|
| Q9 | `group_direct.rs` | Sort `(RegionID, UserID)` for COUNT DISTINCT |
| Q14 | `group_fused.rs` | Sort `(phrase_hash, UserID)` for COUNT DISTINCT |
| Q17, Q19 | `group_fused.rs` | Sort user+phrase / user+minute+phrase pairs |
| Q18 | `group_fused.rs` | Track first LIMIT distinct groups only (no ORDER BY) |
| Q23 | `group_fused_q23.rs` | Disk-stream mask + count + batched top-10 detail (7 column passes) |
| Q24 | `scan_stream.rs` | URL/EventTime top-K heap; sequential `project_rows` for `SELECT *` |
| Q36 | `group_columnar.rs` | Sort ClientIP u32 (still ~328× CH @ 100M — REGEXP in Q36) |
| Q41 | `group_columnar.rs` | Collect zone-matching URLHash+EventDate keys, sort, top-K |

## Local benchmarking (demo)

```bash
./scripts/bench-demo.sh           # default 100k synthetic rows
./scripts/bench-demo.sh 500000  # heavier
```

Compare before/after on the same machine with the same `ROWS` argument.

Committed bench snapshots: [`docs/benchmarks/`](benchmarks/) · latest all-43 table: [`demo-100k/latest.md`](benchmarks/demo-100k/latest.md).

```bash
./scripts/bench-all.sh 100000              # dev regression (CLI per query — not ClickBench hot)
./scripts/bench-serve.sh 100000              # warm serve, 3 tries, hot summary on stderr
./scripts/bench-regression.sh 100000       # smoke + bench-demo + logs
./scripts/bench-regression.sh 500000       # stress @ 500k
```

Measurement guide: [`docs/benchmarks/MEASUREMENT.md`](benchmarks/MEASUREMENT.md).

## Implemented

| Area | What |
|------|------|
| **Column pruning** | Load only columns referenced by the query (`open_table_for_query`) |
| **Vectorized filters** | Fast paths for `AND`/`OR`, `col <> 0`, `col <> ''`, date ranges, `LIKE '%x%'` |
| **Fast global aggregates** | `COUNT(*)` / `SUM` / `AVG` / `MIN`/`MAX` on one column without per-row interpreter |
| **LZ4 column files** | Optional compression on flush for payloads &gt; 4 KB (backward-compatible read) |
| **Integer GROUP BY** | Packed `u128` keys for up to four int/date keys incl. `col - N` (Q36) (`group_int.rs`) |
| **Referer host GROUP BY** | Fused single-pass host agg on real Parquet (`group_fused_q29.rs`; was `group_referrer.rs`) |
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
| **Fused SearchPhrase aggs** | Q11/Q22 (`group_fused_q11.rs`, `group_fused_q22.rs`); Q23 sharded incremental agg (`group_fused_q23.rs`) |
| **Sort-based top-K @ 100M** | `agg_sort.rs` — Q9, Q14, Q17–Q19, Q36, Q41 (see above) |
| **Streaming top-K scaffold** | `agg_topk.rs` wired for utf8 COUNT with LIMIT (non-demo) |
| **`PodStorage` / Arc numerics** | Shared POD buffers after column read (`storage/pod.rs`) |
| **Q24 disk streaming** | `scan_stream.rs` — URL LIKE + EventTime top-K without match index vector; evict cache before `project_rows` |
| **Q24 sequential project** | `project_rows` one column LZ4 decode at a time (was parallel → OOM on 32 GiB) |
| **Scan ORDER BY fallbacks** | Mask + streaming top-K heap when fast paths miss (Q25–Q27 fallbacks) |
| **Bench serve restart** | `bench-common.sh` restarts `serve` when check fails after OOM |
| **Near-unique O(limit) GROUP BY** | Q11–15, Q31–34 on demo — skip full hash when one row per group |
| **Q21 URL LIKE COUNT** | Direct utf8 LIKE count without filter mask allocation |
| **Metadata-only I/O** | Empty referenced column set skips all `.col` file loads (Q1) |
| **Row-indexed projection** | `read_cells_at` + `project_rows` for Q24 LIMIT rows |
| **Fused AND filter** | Single-pass mask for multi-LIKE / `<> ''` predicate trees |
| **Zone v2 EventTime** | Per-zone min/max EventTime in PK index |
| **Multi global agg** | One mask pass for `SUM` + `COUNT(*)` + `AVG` (Q3) |
| **Global COUNT DISTINCT** | Dedicated fast path for int/utf8 columns (Q5–Q6) |
| **Column-order scan** | `SELECT col ORDER BY col LIMIT` sorts via row indices (Q25–Q26) |
| **Utf8 offset sidecar** | `.col.idx` per utf8 column for O(1) `read_cells_at` |
| **Contiguous utf8 in memory** | `Utf8Column` blob + offsets (no per-row `String` on load); zero-copy scan |
| **Serve table cache** | `Database::cached_hits` keeps loaded columns across warm `serve` queries; **evicts** columns not referenced by the current query (`Table::retain_columns`) |
| **Streaming scan top-K** | Q25/Q26 heap over rows — no full filtered index vector |
| **Q24 row projection** | `project_rows` sequential one-column decode @ 100M (`eb414c9`; parallel rayon caused OOM) |
| **Zone EventTime top-K** | Monotonic forward scan + v2 zone prune for ORDER BY EventTime LIMIT |
| **Q6 ahash DISTINCT** | COUNT DISTINCT SearchPhrase without utf8 arena intern |
| **Group hash reserve** | Pre-size hash tables from filtered row count |
| **Q27 scan** | Two-key `ORDER BY EventTime, SearchPhrase` |
| **Q29 fast path** | Wide SUM fast path (Q30 in `fast_q29.rs`); referer host fused GROUP BY (Q29 in `group_fused_q29.rs`) |
| **Q28 fused** | CounterID + `AVG(length(URL))` + HAVING without per-row interpreter |
| **Q19 fused + top-K** | Fused path accepts `Other` projections; exact counts + tie-break on group keys |
| **Sparse masks** | Iterate selected row indices when filter is selective |
| **mmap columns** | Files &gt; 64 KB decoded via `memmap2` |
| **Parallel Parquet load** | Per-batch column extract with `rayon` |
| **Q29 referer fused** | Single-pass host agg on real Parquet (`group_fused_q29.rs`) |
| **Q35 / Q43 fused** | Top-K `GROUP BY 1, URL`; minute-bucket DATE_TRUNC path |
| **ClickHouse validate** | `validate-parquet.sh` + CI job; semantics aligned (LENGTH bytes, UTC timestamps, Float64 AVG) |
| **Q41 single-pass fused** | Dashboard filter + URLHash/EventDate COUNT without 1M sub-masks (`group_fused_q41.rs`) |
| **Dashboard mask narrow** | In-place AND on sparse dashboard mask (no alloc per predicate) |
| **ClickHouse parquet bench** | `bench-clickhouse-parquet.sh` — committed `clickhouse-hot.md` + `compare-hot.md` |
| **bench-all.sh** | Time all 43 queries on demo data |
| **CI** | Build + demo smoke; separate job validates 1M Parquet vs ClickHouse |

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
| regression script | `bench-regression.sh`; logs under `logs/benchmarks/` |
| bench-all + CI | Q27/Q29 fast paths, sparse masks, mmap, rayon load, all-43 bench |
| batch 2 (12–17) | bench-all baseline, memchr LIKE, IN-list, Q7 min/max, ahash, README/CI badge |
| batch 3 (18–21) | Referer GROUP BY, 4-key int GROUP BY, HAVING shortcut, harness README, bench-compare |
| Q1–Q43 pass | Utf8 GROUP BY, top-K alias fix, Q19 minute extract, Q20 eq scan — see [`perf/`](perf/) |
| fused kernels | `group_fused.rs`: int-pair aggs (Q31–33), utf8 COUNT, int+utf8, Q19 triple, int4 COUNT, Q24 scan |
| pass 3 | Zone v1 pre-agg, sparse dashboard masks, `group_direct`, `group_sorted`, utf8 arena, SIMD nonzero — [`pass-03.md`](benchmarks/demo-100k/pass-03.md) |
| pass 4 | `demo_near_unique` O(limit) paths (Q19/Q35/Q36), Q40 CASE fused, Q19 utf8 intern — [`pass-04.md`](benchmarks/demo-100k/pass-04.md) |
| pass 5 | Sharded Q31–33, fused Q11/Q22/Q23, near-unique Q17–18, DISTINCT intern — [`pass-05.md`](benchmarks/demo-100k/pass-05.md) |
| pass 6 | `PodStorage`/`Arc<[T]>`, StreamingTopK utf8 COUNT, Q24 partial sort, Q6 intern — [`pass-06.md`](benchmarks/demo-100k/pass-06.md) |
| pass 7 | Q24 two-phase I/O, near-unique O(limit) GROUP BY, StreamingAggTopK int-pair — [`pass-07.md`](benchmarks/demo-100k/pass-07.md) |
| pass 8 | Metadata-only COUNT(*), near-unique Q16/Q22/Q23, Q25 partial sort, Q21 LIKE count — [`pass-08.md`](benchmarks/demo-100k/pass-08.md) |
| pass 9 | Row-indexed Q24 `project_rows`, fused AND filter, zone v2 EventTime — [`pass-09.md`](benchmarks/demo-100k/pass-09.md) |
| pass 10 | Utf8 `.col.idx` sidecar, parallel `project_rows`, streaming top-K Q25–26 — [`pass-10.md`](benchmarks/demo-100k/pass-10.md) |
| pass 11 | Zone EventTime top-K, Q6 ahash DISTINCT, Q23/Q27 scan filters — [`pass-11.md`](benchmarks/demo-100k/pass-11.md) |
| bench-serve | Warm-server hot snapshots, compare vs `latest.md` — [`serve-hot.md`](benchmarks/demo-100k/serve-hot.md) |
| parquet 1M | SIMD zone scan, streaming top-K Q36/Q41 — serve-hot **0.84s** — [`compare-hot.md`](benchmarks/parquet-hits-1m/compare-hot.md) (0.62× CH **1.34s**) |
| **Columnar GROUP BY scan** | Q36 ClientIP chunks + Q41 referer-led slices (`group_columnar.rs`, `simd_scan.rs`) — no 1M bool mask |
| Q41 / Q36 pass | SIMD referer prefilter, parallel streaming top-K — [`compare-hot.md`](benchmarks/parquet-hits-1m/compare-hot.md) (0.62× CH **1.34s**); Q36 **0.133s**, Q41 **0.131s** |
| columnar pass | Fused columnar Q36/Q41 — correctness ✓; Q36/Q41 ~0.13s each on 1M (was ~0.22s) |
| **100M sort agg** | `4965730` — `agg_sort.rs`; Q36/Q41 sort paths; Q17/Q19 sort top-K |
| **100M Q14/Q18** | `0a65cf6` — Q14 hash-only distinct sort; Q18 first-LIMIT-groups |
| **100M Q23 formal** | `dde9184` — hot **222.341s** (tries 238.8 / 229.9 / 222.3); was smoke 234s |
| **100M Q24 streaming** | `e40ed01` — disk top-K; serve survives |
| **100M Q24 project OOM** | `eb414c9` — sequential `project_rows`; Q24 **231s** hot, serve alive |
| **100M Q24 parallel (failed)** | `6b64ee7` — rayon scan + 4-wide project; hot **262.3s** (+13%); reverted — see [`NEXT.md`](NEXT.md#p1-follow-up--failed-parallel-attempt-6b64ee7-reverted) |
| **100M warm bench done** | Q1–22 + Q24–43 @ `eb414c9` — [`cloud-100m/`](benchmarks/cloud-100m/) |

## Next (planned)

Prioritized backlog: **[`NEXT.md`](NEXT.md)** (P0 hygiene → P1 Q23/Q24 → P2 Q36/Q41 → measurement).

## Honest scope

Demo timings on a laptop are for **regression testing**, not leaderboard claims. Real Combined scores need `c6a.4xlarge`, full `hits.parquet`, and the ClickBench cold-run protocol.
