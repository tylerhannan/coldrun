# Benchmarks

Committed timing snapshots for local **demo** regression (`./scripts/bench-all.sh`). Not ClickBench leaderboard numbers.

**How to measure:** [`MEASUREMENT.md`](MEASUREMENT.md) — `bench-all` vs `bench-serve` vs `bench-clickbench`.

| Path | What |
|------|------|
| [`MEASUREMENT.md`](MEASUREMENT.md) | Which script matches ClickBench hot/cold |
| [`demo-100k/`](demo-100k/) | All 43 queries @ 100k synthetic rows — **start here** |
| [`demo-500k/`](demo-500k/) | Heavier stress run @ 500k rows |
| [`regression/`](regression/) | Early milestone / batch status notes (historical) |
| [`parquet/`](parquet/) | Real `hits.parquet` validate + measure (no cloud) |

## Run locally

```bash
./scripts/bench-all.sh 100000            # dev regression (CLI per query)
./scripts/bench-serve.sh 100000            # warm serve, hot-shaped (3 tries)
./scripts/bench-regression.sh 100000       # smoke + bench-demo + logs
```

Raw logs (gitignored): `logs/benchmarks/`.

## Per-query engineering notes

Implementation notes per query (not timings): [`../perf/`](../perf/).
