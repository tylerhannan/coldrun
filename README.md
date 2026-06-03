# Coldrun

[![CI](https://github.com/tylerhannan/coldrun/actions/workflows/ci.yml/badge.svg)](https://github.com/tylerhannan/coldrun/actions/workflows/ci.yml)

> **Draft / WIP** — early experiment; not ready for production or serious use.

**A smol columnar SQL toy — an AI tooling experiment.**

Coldrun is a toy analytical SQL engine inspired by [ClickBench](https://benchmark.clickhouse.com/). It exists to explore what agents and modern tooling can build, not to ship a production database.

> **Not a benchmark tool.** 

Coldrun is the database under test. It does not run ClickBench for other systems. For the official harness, see [ClickHouse/ClickBench](https://github.com/ClickHouse/ClickBench).

## Build steps (from [`PROMPT.md`](PROMPT.md))

| Step | Status | Notes |
|------|--------|--------|
| 1. Architecture doc | Done | [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) |
| 2. MVP: load `hits`, queries 1–5 | Done | demo + Parquet (dynamic schema) |
| 3. All 43 queries correct | Done | Demo smoke + **43/43 vs ClickHouse** on 1M Parquet ([`validation-1m.md`](docs/benchmarks/parquet/validation-1m.md)) |
| 4. Optimize Combined score | In progress | 1M hot sum **3.17s** (~1.5× ClickHouse on same slice) — [`docs/PERF.md`](docs/PERF.md) |
| 5. ClickBench PR | Not started | [`clickbench/coldrun/`](clickbench/coldrun/) harness |

## Prerequisites

- Rust stable (`rustup` recommended); ensure `cargo` is on your `PATH` (reload the shell after install, or `source "$HOME/.cargo/env"`)
- **No dataset download required** for local dev — use synthetic demo data below.

## Build

```bash
cargo build --release -p coldrun-cli
# binary: target/release/coldrun
```

## Local development (no download)

Synthetic `hits` rows are generated in-process — same schema shape, not real ClickBench data. Good enough to hack on SQL, storage, and the executor on a laptop.

```bash
./scripts/smoke-demo.sh          # queries 1–15, ~10k rows (default)
./scripts/smoke-all.sh           # all 43 queries; optional row count, e.g. ./scripts/smoke-all.sh 100000
./scripts/bench-all.sh 100000       # time every query (see docs/benchmarks/)
./scripts/bench-compare.sh 100000   # before/after diff on same machine
./scripts/bench-regression.sh 100000  # smoke + bench-demo + logs
./scripts/bench-serve.sh 100000         # warm serve, hot-shaped (see docs/benchmarks/MEASUREMENT.md)
./scripts/install-clickhouse-local.sh   # once, for parquet validate/sample
./scripts/validate-parquet.sh data/hits-1m.parquet   # 43/43 vs ClickHouse
./scripts/measure-parquet.sh data/hits-1m.parquet    # validate + warm bench-serve
./scripts/bench-clickbench.sh --demo 100000 --embedded  # ClickBench output (no per-query restart)
```

Details: [`docs/SMOKE-DEMO.md`](docs/SMOKE-DEMO.md) · [`docs/PERF.md`](docs/PERF.md) · CI on push

Manual one-off:

```bash
./target/release/coldrun local --demo 10000 --sql "SELECT COUNT(*) FROM hits"
./target/release/coldrun --data-dir .coldrun-demo local --sql "SELECT COUNT(*) FROM hits"  # after first load
```

## Full dataset (optional)

Only needed for real ~100M-row benchmarks or ClickBench leaderboard work (~15 GB `hits.parquet`). Skip this on a laptop unless you explicitly want it.

```bash
curl -LO https://datasets.clickhouse.com/hits_compatible/hits.parquet
./scripts/repro-local.sh hits.parquet
```

Or load your own copy from another machine / cloud VM:

```bash
coldrun local --load /path/to/hits.parquet
coldrun local --sql "SELECT COUNT(*) FROM hits"
```

## Binary

One static executable (Rust):

```bash
coldrun serve    # SQL server (TCP, simple text protocol)
coldrun client   # send SQL to serve
coldrun local    # embedded mode (no daemon)
```

## Layout

```
crates/coldrun-core/   # storage, SQL parse, executor
crates/coldrun-cli/    # coldrun binary
clickbench/coldrun/    # ClickBench integration (install, load, query, …)
clickhouse-local/      # bundled ClickHouse binary (gitignored; see README there)
docs/benchmarks/       # committed timing + validation snapshots
```

## In scope (toy)

- Load the ClickBench `hits` dataset (~100M rows)
- Run standard SQL analytical queries
- Optionally pursue ClickBench leaderboard numbers — treated as a learning exercise, not a product promise

## Next (planned)

**Correctness:** demo @ 100k and 1M Parquet both **43/43** (ClickHouse reference).

**Perf (1M Parquet, warm serve):** hot sum **3.17s** — slowest Q23 (0.29s), Q40, Q41, Q38. Roughly **~1.5×** slower than ClickHouse local on the same Parquet slice (~2.1s sum, informal; not an official benchmark). See [`docs/benchmarks/parquet/README.md`](docs/benchmarks/parquet/README.md).

1. **Q40 / Q23 / Q41 / Q38** — close remaining gap on heavy GROUP BY (target ~parity on 1M slice)
2. **ClickHouse parquet bench script** — committed side-by-side snapshot (validate exists; perf compare is manual today)
3. **Scale** — 10M+ slice validation; full 100M only for ClickBench cloud run
4. **ClickBench PR** — `clickbench/coldrun/benchmark.sh` on `c6a.4xlarge`

Snapshots: [`docs/benchmarks/demo-100k/serve-hot.md`](docs/benchmarks/demo-100k/serve-hot.md) (demo) · [`docs/benchmarks/parquet-hits-1m/serve-hot.md`](docs/benchmarks/parquet-hits-1m/serve-hot.md) (1M Parquet) · per-query notes [`docs/perf/`](docs/perf/)

## Out of scope

This is a learning exercise, not a product. We are not building:

- Production reliability, HA, or full SQL standard coverage
- A replacement for ClickHouse, DuckDB, or any real OLAP system
- Anything you should take seriously

## License

[0BSD](LICENSE) — do whatever you want with the code; no attribution required. That does not mean you should run this in production.
