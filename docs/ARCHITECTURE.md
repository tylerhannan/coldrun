# Coldrun architecture

Coldrun is a single-node, columnar analytical SQL engine aimed at the ClickBench `hits` workload. This document tracks the current design after the V2 blockized storage and execution updates.

## Goals and constraints

| Priority | Metric (ClickBench Combined) | Design lever |
|----------|------------------------------|--------------|
| 1 | Hot query latency (60%) | Blockized scans, late materialization, zone-map pruning on PK |
| 2 | Cold query latency (20%) | mmap-friendly `.col` files, minimal metadata reads, no result cache |
| 3 | Load time (10%) | Parallel Parquet decode → column encode, optional deferred sort |
| 4 | On-disk size (10%) | Per-column LZ4/ZSTD, compact PK sparse index |

**Hard rules:** one primary key index only, no MVs, no result cache, standard SQL for the 43 queries, durable on-disk data for benchmark runs.

## High-level layout

```
┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│ coldrun     │────▶│ SQL front-end │────▶│ Logical plan    │
│ client/serve│     │ (sqlparser)   │     │ (subset)        │
└─────────────┘     └──────────────┘     └────────┬────────┘
                                                   │
                     ┌─────────────────────────────▼────────────────────────────┐
                     │ Physical executor (vectorized, single-thread v0 → parallel)   │
                     └─────────────────────────────┬────────────────────────────┘
                                                   │
                     ┌─────────────────────────────▼────────────────────────────┐
                     │ Storage: column files + PK zone index + table manifest   │
                     └──────────────────────────────────────────────────────────┘
```

## Storage (`coldrun-core::storage`)

### On-disk layout (`.coldrun/`)

```
.coldrun/
  manifest.json                 # version, tables, row counts
  hits/
    meta.json                   # column names, types, row_count
    columns/
      CounterID.col             # V1 or V2 payload
      CounterID.blocks.json     # V2 block metadata sidecar
      EventDate.col
      EventDate.blocks.json
      ...
    pk_index/                   # sparse zones over (CounterID, EventDate, UserID, EventTime, WatchID)
      zones.bin
```

Each `.col` file can be:

- **V1** (legacy): single payload block (raw/lz4), no sidecar required.
- **V2** (current write path): row-blockized payload (target 64k rows/block) with per-block metadata in `.blocks.json`.

`ColumnBlockReader` provides:

- `iter_blocks()` for metadata scan
- `read_block(block_id)` for targeted decode
- automatic V1 fallback by exposing V1 payload as one synthetic block

**Load path:** read `hits.parquet` with the Arrow/Parquet reader (ingest only), cast to internal types, append batches to column writers, emit V2 blockized `.col` + `.blocks.json`, then build PK zones.

**Pruning:** for queries with predicates on PK prefix columns (`CounterID`, `EventDate`, …), read zone min/max and skip row ranges before touching column data.

### Known bottlenecks by query class (current)

| Class | Example queries | Bottleneck | Mitigation |
|-------|-----------------|------------|------------|
| Full scan aggregate | Q1–Q7 | Memory bandwidth | Vectorized aggregates, read only needed columns |
| Filtered scan | Q2, Q21–Q22 | Branch + decode | Zone maps + selective column reads; next: blockized string scans |
| Group by + order | Q8–Q18 | Hash table + sort | Open-addressing hash agg, top-K heap for `LIMIT` |
| Point lookup | Q20 | Index seek | PK binary search within zones |
| String LIKE / regex | Q21–Q27, Q36–Q43 | CPU + string decode | Block-at-a-time scans, late materialization, fused kernels |
| Wide SUM | Q29 | CPU | Unrolled loops, optional codegen later |
| Multi-column GROUP BY | Q30–Q35 | Memory + hash | Columnar hash keys, spill later if needed |
| Dashboard filters | Q37–Q43 | PK + date range + string predicates | Prune on `(CounterID, EventDate)` zones + fused filters |

## Execution (`coldrun-core::exec`)

Runtime is now a hybrid dispatcher:

1. **Parse** → `Statement::Query`
2. **Bind** → resolve `hits`, column types
3. **Dispatch**:
   - query-shape fast paths (fused kernels for ClickBench patterns),
   - block-reader streaming paths for string-heavy outliers,
   - generic vectorized interpreter fallback.
4. **Execute**:
   - batch/chunk processing (default 8192),
   - sparse-mask iteration where selective,
   - top-k heap + sort-based aggregation for large group workloads.

Notable current fast-path architecture:

- **Q23** (`group_fused_q23.rs`): block-reader mask/count/pass2 with phase-level perf accounting (`perf:q23`).
- **Q24** (`scan_stream.rs`): block-reader URL/EventTime scan + top-k + late projection with `perf:q24`.
- **Q36/Q41** (`group_columnar.rs` + fused modules): columnar/group kernels with sort/hash strategy depending on shape and cardinality.

Supported types in v0: `Int64`, `Int32`, `Int16`, `Float64` (for AVG), `Date`, `Timestamp`, `Utf8`.

Distinct counts (Q5–Q6, …): `HashSet` per batch merge (v0); later HyperLogLog only if rules allow (they do not for exact SQL).

## SQL surface (`coldrun-core::sql`)

- **Parser:** `sqlparser` with PostgreSQL dialect (ClickBench queries use `<>`, `DATE_TRUNC`, `REGEXP_REPLACE`, `extract`, etc.).
- **Scope:** grow from queries 1–5 → full 43; unsupported constructs return a clear error until implemented.
- **No ClickHouse-specific functions** in the competitive path; the published `queries.sql` is the source of truth.

## CLI binary (`coldrun`)

| Subcommand | Role |
|------------|------|
| `local` | Embedded: open `.coldrun` data dir, run one SQL string or `-f` file |
| `serve` | TCP SQL server (simple text protocol, v0) |
| `client` | REPL / batch to `serve` |

Benchmark integration uses `local` for load and `client` or `local` for timed queries.

## Cold-run protocol (ClickBench)

Per query: stop server → wait until down → `drop_caches` → start → run try 1 (cold), tries 2–3 (hot). Embedded mode (`BENCH_RESTARTABLE=no`) only drops OS page cache between queries; durable data stays on disk.

## Iteration roadmap

1. **MVP (current):** ingest Parquet → column files; correct results for Q1–Q5 via `coldrun local`.
2. **Coverage:** all 43 queries correct — demo smoke + **43/43 vs ClickHouse on 1M Parquet** (CI).
3. **Perf (1M Parquet, warm serve):** hot sum **0.84s** (**0.62×** ClickHouse **1.34s**).
4. **Perf (100M cloud warm):** all-43 hot **321.843s** @ `80c09f0` on `c6a.4xlarge`; Q23/Q24 reduced to **56.151s / 49.990s** via V2 blockized path (see [`benchmarks/cloud-100m/`](benchmarks/cloud-100m/) and [`PERF.md`](PERF.md)).
5. **Next:** apply blockized pattern to remaining outliers (Q36, Q41, Q33–Q35, then Q21/Q22 path hardening) per [`NEXT.md`](NEXT.md).
6. **ClickBench PR:** automated `benchmark.sh`, `results/c6a.4xlarge.json` — after P2/P6 milestones in [`NEXT.md`](NEXT.md).

## Honest tradeoffs (toy framing)

- Single-threaded execution initially; parallelism comes after correctness.
- Full SQL coverage is staged; the engine is not a general OLAP product.
- Beating the Combined leaderboard is an experiment, not a commitment.
