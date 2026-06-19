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
| 4. Optimize Combined score | In progress | 1M hot sum **0.84s** (0.62× ClickHouse **1.34s**) — [`compare-hot.md`](docs/benchmarks/parquet-hits-1m/compare-hot.md) |
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
./scripts/measure-parquet.sh data/hits-1m.parquet    # validate + warm bench-serve + CH compare
./scripts/bench-clickhouse-parquet.sh data/hits-1m.parquet --write-snapshot --compare
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

**Done on laptop:** demo + 1M Parquet **43/43** vs ClickHouse; warm-serve hot sum **0.84s** vs CH **1.34s** (**0.62×**) — [`compare-hot.md`](docs/benchmarks/parquet-hits-1m/compare-hot.md).

1. **100M cloud load** — finish load + smoke (Q1/Q36/Q41) on AWS; streaming loader + u64 UTF8 offsets (`4365385`…`e134699`). Target **`c6a.4xlarge`** — see [`docs/CLOUD-RUN.md`](docs/CLOUD-RUN.md).
2. **Warm bench vs ClickHouse** on 100M Parquet (`bench-serve.sh` / `bench-clickhouse-parquet.sh`) before committing Combined numbers.
3. **Q36 / Q41** — largest 1M hot gaps (**0.13s** vs CH **0.007–0.018s**); tune further once 100M data is loaded.
4. **ClickBench PR** — official `clickbench/coldrun/benchmark.sh` (cold + hot, load + disk size) after cloud smoke passes.

Snapshots: [`serve-hot.md`](docs/benchmarks/demo-100k/serve-hot.md) (demo) · [`compare-hot.md`](docs/benchmarks/parquet-hits-1m/compare-hot.md) (1M Parquet) · per-query notes [`docs/perf/`](docs/perf/)

## Out of scope

This is a learning exercise, not a product. We are not building:

- Production reliability, HA, or full SQL standard coverage
- A replacement for ClickHouse, DuckDB, or any real OLAP system
- Anything you should take seriously

## License

[0BSD](LICENSE) — do whatever you want with the code; no attribution required. That does not mean you should run this in production.
