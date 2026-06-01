# Benchmarks

Committed timing snapshots for local regression. **Demo** = synthetic `hits`; **Parquet** = real ClickHouse dataset slice.

**How to measure:** [`MEASUREMENT.md`](MEASUREMENT.md) — `bench-all` vs `bench-serve` vs `bench-clickbench`.

| Path | What |
|------|------|
| [`MEASUREMENT.md`](MEASUREMENT.md) | Which script matches ClickBench hot/cold |
| [`demo-100k/`](demo-100k/) | All 43 queries @ 100k synthetic rows — **dev regression** |
| [`demo-500k/`](demo-500k/) | Heavier stress run @ 500k rows |
| [`parquet/`](parquet/) | Real `hits` Parquet: ClickHouse validate + measure |
| [`parquet-hits-1m/`](parquet-hits-1m/) | **1M row** warm-serve hot snapshot (**3.39s** sum) |
| [`regression/`](regression/) | Early milestone / batch status notes (historical) |

## Run locally

```bash
./scripts/bench-all.sh 100000            # dev regression (CLI per query)
./scripts/bench-serve.sh 100000            # warm serve, hot-shaped (3 tries)
./scripts/bench-regression.sh 100000       # smoke + bench-demo + logs
```

Raw logs (gitignored): `logs/benchmarks/`.

## Per-query engineering notes

Implementation notes per query (not timings): [`../perf/`](../perf/).
