# Performance work

Coldrun optimizes for ClickBench **Combined** (hot 60%, cold 20%, load 10%, disk 10%). See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full design.

## Local benchmarking (no `hits.parquet`)

```bash
./scripts/bench-demo.sh           # default 100k synthetic rows
./scripts/bench-demo.sh 500000  # heavier
```

Compare before/after on the same machine with the same `ROWS` argument.

## Implemented (v0.1)

| Area | What |
|------|------|
| **Column pruning** | Load only columns referenced by the query (`open_table_for_query`) |
| **Vectorized filters** | Fast paths for `AND`/`OR`, `col <> 0`, `col <> ''`, date ranges, `LIKE '%x%'` |
| **Fast global aggregates** | `COUNT(*)` / `SUM` / `AVG` / `MIN`/`MAX` on one column without per-row interpreter |
| **LZ4 column files** | Optional compression on flush for payloads &gt; 4 KB (backward-compatible read) |

## Next (planned)

1. **PK zone maps** on `(CounterID, EventDate, …)` — skip row ranges for dashboard queries (Q36–43)
2. **Columnar group-by keys** — integer keys without `String` allocation per row
3. **Top-K heaps** — `ORDER BY … LIMIT` without full sort
4. **Parallel load** — Parquet decode threads
5. **SIMD** — aggregations and string `contains` for `LIKE`

## Honest scope

Demo timings on a laptop are for **regression testing**, not leaderboard claims. Real Combined scores need `c6a.4xlarge`, full `hits.parquet`, and the ClickBench cold-run protocol.
