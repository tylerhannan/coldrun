# Benchmarks

Committed timing snapshots for local **demo** regression (`./scripts/bench-all.sh`). Not ClickBench leaderboard numbers.

| Path | What |
|------|------|
| [`demo-100k/`](demo-100k/) | All 43 queries @ 100k synthetic rows — **start here** |
| [`demo-500k/`](demo-500k/) | Heavier stress run @ 500k rows |
| [`regression/`](regression/) | Early milestone / batch status notes (historical) |

## Run locally

```bash
./scripts/bench-all.sh 100000          # time all 43 queries (stdout)
./scripts/bench-regression.sh 100000   # smoke + bench-demo + logs
```

Raw logs (gitignored): `logs/benchmarks/`.

## Per-query engineering notes

Implementation notes per query (not timings): [`../perf/`](../perf/).
