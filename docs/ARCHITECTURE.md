# Coldrun architecture

Coldrun is a single-node, columnar analytical SQL engine aimed at the ClickBench `hits` workload. This document is the v0 design reference; it will evolve as we optimize.

## Goals and constraints

| Priority | Metric (ClickBench Combined) | Design lever |
|----------|------------------------------|--------------|
| 1 | Hot query latency (60%) | Vectorized scans, late materialization, zone-map pruning on PK |
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
  manifest.json          # version, tables, row counts
  hits/
    meta.json            # column names, types, row_count
    columns/
      CounterID.col
      EventDate.col
      ...
    pk_index/            # sparse zones over (CounterID, EventDate, UserID, EventTime, WatchID)
      zones.bin
```

Each `.col` file (v0):

1. Magic `CRUN`
2. `row_count: u64`
3. `encoding` (raw | lz4)
4. Payload: contiguous fixed-width values, or length-prefixed strings

**Load path:** read `hits.parquet` with the Arrow/Parquet reader (ingest only), cast to internal types, append batches to column writers, then build PK zones in a second pass (or during sorted ingest when we enable sort-on-load).

**Pruning:** for queries with predicates on PK prefix columns (`CounterID`, `EventDate`, …), read zone min/max and skip row ranges before touching column data.

### Known bottlenecks by query class

| Class | Example queries | Bottleneck | Mitigation |
|-------|-----------------|------------|------------|
| Full scan aggregate | Q1–Q7 | Memory bandwidth | Vectorized aggregates, read only needed columns |
| Filtered scan | Q2, Q21–Q22 | Branch + I/O | Zone maps + selective column reads |
| Group by + order | Q8–Q18 | Hash table + sort | Open-addressing hash agg, top-K heap for `LIMIT` |
| Point lookup | Q20 | Index seek | PK binary search within zones |
| String LIKE / regex | Q21–Q27 | CPU on strings | SIMD contains, regex cache (no result cache) |
| Wide SUM | Q29 | CPU | Unrolled loops, optional codegen later |
| Multi-column GROUP BY | Q30–Q35 | Memory + hash | Columnar hash keys, spill later if needed |
| Dashboard filters | Q36–Q43 | PK + date range | Prune on `(CounterID, EventDate)` zones |

## Execution (`coldrun-core::exec`)

v0 interpreter pipeline:

1. **Parse** → `Statement::Query`
2. **Bind** → resolve `hits`, column types
3. **Plan** → `Scan` → optional `Filter` → `Aggregate` / `Project` / `Sort` / `Limit`
4. **Execute** → batch size 8192 (tunable), operate on `ColumnVector` enums

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
3. **Perf (1M Parquet, warm serve):** hot sum **0.84s** (**0.62×** ClickHouse **1.34s**). **100M warm** logged @ `eb414c9` — **~681s** hot (see [`benchmarks/cloud-100m/`](benchmarks/cloud-100m/)). Next: [`NEXT.md`](NEXT.md).
4. **ClickBench PR:** automated `benchmark.sh`, `results/c6a.4xlarge.json` — after P1 fixes in [`NEXT.md`](NEXT.md).

## Honest tradeoffs (toy framing)

- Single-threaded execution initially; parallelism comes after correctness.
- Full SQL coverage is staged; the engine is not a general OLAP product.
- Beating the Combined leaderboard is an experiment, not a commitment.
