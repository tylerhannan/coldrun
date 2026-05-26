# Coldrun

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
| 3. All 43 queries correct | In progress | Q1–10 on demo; GROUP BY / ORDER BY / LIMIT |
| 4. Optimize Combined score | Not started | PK zones, vectorization, compression |
| 5. ClickBench PR | Not started | [`clickbench/coldrun/`](clickbench/coldrun/) harness |

## Prerequisites

- Rust stable (`rustup` recommended); ensure `cargo` is on your `PATH` (reload the shell after install, or `source "$HOME/.cargo/env"`)
- For full dataset: ~15 GB download for `hits.parquet`

## Build

```bash
cargo build --release -p coldrun-cli
# binary: target/release/coldrun
```

## Quick smoke (no download)

Synthetic ~10k rows, runs ClickBench queries 1–10. Full details: [`docs/SMOKE-DEMO.md`](docs/SMOKE-DEMO.md).

```bash
./scripts/smoke-demo.sh
```

Or manually (after `cargo build --release -p coldrun-cli`):

```bash
./target/release/coldrun local --demo 10000 --sql "SELECT COUNT(*) FROM hits"
./target/release/coldrun --data-dir .coldrun-demo local --sql "SELECT COUNT(*) FROM hits"  # after first load
```

## Full dataset

```bash
curl -LO https://datasets.clickhouse.com/hits_compatible/hits.parquet
./scripts/repro-local.sh hits.parquet
```

Or:

```bash
coldrun local --load hits.parquet
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
docs/ARCHITECTURE.md
docs/SMOKE-DEMO.md     # quick local smoke test (scripts/smoke-demo.sh)
```

## Goals (toy scope)

- Load the ClickBench `hits` dataset (~100M rows)
- Run standard SQL analytical queries
- Optionally pursue ClickBench leaderboard numbers — treated as a learning exercise, not a product promise

## Non-goals

- Production reliability, HA, or full SQL standard coverage
- Replacing ClickHouse, DuckDB, or any real OLAP system
- Being taken seriously

## License

[0BSD](LICENSE) — do whatever you want with the code; no attribution required. That does not mean you should run this in production.
