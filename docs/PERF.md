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
| **Integer GROUP BY** | Packed `u128` keys for up to two int/date group columns (`group_int.rs`) |
| **Top-K partial sort** | `select_nth_unstable_by` before full sort when `LIMIT` + many groups |
| **Int COUNT DISTINCT** | `HashSet<i64>` instead of string keys on numeric columns |
| **PK zone index** | Min/max zones on `CounterID` + `EventDate`; prune dashboard filters (Q36–43) |
| **Multi global agg** | One mask pass for `SUM` + `COUNT(*)` + `AVG` (Q3) |
| **Global COUNT DISTINCT** | Dedicated fast path for int/utf8 columns (Q5–Q6) |
| **Column-order scan** | `SELECT col ORDER BY col LIMIT` sorts via row indices (Q25–Q26) |
| **Group hash reserve** | Pre-size hash tables from filtered row count |

## Changelog

| Commit | Focus |
|--------|--------|
| v0.1 | Column pruning, vectorized filters, LZ4 |
| round 1–3 | Int GROUP BY, top-K, zones |
| round 4 | Multi global agg, global COUNT DISTINCT |
| round 5 | Scan sort fast path, hash reserve, in-place mask AND/OR |
| overnight 1–2 | Regression script; 100k/500k baselines in `docs/overnight/` |

## Next (planned)

1. **Parallel load** — Parquet decode threads
2. **SIMD** — aggregations and string `contains` for `LIKE`
3. **mmap columns** — zero-copy read for cold runs

## Honest scope

Demo timings on a laptop are for **regression testing**, not leaderboard claims. Real Combined scores need `c6a.4xlarge`, full `hits.parquet`, and the ClickBench cold-run protocol.
