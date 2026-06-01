# Coldrun — Build Prompt

**Coldrun** — *a smol columnar SQL toy (AI tooling experiment).*

Use this prompt to kick off implementation. Coldrun is the **database under test**, not a ClickBench runner or benchmark harness.

Ship as a **single static binary** named `coldrun` (server + embedded CLI client), similar to ClickHouse.

---

## North-star metric

Optimize for ClickBench **Combined**: weighted geometric mean of

| Component      | Weight |
|----------------|--------|
| Hot run times  | 60%    |
| Cold run times | 20%    |
| Load time      | 10%    |
| On-disk size   | 10%    |

Per-query scoring is **relative**: each of the 43 queries is compared to the current best time for that query across all systems, with a +10ms floor in the ratio formula. Missing/failed queries get a heavy penalty.

- **Hot run** = min(2nd, 3rd) execution of each query
- **Cold run** = 1st execution under true-cold rules (see Constraints)

**Target:** beat the current #1 Combined entry on `c6a.4xlarge`. Document `c6a.metal` as a stretch goal. Treat this as an experiment; the project name references ClickBench’s cold-run metric, not a claim of victory.

---

## Workload you must win

### Dataset

ClickBench `hits_compatible` — single flat table, ~99,997,497 rows, production-like web analytics distributions.

- https://datasets.clickhouse.com/hits_compatible/hits.parquet (preferred)
- Alternatives: `.csv.gz`, `.tsv.gz`, `.json.gz`

### Schema

Implement the ClickBench `hits` table with:

```sql
PRIMARY KEY (CounterID, EventDate, UserID, EventTime, WatchID)
```

Reference: [ClickHouse/ClickBench `clickhouse/create.sql`](https://github.com/ClickHouse/ClickBench/blob/main/clickhouse/create.sql)

### Queries

Run all **43 standard SQL queries** from ClickBench `queries.sql` **unchanged**. No ClickHouse-specific functions unless you also provide a standards-compliant alternate path — prefer running the published queries as-is.

### Query mix

Full scans, filtered scans, index lookups, `GROUP BY`, `ORDER BY`, string aggregations, time-range filters — typical ad-hoc analytics / dashboard workload.

---

## Hard constraints (competition rules)

1. **SQL compliance** — standard SQL for DDL/DML and all 43 queries. Wire protocol or CLI acceptable (`psql`-like, MySQL, JDBC, HTTP-SQL — pick one, document it).
2. **Single-node** primary result on AWS `c6a.4xlarge`, Ubuntu 24.04+, 500 GB gp2 (ClickBench default).
3. **Load** — single-file ingest, straightforward path; measure wall-clock load time; do not split for parallel load unless unavoidable (document if so).
4. **Indexing** — one primary key index only; no manual secondary indexes; auto-created indexes OK.
5. **No pre-aggregation** — no materialized views, projections, or benchmark-specific MVs for the competitive entry.
6. **No query-result caching** — disable result caches. Source-data caching (buffer pool) OK. Late-pipeline caches that behave like result caches must be off.
7. **True cold runs** — restart database + drop page cache before each query's 1st run (or equivalent for embedded engines). Lukewarm-only is allowed but tagged; not the primary goal.
8. **End-to-end timing** — client send → server execute → return rows to client. No Null/discard output tricks.
9. **Vanilla config** for the official score; any tuned variant is a separate labeled entry (e.g. `coldrun-tuned`).
10. **Open source** license compatible with ClickBench submission.
11. **Single binary** — `coldrun` with subcommands: `serve`, `client`, `local`.

---

## Engineering priorities (in order)

1. **Hot query latency (60%)** — vectorized execution, late materialization, min-max pruning on sort key, fast aggregations, efficient string ops.
2. **Cold query latency (20%)** — mmap-friendly columnar format, read-ahead without cheating cold definition, fast metadata/statistics for pruning.
3. **Load time (10%)** — parallel decode + encode, minimal post-load compaction blocking, sane fsync policy documented.
4. **Storage size (10%)** — columnar compression (LZ4/ZSTD-class), low overhead primary index, no redundant statistics blobs on ingest.

**Language:** Rust. Custom storage + vectorized execution; do not treat DataFusion/Polars as the competitive core.

---

## Deliverables

1. **Single-binary `coldrun`** with install script for Ubuntu 24.04.
2. **ClickBench integration directory** at `clickbench/coldrun/` matching upstream layout:
   - `benchmark.sh` (fully automated on fresh VM)
   - `create.sql`
   - `queries.sql` (unchanged from ClickBench)
   - `run.sh` (3 runs per query, true cold protocol)
   - `results/c6a.4xlarge.json` with valid output format:
     ```
     Load time: <seconds>
     Data size: <bytes>
     43 lines of [cold, hot2, hot3] triples
     ```
3. **README** — architecture (storage, execution, optimizer), honest toy framing, known tradeoffs.
4. **Repro script** — one command to reproduce metrics on `c6a.4xlarge` within stated variance.

---

## Acceptance tests (definition of done)

- [ ] All 43 queries return correct results (validate against ClickHouse on Parquet sample + full checksum on aggregates).
- [ ] `benchmark.sh` completes unattended on clean Ubuntu 24.04 VM.
- [ ] Combined score beats current leader on `c6a.4xlarge` (publish numbers in README, or document gap if toy scope stops earlier).
- [ ] Load time and data size reported honestly (indexes included).
- [ ] No competitive-rule violations (no extra indexes, no MVs, no result cache).
- [ ] PR-ready folder for [ClickHouse/ClickBench](https://github.com/ClickHouse/ClickBench) ("How To Add a New Result").
- [ ] `cargo build --release` produces static binary `coldrun`.

---

## Out of scope for v1

- Beating ClickHouse on every conceivable workload
- Distributed/cluster leaderboard (optional stretch)
- OLTP, multi-table warehouse models, concurrent query saturation
- GPU path (separate SKU if ever)

---

## References

- Benchmark site: https://benchmark.clickhouse.com/
- Rules & methodology: https://github.com/ClickHouse/ClickBench/blob/main/README.md
- Dataset: https://datasets.clickhouse.com/hits_compatible/
- Baseline to beat: current top Combined entry on the leaderboard (record SHA/date when you start).

---

## Suggested execution order

1. Architecture doc (max 2 pages): expected bottlenecks per query class and how the engine addresses each.
2. MVP: load `hits`, run queries 1–5 correctly via `coldrun local`.
3. Scale to all 43 queries with correct results.
4. Optimize until Combined wins (or document where the toy stops).
5. Submit ClickBench PR.
